// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The transport stream on its way from the daemon to the host.
//!
//! The two ends want different things. The daemon writes whenever the card has
//! filled a buffer; the host asks "is there anything yet" and then takes what
//! there is. So a thread reads as fast as the daemon writes and puts what it
//! gets in a queue.
//!
//! The queue is bounded. A host that stops reading must not turn into a driver
//! that eats the machine's memory, so the oldest bytes are dropped instead.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use coax::daemon::{Connection, Daemon, Interrupter};
use coax::http::Response;
use isdb::bond::Bond;
use isdb::tsmf::{Carriage, Frames, Splitter};

/// How much to keep waiting for the host. Two seconds of a bonded 8K carriage,
/// which runs at a hundred megabits and more: a host that stops to think must
/// not lose the picture over it. A host that has stopped for good loses the
/// oldest bytes.
const MOST_HELD: usize = 32 * 1024 * 1024;

/// How much to take from the daemon at a time. A multiplex runs at about
/// 3 MB/s, so this is a few milliseconds of it.
const READ_SIZE: usize = 64 * 1024;

#[derive(Debug, Default)]
struct Queue {
    waiting: Vec<Vec<u8>>,
    held: usize,
    /// How wide a piece is, so that what is thrown away is whole pieces and
    /// what is left still starts at the beginning of one. Nought is one.
    unit: usize,
    /// Set when the reader has stopped, so a host waiting for bytes is told
    /// rather than left waiting.
    ended: bool,
    /// How much was thrown away for want of a reader, which is worth knowing
    /// when a recording turns out to have holes.
    dropped: u64,
    /// Everything that arrived and everything that was handed over, for
    /// telling a stream that lost bytes here from one that arrived short.
    arrived_bytes: u64,
    handed_bytes: u64,
    said: Option<Instant>,
}

#[derive(Debug, Default)]
struct Shared {
    queue: Mutex<Queue>,
    arrived: Condvar,
}

impl Shared {
    fn new(unit: usize) -> Self {
        Self {
            queue: Mutex::new(Queue {
                unit,
                ..Queue::default()
            }),
            arrived: Condvar::new(),
        }
    }
}

impl Queue {
    /// Throws away the oldest bytes, at least `at_least` of them, rounded up
    /// to whole pieces. What was handed over and what was thrown away together
    /// always end on a piece boundary, so what is left starts on one.
    fn discard(&mut self, at_least: usize) -> usize {
        let unit = self.unit.max(1);
        let whole = self.held - self.held % unit;
        let mut left = at_least.div_ceil(unit).saturating_mul(unit).min(whole);
        let thrown = left;
        while left > 0 {
            let Some(front) = self.waiting.first_mut() else {
                break;
            };
            if front.len() <= left {
                left -= front.len();
                self.waiting.remove(0);
            } else {
                front.drain(..left);
                left = 0;
            }
        }
        self.held -= thrown;
        thrown
    }
}

/// How often to ask the daemon what the tuner is seeing. A host asks for the
/// signal level as often as it redraws, which is far too often to put an HTTP
/// request behind.
const MEASURE_EVERY: Duration = Duration::from_millis(1000);

/// How long one measurement may wait for the daemon. Letting go of a stream
/// waits for the thread that measures, so this is also the longest a daemon
/// that has stopped answering can hold up closing the tuner.
const MEASURE_PATIENCE: Duration = Duration::from_secs(2);

/// A stream being received.
#[derive(Debug)]
pub struct Stream {
    shared: Arc<Shared>,
    /// How wide a piece of this stream is: a host takes transport packets whole
    /// or not at all, and one handed a part of one refuses the lot.
    unit: usize,
    /// The connections of the other carriers of a split multiplexing frame,
    /// held so that letting go of this lets go of all of them.
    others: Vec<Interrupter>,
    bonded: Vec<JoinHandle<()>>,
    /// The last carrier-to-noise ratio the daemon reported, as the bits of a
    /// float, or nothing measured yet.
    cnr: Arc<AtomicU32>,
    measuring: Option<JoinHandle<()>>,
    /// Held so the read can be cut short: closing the connection is what ends
    /// the thread's blocking read.
    interrupter: Interrupter,
    stopping: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

/// Takes what is waiting, up to a limit, in whole pieces.
///
/// A transport stream is packets, and a host handed a part of one says the size
/// is wrong and throws the lot away -- the pieces arrive as the socket happened
/// to break them up. So the hand-over is cut to a whole number of them.
fn take_whole_pieces(shared: &Shared, unit: usize, limit: usize) -> Option<Vec<u8>> {
    let limit = limit - limit % unit;
    let mut queue = shared.queue.lock().expect("the queue is not poisoned");
    if queue.waiting.is_empty() || limit == 0 {
        return None;
    }
    let mut taken = Vec::new();
    while let Some(front) = queue.waiting.first() {
        if taken.is_empty() && front.len() > limit {
            // One piece is more than the caller has room for, so it is cut and
            // the rest stays at the front for the next call.
            let mut piece = queue.waiting.remove(0);
            let rest = piece.split_off(limit);
            queue.waiting.insert(0, rest);
            queue.held -= piece.len();
            queue.handed_bytes += piece.len() as u64;
            return Some(piece);
        }
        if taken.len() + front.len() > limit {
            break;
        }
        let piece = queue.waiting.remove(0);
        queue.held -= piece.len();
        taken.extend_from_slice(&piece);
    }
    let over = taken.len() % unit;
    if over != 0 {
        let tail = taken.split_off(taken.len() - over);
        queue.held += tail.len();
        queue.waiting.insert(0, tail);
    }
    queue.handed_bytes += taken.len() as u64;
    if taken.is_empty() {
        return None;
    }
    Some(taken)
}

/// How wide a piece of a stream is. Transport packets are handed over whole;
/// the advanced services arrive as TLV, which is a byte stream to whatever
/// reads it and has no size a host checks.
fn unit_of(relative: Option<(u8, Carriage)>) -> usize {
    match relative {
        Some((_, Carriage::Tlv)) => 1,
        _ => TS_PACKET,
    }
}

/// The size of a transport packet.
const TS_PACKET: usize = 188;

impl Stream {
    /// Opens a channel and starts reading it.
    ///
    /// A channel that names a relative stream is one of several sharing a
    /// carrier, and only its own slots go into the queue: a host handed the
    /// whole carrier sees several sets of tables in one stream. A slot that
    /// carries the advanced services is put back together as the TLV it is.
    pub fn open(
        daemon: &Daemon,
        properties: &[(&str, String)],
        relative: Option<(u8, Carriage)>,
    ) -> std::io::Result<Self> {
        let (response, interrupter) = daemon.stream(properties)?;
        // Which tuner the daemon gave out. It says so in a header, which is
        // the only way to know when the channel did not name one.
        let tuner = response.header("x-coax-tuner").map(str::to_owned);
        let shared = Arc::new(Shared::new(unit_of(relative)));
        let stopping = Arc::new(AtomicBool::new(false));
        let reader = thread::Builder::new()
            .name("coax stream".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                let stopping = Arc::clone(&stopping);
                move || read_into(response, &shared, &stopping, relative)
            })?;
        let cnr = Arc::new(AtomicU32::new(NOT_MEASURED));
        let measuring = tuner.map(|tuner| {
            let daemon = daemon.clone().answering_within(MEASURE_PATIENCE);
            let cnr = Arc::clone(&cnr);
            let stopping = Arc::clone(&stopping);
            thread::Builder::new()
                .name("coax signal".to_owned())
                .spawn(move || measure(&daemon, &tuner, &cnr, &stopping))
        });
        let measuring = match measuring {
            Some(Ok(thread)) => Some(thread),
            _ => None,
        };
        Ok(Self {
            shared,
            unit: unit_of(relative),
            others: Vec::new(),
            bonded: Vec::new(),
            cnr,
            measuring,
            interrupter,
            stopping,
            reader: Some(reader),
        })
    }

    /// Opens every carrier of a split multiplexing frame and bonds them.
    ///
    /// A frame too wide for one carrier is sent over several, and a stream on
    /// it is on all of them at once. So every carrier is tuned -- which takes
    /// as many tuners as there are carriers -- and their slots are put back in
    /// the order they were sent. `asks` is what each carrier is asked, in the
    /// order they were sent: its frequency, and the tuner for it with what
    /// that tuner's setting adds.
    pub fn bond(
        daemon: &Daemon,
        asks: &[Vec<(&str, String)>],
        relative: (u8, Carriage),
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::new(unit_of(Some(relative))));
        let stopping = Arc::new(AtomicBool::new(false));
        let bond = Arc::new(Mutex::new(Bond::new(relative.1)));
        // What has been opened so far. A carrier that cannot be had -- its
        // tuner is busy, say -- leaves the ones before it opened, reading and
        // holding their tuners; this lets go of them on the way out.
        let mut opened = Opened {
            stopping: Arc::clone(&stopping),
            interrupters: Vec::new(),
            readers: Vec::new(),
        };
        let mut first_tuner = None;

        for asked in asks {
            let (response, interrupter) = daemon.stream(asked)?;
            if first_tuner.is_none() {
                first_tuner = response.header("x-coax-tuner").map(str::to_owned);
            }
            opened.interrupters.push(interrupter.clone());
            let reader = thread::Builder::new()
                .name("coax carrier".to_owned())
                .spawn({
                    let shared = Arc::clone(&shared);
                    let stopping = Arc::clone(&stopping);
                    let bond = Arc::clone(&bond);
                    let wanted = relative.0;
                    let everyone = opened.interrupters.clone();
                    move || read_bonded(response, &shared, &stopping, &bond, wanted, &everyone)
                });
            match reader {
                Ok(reader) => opened.readers.push(reader),
                Err(reason) => {
                    interrupter.interrupt();
                    return Err(reason);
                }
            }
        }
        let mut interrupters = std::mem::take(&mut opened.interrupters);
        let mut readers = std::mem::take(&mut opened.readers);
        drop(opened);
        let (Some(interrupter), Some(reader)) = (interrupters.pop(), readers.pop()) else {
            return Err(std::io::Error::other("a split frame with no carriers"));
        };

        let cnr = Arc::new(AtomicU32::new(NOT_MEASURED));
        let measuring = match first_tuner.map(|tuner| {
            let daemon = daemon.clone().answering_within(MEASURE_PATIENCE);
            let cnr = Arc::clone(&cnr);
            let stopping = Arc::clone(&stopping);
            thread::Builder::new()
                .name("coax signal".to_owned())
                .spawn(move || measure(&daemon, &tuner, &cnr, &stopping))
        }) {
            Some(Ok(thread)) => Some(thread),
            _ => None,
        };

        Ok(Self {
            shared,
            unit: unit_of(Some(relative)),
            others: interrupters,
            bonded: readers,
            cnr,
            measuring,
            interrupter,
            stopping,
            reader: Some(reader),
        })
    }

    /// Something to wait on that does not borrow the stream, so the wait can
    /// happen while the stream itself is free to be replaced.
    pub fn waiter(&self) -> Waiter {
        Waiter {
            shared: Arc::clone(&self.shared),
            unit: self.unit,
        }
    }

    /// Takes what is waiting, up to a limit, oldest first. As much as the limit
    /// allows rather than one read's worth: a host asks once per turn round its
    /// own loop, and handing it sixty-four kilobytes a turn caps the stream at
    /// whatever that loop runs at -- measured, far below what was arriving, and
    /// the rest was dropped.
    pub fn take_upto(&self, limit: usize) -> Option<Vec<u8>> {
        take_whole_pieces(&self.shared, self.unit, limit)
    }

    /// How much is waiting to be taken, in whole pieces, which is what the
    /// interface's own count means. The pieces the stream arrived in are not
    /// the pieces it is handed over in.
    pub fn ready(&self) -> u32 {
        let queue = self.shared.queue.lock().expect("the queue is not poisoned");
        u32::try_from(queue.held / self.unit).unwrap_or(u32::MAX)
    }

    /// Throws away what has arrived but not been taken, which is what a host
    /// asks for when it wants to start again from now.
    ///
    /// The part of a piece at the end stays: the rest of it is on its way, and
    /// throwing away its start would leave every piece after it cut wrong.
    pub fn purge(&self) {
        let mut queue = self.shared.queue.lock().expect("the queue is not poisoned");
        let held = queue.held;
        queue.discard(held);
    }

    /// Whether the daemon has stopped sending.
    pub fn ended(&self) -> bool {
        self.shared
            .queue
            .lock()
            .expect("the queue is not poisoned")
            .ended
    }

    /// What the tuner is seeing, in decibels, or zero before anything has been
    /// measured. Zero is also what a backend that cannot measure gives, which
    /// is the same thing as far as a meter is concerned.
    pub fn cnr(&self) -> f32 {
        match self.cnr.load(Ordering::Relaxed) {
            NOT_MEASURED => 0.0,
            bits => f32::from_bits(bits),
        }
    }

    /// How much was dropped for want of a reader.
    pub fn dropped(&self) -> u64 {
        self.shared
            .queue
            .lock()
            .expect("the queue is not poisoned")
            .dropped
    }
}

/// A wait for a stream, holding the queue rather than the stream.
#[derive(Debug)]
pub struct Waiter {
    shared: Arc<Shared>,
    unit: usize,
}

impl Waiter {
    /// Waits for something to read, and says whether there is any.
    ///
    /// Something to read is a whole piece. The tail of what came in is usually
    /// a part of a packet, and a part of a packet is not handed over: a host
    /// told the stream was ready and then handed nothing asks again at once,
    /// and that loop runs at full tilt round the same lock the reading thread
    /// needs to put the rest of the packet in -- so the tail never is completed
    /// and the stream stops for good a few packets in.
    pub fn wait(&self, timeout: Duration) -> bool {
        let queue = self.shared.queue.lock().expect("the queue is not poisoned");
        if queue.held >= self.unit {
            return true;
        }
        // An ended stream has nothing more to say, but a host told so at once
        // asks again at once.
        if queue.ended {
            drop(queue);
            thread::sleep(timeout);
            return false;
        }
        let (queue, _) = self
            .shared
            .arrived
            .wait_timeout(queue, timeout)
            .expect("the queue is not poisoned");
        queue.held >= self.unit
    }
}

/// What the measurement holds before there has been one. A ratio is never a
/// signalling NaN, so this cannot be mistaken for a reading.
const NOT_MEASURED: u32 = 0x7fc0_0001;

/// Asks the daemon what the tuner is seeing, until the stream is let go of.
fn measure(daemon: &Daemon, tuner: &str, cnr: &AtomicU32, stopping: &AtomicBool) {
    while !stopping.load(Ordering::SeqCst) {
        if let Ok(Some(measured)) = daemon.cnr(tuner) {
            cnr.store(measured.to_bits(), Ordering::Relaxed);
        }
        // Slept in short pieces so that letting go of the stream is not held
        // up by a measurement that has just been taken.
        for _ in 0..10 {
            if stopping.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(MEASURE_EVERY / 10);
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // The reader is inside a blocking read; closing the connection is what
        // brings it back out. Waiting for it without this would be waiting for
        // the daemon to say something it has no reason to say.
        self.interrupter.interrupt();
        for other in &self.others {
            other.interrupt();
        }
        for reader in self.bonded.drain(..) {
            let _ = reader.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(measuring) = self.measuring.take() {
            let _ = measuring.join();
        }
    }
}

/// The carriers of a split frame opened so far, let go of together unless they
/// are taken out first.
struct Opened {
    stopping: Arc<AtomicBool>,
    interrupters: Vec<Interrupter>,
    readers: Vec<JoinHandle<()>>,
}

impl Drop for Opened {
    fn drop(&mut self) {
        if self.readers.is_empty() && self.interrupters.is_empty() {
            return;
        }
        self.stopping.store(true, Ordering::SeqCst);
        for interrupter in &self.interrupters {
            interrupter.interrupt();
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

/// Reads one carrier of a split frame into the bond, and puts whatever the
/// bond can then say into the queue.
///
/// When one carrier ends the stream has: what the others carry cannot be put
/// together without it. So the others are stopped too, rather than left
/// filling the bond with what will never be handed on. `everyone` is the
/// carriers opened up to and including this one; the stopping flag reaches
/// the ones opened after.
fn read_bonded(
    mut response: Response<Connection>,
    shared: &Shared,
    stopping: &AtomicBool,
    bond: &Mutex<Bond>,
    wanted: u8,
    everyone: &[Interrupter],
) {
    let mut frames = Frames::new(wanted);
    let mut buffer = vec![0u8; READ_SIZE];
    loop {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        match response.read_body(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let arrived = frames.feed(&buffer[..read]);
                if arrived.is_empty() {
                    continue;
                }
                let ready = bond.lock().expect("the bond is not poisoned").feed(arrived);
                if !ready.is_empty() {
                    put(shared, &ready);
                }
            }
        }
    }
    stopping.store(true, Ordering::SeqCst);
    for other in everyone {
        other.interrupt();
    }
    let mut queue = shared.queue.lock().expect("the queue is not poisoned");
    queue.ended = true;
    shared.arrived.notify_all();
}

fn read_into(
    mut response: Response<Connection>,
    shared: &Shared,
    stopping: &AtomicBool,
    relative: Option<(u8, Carriage)>,
) {
    let mut buffer = vec![0u8; READ_SIZE];
    let mut splitter = relative.map(|(number, carriage)| Splitter::of(number, carriage));
    loop {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        match response.read_body(&mut buffer) {
            Ok(0) => break,
            Ok(read) => match &mut splitter {
                Some(splitter) => {
                    let kept = splitter.feed(&buffer[..read]);
                    if !kept.is_empty() {
                        put(shared, &kept);
                    }
                }
                None => put(shared, &buffer[..read]),
            },
            // A read that timed out is a carrier that has gone quiet, not a
            // stream that has ended: a tuner whose picture stops for a few
            // seconds comes back. The connection is let go of only when it is
            // really gone.
            Err(reason)
                if matches!(
                    reason.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let mut queue = shared.queue.lock().expect("the queue is not poisoned");
    queue.ended = true;
    shared.arrived.notify_all();
}

fn put(shared: &Shared, bytes: &[u8]) {
    let mut queue = shared.queue.lock().expect("the queue is not poisoned");
    queue.waiting.push(bytes.to_vec());
    queue.held += bytes.len();
    queue.arrived_bytes += bytes.len() as u64;
    // Every few seconds, what came in against what went out. A host that is
    // behind shows up as the two drifting apart with the queue holding the
    // difference; bytes lost anywhere else show up as neither.
    let say = queue
        .said
        .is_none_or(|said| said.elapsed() >= Duration::from_secs(5));
    if say && coax_settings::trace::enabled() {
        queue.said = Some(Instant::now());
        coax_settings::trace!(
            "driver: arrived {} bytes, handed over {}, waiting {}, dropped {}",
            queue.arrived_bytes,
            queue.handed_bytes,
            queue.held,
            queue.dropped,
        );
    }
    let was = queue.dropped;
    if queue.held > MOST_HELD {
        let over = queue.held - MOST_HELD;
        let thrown = queue.discard(over);
        queue.dropped += thrown as u64;
    }
    if queue.dropped != was {
        // The host has not taken anything for long enough to fill the buffer,
        // so the oldest bytes go. Worth saying: what a viewer sees is a hole
        // in the picture, and nothing else in the account would show why.
        coax_settings::trace!(
            "driver: the host is behind; dropped {} bytes, {} in all",
            queue.dropped - was,
            queue.dropped,
        );
    }
    shared.arrived.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(pieces: &[usize]) -> Shared {
        let shared = Shared::default();
        {
            let mut queue = shared.queue.lock().expect("the queue is not poisoned");
            for (index, length) in pieces.iter().enumerate() {
                queue.waiting.push(vec![index as u8; *length]);
                queue.held += length;
            }
        }
        shared
    }

    #[test]
    fn a_host_is_handed_whole_packets_however_the_socket_broke_the_stream_up() {
        // 300 bytes is one packet and a bit; the bit stays behind.
        let shared = queued(&[300]);
        let taken = take_whole_pieces(&shared, TS_PACKET, 64 * 1024).expect("something is waiting");
        assert_eq!(taken.len(), TS_PACKET);
        assert!(
            take_whole_pieces(&shared, TS_PACKET, 64 * 1024).is_none(),
            "the tail alone is not a packet yet"
        );
    }

    #[test]
    fn pieces_are_joined_and_then_cut_at_a_packet_boundary() {
        let shared = queued(&[100, 100, 100]);
        let taken = take_whole_pieces(&shared, TS_PACKET, 64 * 1024).expect("something is waiting");
        assert_eq!(taken.len(), TS_PACKET);
    }

    #[test]
    fn a_hosts_own_buffer_is_rounded_down_to_a_packet() {
        let shared = queued(&[10 * TS_PACKET]);
        let taken = take_whole_pieces(&shared, TS_PACKET, 500).expect("something is waiting");
        assert_eq!(taken.len(), 2 * TS_PACKET);
    }

    /// TLV is a byte stream to whatever reads it, so nothing is held back.
    #[test]
    fn a_tlv_stream_is_handed_over_as_it_arrives() {
        let shared = queued(&[300]);
        let taken = take_whole_pieces(&shared, 1, 64 * 1024).expect("something is waiting");
        assert_eq!(taken.len(), 300);
    }

    /// What is thrown away is whole packets, so the stream left behind still
    /// starts at the beginning of one however the socket broke it up.
    #[test]
    fn throwing_away_keeps_the_packet_boundary() {
        let shared = queued(&[300, 300]);
        let mut queue = shared.queue.lock().expect("the queue is not poisoned");
        queue.unit = TS_PACKET;
        assert_eq!(queue.discard(100), TS_PACKET);
        assert_eq!(queue.held, 600 - TS_PACKET);
        assert_eq!(queue.waiting[0].len(), 300 - TS_PACKET);
    }

    /// Starting again from now keeps the start of a packet whose rest has not
    /// arrived yet.
    #[test]
    fn a_purge_keeps_the_part_of_a_packet_at_the_end() {
        let shared = queued(&[300]);
        shared.queue.lock().expect("the queue is not poisoned").unit = TS_PACKET;
        let stream_held = |shared: &Shared| shared.queue.lock().expect("not poisoned").held;
        let mut queue = shared.queue.lock().expect("the queue is not poisoned");
        let held = queue.held;
        queue.discard(held);
        drop(queue);
        assert_eq!(stream_held(&shared), 300 - TS_PACKET);
    }
}

// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What the interface's calls mean, with the ABI left behind in [`crate::abi`].
//!
//! A BonDriver is one tuner, but its caller does not always drive it from one
//! place: a host may wait for the stream on one thread while it reads the
//! signal level or changes the channel on another. So the state is plain and
//! the whole of it sits behind one lock, [`Locked`], which is what the two
//! interfaces hold.

use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use coax::daemon::{self, Daemon};
use coax::launch::Launcher;
use coax_settings::channels::{Channel, Lineup, Route};
use coax_settings::tuner_settings::{Listed, TunerSettings};
use isdb::tsmf::Carriage;

use coax_settings::trace;

use crate::stream::{Stream, Waiter};
use coax_settings::wide::Wide;

/// One tuner as the host sees it.
#[derive(Debug)]
pub struct Driver {
    /// The spaces and channels this driver hands out. Built once, at load, so
    /// that a name handed to the host stays where it was put.
    lineup: Lineup,
    /// The names, kept because the interface hands out pointers into them.
    tuner_name: Wide,
    space_names: Vec<Wide>,
    channel_names: Vec<Vec<Wide>>,
    /// Where the tuner is, or nothing before the first successful tune. The
    /// interface answers with 0xFFFFFFFF until then, as `GetCurSpace` and
    /// `GetCurChannel` mean.
    tuned: Option<(u32, u32)>,
    open: bool,
    /// The daemon that owns the hardware.
    daemon: Daemon,
    /// How to start the daemon when nothing is answering, or nothing when the
    /// settings said not to.
    launcher: Option<Launcher>,
    /// The channel being received, if any. Dropping it is what gives the tuner
    /// back to the daemon.
    stream: Option<Stream>,
    /// The piece last handed to the host. Held because the interface's pointer
    /// form promises the bytes stay put until the next call.
    handed_out: Vec<u8>,
    /// The tuners the daemon has, as they were when last asked. A space names
    /// its tuners by pattern and every tune has to turn those back into
    /// identifiers; read again on every change of channel, since a board may
    /// have been pulled out or plugged in since. Kept with what each tuner said
    /// of itself and what it lets a channel set, which is what a tune adds to
    /// for that tuner.
    listed: Listed,
    /// What the settings' routes say the tuners receive. A tuner cannot always
    /// be asked -- a Windows BDA tuner is described by what its driver
    /// registered, which on plenty of cards names one standard of several -- so
    /// the file says instead.
    declared: Vec<(String, Vec<String>)>,
    /// What `[TunerSettings]` sets for each tuner beside the channel.
    settings: TunerSettings,
    /// Getting the channel back after its stream ended without the host
    /// asking, or nothing while the stream is running.
    recovery: Option<Recovery>,
}

/// Getting a channel back whose stream ended: the daemon stopped, or the board
/// was pulled out. A host does not tune again by itself; it goes on waiting
/// for a stream that will not come.
#[derive(Debug)]
struct Recovery {
    /// When to try next.
    at: Instant,
    /// How long to wait after the next failure.
    wait: Duration,
    /// A start of the daemon under way. Starting it takes seconds, which are
    /// spent on a thread of their own rather than under the driver's lock.
    starting: Option<JoinHandle<io::Result<String>>>,
}

/// How long a failed recovery waits before the next, at first and at most.
/// A daemon that cannot be started or a board that is not there is not asked
/// again and again by a host that keeps reading.
const RECOVERY_FIRST: Duration = Duration::from_secs(2);
const RECOVERY_LONGEST: Duration = Duration::from_secs(30);

/// Whether a refusal was about the signal rather than about the tuner.
///
/// A frequency nothing is transmitting on is refused by every tuner alike, and
/// each one asked spends the lock timeout finding that out again -- over a
/// whole band, the difference between minutes and a quarter of an hour. So a
/// refusal that names the signal ends the round; refusals about the tuner
/// itself go on to the next.
fn signal_is_missing(reason: &str) -> bool {
    reason.contains("no_lock") || reason.contains("no_sync")
}

/// Which stream of a multiplexing frame a channel names, and what it carries.
/// Anything outside the fifteen a frame can hold is no answer, and the carrier
/// is taken whole rather than filtered down to nothing.
fn relative_stream(parameters: &[(String, String)]) -> Option<(u8, Carriage)> {
    let named = parameters.iter().find_map(|(name, value)| {
        if name.eq_ignore_ascii_case(RELATIVE_STREAM) {
            return Some((value, Carriage::Transport));
        }
        if name.eq_ignore_ascii_case(RELATIVE_TLV) {
            return Some((value, Carriage::Tlv));
        }
        None
    })?;
    let number = named
        .0
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=15).contains(n))?;
    Some((number, named.1))
}

/// A pattern every tuner matches.
const ANY_TUNER: &str = "*";

/// Words with the names borrowed, as the daemon's query takes them.
fn borrowed(words: &[(String, String)]) -> Vec<(&str, String)> {
    words
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect()
}

/// What `GetCurSpace` and `GetCurChannel` answer before anything is tuned.
pub const NOT_TUNED: u32 = 0xFFFF_FFFF;

/// What `WaitTsStream` answers, which is the vocabulary of
/// `WaitForSingleObject`: zero for something to read, this for nothing yet.
/// Anything else is read by a host as "ready", and it then asks for bytes that
/// are not there.
pub const WAIT_TIMEOUT: u32 = 258;

/// What a channel calls the stream it wants out of a multiplexing frame. The
/// names are the ones the command line tool's channel files use. The second
/// names a slot that carries the advanced services, which arrive as TLV inside
/// the packets.
const RELATIVE_STREAM: &str = "TSMF_REL_TS";
const RELATIVE_TLV: &str = "TSMF_REL_TLV";

/// The most to hand a host in one call of the form that gives a pointer. Enough
/// that a host asking eighty times a second keeps up with a bonded 8K carriage
/// twice over, and no more: one host measured here stopped producing anything
/// when handed megabytes.
const MOST_AT_ONCE: usize = 256 * 1024;

/// The most the copying form of `GetTsStream` writes into a host's buffer at
/// once. That form says nothing about how big the buffer is; a host that sets
/// the size it passes in is held to it, and one that does not is never written
/// more than this -- the piece hosts have long sized their buffers for, 256
/// transport packets.
pub const MOST_COPIED: usize = 188 * 256;

/// The longest this will sit in one wait. A host may ask to wait for ever, and
/// a driver that does cannot be closed.
const LONGEST_WAIT: Duration = Duration::from_millis(1000);

/// What one tuner is asked for a channel: the channel's words, the tuner, and
/// what that tuner's setting adds that it can take (see
/// [`Listed::asking`]).
fn asked_of(
    listed: &Listed,
    settings: &TunerSettings,
    tuner: Option<&str>,
    parameters: &[(String, String)],
) -> Vec<(String, String)> {
    listed.asking(tuner, settings, parameters)
}

/// What each carrier of a split multiplexing frame is asked. Every carrier of a
/// group takes the same words bar the frequency and the tuner -- they are
/// received at the same time, so one tuner cannot serve two of them -- and
/// what a tuner's setting adds goes with the tuner it is for.
fn carrier_asks(
    listed: &Listed,
    settings: &TunerSettings,
    parameters: &[(String, String)],
    carriers: &[String],
    tuners: &[String],
) -> Vec<Vec<(String, String)>> {
    carriers
        .iter()
        .enumerate()
        .map(|(which, frequency)| {
            let mut words: Vec<(String, String)> = parameters
                .iter()
                .filter(|(name, _)| {
                    !name.eq_ignore_ascii_case("FREQUENCY") && !name.eq_ignore_ascii_case("tuner")
                })
                .cloned()
                .collect();
            words.push(("FREQUENCY".to_owned(), frequency.clone()));
            asked_of(
                listed,
                settings,
                tuners.get(which).map(String::as_str),
                &words,
            )
        })
        .collect()
}

impl Driver {
    /// Builds a driver over a lineup, rendering every name once. `declared` is
    /// what the settings' routes say the tuners receive, and `settings` what
    /// `[TunerSettings]` sets for them.
    pub fn new(
        lineup: Lineup,
        tuner_name: &str,
        daemon: Daemon,
        launcher: Option<Launcher>,
        declared: Vec<(String, Vec<String>)>,
        settings: TunerSettings,
    ) -> Self {
        let space_names = lineup
            .spaces()
            .iter()
            .map(|space| Wide::new(&space.name))
            .collect();
        let channel_names = lineup
            .spaces()
            .iter()
            .map(|space| {
                space
                    .channels
                    .iter()
                    .map(|one| Wide::new(&one.name))
                    .collect()
            })
            .collect();
        Self {
            lineup,
            tuner_name: Wide::new(tuner_name),
            space_names,
            channel_names,
            tuned: None,
            open: false,
            daemon,
            launcher,
            stream: None,
            handed_out: Vec::new(),
            listed: Listed::default(),
            declared,
            settings,
            recovery: None,
        }
    }

    /// Says whether the hardware can be reached, which is what a host reads as
    /// "this tuner is usable". Nothing is claimed yet: the daemon hands a tuner
    /// out per stream, so claiming one here would hold it through every channel
    /// the host merely looks at.
    pub fn open_tuner(&mut self) -> bool {
        let reached = match &self.launcher {
            Some(launcher) => self.daemon.ensure_running(launcher),
            None => self.daemon.version(),
        };
        match reached {
            Ok(version) => {
                trace!("daemon: {version}");
                self.open = true;
                true
            }
            Err(reason) => {
                trace!("daemon unreachable: {reason}");
                false
            }
        }
    }

    pub fn close_tuner(&mut self) {
        self.let_go_of_the_stream();
        self.open = false;
        self.tuned = None;
        self.recovery = None;
    }

    /// Gives the tuner back, saying what was lost on the way if anything was.
    /// Bytes are dropped when the host stops reading, and a recording with a
    /// hole in it is otherwise a mystery: from the host's side nothing went
    /// wrong.
    fn let_go_of_the_stream(&mut self) {
        if let Some(stream) = self.stream.take() {
            let dropped = stream.dropped();
            if dropped > 0 {
                trace!("{dropped} bytes were dropped: the host did not keep up");
            }
            if stream.ended() {
                trace!("the daemon had already stopped sending");
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn tuner_name(&self) -> *const u16 {
        self.tuner_name.as_ptr()
    }

    /// The name of one space, or null past the last -- which is how a caller
    /// finds out how many there are.
    pub fn tuning_space_name(&self, space: u32) -> *const u16 {
        match self.space_names.get(space as usize) {
            Some(name) => name.as_ptr(),
            None => core::ptr::null(),
        }
    }

    /// The name of one channel, or null past the last of its space. A caller
    /// walks the indexes from zero and stops at the first null, so the list has
    /// to be dense.
    pub fn channel_name(&self, space: u32, channel: u32) -> *const u16 {
        match self
            .channel_names
            .get(space as usize)
            .and_then(|names| names.get(channel as usize))
        {
            Some(name) => name.as_ptr(),
            None => core::ptr::null(),
        }
    }

    /// The channel a space and an index name, or nothing where they name none.
    pub fn channel(&self, space: u32, channel: u32) -> Option<&Channel> {
        self.lineup
            .spaces()
            .get(space as usize)
            .and_then(|space| space.channels.get(channel as usize))
    }

    pub fn set_channel(&mut self, space: u32, channel: u32) -> bool {
        let Some(wanted) = self.channel(space, channel) else {
            return false;
        };
        // Owned rather than borrowed: the stream below is replaced, and the
        // lineup the channel came from lives in the same driver.
        //
        // The ways of receiving it, in the order to try them: what the channel
        // itself says, and then the others the same stream is on. A lead whose
        // tuners are all busy is a reason to try the other rather than to fail.
        let ways: Vec<Route> = std::iter::once(Route {
            tune: wanted.tune.clone(),
            tuners: wanted.tuners.clone(),
            carriers: wanted.carriers.clone(),
        })
        .chain(wanted.also.iter().cloned())
        .filter(|way| !way.tune.is_empty())
        .collect();
        if ways.is_empty() {
            trace!("SetChannel({space}, {channel}): nothing known about how to tune it");
            return false;
        }
        self.recovery = None;
        // The old stream goes first. The daemon holds a tuner for as long as
        // the connection that asked for it, so keeping this would ask for a
        // second tuner.
        self.let_go_of_the_stream();

        // The daemon was there at OpenTuner, which may have been hours ago.
        if let Some(launcher) = &self.launcher
            && self.daemon.version().is_err()
        {
            match self.daemon.ensure_running(launcher) {
                Ok(version) => trace!("daemon started again: {version}"),
                Err(reason) => {
                    trace!("SetChannel({space}, {channel}): daemon unreachable: {reason}");
                    self.tuned = None;
                    return false;
                }
            }
        }
        // Boards come and go while the daemon runs, and one plugged into
        // another socket comes back under another name.
        self.listed = Listed::default();

        let mut refusal = String::from("no tuner to ask");
        for way in &ways {
            match self.tune(space, way) {
                Ok(stream) => {
                    self.stream = Some(stream);
                    self.tuned = Some((space, channel));
                    return true;
                }
                Err(why) => {
                    refusal = why;
                    trace!("SetChannel({space}, {channel}): {refusal}");
                }
            }
        }
        trace!("SetChannel({space}, {channel}) failed: {refusal}");
        self.tuned = None;
        false
    }

    /// Opens one way of receiving a channel, trying the tuners that can do it.
    fn tune(&mut self, space: u32, way: &Route) -> Result<Stream, String> {
        let mut parameters = way.tune.clone();
        let system = parameters
            .iter()
            .find(|(name, _)| name == "DELIVERY_SYSTEM")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        // Which stream of a multiplexing frame this channel is. Taken out here
        // rather than sent on: the daemon hands over the carrier as it comes
        // off the card.
        let relative = relative_stream(&parameters);
        parameters.retain(|(name, _)| {
            !name.eq_ignore_ascii_case(RELATIVE_STREAM) && !name.eq_ignore_ascii_case(RELATIVE_TLV)
        });
        // A channel on a split frame needs as many tuners as it has carriers,
        // and they are received at once: the whole list is handed over rather
        // than one at a time.
        let candidates = self.candidates(space, way, &system);
        let for_each_carrier: Vec<String> = if way.carriers.len() > 1 {
            candidates.iter().flatten().cloned().collect()
        } else {
            Vec::new()
        };
        let mut refusal = String::from("no tuner to ask");
        for tuner in candidates {
            // A channel on a split multiplexing frame is on every carrier of
            // its group at once, and takes a tuner for each.
            let opened = match relative {
                Some(relative) if way.carriers.len() > 1 => {
                    let asks = carrier_asks(
                        &self.listed,
                        &self.settings,
                        &parameters,
                        &way.carriers,
                        &for_each_carrier,
                    );
                    let asks: Vec<Vec<(&str, String)>> =
                        asks.iter().map(|ask| borrowed(ask)).collect();
                    Stream::bond(&self.daemon, &asks, relative)
                }
                _ => {
                    let asked =
                        asked_of(&self.listed, &self.settings, tuner.as_deref(), &parameters);
                    Stream::open(&self.daemon, &borrowed(&asked), relative)
                }
            };
            match opened {
                Ok(stream) => return Ok(stream),
                Err(reason) => {
                    refusal = match &tuner {
                        Some(named) => format!("{named}: {reason}"),
                        None => reason.to_string(),
                    };
                    if signal_is_missing(&refusal) {
                        // The tuners are not the trouble: the carrier is not
                        // there. Another tuner on the same lead would say the
                        // same.
                        break;
                    }
                }
            }
        }
        Err(refusal)
    }

    /// The tuners to ask for one way of receiving a channel, in the order to
    /// ask them.
    ///
    /// A way that names its own tuners is received on those and not on the
    /// space's: a stream filed under the platform it belongs to may have come
    /// down another lead. Where neither says, the one candidate is nothing at
    /// all, which leaves the choice to the daemon -- unless `[TunerSettings]`
    /// says something, which has to know which tuner it is going to: then every
    /// tuner that receives the system is a candidate.
    fn candidates(&mut self, space: u32, way: &Route, system: &str) -> Vec<Option<String>> {
        let patterns = if !way.tuners.is_empty() {
            way.tuners.clone()
        } else {
            match self.lineup.spaces().get(space as usize) {
                Some(space) if !space.tuners.is_empty() => space.tuners.clone(),
                _ if !self.settings.is_empty() => vec![ANY_TUNER.to_owned()],
                _ => return vec![None],
            }
        };
        let matched = daemon::candidates(&self.listed.tuners, &patterns, system);
        if matched.iter().any(Option::is_some) {
            return matched;
        }
        // Not asked yet for this tune, or a pattern of another way matched
        // nothing in what was.
        self.listed = match self.daemon.listing() {
            // What the routes say a tuner receives goes on top of what the
            // tuner said: a Windows BDA tuner's driver often registers less
            // than the card does.
            Ok(boards) => Listed::of(boards, &self.declared),
            Err(reason) => {
                trace!("the daemon would not list its tuners: {reason}");
                Listed::default()
            }
        };
        daemon::candidates(&self.listed.tuners, &patterns, system)
    }

    pub fn current_space(&self) -> u32 {
        self.tuned.map_or(NOT_TUNED, |(space, _)| space)
    }

    pub fn current_channel(&self) -> u32 {
        self.tuned.map_or(NOT_TUNED, |(_, channel)| channel)
    }

    /// The figure a host puts on its signal meter, which is the
    /// carrier-to-noise ratio in decibels. Read from what the stream last
    /// measured rather than from the hardware: a host asks for this as often as
    /// it redraws. Zero before the first measurement, and on a backend that
    /// cannot measure.
    pub fn signal_level(&self) -> f32 {
        self.stream.as_ref().map_or(0.0, Stream::cnr)
    }

    /// Waits for something to read. The wait is honoured rather than answered
    /// at once: a host told "nothing yet" without any delay asks again
    /// immediately -- measured here, three hundred million times in ten
    /// seconds.
    #[cfg(test)]
    pub fn wait_ts_stream(&mut self, timeout_ms: u32) -> u32 {
        self.recover();
        wait_on(self.waiter(), timeout_ms)
    }

    /// What a wait for the stream waits on, taken out so the wait itself can
    /// happen without the driver's lock held.
    fn waiter(&self) -> Option<Waiter> {
        self.stream.as_ref().map(Stream::waiter)
    }

    pub fn ready_count(&self) -> u32 {
        self.stream.as_ref().map_or(0, Stream::ready)
    }

    /// Hands over what has arrived, or nothing when nothing has. The form that
    /// hands back a pointer has no buffer to fill, so it gives as much as has
    /// piled up.
    pub fn take_stream(&mut self) -> Option<&[u8]> {
        self.recover();
        let taken = self.stream.as_ref()?.take_upto(MOST_AT_ONCE)?;
        self.handed_out = taken;
        Some(&self.handed_out)
    }

    /// The same, for the form of the call that copies into a buffer of the
    /// host's own: never more than the host said it had room for.
    pub fn take_stream_upto(&mut self, limit: usize) -> Option<&[u8]> {
        self.recover();
        let taken = self.stream.as_ref()?.take_upto(limit.min(MOST_AT_ONCE))?;
        self.handed_out = taken;
        Some(&self.handed_out)
    }

    pub fn purge_ts_stream(&mut self) {
        if let Some(stream) = &self.stream {
            stream.purge();
        }
    }

    /// Tunes the channel again when its stream has ended and everything it
    /// sent has been handed over. Called as the host reads, which is the only
    /// time a driver is called.
    fn recover(&mut self) {
        let Some((space, channel)) = self.tuned else {
            self.recovery = None;
            return;
        };
        let lost = self
            .stream
            .as_ref()
            .is_none_or(|stream| stream.ended() && stream.ready() == 0);
        if !lost {
            self.recovery = None;
            return;
        }
        let recovery = self.recovery.get_or_insert_with(|| {
            trace!("the stream of {space}/{channel} ended; tuning it again");
            Recovery {
                at: Instant::now(),
                wait: RECOVERY_FIRST,
                starting: None,
            }
        });
        if Instant::now() < recovery.at {
            return;
        }
        if let Some(starting) = recovery.starting.take() {
            if !starting.is_finished() {
                recovery.starting = Some(starting);
                return;
            }
            match starting.join() {
                Ok(Ok(version)) => trace!("daemon started again: {version}"),
                Ok(Err(reason)) => {
                    trace!("daemon unreachable: {reason}");
                    self.recovery_failed();
                    return;
                }
                Err(_) => {
                    self.recovery_failed();
                    return;
                }
            }
        } else if self.daemon.version().is_err() {
            // Without a launcher the daemon is left to whoever runs it, and
            // asked again later.
            let Some(launcher) = self.launcher.clone() else {
                self.recovery_failed();
                return;
            };
            let daemon = self.daemon.clone();
            match thread::Builder::new()
                .name("coax daemon start".to_owned())
                .spawn(move || daemon.ensure_running(&launcher))
            {
                Ok(starting) => recovery.starting = Some(starting),
                Err(_) => self.recovery_failed(),
            }
            return;
        }
        let wait = recovery.wait;
        if self.set_channel(space, channel) {
            trace!("the stream of {space}/{channel} is back");
            return;
        }
        // Kept, so that the next try knows what to tune. Tuning dropped the
        // recovery, which is put back as it was.
        self.tuned = Some((space, channel));
        self.recovery = Some(Recovery {
            at: Instant::now(),
            wait,
            starting: None,
        });
        self.recovery_failed();
    }

    fn recovery_failed(&mut self) {
        if let Some(recovery) = &mut self.recovery {
            recovery.at = Instant::now() + recovery.wait;
            recovery.wait = (recovery.wait * 2).min(RECOVERY_LONGEST);
        }
    }
}

/// Waits on a stream, or for the whole wait when there is none: a host told
/// "nothing yet" at once asks again at once.
fn wait_on(waiter: Option<Waiter>, timeout_ms: u32) -> u32 {
    let waiting = Duration::from_millis(u64::from(timeout_ms)).min(LONGEST_WAIT);
    let Some(waiter) = waiter else {
        thread::sleep(waiting);
        return WAIT_TIMEOUT;
    };
    if waiter.wait(waiting) {
        0
    } else {
        WAIT_TIMEOUT
    }
}

/// A driver as the interfaces hold it: behind a lock, so that two of the
/// host's threads calling at once take turns rather than one of them reading
/// a stream the other is replacing.
#[derive(Debug)]
pub struct Locked(Mutex<Driver>);

impl Locked {
    pub fn new(driver: Driver) -> Self {
        Self(Mutex::new(driver))
    }

    /// The driver, for one call. A call that panicked part way left nothing
    /// half-made that the next one cannot cope with, so a poisoned lock is
    /// taken as it stands.
    pub fn lock(&self) -> MutexGuard<'_, Driver> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for something to read without holding the lock while it waits:
    /// a host waiting on the stream on one thread must not hold up the signal
    /// meter or a change of channel on another for the length of the wait.
    pub fn wait_ts_stream(&self, timeout_ms: u32) -> u32 {
        let waiter = {
            let mut driver = self.lock();
            driver.recover();
            driver.waiter()
        };
        wait_on(waiter, timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::signal_is_missing;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use coax::daemon::{Board, Controls, Daemon, Endpoint, GainControl, Held, LnbControl, Tuner};
    use coax_settings::channels::{Channel, Lineup, Route, Space};
    use coax_settings::tuner_settings::{Listed, TunerSettings};

    use super::{Driver, NOT_TUNED, RECOVERY_FIRST, WAIT_TIMEOUT, asked_of, carrier_asks};

    /// The daemon's listing of these tuners, each on a board of its own, with
    /// the controls each lets a channel set.
    fn listed(tuners: Vec<(Tuner, Controls)>) -> Listed {
        let boards = tuners
            .into_iter()
            .map(|(tuner, controls)| Board {
                name: tuner.id.clone(),
                tuners: vec![Held {
                    tuner,
                    controls,
                    ..Held::default()
                }],
                ..Board::default()
            })
            .collect();
        Listed::of(boards, &[])
    }

    fn words(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// A tuner that can switch a dish's supply and has an amplifier.
    fn pt3(id: &str) -> (Tuner, Controls) {
        (
            Tuner {
                id: id.to_owned(),
                deliveries: vec!["ISDB-S".to_owned()],
                ..Tuner::default()
            },
            Controls {
                lnb: Some(LnbControl {
                    levels: vec!["off".to_owned(), "high".to_owned()],
                    ..LnbControl::default()
                }),
                gain: Some(GainControl::default()),
            },
        )
    }

    /// A daemon at an address nothing is listening on, for the tests that are
    /// about the bookkeeping rather than the tune.
    fn nowhere() -> Daemon {
        Daemon::new(Endpoint::Network("127.0.0.1:1".to_owned()))
    }

    fn lineup() -> Lineup {
        Lineup::new(vec![Space {
            name: "地デジ".to_owned(),
            tuners: Vec::new(),
            channels: vec![
                Channel::tuned(
                    "13",
                    vec![
                        ("DELIVERY_SYSTEM".to_owned(), "ISDBT".to_owned()),
                        ("FREQUENCY".to_owned(), "473142857".to_owned()),
                    ],
                ),
                Channel::named("14"),
            ],
        }])
    }

    #[test]
    fn the_enumeration_ends_in_a_null_which_is_how_a_caller_finds_its_length() {
        let driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(!driver.tuning_space_name(0).is_null());
        assert!(driver.tuning_space_name(1).is_null());
        assert!(!driver.channel_name(0, 1).is_null());
        assert!(driver.channel_name(0, 2).is_null());
        assert!(driver.channel_name(1, 0).is_null());
    }

    #[test]
    fn nothing_is_tuned_until_something_is() {
        let driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert_eq!(driver.current_space(), NOT_TUNED);
        assert_eq!(driver.current_channel(), NOT_TUNED);
    }

    #[test]
    fn a_channel_the_lineup_does_not_have_is_refused_rather_than_remembered() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(!driver.set_channel(0, 2));
        assert_eq!(driver.current_space(), NOT_TUNED);
    }

    /// A band plan entry nothing has worked out how to tune is refused rather
    /// than sent to the daemon as an empty request.
    #[test]
    fn a_channel_with_nothing_known_about_it_is_refused() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(!driver.set_channel(0, 1));
        assert_eq!(driver.current_space(), NOT_TUNED);
    }

    #[test]
    fn a_daemon_that_cannot_be_reached_is_a_tuner_that_did_not_open() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(!driver.open_tuner());
        assert!(!driver.is_open());
    }

    /// The whole path, against a daemon that is only a socket: the request goes
    /// out, the chunked answer comes back, and the host takes it a piece at a
    /// time.
    #[test]
    fn a_tuned_channel_hands_the_stream_to_the_host() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port is free");
        let address = listener
            .local_addr()
            .expect("the port is known")
            .to_string();
        let asked = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the driver connects");
            // Read to the blank line that ends the headers: one read can come
            // back with only part of the request.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match connection.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => request.push(byte[0]),
                }
            }
            // One whole transport packet: a host is handed packets, not bytes,
            // so anything shorter would be held back rather than passed on.
            let mut packet = vec![0x47u8; 188];
            packet[1..4].copy_from_slice(b"abc");
            let mut answer =
                b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\n\r\nbc\r\n"
                    .to_vec();
            answer.extend_from_slice(&packet);
            answer.extend_from_slice(b"\r\n0\r\n\r\n");
            connection.write_all(&answer).expect("the answer goes out");
            connection.flush().expect("the answer goes out");
            String::from_utf8_lossy(&request).into_owned()
        });

        let mut driver = Driver::new(
            lineup(),
            "coax",
            Daemon::new(Endpoint::Network(address)),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(driver.set_channel(0, 0), "the channel is tuned");
        assert_eq!(driver.current_space(), 0);
        assert_eq!(driver.current_channel(), 0);

        assert_eq!(driver.wait_ts_stream(500), 0, "something arrived");
        let handed = driver.take_stream().expect("the packet is handed over");
        assert_eq!(handed.len(), 188, "a host is handed whole packets");
        assert_eq!(&handed[0..4], b"Gabc");

        let request = asked.join().expect("the fake daemon did not fail");
        assert!(request.starts_with("GET /stream?"), "asked for {request}");
        assert!(
            request.contains("DELIVERY_SYSTEM=ISDBT"),
            "asked for {request}"
        );
        assert!(
            request.contains("FREQUENCY=473142857"),
            "asked for {request}"
        );
    }

    /// The stream arrives in the pieces the socket happened to break it into,
    /// so what is left over at the end of one is usually a part of a packet --
    /// which is not something to read: a host told otherwise is handed nothing
    /// and asks again without pausing.
    #[test]
    fn a_part_of_a_packet_is_not_reported_as_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port is free");
        let address = listener
            .local_addr()
            .expect("the port is known")
            .to_string();
        let served = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("the driver connects");
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match connection.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => request.push(byte[0]),
                }
            }
            // A packet and a hundred bytes of the next one, which is how a
            // read off a socket in the middle of a stream comes back.
            let mut body = vec![0x47u8; 188];
            body.extend_from_slice(&[0x47u8; 100]);
            let mut answer =
                b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\n\r\n"
                    .to_vec();
            // One chunk and no ending: a stream goes on, and a body that ended
            // would be a stream that ended.
            answer.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            answer.extend_from_slice(&body);
            answer.extend_from_slice(b"\r\n");
            let _ = connection.write_all(&answer);
            let _ = connection.flush();
            // Held open: a closed connection would end the stream, and what is
            // under test is what a live one with a part of a packet in it says.
            thread::sleep(Duration::from_secs(2));
        });

        let mut driver = Driver::new(
            lineup(),
            "coax",
            Daemon::new(Endpoint::Network(address)),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(driver.set_channel(0, 0), "the channel is tuned");
        assert_eq!(driver.wait_ts_stream(500), 0, "the packet arrived");
        assert_eq!(
            driver
                .take_stream()
                .expect("the packet is handed over")
                .len(),
            188
        );
        assert_eq!(driver.ready_count(), 0, "what is left is not a packet");
        let started = std::time::Instant::now();
        assert_eq!(
            driver.wait_ts_stream(200),
            WAIT_TIMEOUT,
            "a part of a packet is not something to read"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "the wait is honoured rather than answered at once"
        );
        drop(driver);
        let _ = served.join();
    }

    /// With nothing being received, the wait is honoured rather than answered
    /// at once: a host told "nothing yet" without delay asks again immediately.
    #[test]
    fn an_untuned_driver_makes_the_host_wait() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        let started = std::time::Instant::now();
        assert_eq!(driver.wait_ts_stream(50), WAIT_TIMEOUT);
        assert!(started.elapsed() >= Duration::from_millis(40));
    }

    /// A space names its tuners by pattern, and the free ones are asked first:
    /// one someone else holds is refused.
    #[test]
    fn the_free_tuners_of_a_space_are_asked_first() {
        let mut lineup = lineup();
        let mut spaces = lineup.spaces().to_vec();
        spaces[0].tuners = vec!["tbs:*".to_owned()];
        lineup = Lineup::new(spaces);

        let mut driver = Driver::new(
            lineup,
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        driver.listed = listed(vec![
            (
                Tuner {
                    id: "tbs:abc:0".to_owned(),
                    model: String::new(),
                    name: String::new(),
                    deliveries: vec!["ISDBT".to_owned()],
                    leased: true,
                },
                Controls::default(),
            ),
            (
                Tuner {
                    id: "tbs:abc:1".to_owned(),
                    model: String::new(),
                    name: String::new(),
                    deliveries: vec!["ISDBT".to_owned()],
                    leased: false,
                },
                Controls::default(),
            ),
            // Neither the right lead nor the right delivery system.
            (
                Tuner {
                    id: "pt3:xyz:0".to_owned(),
                    model: String::new(),
                    name: String::new(),
                    deliveries: vec!["ISDBT".to_owned()],
                    leased: false,
                },
                Controls::default(),
            ),
            (
                Tuner {
                    id: "tbs:abc:2".to_owned(),
                    model: String::new(),
                    name: String::new(),
                    deliveries: vec!["ISDBS".to_owned()],
                    leased: false,
                },
                Controls::default(),
            ),
        ]);
        assert_eq!(
            driver.candidates(0, &Route::default(), "ISDBT"),
            vec![Some("tbs:abc:1".to_owned()), Some("tbs:abc:0".to_owned())]
        );
        // A way that names its own tuners is received on those instead: the
        // stream came down another lead than the rest of its space.
        let cable = Route {
            tuners: vec!["pt3:*".to_owned()],
            ..Route::default()
        };
        assert_eq!(
            driver.candidates(0, &cable, "ISDBT"),
            vec![Some("pt3:xyz:0".to_owned())]
        );
    }

    /// A space that names no tuner leaves the choice to the daemon, which is
    /// what a machine with one lead wants.
    #[test]
    fn a_space_with_no_tuner_named_asks_for_none() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert_eq!(driver.candidates(0, &Route::default(), "ISDBT"), vec![None]);
    }

    /// A refusal about the signal ends the round of tuners; one about a tuner
    /// moves on to the next.
    #[test]
    fn a_channel_nothing_is_transmitting_on_is_not_asked_of_every_tuner() {
        assert!(signal_is_missing(
            "tbs:abc:0: the daemon refused the channel: 409 Conflict {\"code\":\"no_lock\"}"
        ));
        assert!(signal_is_missing("{\"code\":\"no_sync\"}"));
        assert!(!signal_is_missing("{\"code\":\"tuner_busy\"}"));
        assert!(!signal_is_missing("{\"code\":\"device_gone\"}"));
    }

    /// A tune goes to its tuner with what that tuner's setting adds, and only
    /// what the tuner can set: the supply on a satellite tune, nothing a tuner
    /// with no controls would refuse.
    #[test]
    fn a_tuner_is_asked_with_its_own_setting() {
        let listed = listed(vec![
            pt3("pt3:a:0"),
            (
                Tuner {
                    id: "bda:1:0".to_owned(),
                    ..Tuner::default()
                },
                Controls::default(),
            ),
        ]);
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:*", "lnb=high, gain=auto");
        settings.add_line("bda:*", "lnb=high");
        let channel = words(&[
            ("DELIVERY_SYSTEM", "ISDBS"),
            ("FREQUENCY", "1049480"),
            ("STREAM_ID", "16625"),
        ]);
        assert_eq!(
            asked_of(&listed, &settings, Some("pt3:a:0"), &channel),
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS"),
                ("FREQUENCY", "1049480"),
                ("STREAM_ID", "16625"),
                ("tuner", "pt3:a:0"),
                ("lnb_power", "high"),
                ("gain", "auto"),
            ])
        );
        // A BDA tuner lists no controls, and is asked for the channel alone.
        assert_eq!(
            asked_of(&listed, &settings, Some("bda:1:0"), &channel),
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS"),
                ("FREQUENCY", "1049480"),
                ("STREAM_ID", "16625"),
                ("tuner", "bda:1:0"),
            ])
        );
        // The supply is a satellite's: a terrestrial tune of the same tuner
        // takes the gain only.
        let air = words(&[("DELIVERY_SYSTEM", "ISDBT"), ("FREQUENCY", "473142857")]);
        assert_eq!(
            asked_of(&listed, &settings, Some("pt3:a:0"), &air),
            words(&[
                ("DELIVERY_SYSTEM", "ISDBT"),
                ("FREQUENCY", "473142857"),
                ("tuner", "pt3:a:0"),
                ("gain", "auto"),
            ])
        );
    }

    /// Each carrier of a split frame goes to its own tuner with that tuner's
    /// setting, and a carrier with no tuner left for it is asked for unnamed
    /// and with nothing added.
    #[test]
    fn each_carrier_of_a_split_frame_takes_its_own_tuners_setting() {
        let listed = listed(vec![pt3("tbs:a:0"), pt3("tbs:a:1")]);
        let mut settings = TunerSettings::default();
        settings.add_line("tbs:a:1", "gain=max");
        let channel = words(&[
            ("DELIVERY_SYSTEM", "ISDBC"),
            ("FREQUENCY", "153000000"),
            ("tuner", "tbs:a:0"),
        ]);
        let carriers = [
            "153000000".to_owned(),
            "159000000".to_owned(),
            "165000000".to_owned(),
        ];
        let tuners = ["tbs:a:0".to_owned(), "tbs:a:1".to_owned()];
        let asks = carrier_asks(&listed, &settings, &channel, &carriers, &tuners);
        assert_eq!(
            asks,
            vec![
                words(&[
                    ("DELIVERY_SYSTEM", "ISDBC"),
                    ("FREQUENCY", "153000000"),
                    ("tuner", "tbs:a:0"),
                ]),
                words(&[
                    ("DELIVERY_SYSTEM", "ISDBC"),
                    ("FREQUENCY", "159000000"),
                    ("tuner", "tbs:a:1"),
                    ("gain", "max"),
                ]),
                words(&[("DELIVERY_SYSTEM", "ISDBC"), ("FREQUENCY", "165000000")]),
            ]
        );
    }

    /// A setting has to know which tuner it goes to, so a space that names no
    /// tuner names every tuner that receives the system -- the free ones first
    /// -- rather than leaving the choice to the daemon.
    #[test]
    fn a_tuner_is_named_where_a_setting_would_otherwise_go_nowhere() {
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:*", "lnb=high");
        let mut driver = Driver::new(lineup(), "coax", nowhere(), None, Vec::new(), settings);
        let (mut busy, controls) = pt3("pt3:a:0");
        busy.leased = true;
        driver.listed = listed(vec![
            (busy, controls),
            pt3("pt3:a:1"),
            (
                Tuner {
                    id: "pt3:a:2".to_owned(),
                    deliveries: vec!["ISDB-T".to_owned()],
                    ..Tuner::default()
                },
                Controls::default(),
            ),
        ]);
        assert_eq!(
            driver.candidates(0, &Route::default(), "ISDBS"),
            vec![Some("pt3:a:1".to_owned()), Some("pt3:a:0".to_owned())]
        );
    }

    #[test]
    fn closing_forgets_where_the_tuner_was() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        driver.close_tuner();
        assert!(!driver.is_open());
        assert_eq!(driver.current_space(), NOT_TUNED);
    }

    /// A daemon that answers every request and ends each stream after one
    /// packet, numbered by which stream it is. Its one tuner is named after
    /// how many streams have been asked for, as a board plugged into another
    /// socket each time would be. Hands back the stream requests once
    /// `streams` have been made or the time is up.
    fn ending_daemon(streams: u8) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port is free");
        let address = listener
            .local_addr()
            .expect("the port is known")
            .to_string();
        listener
            .set_nonblocking(true)
            .expect("the listener can be polled");
        let served = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut asked = Vec::new();
            while asked.len() < usize::from(streams) && Instant::now() < deadline {
                let Ok((mut connection, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                connection
                    .set_nonblocking(false)
                    .expect("the connection blocks");
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    match connection.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => request.push(byte[0]),
                    }
                }
                if request.starts_with(b"GET /stream?") {
                    asked.push(String::from_utf8_lossy(&request).into_owned());
                    let mut packet = vec![0x47u8; 188];
                    packet[4] = asked.len() as u8;
                    let mut answer = b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\n\r\nbc\r\n".to_vec();
                    answer.extend_from_slice(&packet);
                    answer.extend_from_slice(b"\r\n0\r\n\r\n");
                    let _ = connection.write_all(&answer);
                } else if request.starts_with(b"GET /tuners ") {
                    let listing = format!(
                        r#"[{{"name": "zd:p{0}", "backend": "zd", "tuners": [{{"id": "zd:p{0}:0", "deliveries": ["ISDB-T"]}}]}}]"#,
                        asked.len() + 1
                    );
                    let _ = connection.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{listing}",
                            listing.len()
                        )
                        .as_bytes(),
                    );
                } else {
                    let _ = connection.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\ntest",
                    );
                }
            }
            asked
        });
        (address, served)
    }

    /// Reads until a packet arrives or the time is up, as a host would.
    fn next_packet(driver: &mut Driver, within: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            driver.wait_ts_stream(50);
            if let Some(taken) = driver.take_stream() {
                return Some(taken.to_vec());
            }
        }
        None
    }

    /// A stream that ends without the host asking -- the daemon stopped, the
    /// board was pulled out -- is tuned again on the same channel once what
    /// it sent has been read.
    #[test]
    fn an_ended_stream_is_tuned_again() {
        let (address, served) = ending_daemon(2);
        let mut driver = Driver::new(
            lineup(),
            "coax",
            Daemon::new(Endpoint::Network(address)),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(driver.set_channel(0, 0));
        let first = next_packet(&mut driver, Duration::from_secs(2)).expect("the first stream");
        assert_eq!(first[4], 1);
        let second = next_packet(&mut driver, Duration::from_secs(3)).expect("the stream is back");
        assert_eq!(second[4], 2);
        assert_eq!(driver.current_channel(), 0);
        assert_eq!(
            served.join().expect("the fake daemon did not fail").len(),
            2
        );
    }

    /// A host that closed the tuner wanted the stream to end.
    #[test]
    fn a_closed_tuner_is_not_tuned_again() {
        let (address, served) = ending_daemon(2);
        let mut driver = Driver::new(
            lineup(),
            "coax",
            Daemon::new(Endpoint::Network(address)),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(driver.set_channel(0, 0));
        assert!(next_packet(&mut driver, Duration::from_secs(2)).is_some());
        driver.close_tuner();
        assert!(next_packet(&mut driver, Duration::from_millis(500)).is_none());
        assert_eq!(
            served.join().expect("the fake daemon did not fail").len(),
            1
        );
    }

    /// A channel that cannot be had back is not asked for again until the
    /// wait is over, and the wait grows.
    #[test]
    fn a_failed_recovery_waits_before_the_next() {
        let mut driver = Driver::new(
            lineup(),
            "coax",
            nowhere(),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        driver.tuned = Some((0, 0));
        driver.recover();
        let recovery = driver.recovery.as_ref().expect("still recovering");
        assert!(recovery.at > Instant::now() + Duration::from_secs(1));
        assert_eq!(recovery.wait, RECOVERY_FIRST * 2);
        assert_eq!(
            driver.current_channel(),
            0,
            "the channel is kept for the next try"
        );
    }

    /// A board pulled out and plugged into another socket while the daemon
    /// runs comes back under another name, which the next change of channel
    /// asks for.
    #[test]
    fn each_change_of_channel_asks_for_the_tuners_again() {
        let (address, served) = ending_daemon(2);
        let mut lineup = lineup();
        lineup = Lineup::new(
            lineup
                .spaces()
                .iter()
                .cloned()
                .map(|space| Space {
                    tuners: vec!["zd:*".to_owned()],
                    ..space
                })
                .collect(),
        );
        let mut driver = Driver::new(
            lineup,
            "coax",
            Daemon::new(Endpoint::Network(address)),
            None,
            Vec::new(),
            TunerSettings::default(),
        );
        assert!(driver.set_channel(0, 0));
        assert!(next_packet(&mut driver, Duration::from_secs(2)).is_some());
        assert!(driver.set_channel(0, 0));
        let asked = served.join().expect("the fake daemon did not fail");
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains("tuner=zd%3Ap1%3A0"), "asked {}", asked[0]);
        assert!(asked[1].contains("tuner=zd%3Ap2%3A0"), "asked {}", asked[1]);
    }
}

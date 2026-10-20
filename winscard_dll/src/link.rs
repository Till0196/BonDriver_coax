// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The connection to `coaxd`'s cards.
//!
//! This library is loaded into a viewer's television software, which rules out
//! depending on anything else being run. So it reaches the daemon itself, over
//! the same HTTP the driver beside it uses.
//!
//! A `Link` is a conversation with the daemon, cards are named by numbers this
//! side hands out, and every call answers with a `SCARD_*` code. The daemon
//! holds a card against the connection it was opened on, so an open card here
//! owns a connection of its own and the daemon has nothing to remember.

// Most of this is called by the exported calls, which are built only where they
// mean anything. The tests at the foot of the file are what it is built for
// elsewhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use coax::card::{Cards, Held, Refusal, SlotState};
use coax::daemon::{Daemon, Endpoint};
use coax::launch::Launcher;

use crate::abi::{
    SCARD_E_INVALID_HANDLE, SCARD_E_INVALID_PARAMETER, SCARD_E_NO_SERVICE, SCARD_E_UNKNOWN_READER,
    SCARD_PROTOCOL_T1, SCARD_S_SUCCESS,
};
use crate::wire::{self, ReaderState};

/// The attributes answered here, from `reader.h`. Now that the open reports
/// what they hold, they are answered without asking the daemon.
mod attr {
    pub const VENDOR_NAME: u32 = 0x0001_0100;
    pub const VENDOR_IFD_TYPE: u32 = 0x0001_0101;
    pub const MAX_INPUT: u32 = 0x0007_A007;
    pub const CURRENT_PROTOCOL: u32 = 0x0008_0201;
    pub const ATR_STRING: u32 = 0x0009_0303;
    pub const DEVICE_FRIENDLY_A: u32 = 0x7FFF_0003;
    pub const DEVICE_SYSTEM_A: u32 = 0x7FFF_0004;
    pub const DEVICE_FRIENDLY_W: u32 = 0x7FFF_0005;
    pub const DEVICE_SYSTEM_W: u32 = 0x7FFF_0006;
}

/// Where the daemon is.
///
/// The settings file beside this library is read first, because a machine whose
/// daemon is elsewhere said so there and should not have to say it twice.
///
/// That file is `winscard.ini` -- this library's own name with `.ini` for an
/// extension, the same rule every library here follows. It has one setting
/// worth writing, `Host`, and it is a file of its own because an installation
/// holds a settings file per driver and a card is not on a lead.
///
/// No file is the ordinary case: a daemon nobody configured is on the socket
/// both sides work out the same way. After the file, the environment --
/// `COAX_HOST`, then `COAXD_SOCKET` -- and then that socket. Same variables and
/// order as `coax` reads them.
pub fn endpoint() -> Endpoint {
    coax_settings::name_agent();
    Endpoint::parse(&settings().host)
}

/// What `winscard.ini` says, or what a file that is not there would have said.
/// A missing file is the ordinary case and means the defaults, which is why
/// this never fails.
fn settings() -> coax_settings::config::Config {
    coax_settings::beside::read_settings()
        .map(|text| coax_settings::config::Config::parse(&text))
        .unwrap_or_default()
}

/// Where to look for a daemon to start: what the file names, else the directory
/// this library was unpacked into. The same order the driver and `coax` use.
fn launcher(settings: &coax_settings::config::Config) -> Launcher {
    Launcher {
        named: (!settings.daemon.is_empty()).then(|| PathBuf::from(&settings.daemon)),
        beside: coax_settings::beside::own_directory(),
    }
}

/// Whether starting a daemon is worth trying at all: the settings allow it and
/// there is something to start.
///
/// Asked before an application is told there is no smart card service, and
/// cheap enough to ask there. The answer decides between two honest ones:
/// "there is no service", and a context that will have readers on it once the
/// daemon is up.
pub fn can_start() -> bool {
    let settings = settings();
    settings.start_daemon && launcher(&settings).find().is_some()
}

/// Starts a daemon and waits for it to answer, for the machine that reads cards
/// without ever opening a tuner.
///
/// The driver beside this library starts one too, and a television host has
/// both in one process. Neither waits for the other and neither has to: only
/// one of them is allowed to start a daemon at a time -- see
/// `coax::launch::Claim`.
///
/// A machine whose daemon is somewhere else is left alone: a daemon started
/// here would listen on this machine's own socket and never answer the address
/// that was asked for.
pub fn start(where_it_is: &Endpoint) -> bool {
    let settings = settings();
    if !settings.start_daemon {
        return false;
    }
    if !matches!(where_it_is, Endpoint::Socket(_)) {
        return false;
    }
    let daemon = Daemon::new(where_it_is.clone());
    match daemon.ensure_running(&launcher(&settings)) {
        Ok(version) => {
            coax_settings::trace!("winscard: started a daemon, {version}");
            true
        }
        Err(reason) => {
            coax_settings::trace!("winscard: no daemon at {where_it_is:?}: {reason}");
            false
        }
    }
}

/// Whether a daemon answers there now. Making a context asks the daemon
/// nothing, so a context made says nothing about whether one is listening.
pub fn answering(where_it_is: &Endpoint) -> bool {
    Daemon::new(where_it_is.clone()).version().is_ok()
}

/// Whether it is worth trying at all. A socket that is not there means no
/// daemon, and saying so without dialling keeps `SCardEstablishContext` from
/// pausing a television's startup. An address cannot be checked this cheaply,
/// so one is always worth a try.
pub fn reachable(endpoint: &Endpoint) -> bool {
    match endpoint {
        Endpoint::Socket(path) => std::path::Path::new(path).exists(),
        Endpoint::Network(_) => true,
    }
}

/// A conversation with the daemon's cards.
#[derive(Debug)]
pub struct Link {
    cards: Cards,
    /// The cards open on this link, by the number this side gave each. The
    /// numbers are ours: the daemon names a card by the connection it is on.
    open: BTreeMap<i32, Held>,
    next: i32,
}

impl Link {
    pub fn open(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(Self {
            cards: Cards::new(endpoint.clone()),
            open: BTreeMap::new(),
            next: 1,
        })
    }

    /// A context costs the daemon nothing, so one is made here rather than
    /// asked for. What an application's context stands for is bookkeeping this
    /// side already does.
    pub fn establish_context(&mut self, _scope: u32) -> io::Result<(u32, u32)> {
        Ok((1, SCARD_S_SUCCESS as u32))
    }

    /// Lets go of everything still open, which is what an application that
    /// released its context without closing its cards means.
    pub fn release_context(&mut self, _context: u32) -> io::Result<u32> {
        for (_, mut card) in std::mem::take(&mut self.open) {
            let _ = card.close("leave");
        }
        Ok(SCARD_S_SUCCESS as u32)
    }

    /// The readers and what is known about them.
    ///
    /// The listing is asked for; the card state comes from the table, which a
    /// running wait keeps fresh (see [`Watch`]). Reading a slot means opening
    /// the board a tuner streams from, so a slot nothing has looked at is
    /// published as unmeasured, not as empty.
    pub fn readers(&mut self) -> io::Result<Vec<ReaderState>> {
        let names = table().names(&self.cards)?;
        Ok(names
            .into_iter()
            .map(|name| {
                let seen = table().state(&name);
                ReaderState {
                    events: seen.as_ref().map_or(0, |one| one.events),
                    state: seen.as_ref().map_or(0, flags_for),
                    sharing: seen.as_ref().map_or(0, |one| one.sharing),
                    atr: seen.as_ref().map(|one| one.atr.clone()).unwrap_or_default(),
                    protocol: if seen.as_ref().is_some_and(|one| one.present) {
                        SCARD_PROTOCOL_T1
                    } else {
                        0
                    },
                    name,
                }
            })
            .collect())
    }

    pub fn connect_card(
        &mut self,
        _context: u32,
        reader: &str,
        share: u32,
        protocols: u32,
    ) -> io::Result<(i32, u32, u32)> {
        const SHARE_EXCLUSIVE: u32 = 1;
        const SHARE_SHARED: u32 = 2;
        if share != SHARE_EXCLUSIVE && share != SHARE_SHARED {
            return Ok((0, 0, SCARD_E_INVALID_PARAMETER as u32));
        }
        if protocols & SCARD_PROTOCOL_T1 == 0 {
            return Ok((0, 0, SCARD_E_INVALID_PARAMETER as u32));
        }
        match self.cards.open(reader, WAIT_SECONDS) {
            Ok(held) => {
                let number = self.next;
                self.next += 1;
                self.open.insert(number, held);
                // A slot that has just answered an open is a slot that has been
                // looked at, so the table is told rather than left to find out
                // on its own schedule.
                table().refresh(&self.cards, reader);
                Ok((number, SCARD_PROTOCOL_T1, SCARD_S_SUCCESS as u32))
            }
            Err(Refusal::Broken(reason)) => Err(reason),
            Err(refused) => Ok((0, 0, status_for(&refused))),
        }
    }

    /// A reconnect is a disconnect with the initialization applied and an open
    /// after it, because the connection is the claim: a fresh card means a
    /// fresh connection.
    pub fn reconnect(
        &mut self,
        card: i32,
        share: u32,
        protocols: u32,
        initialization: u32,
    ) -> io::Result<(u32, u32)> {
        let Some(mut held) = self.open.remove(&card) else {
            return Ok((0, SCARD_E_INVALID_HANDLE as u32));
        };
        let reader = held.reader().to_owned();
        let _ = held.close(disposition_name(initialization));
        drop(held);

        match self.connect_card(0, &reader, share, protocols)? {
            (number, protocol, result) if result == SCARD_S_SUCCESS as u32 => {
                // The application still knows this card by its old number, so
                // the new connection is filed under it.
                if let Some(reopened) = self.open.remove(&number) {
                    self.open.insert(card, reopened);
                }
                Ok((protocol, result))
            }
            (_, _, result) => Ok((0, result)),
        }
    }

    pub fn disconnect(&mut self, card: i32, disposition: u32) -> io::Result<u32> {
        let Some(mut held) = self.open.remove(&card) else {
            return Ok(SCARD_E_INVALID_HANDLE as u32);
        };
        match held.close(disposition_name(disposition)) {
            Ok(()) => Ok(SCARD_S_SUCCESS as u32),
            Err(Refusal::Broken(reason)) => Err(reason),
            Err(refused) => Ok(status_for(&refused)),
        }
    }

    pub fn begin_transaction(&mut self, card: i32) -> io::Result<u32> {
        self.on(card, |held| held.begin_transaction())
    }

    pub fn end_transaction(&mut self, card: i32, disposition: u32) -> io::Result<u32> {
        let name = disposition_name(disposition);
        self.on(card, |held| held.end_transaction(name))
    }

    /// Asks after the card, which is also what makes the daemon look: the state
    /// the table reports is a memory of the last look until something asks for
    /// a new one.
    pub fn status(&mut self, card: i32) -> io::Result<u32> {
        let Some(held) = self.open.get(&card) else {
            return Ok(SCARD_E_INVALID_HANDLE as u32);
        };
        let reader = held.reader().to_owned();
        table().refresh(&self.cards, &reader);
        Ok(SCARD_S_SUCCESS as u32)
    }

    /// One command APDU.
    pub fn transmit(
        &mut self,
        card: i32,
        protocol: u32,
        command: &[u8],
        _room: u32,
    ) -> io::Result<(u32, u32, Vec<u8>)> {
        if protocol != SCARD_PROTOCOL_T1 {
            return Ok((SCARD_E_INVALID_PARAMETER as u32, 0, Vec::new()));
        }
        let Some(held) = self.open.get_mut(&card) else {
            return Ok((SCARD_E_INVALID_HANDLE as u32, 0, Vec::new()));
        };
        if held.max_input() > 0 && command.len() > held.max_input() {
            return Ok((SCARD_E_INVALID_PARAMETER as u32, 0, Vec::new()));
        }
        match held.transmit(command) {
            Ok(answer) => Ok((SCARD_S_SUCCESS as u32, SCARD_PROTOCOL_T1, answer)),
            Err(Refusal::Broken(reason)) => {
                self.open.remove(&card);
                Err(reason)
            }
            Err(refused) => Ok((status_for(&refused), 0, Vec::new())),
        }
    }

    /// A reader control code. The card interfaces this reaches have none of
    /// their own, and the daemon publishes none either.
    pub fn control(
        &mut self,
        card: i32,
        _code: u32,
        _command: &[u8],
        _room: u32,
    ) -> io::Result<(u32, Vec<u8>)> {
        if !self.open.contains_key(&card) {
            return Ok((SCARD_E_INVALID_HANDLE as u32, Vec::new()));
        }
        Ok((UNSUPPORTED_FEATURE, Vec::new()))
    }

    /// Answers from what the open already reported. The attributes client
    /// software actually asks for are the answer to reset, the protocol and how
    /// long a command may be, and all three came back when the card was opened.
    pub fn get_attrib(&mut self, card: i32, attribute: u32) -> io::Result<(u32, Vec<u8>)> {
        let Some(held) = self.open.get(&card) else {
            return Ok((SCARD_E_INVALID_HANDLE as u32, Vec::new()));
        };
        let value = match attribute {
            attr::ATR_STRING => held.atr().to_vec(),
            attr::CURRENT_PROTOCOL => SCARD_PROTOCOL_T1.to_le_bytes().to_vec(),
            attr::MAX_INPUT if held.max_input() > 0 => {
                (held.max_input() as u32).to_le_bytes().to_vec()
            }
            attr::VENDOR_NAME => text("coax"),
            attr::VENDOR_IFD_TYPE => text("coaxd"),
            attr::DEVICE_FRIENDLY_A | attr::DEVICE_SYSTEM_A => text(held.reader()),
            attr::DEVICE_FRIENDLY_W | attr::DEVICE_SYSTEM_W => wide(held.reader()),
            _ => return Ok((UNSUPPORTED_FEATURE, Vec::new())),
        };
        Ok((SCARD_S_SUCCESS as u32, value))
    }

    /// Writes nothing: none of these attributes is this side's to change.
    pub fn set_attrib(&mut self, card: i32, _attribute: u32, _value: &[u8]) -> io::Result<u32> {
        if !self.open.contains_key(&card) {
            return Ok(SCARD_E_INVALID_HANDLE as u32);
        }
        Ok(UNSUPPORTED_FEATURE)
    }

    fn on(
        &mut self,
        card: i32,
        exchange: impl FnOnce(&mut Held) -> Result<(), Refusal>,
    ) -> io::Result<u32> {
        let Some(held) = self.open.get_mut(&card) else {
            return Ok(SCARD_E_INVALID_HANDLE as u32);
        };
        match exchange(held) {
            Ok(()) => Ok(SCARD_S_SUCCESS as u32),
            Err(Refusal::Broken(reason)) => {
                self.open.remove(&card);
                Err(reason)
            }
            Err(refused) => Ok(status_for(&refused)),
        }
    }
}

/// `SCARD_E_UNSUPPORTED_FEATURE`, which the constants in `abi` do not carry
/// because nothing else here answers it.
const UNSUPPORTED_FEATURE: u32 = 0x8010_001F;

/// How long a card request may sit before it is given up on. Long enough for a
/// board that is loading firmware, short enough that a daemon which has stopped
/// answering is noticed.
const WAIT_SECONDS: u32 = 30;

/// What the daemon's refusal means to an application.
fn status_for(refused: &Refusal) -> u32 {
    match refused.code() {
        Some("unknown_tuner") => SCARD_E_UNKNOWN_READER as u32,
        Some("invalid_parameter") => SCARD_E_INVALID_PARAMETER as u32,
        Some("card_error") => SCARD_E_INVALID_HANDLE as u32,
        _ => SCARD_E_NO_SERVICE as u32,
    }
}

/// What the daemon's query parameter calls a disposition.
fn disposition_name(disposition: u32) -> &'static str {
    match disposition {
        1 => "reset",
        2 => "unpower",
        3 => "eject",
        _ => "leave",
    }
}

/// The state bits a measured slot reports, as pcsc-lite writes them. How the
/// slot is being shared is a field of its own; see `flags_of` in `api`, which
/// reads both.
fn flags_for(seen: &SlotState) -> u32 {
    if seen.present {
        wire::STATE_PRESENT | wire::STATE_SPECIFIC
    } else {
        wire::STATE_ABSENT
    }
}

fn text(value: &str) -> Vec<u8> {
    let mut written = value.as_bytes().to_vec();
    written.push(0);
    written
}

/// A string attribute in the two-byte units the W attributes are defined in.
fn wide(value: &str) -> Vec<u8> {
    let mut written = Vec::with_capacity((value.len() + 1) * 2);
    for letter in value.encode_utf16() {
        written.extend_from_slice(&letter.to_le_bytes());
    }
    written.extend_from_slice(&[0, 0]);
    written
}

// ------------------------------------------------------------------- table

/// What is known about the slots, and who is watching them.
///
/// A wait registers here rather than at the daemon. Each slot has one thread
/// holding a request open that the daemon does not answer until the slot
/// moves, so the looking happens once, beside the hardware.
/// `SCardGetStatusChange` reads its answers from what those heard.
///
/// The threads outlive the waits that started them by a little: an
/// application that polls with a short timeout registers and lets go many
/// times a second, and a thread per registration -- each holding a request
/// the daemon may sit on for a minute -- would pile up by the hundred. So one
/// thread per slot, which stays while anybody has waited recently and goes
/// once nobody has.
#[derive(Debug, Default)]
struct Table {
    inner: Mutex<Known>,
    watching: Mutex<Watching>,
}

#[derive(Debug, Default)]
struct Known {
    names: Vec<String>,
    listed: Option<Instant>,
    slots: BTreeMap<String, SlotState>,
}

/// Who is waiting, when somebody last was, and which slots have a thread.
#[derive(Debug, Default)]
struct Watching {
    watchers: usize,
    last_wanted: Option<Instant>,
    running: BTreeSet<String>,
}

impl Watching {
    /// Whether a slot's thread has a reason to go on.
    fn wanted(&self) -> bool {
        self.watchers > 0 || self.last_wanted.is_some_and(|when| when.elapsed() < LINGER)
    }
}

/// How long a slot's thread stays after the last wait let go, so that the
/// next one finds it still there.
const LINGER: Duration = Duration::from_secs(5);

/// How long a listing stands before it is asked for again. The daemon answers
/// it without opening a board, so this is only about not asking five times a
/// second.
const LISTING_STANDS: Duration = Duration::from_secs(1);

/// How long one request asks the daemon to hold a wait open for. The daemon has
/// a ceiling of its own and answers sooner.
const WATCH_SECONDS: u32 = 60;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

impl Table {
    /// The reader names, asking the daemon when what is held has gone stale.
    fn names(&self, cards: &Cards) -> io::Result<Vec<String>> {
        {
            let held = self.inner.lock().map_err(poisoned)?;
            if held
                .listed
                .is_some_and(|when| when.elapsed() < LISTING_STANDS)
            {
                return Ok(held.names.clone());
            }
        }
        let listed = match cards.list() {
            Ok(listed) => listed,
            Err(Refusal::Broken(reason)) => return Err(reason),
            Err(refused) => return Err(io::Error::other(refused.to_string())),
        };
        let names: Vec<String> = listed.into_iter().map(|one| one.reader).collect();
        let mut held = self.inner.lock().map_err(poisoned)?;
        held.names = names.clone();
        held.listed = Some(Instant::now());
        // A slot that has gone is not a slot whose last state still stands.
        held.slots.retain(|name, _| names.contains(name));
        Ok(names)
    }

    fn state(&self, reader: &str) -> Option<SlotState> {
        self.inner.lock().ok()?.slots.get(reader).cloned()
    }

    fn remember(&self, reader: &str, seen: SlotState) {
        if let Ok(mut held) = self.inner.lock() {
            held.slots.insert(reader.to_owned(), seen);
        }
    }

    /// Looks at one slot now, for a caller that has just done something which
    /// makes the answer different.
    fn refresh(&self, cards: &Cards, reader: &str) {
        if let Ok(seen) = cards.events(reader, u32::MAX, 0) {
            self.remember(reader, seen);
        }
    }

    /// Starts watching every slot that has no thread on it yet.
    fn watch(&self, cards: &Cards) {
        {
            let Ok(mut watching) = self.watching.lock() else {
                return;
            };
            watching.watchers += 1;
            watching.last_wanted = Some(Instant::now());
        }
        let names = self.names(cards).unwrap_or_default();
        let Ok(mut watching) = self.watching.lock() else {
            return;
        };
        for name in names {
            if watching.running.contains(&name) {
                continue;
            }
            // The thread may still be waiting on the daemon when the
            // application lets this library go, and nothing the application
            // calls says when that is.
            coax_settings::beside::stay_loaded();
            let cards = cards.clone();
            let reader = name.clone();
            let started = std::thread::Builder::new()
                .name(String::from("coax card watch"))
                .spawn(move || table().watch_one(&cards, &reader));
            if started.is_ok() {
                watching.running.insert(name);
            }
        }
    }

    /// One slot's thread: asks the daemon to say when the slot moves, for as
    /// long as somebody wants to know.
    fn watch_one(&self, cards: &Cards, reader: &str) {
        loop {
            {
                // Decided under the same lock `watch` starts threads under, so
                // a wait arriving as this one leaves either finds it still
                // running or starts another.
                let Ok(mut watching) = self.watching.lock() else {
                    return;
                };
                if !watching.wanted() {
                    watching.running.remove(reader);
                    return;
                }
            }
            let since = self.state(reader).map_or(u32::MAX, |seen| seen.events);
            match cards.events(reader, since, WATCH_SECONDS) {
                Ok(seen) => self.remember(reader, seen),
                Err(_) => {
                    // Nothing answered. Leaving the daemon alone for a moment
                    // keeps one that has gone from being asked in a tight loop.
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    /// Stops watching, when this was the last one. The threads notice on
    /// their own, once they have lingered.
    fn unwatch(&self) {
        if let Ok(mut watching) = self.watching.lock() {
            watching.watchers = watching.watchers.saturating_sub(1);
            watching.last_wanted = Some(Instant::now());
        }
    }
}

fn poisoned(_: impl std::fmt::Debug) -> io::Error {
    io::Error::other("the card table is not usable")
}

/// A registration that keeps the slots being watched. Held for as long as a
/// `SCardGetStatusChange` is running and given back the moment it is done, so
/// that a slot nobody is waiting on costs nothing.
#[derive(Debug)]
pub struct Watch {
    _private: (),
}

impl Watch {
    pub fn open(endpoint: &Endpoint) -> io::Result<Self> {
        table().watch(&Cards::new(endpoint.clone()));
        Ok(Self { _private: () })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        table().unwatch();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use coax::daemon::Endpoint;

    use super::{Link, SCARD_S_SUCCESS};

    /// A daemon that answers the card endpoints the way `coaxd` does, holding a
    /// card against the connection it was opened on. The whole shape rests on
    /// that rule.
    struct Fake {
        path: String,
        reader: String,
        opens: Arc<AtomicUsize>,
        sent: Arc<std::sync::Mutex<Vec<String>>>,
    }

    /// Each test names its slot differently. The table these read through is
    /// one per process, so tests sharing a slot name would see each other's
    /// measurements.
    fn start(reader: &str) -> Fake {
        let directory =
            std::env::temp_dir().join(format!("coax-winscard-{}-{reader}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch directory can be made");
        let path = directory.join("http.sock").to_string_lossy().into_owned();

        let listener = UnixListener::bind(&path).expect("the socket can be bound");
        let opens = Arc::new(AtomicUsize::new(0));
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let counted = Arc::clone(&opens);
        let recorded = Arc::clone(&sent);
        let named = reader.to_owned();
        std::thread::spawn(move || {
            for accepted in listener.incoming() {
                let Ok(connection) = accepted else { break };
                let counted = Arc::clone(&counted);
                let recorded = Arc::clone(&recorded);
                let named = named.clone();
                std::thread::spawn(move || answer(connection, &named, &counted, &recorded));
            }
        });
        Fake {
            path,
            reader: reader.to_owned(),
            opens,
            sent,
        }
    }

    /// One connection, for as long as it carries requests. A card opened on it
    /// is held until it closes, which is the daemon's rule.
    fn answer(
        connection: UnixStream,
        slot: &str,
        opens: &AtomicUsize,
        sent: &std::sync::Mutex<Vec<String>>,
    ) {
        let mut reader = BufReader::new(connection);
        let mut holding = false;
        loop {
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 {
                break;
            }
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 {
                    return;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some(value) = header.strip_prefix("Content-Length: ") {
                    length = value.parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            if length > 0 && reader.read_exact(&mut body).is_err() {
                return;
            }
            let body = String::from_utf8_lossy(&body).into_owned();

            let request = request.trim_end().to_owned();
            let answer = if request.starts_with("GET /cards ") {
                format!(
                    r#"[{{"id":"{slot}","reader":"{slot}","backend":"px4","board":"px4:A@1-2"}}]"#
                )
            } else if request.starts_with(&format!("GET /cards/{slot}/events")) {
                r#"{"events":3,"changed":false,"present":true,"atr":"3B F0 11","sharing":1}"#
                    .to_owned()
            } else if request.starts_with(&format!("POST /cards/{slot}/connect")) {
                if holding {
                    write_answer(
                        reader.get_mut(),
                        409,
                        r#"{"error":{"code":"card_error","message":"already held"}}"#,
                    );
                    continue;
                }
                holding = true;
                opens.fetch_add(1, Ordering::SeqCst);
                r#"{"atr":"3B F0 11","protocol":"T=1","max_input":260}"#.to_owned()
            } else if request.starts_with(&format!("DELETE /cards/{slot}/connect")) {
                let released = holding;
                if holding {
                    opens.fetch_sub(1, Ordering::SeqCst);
                }
                holding = false;
                format!(r#"{{"released":{released}}}"#)
            } else if request.starts_with(&format!("POST /cards/{slot}/transmit")) {
                if !holding {
                    write_answer(
                        reader.get_mut(),
                        409,
                        r#"{"error":{"code":"card_error","message":"no card"}}"#,
                    );
                    continue;
                }
                let apdu = body.split('"').nth(3).unwrap_or_default().to_owned();
                sent.lock().expect("not poisoned").push(apdu);
                r#"{"response":"12 90 00","status":"90 00","protocol":"T=1"}"#.to_owned()
            } else if request.contains(&format!("/cards/{slot}/transaction")) {
                r#"{"released":true}"#.to_owned()
            } else if request.starts_with("POST /cards/missing/connect") {
                write_answer(
                    reader.get_mut(),
                    404,
                    r#"{"error":{"code":"unknown_tuner","message":"no card reader is called missing"}}"#,
                );
                continue;
            } else {
                write_answer(
                    reader.get_mut(),
                    404,
                    r#"{"error":{"code":"unknown","message":"no"}}"#,
                );
                continue;
            };
            write_answer(reader.get_mut(), 200, &answer);
        }
    }

    fn write_answer(connection: &mut UnixStream, status: u16, body: &str) {
        let _ = write!(
            connection,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = connection.flush();
    }

    fn link(fake: &Fake) -> Link {
        Link::open(&Endpoint::Socket(fake.path.clone())).expect("the daemon is reachable")
    }

    /// The point of the whole thing: a run of commands with no reset between
    /// them, reached by a library loaded into a viewer's television and nothing
    /// else running.
    #[test]
    fn a_card_is_held_across_commands() {
        let fake = start("held");
        let mut link = link(&fake);

        let (card, protocol, result) = link
            .connect_card(1, &fake.reader, 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        assert_eq!(result, SCARD_S_SUCCESS as u32);
        assert_eq!(protocol, super::SCARD_PROTOCOL_T1);
        assert_eq!(fake.opens.load(Ordering::SeqCst), 1);

        for command in [&[0x90u8, 0x30, 0, 0, 0][..], &[0x90, 0x32, 0, 0, 0][..]] {
            let (result, _, answer) = link
                .transmit(card, super::SCARD_PROTOCOL_T1, command, 264)
                .expect("the daemon answers");
            assert_eq!(result, SCARD_S_SUCCESS as u32);
            assert_eq!(answer, vec![0x12, 0x90, 0x00]);
        }

        let sent = fake.sent.lock().expect("not poisoned").clone();
        assert_eq!(sent, vec!["90 30 00 00 00", "90 32 00 00 00"]);

        // One card, one connection: the second command did not open another.
        assert_eq!(fake.opens.load(Ordering::SeqCst), 1);

        assert_eq!(
            link.disconnect(card, 0).expect("the daemon answers"),
            SCARD_S_SUCCESS as u32
        );
        assert_eq!(fake.opens.load(Ordering::SeqCst), 0);
    }

    /// Two cards at once are two connections, since a connection is what holds
    /// one. Reusing a single one would have the daemon refuse the second.
    #[test]
    fn two_cards_at_once_take_two_connections() {
        let fake = start("two");
        let mut link = link(&fake);
        let (first, _, result) = link
            .connect_card(1, &fake.reader, 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        assert_eq!(result, SCARD_S_SUCCESS as u32);
        let (second, _, result) = link
            .connect_card(1, &fake.reader, 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        assert_eq!(result, SCARD_S_SUCCESS as u32);
        assert_ne!(first, second);
        assert_eq!(fake.opens.load(Ordering::SeqCst), 2);
    }

    /// Releasing the context lets go of what was opened under it, so an
    /// application that forgot a card leaves none held.
    #[test]
    fn releasing_the_context_lets_go_of_its_cards() {
        let fake = start("released");
        let mut link = link(&fake);
        link.connect_card(1, &fake.reader, 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        link.release_context(1).expect("the daemon answers");
        assert_eq!(fake.opens.load(Ordering::SeqCst), 0);
    }

    /// A slot nothing is called is the caller's mistake, told apart from a
    /// card that would not answer.
    #[test]
    fn an_unknown_slot_is_reported_as_one() {
        let fake = start("known");
        let mut link = link(&fake);
        let (_, _, result) = link
            .connect_card(1, "missing", 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        assert_eq!(result, super::SCARD_E_UNKNOWN_READER as u32);
    }

    /// The listing costs a board nothing, so a slot nothing has looked at
    /// comes back unmeasured rather than empty.
    #[test]
    fn a_listing_does_not_claim_to_have_looked() {
        let fake = start("unlooked");
        let mut link = link(&fake);
        let readers = link.readers().expect("the daemon answers");
        assert_eq!(readers.len(), 1);
        assert_eq!(readers[0].name, fake.reader);
        assert!(!readers[0].measured());
    }

    /// Attributes are answered from what the open reported, without asking the
    /// daemon again.
    #[test]
    fn the_attributes_come_from_the_open() {
        let fake = start("attributes");
        let mut link = link(&fake);
        let (card, _, _) = link
            .connect_card(1, &fake.reader, 2, super::SCARD_PROTOCOL_T1)
            .expect("the daemon answers");
        let (result, atr) = link
            .get_attrib(card, super::attr::ATR_STRING)
            .expect("answered");
        assert_eq!(result, SCARD_S_SUCCESS as u32);
        assert_eq!(atr, vec![0x3B, 0xF0, 0x11]);
        let (result, room) = link
            .get_attrib(card, super::attr::MAX_INPUT)
            .expect("answered");
        assert_eq!(result, SCARD_S_SUCCESS as u32);
        assert_eq!(
            u32::from_le_bytes(room.try_into().expect("four bytes")),
            260
        );
    }
}

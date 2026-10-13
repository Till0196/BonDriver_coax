// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The handles this library hands out, and what is behind them.
//!
//! Every entry point takes a handle and has to decide, from the handle alone,
//! whether it belongs to the built-in slot or to the system's own resource
//! manager. The table below is that decision. The values carry a mark as well,
//! so that a handle from the other side is never looked up as one of ours: the
//! mark sits above the address space a process of this width can hold.

use std::alloc::Layout;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::abi::Handle;
use coax::daemon::Endpoint;

use crate::link::{self, Link};
use crate::wire::ReaderState;

/// The mark on the handles this library issues.
const MARK: Handle = 0xC0A5 << (Handle::BITS - 16);
const MARK_MASK: Handle = 0xFFFF << (Handle::BITS - 16);

/// Whether a handle is one of ours to begin with. A handle that is not is not
/// looked up at all, which is what keeps a system handle from colliding with
/// an entry of the table.
pub fn marked(handle: Handle) -> bool {
    handle & MARK_MASK == MARK
}

/// How long to leave the daemon alone after a connection was refused. It bounds
/// the starting as well as the dialling: a machine with no daemon to start
/// refuses in a moment and would otherwise be asked again on every call.
const TRY_AGAIN: Duration = Duration::from_secs(1);

/// The daemon's side of a context: the connection and the number the daemon
/// knows the context by. The two go together because the number belongs to
/// the connection -- a new connection is at nobody's count.
#[derive(Debug)]
struct Session {
    link: Link,
    remote: u32,
}

/// What a call about a card gets back.
#[derive(Debug)]
pub enum CardCall<T> {
    Done(T),
    /// The connection this card was opened on has gone, so the number naming
    /// it means nothing now.
    Gone,
    /// Nothing is answering at all.
    NoService,
}

/// One context: a connection to the daemon and, beside it, a context on the
/// system's own resource manager, because a single application context has to
/// answer for the readers of both.
#[derive(Debug)]
pub struct Context {
    daemon: Mutex<Attempt>,
    /// The system's context, or zero when there is no smart card service.
    pub system: Handle,
    /// Set by `SCardCancel` and cleared by the wait it ends.
    cancelled: AtomicBool,
}

#[derive(Debug, Default)]
struct Attempt {
    session: Option<Session>,
    /// When the last connection was tried, so that a daemon that is not there
    /// is not dialled on every call.
    last: Option<Instant>,
    /// Counts the connections made. A card opened at one generation and used
    /// at another names a conversation that is over.
    generation: u64,
}

impl Attempt {
    /// Makes sure there is a session, unless one was tried for too recently.
    fn ready(&mut self, scope: u32) -> Option<&mut Session> {
        if self.session.is_none() {
            if self.last.is_some_and(|last| last.elapsed() < TRY_AGAIN) {
                return None;
            }
            self.last = Some(Instant::now());
            self.session = open(scope);
            if self.session.is_some() {
                self.generation += 1;
            }
        }
        self.session.as_mut()
    }
}

/// Opens a connection and takes a context on it, starting a daemon if nothing
/// is answering.
///
/// The driver beside this library starts one when the viewer opens a tuner, and
/// where both are installed that is usually what happens first. But a machine
/// that decodes a stream recorded elsewhere has the card and no tuner at all.
///
/// Waiting happens here rather than at `SCardEstablishContext`, which is called
/// before an application has asked for anything and should be quick.
///
/// The daemon is asked before the connection is taken as made. Making a context
/// talks to nobody, so without asking, a daemon the driver has only just started
/// -- it opens its socket once it has looked at every board -- would be found
/// missing at the first real question, and the viewer told its reader does not
/// exist. `start` also waits for a daemon somebody else is starting.
fn open(scope: u32) -> Option<Session> {
    let where_it_is = endpoint();
    if !link::answering(where_it_is) && !link::start(where_it_is) {
        return None;
    }
    connect(where_it_is, scope)
}

fn connect(where_it_is: &Endpoint, scope: u32) -> Option<Session> {
    let mut link = match Link::open(where_it_is) {
        Ok(link) => link,
        Err(reason) => {
            coax_settings::trace!("winscard: {where_it_is:?} could not be reached: {reason}");
            return None;
        }
    };
    match link.establish_context(scope) {
        Ok((remote, 0)) => Some(Session { link, remote }),
        _ => None,
    }
}

impl Context {
    pub fn new(system: Handle) -> Self {
        Self {
            daemon: Mutex::new(Attempt::default()),
            system,
            cancelled: AtomicBool::new(false),
        }
    }

    /// Runs one exchange with the daemon, connecting if this is the first call
    /// or if the last connection broke. Nothing at all means the daemon is not
    /// there, and every caller answers for that itself.
    pub fn with_session<T>(
        &self,
        exchange: impl FnOnce(&mut Link, u32) -> std::io::Result<T>,
    ) -> Option<T> {
        let mut held = self.daemon.lock().ok()?;
        let session = held.ready(SCOPE_SYSTEM)?;
        let remote = session.remote;
        match exchange(&mut session.link, remote) {
            Ok(answer) => Some(answer),
            Err(reason) => {
                // The connection carried the contexts and cards the daemon knew
                // about, so a broken one has taken them with it. Dropping it
                // here is what makes the next call open a new one.
                coax_settings::trace!("winscard: the card socket broke: {reason}");
                held.session = None;
                None
            }
        }
    }

    /// Runs an exchange about a card, refusing one from a connection that has
    /// since been remade.
    pub fn with_card<T>(
        &self,
        generation: u64,
        exchange: impl FnOnce(&mut Link) -> std::io::Result<T>,
    ) -> CardCall<T> {
        let Ok(mut held) = self.daemon.lock() else {
            return CardCall::NoService;
        };
        if held.generation != generation || held.session.is_none() {
            return CardCall::Gone;
        }
        let session = held.session.as_mut().expect("just checked");
        match exchange(&mut session.link) {
            Ok(answer) => CardCall::Done(answer),
            Err(reason) => {
                coax_settings::trace!("winscard: the card socket broke: {reason}");
                held.session = None;
                CardCall::NoService
            }
        }
    }

    /// Which connection is current, which is what a card handle remembers.
    pub fn generation(&self) -> u64 {
        self.daemon
            .lock()
            .map(|held| held.generation)
            .unwrap_or_default()
    }

    /// The built-in readers, or none when the daemon is not there.
    pub fn readers(&self) -> Vec<ReaderState> {
        self.with_session(|link, _| link.readers())
            .unwrap_or_default()
    }

    /// Whether a reader name is one of the daemon's rather than the system's.
    pub fn owns(&self, reader: &str) -> bool {
        self.knows(reader) == Some(true)
    }

    /// Whether the daemon lists this reader, or `None` when it could not be
    /// asked.
    pub fn knows(&self, reader: &str) -> Option<bool> {
        self.with_session(|link, _| link.readers())
            .map(|readers| readers.iter().any(|known| known.name == reader))
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether a wait was cancelled, which also clears the request: a cancel
    /// ends the wait that was running and is not kept for the next one.
    pub fn take_cancelled(&self) -> bool {
        self.cancelled.swap(false, Ordering::SeqCst)
    }

    /// Gives the context back to the daemon. Called once, when the application
    /// releases it.
    pub fn release(&self) {
        self.with_session(|link, remote| link.release_context(remote));
        if let Ok(mut held) = self.daemon.lock() {
            held.session = None;
        }
    }
}

/// `SCARD_SCOPE_SYSTEM`, which is the only scope that means anything here: the
/// readers are a machine's hardware however an application asked for them.
const SCOPE_SYSTEM: u32 = 2;

/// One connected card.
#[derive(Debug)]
pub struct Card {
    pub context: Arc<Context>,
    /// The daemon's number for it.
    pub remote: i32,
    /// The reader it is in, which `SCardStatus` has to name and the reader
    /// table is looked up by.
    pub reader: String,
    /// The connection it was opened on.
    pub generation: u64,
}

#[derive(Debug, Default)]
struct Registry {
    contexts: HashMap<Handle, Arc<Context>>,
    cards: HashMap<Handle, Arc<Card>>,
    /// The buffers handed out for `SCARD_AUTOALLOCATE`, with the layout each
    /// was made with so that freeing one is exact.
    allocations: HashMap<usize, Layout>,
    next: Handle,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// The next handle: a number under the mark, never one in use. The number
/// goes round rather than growing into the mark -- on 32 bits there are only
/// sixteen bits below it, and an application that opens and closes a context
/// per call gets through those -- and skips nought and whatever is still held.
fn issue(registry: &mut Registry) -> Handle {
    loop {
        registry.next = registry.next.wrapping_add(1) & !MARK_MASK;
        let handle = MARK | registry.next;
        if registry.next != 0
            && !registry.contexts.contains_key(&handle)
            && !registry.cards.contains_key(&handle)
        {
            return handle;
        }
    }
}

pub fn add_context(context: Context) -> Handle {
    let mut held = registry().lock().expect("the handle table is not poisoned");
    let handle = issue(&mut held);
    held.contexts.insert(handle, Arc::new(context));
    handle
}

pub fn context(handle: Handle) -> Option<Arc<Context>> {
    if !marked(handle) {
        return None;
    }
    let held = registry().lock().ok()?;
    held.contexts.get(&handle).cloned()
}

pub fn remove_context(handle: Handle) -> Option<Arc<Context>> {
    let mut held = registry().lock().ok()?;
    let context = held.contexts.remove(&handle)?;
    // The cards opened through it go with it: the daemon drops them when the
    // connection closes, and a handle that would name one afterwards must not
    // be answered.
    held.cards
        .retain(|_, card| !Arc::ptr_eq(&card.context, &context));
    Some(context)
}

pub fn add_card(card: Card) -> Handle {
    let mut held = registry().lock().expect("the handle table is not poisoned");
    let handle = issue(&mut held);
    held.cards.insert(handle, Arc::new(card));
    handle
}

pub fn card(handle: Handle) -> Option<Arc<Card>> {
    if !marked(handle) {
        return None;
    }
    let held = registry().lock().ok()?;
    held.cards.get(&handle).cloned()
}

pub fn remove_card(handle: Handle) -> Option<Arc<Card>> {
    let mut held = registry().lock().ok()?;
    held.cards.remove(&handle)
}

/// Room for an answer the caller asked this library to find, which it will
/// hand back to `SCardFreeMemory`.
pub fn allocate(bytes: usize) -> *mut u8 {
    let layout = match Layout::from_size_align(bytes.max(1), 8) {
        Ok(layout) => layout,
        Err(_) => return std::ptr::null_mut(),
    };
    let block = unsafe { std::alloc::alloc_zeroed(layout) };
    if block.is_null() {
        return block;
    }
    if let Ok(mut held) = registry().lock() {
        held.allocations.insert(block as usize, layout);
    }
    block
}

/// Frees a buffer this library allocated, saying whether it was one.
pub fn free(block: *const std::ffi::c_void) -> bool {
    let Ok(mut held) = registry().lock() else {
        return false;
    };
    let Some(layout) = held.allocations.remove(&(block as usize)) else {
        return false;
    };
    unsafe { std::alloc::dealloc(block as *mut u8, layout) };
    true
}

/// Every context ever established goes to the same daemon, worked out once so
/// that a machine whose environment changes mid-run does not end up with two
/// sets of readers.
pub fn endpoint() -> &'static Endpoint {
    static WHERE: OnceLock<Endpoint> = OnceLock::new();
    WHERE.get_or_init(link::endpoint)
}

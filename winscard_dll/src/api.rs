// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The entry points an application calls.
//!
//! Each one decides which side the handle belongs to and does the same small
//! thing: hand the built-in slot's work to the daemon, and everything else to
//! the system library. The buffer rules are the API's, not the daemon's -- a
//! null buffer asks for the length, `SCARD_AUTOALLOCATE` asks this library to
//! find the room -- so they are applied here.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::abi::*;
use crate::link::{Link, Watch};
use crate::state::{self, Card, CardCall, Context};
use crate::system::system;
use crate::wire::{self, ReaderState};

/// The protocol headers an application passes to `SCardTransmit`. They are
/// exported as data because that is how the API publishes them.
#[unsafe(no_mangle)]
pub static g_rgSCardT0Pci: IoRequest = IoRequest {
    protocol: 0x0001,
    pci_length: 8,
};
#[unsafe(no_mangle)]
pub static g_rgSCardT1Pci: IoRequest = IoRequest {
    protocol: 0x0002,
    pci_length: 8,
};
#[unsafe(no_mangle)]
pub static g_rgSCardRawPci: IoRequest = IoRequest {
    protocol: 0x0001_0000,
    pci_length: 8,
};

/// The group a reader is in when nobody arranged any.
const DEFAULT_READER_GROUP: &str = "SCard$DefaultReaders";

/// How long to give the daemon to take its first look at a slot.
const SETTLE: Duration = Duration::from_millis(2500);

/// How long to wait between looks while a `SCardGetStatusChange` is running.
/// Card movement is not an interrupt on any of these interfaces, so a wait is
/// a poll however it is dressed up.
const LOOK_AGAIN: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------- contexts

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardEstablishContext(
    scope: u32,
    reserved1: *const c_void,
    reserved2: *const c_void,
    context: *mut Handle,
) -> Status {
    if context.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    let mut theirs: Handle = 0;
    if let Some(establish) = system().establish_context {
        let mut handle: Handle = 0;
        if unsafe { establish(scope, reserved1, reserved2, &raw mut handle) } == SCARD_S_SUCCESS {
            theirs = handle;
        }
    }
    // The daemon is neither dialled nor started here. An application
    // establishes its context before it has asked for anything, and both would
    // make it wait: the wait belongs where something asks for a reader. See
    // `state::open`.
    //
    // What is decided here is only whether to say there is no service at all:
    // an application told so goes and does something else rather than waiting
    // on a reader list that will always be empty.
    if theirs == 0 && !crate::link::reachable(state::endpoint()) && !crate::link::can_start() {
        return SCARD_E_NO_SERVICE;
    }
    let handle = state::add_context(Context::new(theirs));
    coax_settings::trace!("winscard: context {handle:#x}, system {theirs:#x}");
    unsafe { *context = handle };
    SCARD_S_SUCCESS
}

/// `SCARD_S_SUCCESS` as the daemon writes it, which is unsigned on the wire.
#[allow(non_upper_case_globals)]
const SCARD_S_SUCCESS_U32: u32 = 0;

/// One exchange about a card, with the two ways it can fail turned into what
/// the API says about each.
fn on_card<T>(
    card: &Card,
    exchange: impl FnOnce(&mut Link) -> std::io::Result<T>,
) -> Result<T, Status> {
    match card.context.with_card(card.generation, exchange) {
        CardCall::Done(answer) => Ok(answer),
        CardCall::Gone => Err(SCARD_E_INVALID_HANDLE),
        CardCall::NoService => Err(SCARD_E_NO_SERVICE),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardReleaseContext(context: Handle) -> Status {
    let Some(ours) = state::remove_context(context) else {
        return unsafe { forward_handle(context, system().release_context) };
    };
    ours.release();
    if ours.system != 0 {
        if let Some(release) = system().release_context {
            unsafe { release(ours.system) };
        }
    }
    SCARD_S_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardIsValidContext(context: Handle) -> Status {
    match state::context(context) {
        Some(_) => SCARD_S_SUCCESS,
        None => unsafe { forward_handle(context, system().is_valid_context) },
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardCancel(context: Handle) -> Status {
    let Some(ours) = state::context(context) else {
        return unsafe { forward_handle(context, system().cancel) };
    };
    ours.cancel();
    if ours.system != 0 {
        if let Some(cancel) = system().cancel {
            unsafe { cancel(ours.system) };
        }
    }
    SCARD_S_SUCCESS
}

// ----------------------------------------------------------------- listing

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardListReadersA(
    context: Handle,
    groups: *const u8,
    readers: *mut u8,
    length: *mut u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().list_readers_a {
            Some(list) => unsafe { list(context, groups, readers, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let mut names = ours
        .readers()
        .into_iter()
        .map(|one| one.name)
        .collect::<Vec<_>>();
    coax_settings::trace!("winscard: listing, {} built-in", names.len());
    let mut theirs = SCARD_S_SUCCESS;
    if ours.system != 0 {
        if let Some(list) = system().list_readers_a {
            let (status, more) = unsafe { system_readers_a(ours.system, groups, list) };
            theirs = status;
            coax_settings::trace!(
                "winscard: the system answered {status:#010x} with {} reader(s)",
                more.len()
            );
            names.extend(more);
        }
    } else {
        coax_settings::trace!("winscard: no system context on this one");
    }
    // With nothing of our own to add, the system's answer is the answer -- its
    // refusal included, so that an application sees what it would have seen
    // had this library not been in the directory.
    if names.is_empty() && theirs != SCARD_S_SUCCESS {
        return theirs;
    }
    unsafe { write_multi_a(&names, readers, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardListReadersW(
    context: Handle,
    groups: *const u16,
    readers: *mut u16,
    length: *mut u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().list_readers_w {
            Some(list) => unsafe { list(context, groups, readers, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let mut names = ours
        .readers()
        .into_iter()
        .map(|one| one.name)
        .collect::<Vec<_>>();
    coax_settings::trace!("winscard: listing, {} built-in", names.len());
    let mut theirs = SCARD_S_SUCCESS;
    if ours.system != 0 {
        if let Some(list) = system().list_readers_w {
            let (status, more) = unsafe { system_readers_w(ours.system, groups, list) };
            theirs = status;
            coax_settings::trace!(
                "winscard: the system answered {status:#010x} with {} reader(s)",
                more.len()
            );
            names.extend(more);
        }
    } else {
        coax_settings::trace!("winscard: no system context on this one");
    }
    // With nothing of our own to add, the system's answer is the answer -- its
    // refusal included, so that an application sees what it would have seen
    // had this library not been in the directory.
    if names.is_empty() && theirs != SCARD_S_SUCCESS {
        return theirs;
    }
    unsafe { write_multi_w(&names, readers, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardListReaderGroupsA(
    context: Handle,
    groups: *mut u8,
    length: *mut u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().list_reader_groups_a {
            Some(list) => unsafe { list(context, groups, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    // There is no reason to invent groups for hardware nobody arranged, so
    // ours are in the default one; the system's own groups are still its to
    // report.
    let mut names = vec![DEFAULT_READER_GROUP.to_owned()];
    if ours.system != 0 {
        if let Some(list) = system().list_reader_groups_a {
            let mut room: u32 = 0;
            if unsafe { list(ours.system, std::ptr::null_mut(), &raw mut room) } == SCARD_S_SUCCESS
                && room > 0
            {
                let mut buffer = vec![0u8; room as usize];
                if unsafe { list(ours.system, buffer.as_mut_ptr(), &raw mut room) }
                    == SCARD_S_SUCCESS
                {
                    for group in buffer
                        .split(|byte| *byte == 0)
                        .filter(|one| !one.is_empty())
                    {
                        let group = String::from_utf8_lossy(group).into_owned();
                        if !names.contains(&group) {
                            names.push(group);
                        }
                    }
                }
            }
        }
    }
    unsafe { write_multi_a(&names, groups, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardListReaderGroupsW(
    context: Handle,
    groups: *mut u16,
    length: *mut u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().list_reader_groups_w {
            Some(list) => unsafe { list(context, groups, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let mut names = vec![DEFAULT_READER_GROUP.to_owned()];
    if ours.system != 0 {
        if let Some(list) = system().list_reader_groups_w {
            let mut room: u32 = 0;
            if unsafe { list(ours.system, std::ptr::null_mut(), &raw mut room) } == SCARD_S_SUCCESS
                && room > 0
            {
                let mut buffer = vec![0u16; room as usize];
                if unsafe { list(ours.system, buffer.as_mut_ptr(), &raw mut room) }
                    == SCARD_S_SUCCESS
                {
                    for group in buffer
                        .split(|unit| *unit == 0)
                        .filter(|one| !one.is_empty())
                    {
                        let group = String::from_utf16_lossy(group);
                        if !names.contains(&group) {
                            names.push(group);
                        }
                    }
                }
            }
        }
    }
    unsafe { write_multi_w(&names, groups, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardFreeMemory(context: Handle, block: *const c_void) -> Status {
    if state::free(block) {
        return SCARD_S_SUCCESS;
    }
    let handle = match state::context(context) {
        Some(ours) => ours.system,
        None => context,
    };
    match system().free_memory {
        Some(free) => unsafe { free(handle, block) },
        None => SCARD_S_SUCCESS,
    }
}

// -------------------------------------------------------------------- cards

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardConnectA(
    context: Handle,
    reader: *const u8,
    share: u32,
    protocols: u32,
    card: *mut Handle,
    active: *mut u32,
) -> Status {
    let named = unsafe { from_ansi(reader) };
    unsafe { connect(context, named, share, protocols, card, active, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardConnectW(
    context: Handle,
    reader: *const u16,
    share: u32,
    protocols: u32,
    card: *mut Handle,
    active: *mut u32,
) -> Status {
    let named = unsafe { from_wide(reader) };
    unsafe { connect(context, named, share, protocols, card, active, true) }
}

/// The body of both `SCardConnect`s: the name decides which side answers, and
/// a name this library does not serve is the system's to refuse or accept.
unsafe fn connect(
    context: Handle,
    reader: Option<String>,
    share: u32,
    protocols: u32,
    card: *mut Handle,
    active: *mut u32,
    wide: bool,
) -> Status {
    if card.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    let Some(named) = reader else {
        return SCARD_E_INVALID_PARAMETER;
    };
    let Some(ours) = state::context(context) else {
        return unsafe {
            connect_through_system(context, &named, share, protocols, card, active, wide)
        };
    };
    match ours.knows(&named) {
        Some(true) => {}
        // The daemon could not be asked, so whether the reader is one of its
        // own is not known. Saying it does not exist is what makes an
        // application stop looking for it.
        None if ours.system == 0 => return SCARD_E_NO_SERVICE,
        _ if ours.system == 0 => return SCARD_E_UNKNOWN_READER,
        _ => {
            return unsafe {
                connect_through_system(ours.system, &named, share, protocols, card, active, wide)
            };
        }
    }
    let Some((handle, agreed, result)) =
        ours.with_session(|link, remote| link.connect_card(remote, &named, share, protocols))
    else {
        return SCARD_E_NO_SERVICE;
    };
    if result != SCARD_S_SUCCESS_U32 {
        return result as Status;
    }
    let issued = state::add_card(Card {
        generation: ours.generation(),
        context: Arc::clone(&ours),
        remote: handle,
        reader: named,
    });
    unsafe { *card = issued };
    if !active.is_null() {
        unsafe { *active = agreed };
    }
    SCARD_S_SUCCESS
}

unsafe fn connect_through_system(
    context: Handle,
    reader: &str,
    share: u32,
    protocols: u32,
    card: *mut Handle,
    active: *mut u32,
    wide: bool,
) -> Status {
    if wide {
        let Some(call) = system().connect_w else {
            return SCARD_E_NO_SERVICE;
        };
        let name = wide_of(reader);
        unsafe { call(context, name.as_ptr(), share, protocols, card, active) }
    } else {
        let Some(call) = system().connect_a else {
            return SCARD_E_NO_SERVICE;
        };
        let name = ansi_of(reader);
        unsafe { call(context, name.as_ptr(), share, protocols, card, active) }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardReconnect(
    card: Handle,
    share: u32,
    protocols: u32,
    initialization: u32,
    active: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().reconnect {
            Some(reconnect) => unsafe { reconnect(card, share, protocols, initialization, active) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let remote = ours.remote;
    let (agreed, result) = match on_card(&ours, |link| {
        link.reconnect(remote, share, protocols, initialization)
    }) {
        Ok(answer) => answer,
        Err(status) => return status,
    };
    if result == SCARD_S_SUCCESS_U32 && !active.is_null() {
        unsafe { *active = agreed };
    }
    result as Status
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardDisconnect(card: Handle, disposition: u32) -> Status {
    let Some(ours) = state::remove_card(card) else {
        return match system().disconnect {
            Some(disconnect) => unsafe { disconnect(card, disposition) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let remote = ours.remote;
    // A card whose connection has already gone is disconnected by saying so:
    // the daemon dropped it when the connection went.
    on_card(&ours, |link| link.disconnect(remote, disposition))
        .map(|result| result as Status)
        .unwrap_or(SCARD_S_SUCCESS)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardBeginTransaction(card: Handle) -> Status {
    let Some(ours) = state::card(card) else {
        return unsafe { forward_handle(card, system().begin_transaction) };
    };
    let remote = ours.remote;
    match on_card(&ours, |link| link.begin_transaction(remote)) {
        Ok(result) => result as Status,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardEndTransaction(card: Handle, disposition: u32) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().end_transaction {
            Some(end) => unsafe { end(card, disposition) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let remote = ours.remote;
    match on_card(&ours, |link| link.end_transaction(remote, disposition)) {
        Ok(result) => result as Status,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardTransmit(
    card: Handle,
    send_pci: *const IoRequest,
    send: *const u8,
    send_length: u32,
    receive_pci: *mut IoRequest,
    receive: *mut u8,
    receive_length: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().transmit {
            Some(transmit) => unsafe {
                transmit(
                    card,
                    send_pci,
                    send,
                    send_length,
                    receive_pci,
                    receive,
                    receive_length,
                )
            },
            None => SCARD_E_NO_SERVICE,
        };
    };
    if send.is_null() || receive.is_null() || receive_length.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    // A caller that passes no header means the protocol it connected with,
    // and these readers speak one.
    let protocol = if send_pci.is_null() {
        SCARD_PROTOCOL_T1
    } else {
        unsafe { (*send_pci).protocol }
    };
    let command = unsafe { std::slice::from_raw_parts(send, send_length as usize) };
    let room = unsafe { *receive_length };
    let remote = ours.remote;
    let (result, agreed, answer) =
        match on_card(&ours, |link| link.transmit(remote, protocol, command, room)) {
            Ok(answer) => answer,
            Err(status) => return status,
        };
    if result != SCARD_S_SUCCESS_U32 {
        return result as Status;
    }
    if answer.len() as u32 > room {
        unsafe { *receive_length = answer.len() as u32 };
        return SCARD_E_INSUFFICIENT_BUFFER;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(answer.as_ptr(), receive, answer.len());
        *receive_length = answer.len() as u32;
        if !receive_pci.is_null() {
            (*receive_pci).protocol = agreed;
            (*receive_pci).pci_length = 8;
        }
    }
    SCARD_S_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardControl(
    card: Handle,
    code: u32,
    input: *const c_void,
    input_length: u32,
    output: *mut c_void,
    output_length: u32,
    returned: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().control {
            Some(control) => unsafe {
                control(
                    card,
                    code,
                    input,
                    input_length,
                    output,
                    output_length,
                    returned,
                )
            },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let command = if input.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(input.cast::<u8>(), input_length as usize) }
    };
    let remote = ours.remote;
    let (result, answer) = match on_card(&ours, |link| {
        link.control(remote, code, command, output_length)
    }) {
        Ok(answer) => answer,
        Err(status) => return status,
    };
    if result != SCARD_S_SUCCESS_U32 {
        return result as Status;
    }
    if answer.len() as u32 > output_length {
        return SCARD_E_INSUFFICIENT_BUFFER;
    }
    if !output.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(answer.as_ptr(), output.cast::<u8>(), answer.len())
        };
    }
    if !returned.is_null() {
        unsafe { *returned = answer.len() as u32 };
    }
    SCARD_S_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardGetAttrib(
    card: Handle,
    attribute: u32,
    value: *mut u8,
    length: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().get_attrib {
            Some(get) => unsafe { get(card, attribute, value, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    if length.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    let remote = ours.remote;
    let (result, answer) = match on_card(&ours, |link| link.get_attrib(remote, attribute)) {
        Ok(answer) => answer,
        Err(status) => return status,
    };
    if result != SCARD_S_SUCCESS_U32 {
        return result as Status;
    }
    unsafe { write_bytes(&answer, value, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardSetAttrib(
    card: Handle,
    attribute: u32,
    value: *const u8,
    length: u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().set_attrib {
            Some(set) => unsafe { set(card, attribute, value, length) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let written = if value.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(value, length as usize) }
    };
    let remote = ours.remote;
    match on_card(&ours, |link| link.set_attrib(remote, attribute, written)) {
        Ok(result) => result as Status,
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardStatusA(
    card: Handle,
    reader: *mut u8,
    reader_length: *mut u32,
    state_out: *mut u32,
    protocol: *mut u32,
    atr: *mut u8,
    atr_length: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().status_a {
            Some(status) => unsafe {
                status(
                    card,
                    reader,
                    reader_length,
                    state_out,
                    protocol,
                    atr,
                    atr_length,
                )
            },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let Some(seen) = look_at_card(&ours) else {
        return SCARD_E_NO_SERVICE;
    };
    let reported = unsafe { report(&seen, state_out, protocol, atr, atr_length) };
    if !reader_length.is_null() {
        let named =
            unsafe { write_multi_a(std::slice::from_ref(&ours.reader), reader, reader_length) };
        if named != SCARD_S_SUCCESS {
            return named;
        }
    }
    reported
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardStatusW(
    card: Handle,
    reader: *mut u16,
    reader_length: *mut u32,
    state_out: *mut u32,
    protocol: *mut u32,
    atr: *mut u8,
    atr_length: *mut u32,
) -> Status {
    let Some(ours) = state::card(card) else {
        return match system().status_w {
            Some(status) => unsafe {
                status(
                    card,
                    reader,
                    reader_length,
                    state_out,
                    protocol,
                    atr,
                    atr_length,
                )
            },
            None => SCARD_E_NO_SERVICE,
        };
    };
    let Some(seen) = look_at_card(&ours) else {
        return SCARD_E_NO_SERVICE;
    };
    let reported = unsafe { report(&seen, state_out, protocol, atr, atr_length) };
    if !reader_length.is_null() {
        let named =
            unsafe { write_multi_w(std::slice::from_ref(&ours.reader), reader, reader_length) };
        if named != SCARD_S_SUCCESS {
            return named;
        }
    }
    reported
}

/// Asks after a connected card, which is also what makes the daemon look: the
/// published state is a memory of the last look until something asks.
fn look_at_card(card: &Card) -> Option<ReaderState> {
    let remote = card.remote;
    on_card(card, |link| link.status(remote)).ok()?;
    card.context
        .readers()
        .into_iter()
        .find(|reader| reader.name == card.reader)
}

/// Writes the parts of `SCardStatus` that are the same in both flavours. The
/// ATR goes by the same rules as every other buffer -- a length asked for, a
/// buffer allocated for the caller, or one too small -- which is what
/// [`write_bytes`] is.
unsafe fn report(
    seen: &ReaderState,
    state_out: *mut u32,
    protocol: *mut u32,
    atr: *mut u8,
    atr_length: *mut u32,
) -> Status {
    if !state_out.is_null() {
        unsafe { *state_out = seen.windows_state() };
    }
    if !protocol.is_null() {
        unsafe { *protocol = seen.protocol };
    }
    if atr_length.is_null() {
        return SCARD_S_SUCCESS;
    }
    unsafe { write_bytes(&seen.atr, atr, atr_length) }
}

// ------------------------------------------------------------------ waiting

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardGetStatusChangeA(
    context: Handle,
    timeout: u32,
    readers: *mut ReaderStateA,
    count: u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().get_status_change_a {
            Some(wait) => unsafe { wait(context, timeout, readers, count) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    if readers.is_null() && count != 0 {
        return SCARD_E_INVALID_PARAMETER;
    }
    let mut entries = unsafe { std::slice::from_raw_parts_mut(readers, count as usize) };
    wait_for_change(&ours, timeout, &mut entries)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn SCardGetStatusChangeW(
    context: Handle,
    timeout: u32,
    readers: *mut ReaderStateW,
    count: u32,
) -> Status {
    let Some(ours) = state::context(context) else {
        return match system().get_status_change_w {
            Some(wait) => unsafe { wait(context, timeout, readers, count) },
            None => SCARD_E_NO_SERVICE,
        };
    };
    if readers.is_null() && count != 0 {
        return SCARD_E_INVALID_PARAMETER;
    }
    let mut entries = unsafe { std::slice::from_raw_parts_mut(readers, count as usize) };
    wait_for_change(&ours, timeout, &mut entries)
}

/// The caller's array, whichever flavour it is in. The wait below is the same
/// work either way; all that differs is how a name is read and where the
/// answer is written.
trait Entries {
    fn count(&self) -> usize;
    fn name(&self, index: usize) -> Option<String>;
    fn current(&self, index: usize) -> u32;
    fn publish(&mut self, index: usize, event: u32, atr: &[u8]);
    /// Asks the system library about the entries it answers for, with no
    /// timeout of its own: this call does the waiting, so that one side
    /// answering does not leave the other still waiting.
    fn ask_system(&mut self, context: &Context, which: &[usize]) -> SystemPass;
    /// Hands the whole call to the system library, timeout and all. Used when
    /// nothing in the array is a reader this library serves, so that a wait on
    /// the system's own readers behaves exactly as it would without this file
    /// in the directory.
    fn forward_all(&mut self, context: &Context, timeout: u32) -> Status;
}

impl Entries for &mut [ReaderStateA] {
    fn count(&self) -> usize {
        self.len()
    }

    fn name(&self, index: usize) -> Option<String> {
        unsafe { from_ansi(self[index].reader) }
    }

    fn current(&self, index: usize) -> u32 {
        self[index].current_state
    }

    fn publish(&mut self, index: usize, event: u32, atr: &[u8]) {
        let entry = &mut self[index];
        entry.event_state = event;
        let length = atr.len().min(entry.atr.len());
        entry.atr[..length].copy_from_slice(&atr[..length]);
        entry.atr_length = length as u32;
    }

    fn forward_all(&mut self, context: &Context, timeout: u32) -> Status {
        let Some(wait) = system().get_status_change_a else {
            return SCARD_E_NO_SERVICE;
        };
        let count = self.len() as u32;
        unsafe { wait(context.system, timeout, self.as_mut_ptr(), count) }
    }

    fn ask_system(&mut self, context: &Context, which: &[usize]) -> SystemPass {
        let (Some(wait), true) = (system().get_status_change_a, context.system != 0) else {
            return unknown_readers(self, which);
        };
        let mut asked: Vec<ReaderStateA> = which.iter().map(|index| self[*index]).collect();
        let result = unsafe { wait(context.system, 0, asked.as_mut_ptr(), asked.len() as u32) };
        if result != SCARD_S_SUCCESS && result != SCARD_E_TIMEOUT {
            return SystemPass::Failed(result);
        }
        let mut changed = false;
        for (slot, index) in which.iter().enumerate() {
            self[*index] = asked[slot];
            changed |= asked[slot].event_state & SCARD_STATE_CHANGED != 0;
        }
        if changed {
            SystemPass::Changed
        } else {
            SystemPass::Quiet
        }
    }
}

impl Entries for &mut [ReaderStateW] {
    fn count(&self) -> usize {
        self.len()
    }

    fn name(&self, index: usize) -> Option<String> {
        unsafe { from_wide(self[index].reader) }
    }

    fn current(&self, index: usize) -> u32 {
        self[index].current_state
    }

    fn publish(&mut self, index: usize, event: u32, atr: &[u8]) {
        let entry = &mut self[index];
        entry.event_state = event;
        let length = atr.len().min(entry.atr.len());
        entry.atr[..length].copy_from_slice(&atr[..length]);
        entry.atr_length = length as u32;
    }

    fn forward_all(&mut self, context: &Context, timeout: u32) -> Status {
        let Some(wait) = system().get_status_change_w else {
            return SCARD_E_NO_SERVICE;
        };
        let count = self.len() as u32;
        unsafe { wait(context.system, timeout, self.as_mut_ptr(), count) }
    }

    fn ask_system(&mut self, context: &Context, which: &[usize]) -> SystemPass {
        let (Some(wait), true) = (system().get_status_change_w, context.system != 0) else {
            return unknown_readers(self, which);
        };
        let mut asked: Vec<ReaderStateW> = which.iter().map(|index| self[*index]).collect();
        let result = unsafe { wait(context.system, 0, asked.as_mut_ptr(), asked.len() as u32) };
        if result != SCARD_S_SUCCESS && result != SCARD_E_TIMEOUT {
            return SystemPass::Failed(result);
        }
        let mut changed = false;
        for (slot, index) in which.iter().enumerate() {
            self[*index] = asked[slot];
            changed |= asked[slot].event_state & SCARD_STATE_CHANGED != 0;
        }
        if changed {
            SystemPass::Changed
        } else {
            SystemPass::Quiet
        }
    }
}

/// What the system library made of the entries it was asked about.
#[derive(Debug)]
enum SystemPass {
    Changed,
    Quiet,
    Failed(Status),
}

/// Answers for readers nobody serves. An application waiting on a reader that
/// is not there expects to be told so at once rather than to wait out its
/// timeout, which is what `SCARD_STATE_UNKNOWN` with the changed bit says.
fn unknown_readers(entries: &mut impl Entries, which: &[usize]) -> SystemPass {
    let mut changed = false;
    for index in which {
        let before = entries.current(*index) & !SCARD_STATE_CHANGED;
        let event = SCARD_STATE_UNKNOWN;
        if event != before {
            changed = true;
        }
        entries.publish(*index, event | SCARD_STATE_CHANGED, &[]);
    }
    if changed {
        SystemPass::Changed
    } else {
        SystemPass::Quiet
    }
}

/// The body of both `SCardGetStatusChange`s. The readers this library serves
/// are watched by looking again every little while: card movement is not an
/// interrupt on any of these interfaces, so a wait is a poll however it is
/// dressed up.
fn wait_for_change(context: &Context, timeout: u32, entries: &mut impl Entries) -> Status {
    let names: Vec<Option<String>> = (0..entries.count())
        .map(|index| entries.name(index))
        .collect();
    let watched: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(index, name)| {
            entries.current(*index) & SCARD_STATE_IGNORE == 0
                && name
                    .as_deref()
                    .is_some_and(|name| name != PNP_NOTIFICATION && context.owns(name))
        })
        .map(|(index, _)| index)
        .collect();
    let elsewhere: Vec<usize> = (0..names.len())
        .filter(|index| !watched.contains(index))
        .collect();

    // Nothing of ours is in the array, so there is nothing for this library to
    // add: the system library waits on its own readers with the timeout it was
    // given, as it would have been asked to directly.
    if watched.is_empty() && context.system != 0 {
        return entries.forward_all(context, timeout);
    }

    // The registration is what makes the daemon look at the slots at all --
    // reading a slot means opening the board a tuner streams from -- and it is
    // given back as soon as this call is done with it.
    let _watch = (!watched.is_empty())
        .then(|| match Watch::open(state::endpoint()) {
            Ok(watch) => Some(watch),
            Err(reason) => {
                coax_settings::trace!("winscard: could not register a wait: {reason}");
                None
            }
        })
        .flatten();

    // The first look takes a moment to arrive, and until it does the slots read
    // as unknown. Answering from that would tell the application the reader is
    // empty, which it stops looking after, so the measurement is waited for --
    // briefly, and whatever timeout the caller asked for.
    if !watched.is_empty() {
        let settled = Instant::now() + SETTLE;
        while Instant::now() < settled {
            let seen = context.readers();
            let all_measured = watched.iter().all(|index| {
                let name = names[*index].as_deref().unwrap_or_default();
                seen.iter()
                    .find(|reader| reader.name == name)
                    .is_none_or(ReaderState::measured)
            });
            if all_measured {
                break;
            }
            std::thread::sleep(LOOK_AGAIN);
        }
    }

    let deadline =
        (timeout != INFINITE).then(|| Instant::now() + Duration::from_millis(u64::from(timeout)));
    loop {
        if context.take_cancelled() {
            return SCARD_E_CANCELLED;
        }
        let mut changed = false;
        if !watched.is_empty() {
            let seen = context.readers();
            for index in &watched {
                let name = names[*index].as_deref().unwrap_or_default();
                let (mut event, atr) = match seen.iter().find(|reader| reader.name == name) {
                    // Nothing has looked yet. Saying so is better than saying
                    // the slot is empty, and the wait goes on.
                    Some(reader) if !reader.measured() => continue,
                    Some(reader) => (flags_of(reader), reader.atr.clone()),
                    // A reader that was in the listing and is not there now.
                    None => (SCARD_STATE_UNKNOWN, Vec::new()),
                };
                let before = entries.current(*index) & !SCARD_STATE_CHANGED;
                if event != before {
                    event |= SCARD_STATE_CHANGED;
                    changed = true;
                }
                entries.publish(*index, event, &atr);
            }
        }
        if !elsewhere.is_empty() {
            match entries.ask_system(context, &elsewhere) {
                SystemPass::Changed => changed = true,
                SystemPass::Quiet => {}
                // A refusal is the answer: a reader that is not there is not
                // something to wait for.
                SystemPass::Failed(status) => return status,
            }
        }
        if changed {
            return SCARD_S_SUCCESS;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return SCARD_E_TIMEOUT;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// The `SCARD_STATE_*` flags for a reader the daemon publishes.
fn flags_of(reader: &ReaderState) -> u32 {
    let mut flags = if reader.present() {
        SCARD_STATE_PRESENT
    } else {
        SCARD_STATE_EMPTY
    };
    if reader.sharing == wire::SHARING_EXCLUSIVE {
        flags |= SCARD_STATE_EXCLUSIVE;
    } else if reader.sharing > 0 {
        flags |= SCARD_STATE_INUSE;
    }
    flags
}

// ------------------------------------------------------------------ buffers

/// Reads a name the caller passed in the system's own code page. The daemon's
/// names are ASCII and read the same either way; a reader of the system's own
/// may be named in anything, and has to come out the far side of the system
/// library spelt as it went in.
unsafe fn from_ansi(text: *const u8) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(text, length) };
    Some(decode_ansi(bytes))
}

/// The system's own code page, which the `A` functions are written in.
const CP_ACP: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        input: *const u8,
        input_length: i32,
        output: *mut u16,
        output_length: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        input: *const u16,
        input_length: i32,
        output: *mut u8,
        output_length: i32,
        default: *const u8,
        used_default: *mut i32,
    ) -> i32;
}

/// Bytes in the system's code page as text.
fn decode_ansi(bytes: &[u8]) -> String {
    if bytes.is_ascii() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let Ok(length) = i32::try_from(bytes.len()) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    let needed =
        unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), length, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), length, wide.as_mut_ptr(), needed)
    };
    String::from_utf16_lossy(&wide[..written.max(0) as usize])
}

/// Text as bytes in the system's code page, without a terminator.
fn encode_ansi(text: &str) -> Vec<u8> {
    if text.is_ascii() {
        return text.as_bytes().to_vec();
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let Ok(length) = i32::try_from(wide.len()) else {
        return text.as_bytes().to_vec();
    };
    let needed = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    if needed <= 0 {
        return text.as_bytes().to_vec();
    }
    let mut bytes = vec![0u8; needed as usize];
    let written = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            length,
            bytes.as_mut_ptr(),
            needed,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    bytes.truncate(written.max(0) as usize);
    bytes
}

unsafe fn from_wide(text: *const u16) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let units = unsafe { std::slice::from_raw_parts(text, length) };
    Some(String::from_utf16_lossy(units))
}

fn ansi_of(text: &str) -> Vec<u8> {
    let mut bytes = encode_ansi(text);
    bytes.push(0);
    bytes
}

fn wide_of(text: &str) -> Vec<u16> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    units
}

/// Writes a multi-string: every name terminated, and an empty one at the end.
/// The three ways to ask are the API's: a null buffer asks for the length,
/// `SCARD_AUTOALLOCATE` asks this library to find the room, and anything else
/// is the caller's own.
unsafe fn write_multi_a(names: &[String], into: *mut u8, length: *mut u32) -> Status {
    if length.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    if names.is_empty() {
        return SCARD_E_NO_READERS_AVAILABLE;
    }
    let mut written: Vec<u8> = Vec::new();
    for name in names {
        written.extend_from_slice(&encode_ansi(name));
        written.push(0);
    }
    written.push(0);
    unsafe { deliver(&written, into, length, 1) }
}

unsafe fn write_multi_w(names: &[String], into: *mut u16, length: *mut u32) -> Status {
    if length.is_null() {
        return SCARD_E_INVALID_PARAMETER;
    }
    if names.is_empty() {
        return SCARD_E_NO_READERS_AVAILABLE;
    }
    let mut written: Vec<u16> = Vec::new();
    for name in names {
        written.extend(name.encode_utf16());
        written.push(0);
    }
    written.push(0);
    let bytes =
        unsafe { std::slice::from_raw_parts(written.as_ptr().cast::<u8>(), written.len() * 2) };
    unsafe { deliver(bytes, into.cast::<u8>(), length, 2) }
}

/// Hands a run of bytes to a caller, in units of the caller's own width: the
/// length of a `SCardListReaders` answer is counted in characters.
unsafe fn deliver(written: &[u8], into: *mut u8, length: *mut u32, unit: usize) -> Status {
    let units = (written.len() / unit) as u32;
    if into.is_null() {
        unsafe { *length = units };
        return SCARD_S_SUCCESS;
    }
    if unsafe { *length } == SCARD_AUTOALLOCATE {
        let block = state::allocate(written.len());
        if block.is_null() {
            return SCARD_E_NO_MEMORY;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(written.as_ptr(), block, written.len());
            // With SCARD_AUTOALLOCATE the buffer argument is a pointer to the
            // pointer the caller wants filling in.
            *into.cast::<*mut u8>() = block;
            *length = units;
        }
        return SCARD_S_SUCCESS;
    }
    if unsafe { *length } < units {
        unsafe { *length = units };
        return SCARD_E_INSUFFICIENT_BUFFER;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(written.as_ptr(), into, written.len());
        *length = units;
    }
    SCARD_S_SUCCESS
}

/// The same three rules for a plain run of bytes, which is what an attribute
/// is answered in.
unsafe fn write_bytes(value: &[u8], into: *mut u8, length: *mut u32) -> Status {
    unsafe { deliver(value, into, length, 1) }
}

/// The readers the system library knows, as a list of names.
unsafe fn system_readers_a(
    context: Handle,
    groups: *const u8,
    list: crate::system::ListReadersA,
) -> (Status, Vec<String>) {
    let mut length: u32 = 0;
    let asked = unsafe { list(context, groups, std::ptr::null_mut(), &raw mut length) };
    if asked != SCARD_S_SUCCESS || length == 0 {
        return (asked, Vec::new());
    }
    let mut buffer = vec![0u8; length as usize];
    let read = unsafe { list(context, groups, buffer.as_mut_ptr(), &raw mut length) };
    if read != SCARD_S_SUCCESS {
        return (read, Vec::new());
    }
    (
        SCARD_S_SUCCESS,
        buffer
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(decode_ansi)
            .collect(),
    )
}

unsafe fn system_readers_w(
    context: Handle,
    groups: *const u16,
    list: crate::system::ListReadersW,
) -> (Status, Vec<String>) {
    let mut length: u32 = 0;
    let asked = unsafe { list(context, groups, std::ptr::null_mut(), &raw mut length) };
    if asked != SCARD_S_SUCCESS || length == 0 {
        return (asked, Vec::new());
    }
    let mut buffer = vec![0u16; length as usize];
    let read = unsafe { list(context, groups, buffer.as_mut_ptr(), &raw mut length) };
    if read != SCARD_S_SUCCESS {
        return (read, Vec::new());
    }
    (
        SCARD_S_SUCCESS,
        buffer
            .split(|unit| *unit == 0)
            .filter(|name| !name.is_empty())
            .map(String::from_utf16_lossy)
            .collect(),
    )
}

/// Passes a call that takes nothing but a handle straight through.
unsafe fn forward_handle(handle: Handle, entry: Option<crate::system::OnHandle>) -> Status {
    match entry {
        Some(call) => unsafe { call(handle) },
        None => SCARD_E_INVALID_HANDLE,
    }
}

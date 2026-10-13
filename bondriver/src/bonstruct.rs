// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The interface that is a table of functions rather than a C++ object.
//!
//! A vtable is a compiler's idea of an object: where the type information sits,
//! how a `dynamic_cast` finds it, how a member function receives its `this`.
//! Two compilers do not have to agree, and MSVC and everybody else do not --
//! which is why [`crate::abi`] and [`crate::rtti`] exist and why they are built
//! only for Windows.
//!
//! EDCB avoids the whole question by asking for `CreateBonStruct` first: a
//! pointer to a structure of plain function pointers, each taking the driver as
//! its first argument. That is what lets this same library serve a recorder
//! built with GCC on Linux and a viewer built with MSVC on Windows.
//!
//! The shapes here are EDCB's `STRUCT_IBONDRIVER` and `STRUCT_IBONDRIVER2`.

use core::ffi::c_void;

use crate::driver::{Driver, Locked};

/// Windows' `BOOL`, which the interface is written in.
type Bool = i32;
const TRUE: Bool = 1;
const FALSE: Bool = 0;

/// The first interface, as a table.
///
/// `context` is what every function is given back; `end` points one past the
/// end of the whole structure, and is how the caller works out whether what it
/// has is a [`BonStruct2`]. That check is what a `dynamic_cast` would have been
/// for.
#[repr(C)]
#[derive(Debug)]
pub struct BonStruct {
    context: *mut c_void,
    end: *const c_void,
    open_tuner: unsafe extern "C" fn(*mut c_void) -> Bool,
    close_tuner: unsafe extern "C" fn(*mut c_void),
    set_channel_byte: unsafe extern "C" fn(*mut c_void, u8) -> Bool,
    signal_level: unsafe extern "C" fn(*mut c_void) -> f32,
    wait_ts_stream: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    ready_count: unsafe extern "C" fn(*mut c_void) -> u32,
    get_ts_stream: unsafe extern "C" fn(*mut c_void, *mut u8, *mut u32, *mut u32) -> Bool,
    get_ts_stream_ptr: unsafe extern "C" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32) -> Bool,
    purge_ts_stream: unsafe extern "C" fn(*mut c_void),
    release: unsafe extern "C" fn(*mut c_void),
}

/// The second interface: the first, and then the names and the two-part
/// channel numbering a host actually tunes by.
#[repr(C)]
#[derive(Debug)]
pub struct BonStruct2 {
    base: BonStruct,
    tuner_name: unsafe extern "C" fn(*mut c_void) -> *const u16,
    is_tuner_opening: unsafe extern "C" fn(*mut c_void) -> Bool,
    enum_tuning_space: unsafe extern "C" fn(*mut c_void, u32) -> *const u16,
    enum_channel_name: unsafe extern "C" fn(*mut c_void, u32, u32) -> *const u16,
    set_channel: unsafe extern "C" fn(*mut c_void, u32, u32) -> Bool,
    current_space: unsafe extern "C" fn(*mut c_void) -> u32,
    current_channel: unsafe extern "C" fn(*mut c_void) -> u32,
}

/// The driver and the table that points at it, in one allocation. They are
/// together because the table outlives the call that made it and the host holds
/// only the table: `context` points at the driver beside it, and `release`
/// frees the pair.
#[repr(C)]
#[derive(Debug)]
struct Held {
    table: BonStruct2,
    driver: Locked,
}

/// Builds the table and hands back what the host keeps.
pub fn into_raw(driver: Driver) -> *const BonStruct2 {
    let held = Box::new(Held {
        // Filled in below, once the box has an address: `context` and `end`
        // both point into it, and neither can be written down before it
        // exists.
        table: BonStruct2 {
            base: BonStruct {
                context: core::ptr::null_mut(),
                end: core::ptr::null(),
                open_tuner,
                close_tuner,
                set_channel_byte,
                signal_level,
                wait_ts_stream,
                ready_count,
                get_ts_stream,
                get_ts_stream_ptr,
                purge_ts_stream,
                release,
            },
            tuner_name,
            is_tuner_opening,
            enum_tuning_space,
            enum_channel_name,
            set_channel,
            current_space,
            current_channel,
        },
        driver: Locked::new(driver),
    });
    let held = Box::into_raw(held);
    unsafe {
        let table = &raw mut (*held).table;
        (*table).base.context = held.cast::<c_void>();
        // One past the end of the larger structure, which is how the caller
        // learns this is a 2 rather than a 1.
        (*table).base.end = table.add(1).cast::<c_void>();
        table.cast_const()
    }
}

/// The driver a call arrived for, or nothing for a null pointer. The pointer
/// is the one handed out above, which is the start of the allocation; the
/// driver sits after the table in it.
///
/// # Safety
///
/// The caller must pass back what `into_raw` returned, unreleased, or null.
unsafe fn locked<'a>(context: *mut c_void) -> Option<&'a Locked> {
    if context.is_null() {
        return None;
    }
    Some(unsafe { &(*context.cast::<Held>()).driver })
}

/// Runs one call on the driver, holding its lock, or answers `otherwise` for a
/// null pointer.
///
/// # Safety
///
/// As for [`locked`].
unsafe fn with<R>(context: *mut c_void, otherwise: R, call: impl FnOnce(&mut Driver) -> R) -> R {
    match unsafe { locked(context) } {
        Some(locked) => call(&mut locked.lock()),
        None => otherwise,
    }
}

unsafe extern "C" fn open_tuner(context: *mut c_void) -> Bool {
    if unsafe { with(context, false, |driver| driver.open_tuner()) } {
        TRUE
    } else {
        FALSE
    }
}

unsafe extern "C" fn close_tuner(context: *mut c_void) {
    unsafe { with(context, (), |driver| driver.close_tuner()) };
}

/// The first interface's channel numbering, which has no space in it. Nothing
/// this driver offers is reachable that way, and a host that has the second
/// interface never asks.
unsafe extern "C" fn set_channel_byte(context: *mut c_void, channel: u8) -> Bool {
    if unsafe {
        with(context, false, |driver| {
            driver.set_channel(0, u32::from(channel))
        })
    } {
        TRUE
    } else {
        FALSE
    }
}

unsafe extern "C" fn signal_level(context: *mut c_void) -> f32 {
    unsafe { with(context, 0.0, |driver| driver.signal_level()) }
}

unsafe extern "C" fn wait_ts_stream(context: *mut c_void, timeout_ms: u32) -> u32 {
    match unsafe { locked(context) } {
        Some(locked) => locked.wait_ts_stream(timeout_ms),
        None => crate::driver::WAIT_TIMEOUT,
    }
}

unsafe extern "C" fn ready_count(context: *mut c_void) -> u32 {
    unsafe { with(context, 0, |driver| driver.ready_count()) }
}

/// The copying form: what is ready goes into the caller's buffer, as much of
/// it as fits, and what is left over is reported so the caller knows to ask
/// again.
unsafe extern "C" fn get_ts_stream(
    context: *mut c_void,
    into: *mut u8,
    size: *mut u32,
    remaining: *mut u32,
) -> Bool {
    let Some(locked) = (unsafe { locked(context) }) else {
        return FALSE;
    };
    if size.is_null() {
        return FALSE;
    }
    let mut driver = locked.lock();
    // See the same call in `abi`: never more than the piece a host sizes its
    // buffer for, whatever the size it passed in says.
    let room = if into.is_null() {
        0
    } else {
        usize::try_from(unsafe { *size })
            .unwrap_or(0)
            .min(crate::driver::MOST_COPIED)
    };
    let taken = driver.take_stream_upto(room);
    let length = match taken {
        Some(bytes) => {
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), into, bytes.len()) };
            bytes.len()
        }
        None => 0,
    };
    unsafe { *size = length as u32 };
    if !remaining.is_null() {
        unsafe { *remaining = driver.ready_count() };
    }
    TRUE
}

/// The lending form: the driver keeps the buffer and the caller reads it
/// before the next call.
unsafe extern "C" fn get_ts_stream_ptr(
    context: *mut c_void,
    into: *mut *mut u8,
    size: *mut u32,
    remaining: *mut u32,
) -> Bool {
    let Some(locked) = (unsafe { locked(context) }) else {
        return FALSE;
    };
    let mut driver = locked.lock();
    let taken = driver.take_stream();
    let (pointer, length) = match taken {
        Some(bytes) => (bytes.as_ptr().cast_mut(), bytes.len()),
        None => (core::ptr::null_mut(), 0),
    };
    if !into.is_null() {
        unsafe { *into = pointer };
    }
    if !size.is_null() {
        unsafe { *size = length as u32 };
    }
    if !remaining.is_null() {
        unsafe { *remaining = driver.ready_count() };
    }
    TRUE
}

unsafe extern "C" fn purge_ts_stream(context: *mut c_void) {
    unsafe { with(context, (), |driver| driver.purge_ts_stream()) };
}

/// Gives the driver up. The table went out with it, so both are freed here.
unsafe extern "C" fn release(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(context.cast::<Held>()) });
}

unsafe extern "C" fn tuner_name(context: *mut c_void) -> *const u16 {
    unsafe { with(context, core::ptr::null(), |driver| driver.tuner_name()) }
}

unsafe extern "C" fn is_tuner_opening(context: *mut c_void) -> Bool {
    if unsafe { with(context, false, |driver| driver.is_open()) } {
        TRUE
    } else {
        FALSE
    }
}

unsafe extern "C" fn enum_tuning_space(context: *mut c_void, space: u32) -> *const u16 {
    unsafe {
        with(context, core::ptr::null(), |driver| {
            driver.tuning_space_name(space)
        })
    }
}

unsafe extern "C" fn enum_channel_name(
    context: *mut c_void,
    space: u32,
    channel: u32,
) -> *const u16 {
    unsafe {
        with(context, core::ptr::null(), |driver| {
            driver.channel_name(space, channel)
        })
    }
}

unsafe extern "C" fn set_channel(context: *mut c_void, space: u32, channel: u32) -> Bool {
    if unsafe { with(context, false, |driver| driver.set_channel(space, channel)) } {
        TRUE
    } else {
        FALSE
    }
}

unsafe extern "C" fn current_space(context: *mut c_void) -> u32 {
    unsafe {
        with(context, crate::driver::NOT_TUNED, |driver| {
            driver.current_space()
        })
    }
}

unsafe extern "C" fn current_channel(context: *mut c_void) -> u32 {
    unsafe {
        with(context, crate::driver::NOT_TUNED, |driver| {
            driver.current_channel()
        })
    }
}

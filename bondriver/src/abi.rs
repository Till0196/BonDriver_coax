// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The BonDriver interface as the callers expect to find it in memory.
//!
//! `IBonDriver` and `IBonDriver2` are C++ abstract classes, not COM: a caller
//! reads a vtable pointer from offset zero of the object and calls through it.
//! So what has to be right is the order of the slots and the calling
//! convention.
//!
//! `IBonDriver2` derives from `IBonDriver`, so the base class's ten slots come
//! first in their declaration order, then the seven the derived class adds.
//! `IBonDriver2` redeclares `Release`, which overrides the base slot -- so
//! seventeen slots, not eighteen.
//!
//! The convention is the C++ default for a member function: `__thiscall` on
//! 32-bit and the one convention there is on 64-bit. Rust rejects `extern
//! "thiscall"` outside x86, so the whole table is written once and generated
//! for each.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use std::sync::Once;

use coax_settings::trace;

use crate::driver::{Driver, Locked};
use crate::rtti;

/// Builds the vtable and the thunks for one calling convention. Every thunk
/// turns the `this` pointer back into a [`Driver`] and calls the method of the
/// same name.
macro_rules! define_abi {
    ($abi:literal) => {
        /// The seventeen entries of `IBonDriver2`, in the order a caller reads
        /// them. See the module documentation for where the order comes from.
        #[repr(C)]
        #[derive(Debug)]
        pub struct VTable {
            // IBonDriver
            pub open_tuner: unsafe extern $abi fn(*mut Object) -> i32,
            pub close_tuner: unsafe extern $abi fn(*mut Object),
            pub set_channel_byte: unsafe extern $abi fn(*mut Object, u8) -> i32,
            pub get_signal_level: unsafe extern $abi fn(*mut Object) -> f32,
            pub wait_ts_stream: unsafe extern $abi fn(*mut Object, u32) -> u32,
            pub get_ready_count: unsafe extern $abi fn(*mut Object) -> u32,
            // The two forms of `GetTsStream` sit the other way round from the
            // way the interface writes them: a compiler lays overloads of one
            // name out in the reverse of their declaration order, and a host
            // reads them there.
            pub get_ts_stream_ptr:
                unsafe extern $abi fn(*mut Object, *mut *mut u8, *mut u32, *mut u32) -> i32,
            pub get_ts_stream: unsafe extern $abi fn(*mut Object, *mut u8, *mut u32, *mut u32) -> i32,
            pub purge_ts_stream: unsafe extern $abi fn(*mut Object),
            pub release: unsafe extern $abi fn(*mut Object),
            // IBonDriver2
            pub get_tuner_name: unsafe extern $abi fn(*mut Object) -> *const u16,
            pub is_tuner_opening: unsafe extern $abi fn(*mut Object) -> i32,
            pub enum_tuning_space: unsafe extern $abi fn(*mut Object, u32) -> *const u16,
            pub enum_channel_name: unsafe extern $abi fn(*mut Object, u32, u32) -> *const u16,
            pub set_channel: unsafe extern $abi fn(*mut Object, u32, u32) -> i32,
            pub get_cur_space: unsafe extern $abi fn(*mut Object) -> u32,
            pub get_cur_channel: unsafe extern $abi fn(*mut Object) -> u32,
        }

        unsafe extern $abi fn open_tuner(this: *mut Object) -> i32 {
            trace!("OpenTuner");
            i32::from(unsafe { with(this, false, |driver| driver.open_tuner()) })
        }

        unsafe extern $abi fn close_tuner(this: *mut Object) {
            trace!("CloseTuner");
            unsafe { with(this, (), |driver| driver.close_tuner()) };
        }

        /// The first interface's channel selection, which names a channel by
        /// one byte and nothing else. Nothing has used it since tuning spaces
        /// arrived, and a driver that answered would be guessing which space
        /// the byte belonged to.
        unsafe extern $abi fn set_channel_byte(_this: *mut Object, _channel: u8) -> i32 {
            trace!("SetChannel(byte {_channel})");
            0
        }

        unsafe extern $abi fn get_signal_level(this: *mut Object) -> f32 {
            trace!("GetSignalLevel");
            unsafe { with(this, 0.0, |driver| driver.signal_level()) }
        }

        unsafe extern $abi fn wait_ts_stream(this: *mut Object, timeout_ms: u32) -> u32 {
            trace!("WaitTsStream({timeout_ms})");
            match unsafe { locked(this) } {
                Some(locked) => locked.wait_ts_stream(timeout_ms),
                None => crate::driver::WAIT_TIMEOUT,
            }
        }

        unsafe extern $abi fn get_ready_count(this: *mut Object) -> u32 {
            trace!("GetReadyCount");
            unsafe { with(this, 0, |driver| driver.ready_count()) }
        }

        unsafe extern $abi fn get_ts_stream(
            this: *mut Object,
            destination: *mut u8,
            size: *mut u32,
            remain: *mut u32,
        ) -> i32 {
            trace!("GetTsStream(copy)");
            let Some(locked) = (unsafe { locked(this) }) else {
                unsafe { write_counts(size, remain, 0, 0) };
                return 0;
            };
            let mut driver = locked.lock();
            // The buffer's size comes in where its length goes out. A host that
            // leaves it at zero is taken at its word rather than written over,
            // and one that leaves it unset is never written more than the
            // piece every host sizes its buffer for.
            let room = if size.is_null() || destination.is_null() {
                0
            } else {
                usize::try_from(unsafe { *size })
                    .unwrap_or(0)
                    .min(crate::driver::MOST_COPIED)
            };
            let Some(taken) = driver.take_stream_upto(room) else {
                unsafe { write_counts(size, remain, 0, 0) };
                return 0;
            };
            if !destination.is_null() {
                unsafe {
                    core::ptr::copy_nonoverlapping(taken.as_ptr(), destination, taken.len());
                }
            }
            let length = taken.len() as u32;
            let waiting = driver.ready_count();
            unsafe { write_counts(size, remain, length, waiting) };
            1
        }

        /// The same, handing back a pointer into the driver's own buffer. The
        /// bytes stay valid until the next call, which is what every caller of
        /// this form already assumes.
        unsafe extern $abi fn get_ts_stream_ptr(
            this: *mut Object,
            destination: *mut *mut u8,
            size: *mut u32,
            remain: *mut u32,
        ) -> i32 {
            trace!("GetTsStream(pointer)");
            let Some(locked) = (unsafe { locked(this) }) else {
                unsafe { write_counts(size, remain, 0, 0) };
                if !destination.is_null() {
                    unsafe { *destination = core::ptr::null_mut() };
                }
                return 0;
            };
            let mut driver = locked.lock();
            let Some(taken) = driver.take_stream() else {
                unsafe { write_counts(size, remain, 0, 0) };
                if !destination.is_null() {
                    unsafe { *destination = core::ptr::null_mut() };
                }
                return 0;
            };
            let length = taken.len() as u32;
            let pointer = taken.as_ptr().cast_mut();
            let waiting = driver.ready_count();
            if !destination.is_null() {
                unsafe { *destination = pointer };
            }
            unsafe { write_counts(size, remain, length, waiting) };
            1
        }

        unsafe extern $abi fn purge_ts_stream(this: *mut Object) {
            trace!("PurgeTsStream");
            unsafe { with(this, (), |driver| driver.purge_ts_stream()) };
        }

        unsafe extern $abi fn release(this: *mut Object) {
            trace!("Release");
            if this.is_null() {
                return;
            }
            drop(unsafe { Box::from_raw(this) });
        }

        unsafe extern $abi fn get_tuner_name(this: *mut Object) -> *const u16 {
            trace!("GetTunerName");
            unsafe { with(this, core::ptr::null(), |driver| driver.tuner_name()) }
        }

        unsafe extern $abi fn is_tuner_opening(this: *mut Object) -> i32 {
            trace!("IsTunerOpening");
            i32::from(unsafe { with(this, false, |driver| driver.is_open()) })
        }

        unsafe extern $abi fn enum_tuning_space(this: *mut Object, space: u32) -> *const u16 {
            trace!("EnumTuningSpace({space})");
            unsafe { with(this, core::ptr::null(), |driver| driver.tuning_space_name(space)) }
        }

        unsafe extern $abi fn enum_channel_name(
            this: *mut Object,
            space: u32,
            channel: u32,
        ) -> *const u16 {
            trace!("EnumChannelName({space}, {channel})");
            unsafe {
                with(this, core::ptr::null(), |driver| {
                    driver.channel_name(space, channel)
                })
            }
        }

        unsafe extern $abi fn set_channel(this: *mut Object, space: u32, channel: u32) -> i32 {
            trace!("SetChannel({space}, {channel})");
            i32::from(unsafe { with(this, false, |driver| driver.set_channel(space, channel)) })
        }

        unsafe extern $abi fn get_cur_space(this: *mut Object) -> u32 {
            trace!("GetCurSpace");
            unsafe { with(this, crate::driver::NOT_TUNED, |driver| driver.current_space()) }
        }

        unsafe extern $abi fn get_cur_channel(this: *mut Object) -> u32 {
            trace!("GetCurChannel");
            unsafe { with(this, crate::driver::NOT_TUNED, |driver| driver.current_channel()) }
        }

        /// The table itself, shared by every object.
        pub const VTABLE: VTable = VTable {
            open_tuner,
            close_tuner,
            set_channel_byte,
            get_signal_level,
            wait_ts_stream,
            get_ready_count,
            get_ts_stream_ptr,
            get_ts_stream,
            purge_ts_stream,
            release,
            get_tuner_name,
            is_tuner_opening,
            enum_tuning_space,
            enum_channel_name,
            set_channel,
            get_cur_space,
            get_cur_channel,
        };
    };
}

/// What a caller holds: a vtable pointer at offset zero, which is what makes
/// this a C++ object, and the driver behind it.
#[repr(C)]
#[derive(Debug)]
pub struct Object {
    pub vtable: *const VTable,
    pub driver: Locked,
}

/// The vtable as a C++ object carries it: the run-time type information one
/// slot before the methods. A host reads that slot to answer `dynamic_cast`,
/// and a table without it is followed into whatever lies before it in memory.
/// See [`crate::rtti`].
#[repr(C)]
struct TypedVTable {
    locator: UnsafeCell<*const c_void>,
    methods: VTable,
}

// The one word that is written is written once, before any object exists.
unsafe impl Sync for TypedVTable {}

static TYPED_VTABLE: TypedVTable = TypedVTable {
    locator: UnsafeCell::new(core::ptr::null()),
    methods: VTABLE,
};

/// Puts the type information in front of the table, once.
static ONCE: Once = Once::new();

fn prepare() {
    ONCE.call_once(|| {
        rtti::prepare();
        unsafe { *TYPED_VTABLE.locator.get() = rtti::locator() };
    });
}

impl Object {
    /// Hands out an object for a caller to keep, in the form it expects: a
    /// pointer it will call through and hand back to `Release`.
    pub fn into_raw(driver: Driver) -> *mut c_void {
        prepare();
        let object = Box::new(Object {
            vtable: &raw const TYPED_VTABLE.methods,
            driver: Locked::new(driver),
        });
        Box::into_raw(object).cast()
    }
}

/// Turns a `this` pointer back into the driver behind it, or nothing for a
/// null one.
///
/// # Safety
///
/// The pointer has to be null or one [`Object::into_raw`] handed out and
/// `Release` has not yet been called on.
unsafe fn locked<'a>(this: *mut Object) -> Option<&'a Locked> {
    if this.is_null() {
        return None;
    }
    Some(unsafe { &(*this).driver })
}

/// Runs one call on the driver behind a `this` pointer, holding its lock, or
/// answers `otherwise` for a null one.
///
/// # Safety
///
/// As for [`locked`].
unsafe fn with<R>(this: *mut Object, otherwise: R, call: impl FnOnce(&mut Driver) -> R) -> R {
    match unsafe { locked(this) } {
        Some(locked) => call(&mut locked.lock()),
        None => otherwise,
    }
}

/// Fills in the two counts every stream call answers with, where the caller
/// asked for them.
///
/// # Safety
///
/// Each pointer is either null or writable, which is what the interface says a
/// caller passes.
unsafe fn write_counts(size: *mut u32, remain: *mut u32, length: u32, waiting: u32) {
    if !size.is_null() {
        unsafe { *size = length };
    }
    if !remain.is_null() {
        unsafe { *remain = waiting };
    }
}

#[cfg(target_arch = "x86")]
define_abi!("thiscall");
#[cfg(not(target_arch = "x86"))]
define_abi!("system");

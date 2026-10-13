// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Calling through the interface, written out independently of the driver.
//!
//! The vtable is declared again here rather than shared with the driver crate:
//! a table shared by both sides would agree with itself whatever it said, and
//! the point of this program is that a caller written from the header finds the
//! table where the header says.

use std::slice;

use crate::CreateBonDriver;

macro_rules! define_calls {
    ($abi:literal) => {
        #[repr(C)]
        #[derive(Debug)]
        struct VTable {
            open_tuner: unsafe extern $abi fn(*mut Object) -> i32,
            close_tuner: unsafe extern $abi fn(*mut Object),
            set_channel_byte: unsafe extern $abi fn(*mut Object, u8) -> i32,
            get_signal_level: unsafe extern $abi fn(*mut Object) -> f32,
            wait_ts_stream: unsafe extern $abi fn(*mut Object, u32) -> u32,
            get_ready_count: unsafe extern $abi fn(*mut Object) -> u32,
            // In the order a host reads them, which is the reverse of the order
            // the interface writes the two forms of `GetTsStream` in.
            get_ts_stream_ptr:
                unsafe extern $abi fn(*mut Object, *mut *mut u8, *mut u32, *mut u32) -> i32,
            get_ts_stream: unsafe extern $abi fn(*mut Object, *mut u8, *mut u32, *mut u32) -> i32,
            purge_ts_stream: unsafe extern $abi fn(*mut Object),
            release: unsafe extern $abi fn(*mut Object),
            get_tuner_name: unsafe extern $abi fn(*mut Object) -> *const u16,
            is_tuner_opening: unsafe extern $abi fn(*mut Object) -> i32,
            enum_tuning_space: unsafe extern $abi fn(*mut Object, u32) -> *const u16,
            enum_channel_name: unsafe extern $abi fn(*mut Object, u32, u32) -> *const u16,
            set_channel: unsafe extern $abi fn(*mut Object, u32, u32) -> i32,
            get_cur_space: unsafe extern $abi fn(*mut Object) -> u32,
            get_cur_channel: unsafe extern $abi fn(*mut Object) -> u32,
        }
    };
}

#[cfg(target_arch = "x86")]
define_calls!("thiscall");
#[cfg(not(target_arch = "x86"))]
define_calls!("system");

#[repr(C)]
#[derive(Debug)]
struct Object {
    vtable: *const VTable,
}

/// A driver handed out by `CreateBonDriver`, released when this is dropped.
#[derive(Debug)]
pub struct Bon {
    object: *mut Object,
}

impl Bon {
    /// Asks the library for a driver. Nothing back is an answer, not a fault:
    /// a library says so when it cannot serve.
    pub fn create(create: CreateBonDriver) -> Option<Self> {
        let object = unsafe { create() }.cast::<Object>();
        if object.is_null() {
            return None;
        }
        Some(Self { object })
    }

    fn vtable(&self) -> &VTable {
        unsafe { &*(*self.object).vtable }
    }

    pub fn tuner_name(&self) -> String {
        read_wide(unsafe { (self.vtable().get_tuner_name)(self.object) })
            .unwrap_or_else(|| "(none)".to_owned())
    }

    pub fn tuning_space_name(&self, space: u32) -> Option<String> {
        read_wide(unsafe { (self.vtable().enum_tuning_space)(self.object, space) })
    }

    pub fn channel_name(&self, space: u32, channel: u32) -> Option<String> {
        read_wide(unsafe { (self.vtable().enum_channel_name)(self.object, space, channel) })
    }

    pub fn open_tuner(&self) -> bool {
        unsafe { (self.vtable().open_tuner)(self.object) != 0 }
    }

    pub fn is_tuner_opening(&self) -> bool {
        unsafe { (self.vtable().is_tuner_opening)(self.object) != 0 }
    }

    pub fn set_channel(&self, space: u32, channel: u32) -> bool {
        unsafe { (self.vtable().set_channel)(self.object, space, channel) != 0 }
    }

    pub fn current_space(&self) -> u32 {
        unsafe { (self.vtable().get_cur_space)(self.object) }
    }

    pub fn current_channel(&self) -> u32 {
        unsafe { (self.vtable().get_cur_channel)(self.object) }
    }

    pub fn signal_level(&self) -> f32 {
        unsafe { (self.vtable().get_signal_level)(self.object) }
    }

    pub fn wait_ts_stream(&self, timeout_ms: u32) -> u32 {
        unsafe { (self.vtable().wait_ts_stream)(self.object, timeout_ms) }
    }

    /// Takes what has arrived, in the form that borrows the driver's buffer.
    /// The bytes are good until the next call, which is what the interface
    /// promises and what every host relies on.
    pub fn take_ts_stream(&self) -> Option<&[u8]> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut size: u32 = 0;
        let mut remain: u32 = 0;
        let taken = unsafe {
            (self.vtable().get_ts_stream_ptr)(self.object, &mut pointer, &mut size, &mut remain)
        };
        if taken == 0 || pointer.is_null() || size == 0 {
            return None;
        }
        Some(unsafe { slice::from_raw_parts(pointer, size as usize) })
    }
}

impl Drop for Bon {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.object) };
    }
}

/// Reads one of the null-terminated UTF-16 strings the interface hands back.
fn read_wide(pointer: *const u16) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    // Bounded, so that a string the driver forgot to terminate is a wrong
    // answer rather than a walk off the end of the world.
    while length < 4096 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    let units = unsafe { slice::from_raw_parts(pointer, length) };
    Some(String::from_utf16_lossy(units))
}

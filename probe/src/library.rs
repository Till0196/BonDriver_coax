// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Loading a driver the way a host loads one: `LoadLibrary`, then
//! `GetProcAddress` for the one undecorated export. Declared here rather than
//! taken from a crate, because the whole point of this program is to depend on
//! nothing but the interface under test.

use std::ffi::{CString, c_void};
use std::path::Path;

use crate::{CreateBonDriver, wide};

type Handle = *mut c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn FreeLibrary(library: Handle) -> i32;
    fn GetProcAddress(library: Handle, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// A loaded driver, unloaded when this is dropped.
#[derive(Debug)]
pub struct Library {
    handle: Handle,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, String> {
        // An absolute path, so that the loader does not go looking down its
        // search order and find another driver of the same name. Absolute
        // rather than canonical: canonicalising on Windows returns
        // `\\?\C:\...`, which leaves the module under a name a debugger will
        // not match.
        let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let name = wide(full.as_os_str());
        let handle = unsafe { LoadLibraryW(name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "LoadLibrary({}) failed with error {}",
                full.display(),
                unsafe { GetLastError() }
            ));
        }
        Ok(Self { handle })
    }

    /// The export a host looks for, spelled exactly as it looks for it.
    pub fn create_bon_driver(&self) -> Result<CreateBonDriver, String> {
        let name = CString::new("CreateBonDriver").expect("the name holds no null");
        let address = unsafe { GetProcAddress(self.handle, name.as_ptr().cast()) };
        if address.is_null() {
            return Err(format!(
                "the library exports no CreateBonDriver (error {})",
                unsafe { GetLastError() }
            ));
        }
        Ok(unsafe { std::mem::transmute::<*mut c_void, CreateBonDriver>(address) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.handle) };
    }
}

// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

#![cfg(windows)]

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[test]
#[ignore = "requires COAX_WINSCARD_TEST_DLL pointing to a built Windows DLL"]
fn decoder_reload_keeps_the_proxy() {
    let path = std::env::var_os("COAX_WINSCARD_TEST_DLL").expect("COAX_WINSCARD_TEST_DLL");
    let path = wide(&path);
    let name = wide(OsStr::new("winscard.dll"));
    unsafe {
        let first = LoadLibraryW(path.as_ptr());
        assert!(!first.is_null(), "load the proxy by its full path");
        let establish = GetProcAddress(first, c"SCardEstablishContext".as_ptr().cast());
        assert!(!establish.is_null());

        // Forward an invalid context to the system DLL without opening a
        // reader, contacting a daemon, or requiring any hardware.
        let address = GetProcAddress(first, c"SCardGetDeviceTypeIdW".as_ptr().cast());
        assert!(!address.is_null());
        let get_device_type: unsafe extern "system" fn(usize, *const u16, *mut u32) -> i32 =
            std::mem::transmute(address);
        let reader = wide(OsStr::new("Test Reader"));
        let mut device_type = 0;
        let _ = get_device_type(0, reader.as_ptr(), &mut device_type);
        assert_ne!(FreeLibrary(first), 0);

        for _ in 0..3 {
            // A decoder imports by basename when it is loaded again. A
            // surviving system DLL must not replace the proxy at this point.
            let again = LoadLibraryW(name.as_ptr());
            assert!(!again.is_null());
            let actual = GetProcAddress(again, c"SCardEstablishContext".as_ptr().cast());
            assert_eq!(
                actual, establish,
                "decoder rebound to the system WinSCard DLL"
            );
            assert_ne!(FreeLibrary(again), 0);
        }
    }
}

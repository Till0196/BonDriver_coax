// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The real `WinSCard.dll`, which this library stands in front of rather than
//! replaces.
//!
//! A viewer who already has a reader plugged in must go on being able to use
//! it: dropping this file beside an application adds the built-in slot rather
//! than taking the others away. So everything that names a reader this does not
//! serve is passed through.
//!
//! It is loaded from the system directory by absolute path. Loading it by name
//! would find this file instead, since the application's own directory is
//! searched first.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::abi::{Handle, IoRequest, ReaderStateA, ReaderStateW, Status};

pub type EstablishContext =
    unsafe extern "system" fn(u32, *const c_void, *const c_void, *mut Handle) -> Status;
pub type OnHandle = unsafe extern "system" fn(Handle) -> Status;
pub type HandleAndWord = unsafe extern "system" fn(Handle, u32) -> Status;
pub type ListReadersA = unsafe extern "system" fn(Handle, *const u8, *mut u8, *mut u32) -> Status;
pub type ListReadersW = unsafe extern "system" fn(Handle, *const u16, *mut u16, *mut u32) -> Status;
pub type ListGroupsA = unsafe extern "system" fn(Handle, *mut u8, *mut u32) -> Status;
pub type ListGroupsW = unsafe extern "system" fn(Handle, *mut u16, *mut u32) -> Status;
pub type FreeMemory = unsafe extern "system" fn(Handle, *const c_void) -> Status;
pub type ConnectA =
    unsafe extern "system" fn(Handle, *const u8, u32, u32, *mut Handle, *mut u32) -> Status;
pub type ConnectW =
    unsafe extern "system" fn(Handle, *const u16, u32, u32, *mut Handle, *mut u32) -> Status;
pub type Reconnect = unsafe extern "system" fn(Handle, u32, u32, u32, *mut u32) -> Status;
pub type StatusA = unsafe extern "system" fn(
    Handle,
    *mut u8,
    *mut u32,
    *mut u32,
    *mut u32,
    *mut u8,
    *mut u32,
) -> Status;
pub type StatusW = unsafe extern "system" fn(
    Handle,
    *mut u16,
    *mut u32,
    *mut u32,
    *mut u32,
    *mut u8,
    *mut u32,
) -> Status;
pub type GetStatusChangeA =
    unsafe extern "system" fn(Handle, u32, *mut ReaderStateA, u32) -> Status;
pub type GetStatusChangeW =
    unsafe extern "system" fn(Handle, u32, *mut ReaderStateW, u32) -> Status;
pub type Transmit = unsafe extern "system" fn(
    Handle,
    *const IoRequest,
    *const u8,
    u32,
    *mut IoRequest,
    *mut u8,
    *mut u32,
) -> Status;
pub type Control = unsafe extern "system" fn(
    Handle,
    u32,
    *const c_void,
    u32,
    *mut c_void,
    u32,
    *mut u32,
) -> Status;
pub type GetAttrib = unsafe extern "system" fn(Handle, u32, *mut u8, *mut u32) -> Status;
pub type SetAttrib = unsafe extern "system" fn(Handle, u32, *const u8, u32) -> Status;

macro_rules! entry_points {
    ($($name:ident : $signature:ty => $exported:literal),* $(,)?) => {
        /// The system library's entry points, each missing when the system has
        /// no smart card service to speak of.
        #[derive(Debug, Default)]
        pub struct System {
            $(pub $name: Option<$signature>,)*
        }

        impl System {
            fn load() -> Self {
                let module = library();
                if module.is_null() {
                    return Self::default();
                }
                Self {
                    $($name: unsafe { resolve(module, concat!($exported, "\0")) },)*
                }
            }
        }
    };
}

entry_points! {
    establish_context: EstablishContext => "SCardEstablishContext",
    release_context: OnHandle => "SCardReleaseContext",
    is_valid_context: OnHandle => "SCardIsValidContext",
    list_readers_a: ListReadersA => "SCardListReadersA",
    list_readers_w: ListReadersW => "SCardListReadersW",
    list_reader_groups_a: ListGroupsA => "SCardListReaderGroupsA",
    list_reader_groups_w: ListGroupsW => "SCardListReaderGroupsW",
    free_memory: FreeMemory => "SCardFreeMemory",
    connect_a: ConnectA => "SCardConnectA",
    connect_w: ConnectW => "SCardConnectW",
    reconnect: Reconnect => "SCardReconnect",
    disconnect: HandleAndWord => "SCardDisconnect",
    begin_transaction: OnHandle => "SCardBeginTransaction",
    end_transaction: HandleAndWord => "SCardEndTransaction",
    status_a: StatusA => "SCardStatusA",
    status_w: StatusW => "SCardStatusW",
    get_status_change_a: GetStatusChangeA => "SCardGetStatusChangeA",
    get_status_change_w: GetStatusChangeW => "SCardGetStatusChangeW",
    transmit: Transmit => "SCardTransmit",
    control: Control => "SCardControl",
    get_attrib: GetAttrib => "SCardGetAttrib",
    set_attrib: SetAttrib => "SCardSetAttrib",
    cancel: OnHandle => "SCardCancel",
}

/// One entry point of the system library by name, for the calls this library
/// has nothing to add to and only passes on.
///
/// # Safety
///
/// The caller must name an entry point whose signature is the one asked for,
/// ending in a nul.
pub unsafe fn entry<T: Copy>(name: &str) -> Option<T> {
    let module = library();
    if module.is_null() {
        return None;
    }
    unsafe { resolve(module, name) }
}

/// The system library, as it is the one and only time it is asked for.
pub fn system() -> &'static System {
    static LOADED: OnceLock<System> = OnceLock::new();
    LOADED.get_or_init(System::load)
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(into: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

fn library() -> *mut c_void {
    static MODULE: OnceLock<usize> = OnceLock::new();
    *MODULE.get_or_init(|| {
        let mut directory = [0u16; 260];
        let length =
            unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
        if length == 0 || length >= directory.len() {
            return 0;
        }
        let mut path: Vec<u16> = directory[..length].to_vec();
        path.extend("\\WinSCard.dll\0".encode_utf16());
        let module = unsafe { LoadLibraryW(path.as_ptr()) };
        module as usize
    }) as *mut c_void
}

/// # Safety
///
/// The caller must name an entry point whose signature is the one asked for.
unsafe fn resolve<T: Copy>(module: *mut c_void, name: &str) -> Option<T> {
    let found = unsafe { GetProcAddress(module, name.as_ptr()) };
    if found.is_null() {
        return None;
    }
    // A function pointer and a code address are the same width; the type is
    // the caller's promise about what is at it.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&found) })
}

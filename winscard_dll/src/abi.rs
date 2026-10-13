// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

// The exported calls read these, and those are built only on the platform
// whose ABI they are. Elsewhere the module is here for the tests below it.
#![cfg_attr(not(windows), allow(dead_code))]

//! The shapes and numbers of the Windows API this library stands in for.
//!
//! Transcribed from `winscard.h`. The daemon's own values are Windows' where
//! they agree, and this file holds the three places they do not: the state of a
//! card, the raw protocol, and the flags of `SCardGetStatusChange`.

use std::ffi::c_void;

/// A context or a card, which the API carries as a pointer-sized number.
pub type Handle = usize;
/// What every entry point returns.
pub type Status = i32;

pub const SCARD_S_SUCCESS: Status = 0;
pub const SCARD_E_CANCELLED: Status = 0x8010_0002u32 as i32;
pub const SCARD_E_INVALID_HANDLE: Status = 0x8010_0003u32 as i32;
pub const SCARD_E_INVALID_PARAMETER: Status = 0x8010_0004u32 as i32;
pub const SCARD_E_NO_MEMORY: Status = 0x8010_0006u32 as i32;
pub const SCARD_E_INSUFFICIENT_BUFFER: Status = 0x8010_0008u32 as i32;
pub const SCARD_E_UNKNOWN_READER: Status = 0x8010_0009u32 as i32;
pub const SCARD_E_TIMEOUT: Status = 0x8010_000Au32 as i32;
pub const SCARD_E_NO_SERVICE: Status = 0x8010_001Du32 as i32;
pub const SCARD_E_NO_READERS_AVAILABLE: Status = 0x8010_002Eu32 as i32;

/// The protocols, which agree with the daemon's for the two that matter.
pub const SCARD_PROTOCOL_T1: u32 = 0x0002;

/// `SCARD_AUTOALLOCATE`: the caller wants the library to find the room.
pub const SCARD_AUTOALLOCATE: u32 = 0xFFFF_FFFF;

/// The flags of `SCardGetStatusChange`.
pub const SCARD_STATE_IGNORE: u32 = 0x0001;
pub const SCARD_STATE_CHANGED: u32 = 0x0002;
pub const SCARD_STATE_UNKNOWN: u32 = 0x0004;
pub const SCARD_STATE_EMPTY: u32 = 0x0010;
pub const SCARD_STATE_PRESENT: u32 = 0x0020;
pub const SCARD_STATE_EXCLUSIVE: u32 = 0x0080;
pub const SCARD_STATE_INUSE: u32 = 0x0100;

/// `INFINITE`, the timeout that never expires.
pub const INFINITE: u32 = 0xFFFF_FFFF;

/// The pseudo-reader an application watches to be told that the set of readers
/// changed, rather than that a card moved.
pub const PNP_NOTIFICATION: &str = "\\\\?PnP?\\Notification";

/// `SCARD_IO_REQUEST`, the protocol header a transmission carries.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoRequest {
    pub protocol: u32,
    pub pci_length: u32,
}

/// `SCARD_READERSTATEA`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReaderStateA {
    pub reader: *const u8,
    pub user_data: *mut c_void,
    pub current_state: u32,
    pub event_state: u32,
    pub atr_length: u32,
    pub atr: [u8; 36],
}

/// `SCARD_READERSTATEW`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReaderStateW {
    pub reader: *const u16,
    pub user_data: *mut c_void,
    pub current_state: u32,
    pub event_state: u32,
    pub atr_length: u32,
    pub atr: [u8; 36],
}

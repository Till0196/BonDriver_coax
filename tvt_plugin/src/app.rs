// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Talking to the host.
//!
//! Everything the host offers goes through one callback: a message number and
//! two words. What is here is the few messages this plugin sends, each with its
//! own name.

use core::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::sdk::{
    CommandInfo, EventCallback, MESSAGE_ADDLOG, MESSAGE_DOCOMMAND, MESSAGE_GETDRIVERFULLPATHNAME,
    MESSAGE_GETDRIVERNAME, MESSAGE_REGISTERCOMMAND, MESSAGE_SETDRIVERNAME,
    MESSAGE_SETEVENTCALLBACK, PluginParam,
};
use coax_settings::wide::Wide;

/// The host, as this plugin talks to it.
#[derive(Debug, Clone, Copy)]
pub struct Host {
    param: *mut PluginParam,
}

// The host calls the plugin from more than one thread and the parameter block
// outlives every call until the plugin is taken down, so this may be carried to
// the thread that does the looking. That thread is stopped and waited for when
// the plugin is taken down; see `forget`.
unsafe impl Send for Host {}
unsafe impl Sync for Host {}

/// Set when the host takes the plugin down. Its parameter block is not this
/// plugin's to read after that, so a thread still finishing up finds every
/// call to the host answering nothing.
static GONE: AtomicBool = AtomicBool::new(false);

/// Stops every [`Host`] reaching the host from now on.
pub fn forget() {
    GONE.store(true, Ordering::SeqCst);
}

/// Lets them reach it again, for a host that loads the plugin a second time
/// without unloading it.
pub fn remember() {
    GONE.store(false, Ordering::SeqCst);
}

impl Host {
    /// # Safety
    ///
    /// The pointer is the host's own, and is good until the plugin is taken
    /// down.
    pub unsafe fn new(param: *mut PluginParam) -> Self {
        Self { param }
    }

    /// The main window, for the messages a viewer has to see.
    pub fn window(&self) -> *mut c_void {
        if GONE.load(Ordering::SeqCst) {
            return core::ptr::null_mut();
        }
        unsafe { (*self.param).window }
    }

    fn send(&self, message: u32, first: isize, second: isize) -> isize {
        if GONE.load(Ordering::SeqCst) {
            return 0;
        }
        let Some(callback) = (unsafe { (*self.param).callback }) else {
            return 0;
        };
        unsafe { callback(self.param, message, first, second) }
    }

    /// Asks to be told about events, which is how a command reaches the
    /// plugin.
    pub fn listen(&self, callback: EventCallback) -> bool {
        self.send(MESSAGE_SETEVENTCALLBACK, callback as usize as isize, 0) != 0
    }

    /// Registers a command, which puts it where a viewer can choose it and
    /// where a keyboard shortcut can be assigned to it.
    pub fn register_command(&self, id: i32, text: &Wide, name: &Wide) -> bool {
        let info = CommandInfo {
            id,
            text: text.as_ptr(),
            name: name.as_ptr(),
        };
        self.send(MESSAGE_REGISTERCOMMAND, (&raw const info) as isize, 1) != 0
    }

    /// Writes a line into the host's own log, which is where a viewer looks
    /// when something took longer than they expected.
    pub fn log(&self, text: &str) {
        let text = Wide::new(text);
        self.send(MESSAGE_ADDLOG, text.as_ptr() as isize, 0);
    }

    /// The file name of the driver in use, without its path.
    pub fn driver_name(&self) -> Option<String> {
        self.string(MESSAGE_GETDRIVERNAME)
    }

    /// Where that driver is, in full.
    pub fn driver_path(&self) -> Option<PathBuf> {
        self.string(MESSAGE_GETDRIVERFULLPATHNAME)
            .map(PathBuf::from)
    }

    /// Loads a driver by name. Naming the one already in use does nothing at
    /// all, so the way to have the channel list built again is
    /// [`Self::close_tuner`] and then this -- opening a tuner is what builds
    /// that list.
    pub fn set_driver(&self, name: &str) -> bool {
        let name = Wide::new(name);
        self.send(MESSAGE_SETDRIVERNAME, name.as_ptr() as isize, 0) != 0
    }

    /// Runs one of the host's own commands, by the name it is registered under.
    /// Worth having for two: "CloseTuner" puts the tuner down, and the name of
    /// a driver file picks that driver up again -- and unlike naming the driver
    /// outright, that one puts the viewer back on the channel they were
    /// watching.
    pub fn command(&self, name: &str) -> bool {
        let name = Wide::new(name);
        self.send(MESSAGE_DOCOMMAND, name.as_ptr() as isize, 0) != 0
    }

    /// Lets go of the tuner, so that something else can have it. What the host
    /// was watching is remembered, and comes back when the driver is picked up
    /// again.
    pub fn close_tuner(&self) -> bool {
        self.command("CloseTuner")
    }

    /// The driver's file name on its own, which is what the host registers its
    /// "switch to this driver" command under.
    pub fn driver_file_name(&self) -> Option<String> {
        self.driver_path()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .or_else(|| self.driver_name())
    }

    /// The messages that answer into a buffer answer their length when asked
    /// with none, so they are asked twice.
    fn string(&self, message: u32) -> Option<String> {
        let length = self.send(message, 0, 0);
        if length <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let written = self.send(message, buffer.as_mut_ptr() as isize, buffer.len() as isize);
        if written <= 0 {
            return None;
        }
        let text: Vec<u16> = buffer
            .into_iter()
            .take_while(|letter| *letter != 0)
            .collect();
        Some(String::from_utf16_lossy(&text))
    }
}

/// Shows a viewer something they asked for the result of.
#[cfg(windows)]
pub fn tell(window: *mut c_void, title: &str, text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(window: *mut c_void, text: *const u16, title: *const u16, kind: u32) -> i32;
    }

    /// An informational icon, and nothing to decide.
    const OK: u32 = 0x0000_0040;

    let text = Wide::new(text);
    let title = Wide::new(title);
    unsafe { MessageBoxW(window, text.as_ptr(), title.as_ptr(), OK) };
}

/// Asks a viewer something with a yes and a no, and says which they chose.
#[cfg(windows)]
pub fn ask(window: *mut c_void, title: &str, text: &str) -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(window: *mut c_void, text: *const u16, title: *const u16, kind: u32) -> i32;
    }

    /// A question, with yes and no to answer it, and no as the default: the
    /// question is asked before something is thrown away.
    const YES_NO: u32 = 0x0000_0004 | 0x0000_0020 | 0x0000_0100;
    const YES: i32 = 6;

    let text = Wide::new(text);
    let title = Wide::new(title);
    unsafe { MessageBoxW(window, text.as_ptr(), title.as_ptr(), YES_NO) == YES }
}

/// The host this is a plugin for runs on one system only. The rest of the
/// plugin is built elsewhere so that its tests can be run there, and this is
/// the one call with nowhere to go.
#[cfg(not(windows))]
pub fn tell(_window: *mut c_void, _title: &str, _text: &str) {}

#[cfg(not(windows))]
pub fn ask(_window: *mut c_void, _title: &str, _text: &str) -> bool {
    false
}

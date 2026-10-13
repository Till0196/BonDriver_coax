// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The part of the host's plugin interface this plugin uses.
//!
//! The interface is a header of enumerations and structures; what is here is
//! the handful this needs, written out. The numbers are fixed: a plugin built
//! against one version of the host runs against the others.

use core::ffi::c_void;

/// The version of the plugin interface this is written against. The host packs
/// it as `(major << 24) | (minor << 12) | revision`, and this is written
/// against 0.0.15.
pub const PLUGIN_VERSION: u32 = 15;

/// An ordinary plugin, as opposed to nothing else the host has yet defined.
pub const PLUGIN_TYPE_NORMAL: u32 = 0;

/// The plugin has a settings dialog, which is what makes the settings button
/// of the host's plugin list pressable. That button is the way a viewer looks
/// for a plugin's own screen, so this plugin answers it with its menu.
pub const PLUGIN_FLAG_HASSETTINGS: u32 = 0x0000_0001;

/// The plugin was switched on or off in the host's plugin menu: which way is
/// the first parameter.
pub const EVENT_PLUGINENABLE: u32 = 0;

pub const MESSAGE_SETEVENTCALLBACK: u32 = 3;
pub const MESSAGE_GETDRIVERNAME: u32 = 11;
pub const MESSAGE_SETDRIVERNAME: u32 = 12;
pub const MESSAGE_REGISTERCOMMAND: u32 = 53;
pub const MESSAGE_DOCOMMAND: u32 = 57;
pub const MESSAGE_ADDLOG: u32 = 54;
pub const MESSAGE_GETDRIVERFULLPATHNAME: u32 = 62;

/// The settings button was pressed: the window to hang a dialog on is the
/// first parameter.
pub const EVENT_PLUGINSETTINGS: u32 = 1;

/// A command was chosen: which one is the first parameter.
/// The host's colours changed: a viewer switched theme, or turned the dark one
/// on.
pub const EVENT_COLORCHANGE: u32 = 11;
pub const EVENT_COMMAND: u32 = 13;

/// What the host hands the plugin, and what the plugin talks back through.
///
/// Packed, like everything else the interface passes: the header packs its
/// structures to a byte, so a compiler's own idea of where a member sits is
/// wrong -- measured here, the host read this plugin's command name from four
/// bytes past where the compiler put it.
#[derive(Debug)]
#[repr(C, packed(1))]
pub struct PluginParam {
    pub callback: Option<unsafe extern "system" fn(*mut PluginParam, u32, isize, isize) -> isize>,
    pub window: *mut c_void,
    pub client_data: *mut c_void,
    /// The host's own, not to be touched.
    pub internal_data: *mut c_void,
}

/// What the plugin says about itself, filled in when the host asks.
#[derive(Debug)]
#[repr(C, packed(1))]
pub struct PluginInfo {
    pub kind: u32,
    pub flags: u32,
    pub name: *const u16,
    pub copyright: *const u16,
    pub description: *const u16,
}

/// One command, as the host registers it: the text a viewer sees and the name
/// it is stored under.
#[derive(Debug)]
#[repr(C, packed(1))]
pub struct CommandInfo {
    pub id: i32,
    pub text: *const u16,
    pub name: *const u16,
}

/// The callback the host sends events to.
pub type EventCallback = unsafe extern "system" fn(u32, isize, isize, *mut c_void) -> isize;

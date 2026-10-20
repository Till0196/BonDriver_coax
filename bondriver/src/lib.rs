// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `BonDriver_coax` — a BonDriver whose tuner is a `coaxd` daemon.
//!
//! The host loads this library, asks for one exported function, and talks to
//! what it hands back through a vtable. See [`abi`] for that shape and
//! [`driver`] for what the calls mean.

#[cfg(windows)]
mod abi;
mod bonstruct;
mod driver;
#[cfg(windows)]
mod rtti;
mod stream;

#[cfg(windows)]
use core::ffi::c_void;

use coax::daemon::{Daemon, Endpoint};
use coax::launch::Launcher;
use coax_settings::beside;
use coax_settings::config::Config;
use coax_settings::defined::Defined;
use coax_settings::lineup;
use coax_settings::trace;

use crate::driver::Driver;
/// Builds the driver from the settings beside this library. Shared by both
/// entry points, because what a host is handed differs between them and what it
/// is handed a view of does not. Builds the driver from the settings beside
/// this library.
///
/// Shared by both entry points, because what a host is handed differs between
/// them and what it is handed a view of does not.
fn build() -> Driver {
    coax_settings::name_agent();
    // The file this copy's own name points at, holding both the settings and
    // the channels something has looked for. A BonDriver is installed by
    // copying it and adding a suffix -- one per lead -- and every copy reading
    // one file would make the suffixes decorative. See `beside::settings_for`.
    let settings = beside::own_path().map(|library| beside::settings_for(&library));
    let text = settings
        .as_deref()
        .and_then(beside::read_file)
        .unwrap_or_default();
    let config = Config::parse(&text);
    let defined = Defined::parse(&text);
    let daemon = Daemon::new(Endpoint::parse(&config.host)).with_priority(config.priority);
    let launcher = config.start_daemon.then(|| Launcher {
        named: (!config.daemon.is_empty()).then(|| config.daemon.clone().into()),
        beside: beside::own_directory(),
    });
    let lineup = lineup::build_with(&config, &defined);
    trace::line(&format!(
        "settings {:?}, daemon at {:?}, priority {}, will start it: {}, spaces: {:?}",
        settings,
        daemon.endpoint(),
        daemon.priority(),
        launcher.is_some(),
        lineup
            .spaces()
            .iter()
            .map(|space| (space.name.as_str(), space.channels.len()))
            .collect::<Vec<_>>()
    ));
    Driver::new(
        lineup,
        "coax",
        daemon,
        launcher,
        config.declared_routes(),
        config.tuner_settings.clone(),
    )
}

/// The one export a host looks for. The name is not decorated: a host calls
/// `GetProcAddress(library, "CreateBonDriver")` and expects that spelling
/// exactly. A null answer means the driver could not be made, which a host
/// reads as "this library is not usable" rather than as a crash.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "C" fn CreateBonDriver() -> *mut c_void {
    trace::line("CreateBonDriver");
    abi::Object::into_raw(build())
}

/// The other way in: a table of plain functions rather than a C++ object.
///
/// EDCB asks for this one first and only falls back to `CreateBonDriver` when
/// it is missing, and it is what makes this library usable outside Windows at
/// all. See `bonstruct`.
#[unsafe(no_mangle)]
pub extern "C" fn CreateBonStruct() -> *const bonstruct::BonStruct2 {
    trace::line("CreateBonStruct");
    bonstruct::into_raw(build())
}

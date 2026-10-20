// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The settings file beside a BonDriver, and what is built out of it.
//!
//! Everything here is downstream of `BonDriver_coax.ini`: reading it, the
//! channels its sections define, writing it back when the plugin has found
//! more, and the list a television host is offered. Beside those sit the two
//! small pieces of Windows plumbing a library loaded into somebody else's
//! process needs -- finding the directory it was loaded from, and handing
//! strings back in the form a C++ vtable expects.
//!
//! It was split out because the other half -- talking to `coaxd` -- is wanted
//! by things that are not BonDrivers at all. What is left here is shaped by the
//! host it serves; `coax` is shaped by the daemon it asks.

pub mod beside;
pub mod channels;
pub mod config;
pub mod defined;
pub mod lineup;
pub mod setup;
pub mod trace;
pub mod tuner_settings;
pub mod wide;

/// Names this release in the `User-Agent` of every request to the daemon, so
/// that its log tells one release from another. Each library here holds its own
/// copy of `coax`, so each calls this before its first request.
pub fn name_agent() {
    coax::http::name_agent(concat!("BonDriver_coax/", env!("CARGO_PKG_VERSION")));
}

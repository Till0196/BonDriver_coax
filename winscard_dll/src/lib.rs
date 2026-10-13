// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `WinSCard.dll` for the card slot built into a tuner board.
//!
//! The slots these boards carry are not CCID readers: the card is reached over
//! the bridge, so the operating system does not see a reader and no PC/SC
//! application can find one. `coaxd` already talks to the card, over the same
//! HTTP it serves tuners on. This library is the shape that is missing: a file
//! called `WinSCard.dll` dropped beside an application, which finds it before
//! the system's own copy.
//!
//! It reaches the daemon itself. That is not a detail: this is loaded into a
//! viewer's television software, so anything it needed another process to
//! provide would be another thing to install and keep running.
//!
//! It stands in front of the system library rather than replacing it. A reader
//! this does not serve is passed straight through, so one program can use both
//! at once -- which is the ordinary case here, a card in the board's own slot
//! and another in a reader plugged in beside it.

// The constants and the daemon link are ordinary code with nothing of the
// platform in them, and are built everywhere so their tests run everywhere.
// What is Windows-only is the parts that are a DLL.
mod abi;
#[cfg(windows)]
mod api;
mod link;
#[cfg(windows)]
mod passthrough;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod system;
mod wire;

// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! A record of what the host asked for, for when the host is the thing that
//! breaks.
//!
//! A driver runs inside someone else's process. When that process dies there is
//! no console and often no debugger that can be attached, so the only account
//! of what happened is one the driver wrote down as it went.
//!
//! Off unless `COAX_TRACE` names a file.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::OnceLock;

/// Where to write, decided once. Absent means tracing is off.
static PATH: OnceLock<Option<String>> = OnceLock::new();

fn path() -> Option<&'static String> {
    PATH.get_or_init(|| std::env::var("COAX_TRACE").ok())
        .as_ref()
}

/// Writes one line, opening and closing the file each time so that a process
/// which dies in the next instant still leaves it behind.
pub fn line(text: &str) {
    let Some(path) = path() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }
}

/// Writes one line, built only when tracing is on.
#[macro_export]
macro_rules! trace {
    ($($argument:tt)*) => {
        if $crate::trace::enabled() {
            $crate::trace::line(&format!($($argument)*));
        }
    };
}

pub fn enabled() -> bool {
    path().is_some()
}

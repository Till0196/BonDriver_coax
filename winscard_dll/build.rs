// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The real `winscard.dll` publishes eighty entry points, and an application
//! binds to every one it imports when it is loaded: a table with a gap in it
//! is an application that will not start. See `coax_exports`.

fn main() {
    coax_exports::write(&["src/api.rs", "src/passthrough.rs"], 80);
}

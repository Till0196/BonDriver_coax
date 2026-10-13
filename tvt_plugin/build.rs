// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! TVTest looks for four functions by name. On 32-bit Windows the linker
//! decorates them and the host then finds none of them -- it does not complain,
//! it just does not list the plugin. See `coax_exports`.

fn main() {
    coax_exports::write(&["src/lib.rs"], 4);
}

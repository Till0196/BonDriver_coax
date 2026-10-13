// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Writes the module definition that fixes a library's export table.
//!
//! Called from the build script of each library that Windows loads by name. Two
//! things are wrong with what a linker exports by itself.
//!
//! On 32-bit Windows the calling convention decorates a name with the size of
//! its arguments: `TVTInitialize` goes out as `TVTInitialize@4`. Whatever loads
//! the library asks for the undecorated spelling, and a library that answers to
//! neither is one the host quietly does not use. GNU linkers have `--kill-at`
//! for this; the one used here does not take it.
//!
//! And on either width the linker adds its own: the C runtime's initialiser,
//! the unwinder's entry points, a Rust symbol or two. Harmless, but a
//! `winscard.dll` is meant to be indistinguishable from the system's own.
//!
//! The names are read out of the source rather than written down again.

use std::collections::BTreeSet;
use std::path::Path;
use std::{env, fs};

/// Writes the definition and tells cargo to link with it.
///
/// `sources` are the files to read the names out of, relative to the crate
/// being built. `expected` is how many exports there should be: it is checked
/// because the failure it guards against is silent -- a declaration [`collect`]
/// does not read leaves the name out, and nothing says so until somebody's
/// television refuses to open.
pub fn write(sources: &[&str], expected: usize) {
    println!("cargo::rerun-if-changed=build.rs");
    for file in sources {
        println!("cargo::rerun-if-changed={file}");
    }

    // Only Windows has an export table to fix. Elsewhere these crates are
    // built for their own tests.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut names = BTreeSet::new();
    for file in sources {
        let source = fs::read_to_string(file).unwrap_or_else(|e| panic!("read {file}: {e}"));
        collect(&source, &mut names);
    }
    assert_eq!(
        names.len(),
        expected,
        "read {} exports from {sources:?}, expected {expected}. Either an entry point was \
         added or removed -- change the number the build script passes -- or one is written \
         in a shape `collect` does not recognise, in which case it is missing from the table.",
        names.len()
    );

    let mut definition = String::from("EXPORTS\n");
    for name in &names {
        definition.push_str(name);
        definition.push('\n');
    }
    let out = Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("exports.def");
    fs::write(&out, definition).unwrap_or_else(|e| panic!("write {}: {e}", out.display()));

    // MSVC's link.exe takes the definition as an option. Elsewhere it is handed
    // to the linker as an ordinary input; a `.def` among the objects is what says
    // which names the table should hold.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo::rustc-link-arg-cdylib=/DEF:{}", out.display());
    } else {
        println!("cargo::rustc-link-arg-cdylib=-Wl,{}", out.display());
    }
}

/// Reads the exported names out of a source file.
///
/// Two shapes carry them. A call written out has the attribute above it:
///
/// ```ignore
/// #[unsafe(no_mangle)]
/// pub unsafe extern "system" fn SCardStatusA(...
/// ```
///
/// and one that only passes through is a macro call, on one line or, where it
/// was too long for that, with its first argument on the next:
///
/// ```ignore
/// on_a_context!(SCardAddReaderToGroupA, Fn3, reader, group);
/// on_a_context!(
///     SCardListReaderGroupsA,
///     Fn3,
///     groups,
///     length,
/// );
/// ```
fn collect(source: &str, into: &mut BTreeSet<String>) {
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("#[unsafe(no_mangle)]") {
            // The declaration is the next line that is not another attribute.
            for following in lines.by_ref() {
                let following = following.trim_start();
                if following.starts_with('#') {
                    continue;
                }
                if let Some(name) = declared(following) {
                    into.insert(name);
                }
                break;
            }
            continue;
        }
        let trimmed = line.trim_start();
        for macro_name in ["on_a_context!", "on_a_card!", "straight_on!"] {
            let Some(rest) = trimmed.strip_prefix(macro_name) else {
                continue;
            };
            let rest = rest.trim_start().trim_start_matches('(').trim();
            let first = if rest.is_empty() {
                lines.peek().map(|next| next.trim()).unwrap_or_default()
            } else {
                rest
            };
            let name = first.split(',').next().unwrap_or_default().trim();
            let name = name.trim_end_matches(')');
            if is_identifier(name) {
                into.insert(name.to_owned());
            }
        }
    }
}

/// The name a `fn` or a `static` declares, when the line declares one.
fn declared(line: &str) -> Option<String> {
    let after = line
        .split_once(" fn ")
        .or_else(|| line.split_once(" static "))?
        .1;
    let name: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    is_identifier(&name).then_some(name)
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

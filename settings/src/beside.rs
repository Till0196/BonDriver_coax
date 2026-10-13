// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Finding the files that belong to this library.
//!
//! A BonDriver's settings sit next to the library, not next to the host that
//! loaded it: a viewer may have several of these in one directory, each with
//! its own file. So the library asks the loader where it is.
//!
//! Which file is its own is decided by the library's own name, because that is
//! the only thing that tells one copy from another. A BonDriver is installed by
//! copying it and adding a suffix -- one per lead or per purpose -- and a host
//! may have several open at once. Every copy reading one file would make the
//! suffixes decorative.

use std::path::{Path, PathBuf};

/// The path this library was loaded from, or nothing when the loader will not
/// say.
#[cfg(windows)]
pub fn own_path() -> Option<PathBuf> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    /// Asks by an address inside the module rather than by name, and does not
    /// take a reference on it: this library is not going anywhere while it is
    /// asking.
    const FROM_ADDRESS: u32 = 0x0000_0004;
    const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }

    let mut module: *mut c_void = std::ptr::null_mut();
    let anchor = (own_path as *const ()).cast::<u16>();
    let found =
        unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &raw mut module) };
    if found == 0 {
        return None;
    }
    let mut name = [0u16; 32768];
    let length = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) };
    if length == 0 {
        return None;
    }
    let path = std::ffi::OsString::from_wide(&name[..length as usize]);
    Some(PathBuf::from(path))
}

/// Keeps this library loaded until the process ends, whatever the host does.
///
/// For a library that leaves a thread of its own running with no call from
/// the host to stop it at: a host that unloads the library while that thread
/// is waiting on the network would have it come back to code that is no
/// longer there. Asked more than once, it is the same as asked once.
#[cfg(windows)]
pub fn stay_loaded() {
    use std::ffi::c_void;
    use std::sync::Once;

    const PIN: u32 = 0x0000_0001;
    const FROM_ADDRESS: u32 = 0x0000_0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    }

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut module: *mut c_void = std::ptr::null_mut();
        let anchor = (stay_loaded as *const ()).cast::<u16>();
        unsafe { GetModuleHandleExW(PIN | FROM_ADDRESS, anchor, &raw mut module) };
    });
}

/// Elsewhere nothing that calls this is built as a library a host unloads.
#[cfg(not(windows))]
pub fn stay_loaded() {}

/// The same, asked of the dynamic loader rather than of Windows. `dladdr`
/// answers what object an address belongs to and where that object was loaded
/// from, which is the one question worth asking here.
#[cfg(unix)]
pub fn own_path() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;

    /// `Dl_info`, of which only the file name is read. The rest is named so
    /// the layout is right; the loader fills all four.
    #[repr(C)]
    struct DlInfo {
        file_name: *const c_char,
        base: *mut c_void,
        symbol_name: *const c_char,
        symbol_address: *mut c_void,
    }

    unsafe extern "C" {
        fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
    }

    let mut info = DlInfo {
        file_name: std::ptr::null(),
        base: std::ptr::null_mut(),
        symbol_name: std::ptr::null(),
        symbol_address: std::ptr::null_mut(),
    };
    let anchor = (own_path as *const ()).cast::<c_void>();
    if unsafe { dladdr(anchor, &raw mut info) } == 0 || info.file_name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(info.file_name) };
    Some(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
}

#[cfg(not(any(windows, unix)))]
pub fn own_path() -> Option<PathBuf> {
    None
}

/// The directory this library was loaded from.
pub fn own_directory() -> Option<PathBuf> {
    own_path()?.parent().map(PathBuf::from)
}

/// Reads a settings file, in whichever encoding a viewer's editor wrote it, or
/// nothing when it is not there. Not being there is the ordinary case: a driver
/// with no settings uses the band plan and lets the daemon choose a tuner.
pub fn read_file(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| decode(&bytes))
}

/// The same, saying why when it could not be read. For a caller about to
/// write the file back: taking a file it could not read for an empty one
/// would write the viewer's settings away. A file that is not there is empty.
pub fn read_for_writing(path: &Path) -> std::io::Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(decode(&bytes)),
        Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(reason) => Err(reason),
    }
}

/// The settings files a copy of the driver may be using, best first.
///
/// The first is the copy's own name with `.ini` for an extension:
/// `BonDriver_coax-S.dll` reads `BonDriver_coax-S.ini`.
///
/// The second, where the name carries a suffix, is the name without it. It is
/// there for the installation that had one file before it had several: a viewer
/// who copied the library keeps the settings they had until they write the
/// copy's own file.
///
/// The suffix is whatever follows the last `-`. A name with no `-` in it is the
/// whole family and has no second candidate.
pub fn settings_candidates(library: &Path) -> Vec<PathBuf> {
    let own = library.with_extension("ini");
    let mut candidates = vec![own];
    if let Some(shared) = shared_settings(library) {
        candidates.push(shared);
    }
    candidates
}

/// The file a family of copies falls back on: the library's name up to the
/// last `-`, with `.ini` for an extension. Nothing for a name that carries no
/// suffix, which is that file itself.
fn shared_settings(library: &Path) -> Option<PathBuf> {
    let stem = library.file_stem()?.to_str()?;
    let (base, _) = stem.rsplit_once('-')?;
    if base.is_empty() {
        return None;
    }
    Some(library.with_file_name(format!("{base}.ini")))
}

/// The settings file a copy of the driver uses: the first candidate that is
/// there, and the copy's own name when none of them is. The last part matters
/// for the plugin, which writes the channels it finds into a file of the
/// driver's own rather than a shared one.
pub fn settings_for(library: &Path) -> PathBuf {
    let candidates = settings_candidates(library);
    for candidate in &candidates {
        if candidate.is_file() {
            return candidate.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| library.with_extension("ini"))
}

/// The settings this library's own copy uses.
pub fn read_settings() -> Option<String> {
    read_file(&settings_for(&own_path()?))
}

/// Every settings file of one library's family, in a settled order, with the
/// one that library uses always among them even where it is not written yet.
/// This is what a viewer is offered to choose between; the family is the name
/// up to the last `-`.
pub fn settings_family(library: &Path) -> Vec<PathBuf> {
    let mine = settings_for(library);
    let mut found = vec![mine.clone()];
    if let (Some(directory), Some(prefix)) = (library.parent(), family_prefix(library)) {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path != mine && is_family_settings(&prefix, &path) && path.is_file() {
                    found.push(path);
                }
            }
        }
    }
    // By name, so that a list read off a directory reads the same twice.
    found[1..].sort();
    found
}

/// The stem every file of a family starts with: the library's name up to the
/// last `-`, or the whole of it when there is none.
fn family_prefix(library: &Path) -> Option<String> {
    let stem = library.file_stem()?.to_str()?;
    Some(match stem.rsplit_once('-') {
        Some((base, _)) if !base.is_empty() => base.to_owned(),
        _ => stem.to_owned(),
    })
}

/// Whether a path is one of a family's settings files. Compared without regard
/// to case, because Windows file names are.
fn is_family_settings(prefix: &str, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".ini").or_else(|| {
        // Only the extension is folded here; the stem is compared below.
        name.len()
            .checked_sub(4)
            .filter(|at| {
                // `get`, because four bytes from the end may fall inside a
                // character of a name that is not a settings file at all.
                name.get(*at..)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case(".ini"))
            })
            .map(|at| &name[..at])
    }) else {
        return false;
    };
    if !stem
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        return false;
    }
    // Either the family's own file or one of its suffixed copies, and not
    // some other library that happens to start with the same letters.
    let rest = &stem[prefix.len()..];
    rest.is_empty() || rest.starts_with('-')
}

/// Reads the text of a settings file: UTF-8 where it is UTF-8, and the system's
/// own code page otherwise. The files a viewer already has were written by
/// editors on their machine, and refusing one because it is not UTF-8 would
/// refuse the common case.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.trim_start_matches('\u{feff}').to_owned(),
        Err(_) => decode_ansi(bytes),
    }
}

#[cfg(windows)]
fn decode_ansi(bytes: &[u8]) -> String {
    use std::ffi::c_void;

    const CP_ACP: u32 = 0;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_length: i32,
            output: *mut u16,
            output_length: i32,
        ) -> i32;
    }

    let _ = std::ptr::null::<c_void>();
    let needed = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            needed,
        )
    };
    String::from_utf16_lossy(&wide[..written.max(0) as usize])
}

#[cfg(not(windows))]
fn decode_ansi(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{family_prefix, is_family_settings, settings_candidates, shared_settings};

    /// A copy renamed with a suffix reads its own file, and falls back on the
    /// family's. This is the whole of how one installation holds a driver per
    /// lead.
    #[test]
    fn a_renamed_copy_looks_for_its_own_file_first() {
        // Written with this platform's separator: the rule is about the file
        // name, and a Windows path spelled out here would be one component on
        // the machine the tests run on and prove nothing about the split.
        let candidates = settings_candidates(Path::new("/tv/BonDriver_coax-S.dll"));
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/tv/BonDriver_coax-S.ini"),
                PathBuf::from("/tv/BonDriver_coax.ini"),
            ]
        );
    }

    /// The library that carries no suffix is the family, so there is nothing
    /// behind it to fall back on.
    #[test]
    fn the_plain_library_has_only_its_own_file() {
        assert_eq!(
            settings_candidates(Path::new("/opt/tv/BonDriver_coax.dll")),
            vec![PathBuf::from("/opt/tv/BonDriver_coax.ini")]
        );
        assert_eq!(
            shared_settings(Path::new("/opt/tv/BonDriver_coax.dll")),
            None
        );
    }

    /// Every shape of the convention: a letter, a number, a word, and the
    /// `lib` prefix a build for a host that wants one carries.
    #[test]
    fn the_suffix_is_whatever_follows_the_last_dash() {
        for (library, want) in [
            ("BonDriver_coax-S.dll", "BonDriver_coax-S.ini"),
            ("BonDriver_coax-T0.dll", "BonDriver_coax-T0.ini"),
            ("BonDriver_coax-1.dll", "BonDriver_coax-1.ini"),
            ("BonDriver_coax-BS4K.dll", "BonDriver_coax-BS4K.ini"),
            ("libBonDriver_coax-2.so", "libBonDriver_coax-2.ini"),
        ] {
            let candidates = settings_candidates(Path::new(library));
            assert_eq!(candidates[0], PathBuf::from(want), "for {library}");
        }
    }

    /// A version number in the name is part of it, not an extension: a copy
    /// called BonDriver_coax-1.0.dll reads BonDriver_coax-1.0.ini.
    #[test]
    fn a_dot_in_the_name_is_part_of_the_name() {
        assert_eq!(
            settings_candidates(Path::new("BonDriver_coax-1.0.dll"))[0],
            PathBuf::from("BonDriver_coax-1.0.ini")
        );
    }

    #[test]
    fn a_family_is_named_by_what_comes_before_the_suffix() {
        assert_eq!(
            family_prefix(Path::new("BonDriver_coax-S.dll")).as_deref(),
            Some("BonDriver_coax")
        );
        assert_eq!(
            family_prefix(Path::new("BonDriver_coax.dll")).as_deref(),
            Some("BonDriver_coax")
        );
    }

    /// What the list a viewer chooses between holds, and what it leaves out:
    /// another library's settings that happen to start with the same letters
    /// are not this one's to offer.
    #[test]
    fn only_the_familys_own_files_are_offered() {
        for (name, want) in [
            ("BonDriver_coax.ini", true),
            ("BonDriver_coax-S.ini", true),
            ("BonDriver_coax-T0.ini", true),
            // Windows file names are not case-sensitive, and a viewer's
            // editor may have written any of these.
            ("bondriver_coax-s.INI", true),
            // A different library, and a file of another kind.
            ("BonDriver_coaxial.ini", false),
            ("BonDriver_other.ini", false),
            ("BonDriver_coax-S.ch2", false),
            ("BonDriver_coax.ini.bak", false),
            // Names whose last four bytes fall inside a character.
            ("ロゴ", false),
            ("録画", false),
            ("BonDriver_coax-録画", false),
        ] {
            assert_eq!(
                is_family_settings("BonDriver_coax", Path::new(name)),
                want,
                "for {name}"
            );
        }
    }
}

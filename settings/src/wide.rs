// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Strings in the form the interface hands back: UTF-16, null-terminated, and
//! owned by the driver for as long as the caller might read them.
//!
//! `EnumTuningSpace` and `EnumChannelName` return a pointer and say nothing
//! about how long it lives. Every caller reads it before the next call, so the
//! strings are kept in the driver rather than rebuilt per call.

/// A null-terminated UTF-16 string the driver owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wide(Vec<u16>);

impl Wide {
    /// Builds one from anything that reads as text.
    pub fn new(text: &str) -> Self {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        Self(units)
    }

    /// The pointer the interface hands back.
    pub fn as_ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::Wide;

    #[test]
    fn a_string_ends_in_a_null_so_a_caller_knows_where_it_stops() {
        let wide = Wide::new("BS");
        assert_eq!(wide.0, vec![b'B' as u16, b'S' as u16, 0]);
    }

    #[test]
    fn japanese_survives_the_crossing() {
        let wide = Wide::new("地デジ");
        assert_eq!(wide.0.len(), 4);
        assert_eq!(wide.0.last(), Some(&0));
        assert_eq!(String::from_utf16_lossy(&wide.0[..3]), "地デジ");
    }
}

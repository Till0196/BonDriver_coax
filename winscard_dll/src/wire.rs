// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What a reader looks like to an application.
//!
//! The names and numbers here are pcsc-lite's, because they are what the API
//! this library implements is defined in terms of: a state is a bit field on
//! one side of the world and an ordinal on the other, and something has to
//! translate.
//!
//! It used to hold rather more, back when the daemon's card was reached over
//! the socket its resource manager published. What crosses now is JSON, so this
//! side builds a reader entry from an answer rather than decoding one off a
//! socket.

// Read by the exported calls, which are built only on the platform they are
// for. Elsewhere this is here for its own tests.
#![cfg_attr(not(windows), allow(dead_code))]

/// The state bits pcsc-lite publishes. They are flags here and ordinals in the
/// Windows API, which is the one difference this file has to translate.
pub const STATE_ABSENT: u32 = 0x0002;
pub const STATE_PRESENT: u32 = 0x0004;
pub const STATE_SWALLOWED: u32 = 0x0008;
pub const STATE_POWERED: u32 = 0x0010;
pub const STATE_NEGOTIABLE: u32 = 0x0020;
pub const STATE_SPECIFIC: u32 = 0x0040;

/// `PCSCLITE_SHARING_EXCLUSIVE_CONTEXT`, which the count of shared connections
/// is replaced by when one connection has the reader to itself.
pub const SHARING_EXCLUSIVE: i32 = -1;

/// One reader, as this library reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaderState {
    pub name: String,
    /// Counts insertions and removals, which is the only thing that tells a
    /// card swapped between two looks from a card that never moved.
    pub events: u32,
    pub state: u32,
    /// `-1` for an exclusive connection, otherwise the number of shared ones.
    pub sharing: i32,
    pub atr: Vec<u8>,
    pub protocol: u32,
}

impl ReaderState {
    /// Whether anything has actually looked at the slot.
    ///
    /// Looking means opening the board a tuner streams from, so it is done only
    /// while something is waiting on an answer. An unknown slot is not an empty
    /// one: an application told the card is absent stops looking.
    pub fn measured(&self) -> bool {
        self.state
            & (STATE_ABSENT
                | STATE_PRESENT
                | STATE_SWALLOWED
                | STATE_POWERED
                | STATE_NEGOTIABLE
                | STATE_SPECIFIC)
            != 0
    }

    /// Whether a card is in the reader.
    pub fn present(&self) -> bool {
        self.state & (STATE_PRESENT | STATE_SWALLOWED | STATE_POWERED | STATE_SPECIFIC) != 0
    }

    /// The state as `SCardStatus` reports it, which is an ordinal on Windows
    /// and a bit field here. The most advanced stage the flags claim is the
    /// one to report, since they accumulate as a card comes up.
    pub fn windows_state(&self) -> u32 {
        const ABSENT: u32 = 1;
        const PRESENT: u32 = 2;
        const SWALLOWED: u32 = 3;
        const POWERED: u32 = 4;
        const NEGOTIABLE: u32 = 5;
        const SPECIFIC: u32 = 6;
        for (bit, ordinal) in [
            (STATE_SPECIFIC, SPECIFIC),
            (STATE_NEGOTIABLE, NEGOTIABLE),
            (STATE_POWERED, POWERED),
            (STATE_SWALLOWED, SWALLOWED),
            (STATE_PRESENT, PRESENT),
            (STATE_ABSENT, ABSENT),
        ] {
            if self.state & bit != 0 {
                return ordinal;
            }
        }
        // SCARD_UNKNOWN, which pcsc-lite numbers 0x0001 and Windows numbers 0.
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(state: u32, sharing: i32) -> ReaderState {
        ReaderState {
            name: "Sample Card Reader (ID: 0)".to_owned(),
            events: 7,
            state,
            sharing,
            atr: vec![0x3b, 0xf0],
            protocol: 2,
        }
    }

    /// A slot nothing has looked at is unknown rather than empty: an
    /// application told the card is absent stops looking.
    #[test]
    fn a_slot_nobody_has_looked_at_is_unknown_rather_than_empty() {
        let unseen = reader(0, 0);
        assert!(!unseen.measured());
        assert!(!unseen.present());
        assert!(reader(STATE_ABSENT, 0).measured());
        assert!(!reader(STATE_ABSENT, 0).present());
    }

    #[test]
    fn the_state_bits_become_the_ordinal_of_the_furthest_stage_reached() {
        let up = reader(STATE_PRESENT | STATE_POWERED | STATE_SPECIFIC, 0);
        assert_eq!(up.windows_state(), 6);
        assert!(up.present());
        assert_eq!(reader(STATE_ABSENT, 0).windows_state(), 1);
        // Nothing measured at all is the one the Windows API numbers zero.
        assert_eq!(reader(0, 0).windows_state(), 0);
    }
}

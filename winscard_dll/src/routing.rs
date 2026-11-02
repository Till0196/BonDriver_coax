// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(not(windows), allow(dead_code))]

use crate::abi::{SCARD_E_NO_SERVICE, Status};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReaderRoute {
    BuiltIn,
    System,
}

/// An unsuccessful listing does not establish that a reader belongs to the
/// system. Only a reader the system actually lists may be sent there then.
pub(crate) fn resolve(
    built_in: Option<bool>,
    system_has_reader: impl FnOnce() -> bool,
) -> Result<ReaderRoute, Status> {
    match built_in {
        Some(true) => Ok(ReaderRoute::BuiltIn),
        Some(false) => Ok(ReaderRoute::System),
        None if system_has_reader() => Ok(ReaderRoute::System),
        None => Err(SCARD_E_NO_SERVICE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_listing_does_not_turn_a_reader_into_a_system_reader() {
        assert_eq!(resolve(None, || false), Err(SCARD_E_NO_SERVICE));
    }

    #[test]
    fn a_system_reader_remains_accessible_when_the_daemon_is_unavailable() {
        assert_eq!(resolve(None, || true), Ok(ReaderRoute::System));
    }

    #[test]
    fn a_successful_listing_decides_without_querying_the_system() {
        assert_eq!(
            resolve(Some(true), || panic!("already listed")),
            Ok(ReaderRoute::BuiltIn)
        );
        assert_eq!(
            resolve(Some(false), || panic!("already listed")),
            Ok(ReaderRoute::System)
        );
    }
}

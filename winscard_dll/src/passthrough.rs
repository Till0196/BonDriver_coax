// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The rest of the API, passed straight on.
//!
//! An application binds to every entry point it imports when it is loaded, so a
//! file called `WinSCard.dll` that is missing one is not a library with a gap
//! -- it is an application that will not start. The system library publishes
//! eighty entry points and this one has something to say about twenty-odd; the
//! others are here so that the rest are answered as they would have been
//! without this file in the directory.
//!
//! Arguments are carried as machine words rather than as their own types. What
//! matters is that as many go out as came in -- on 32-bit Windows the callee
//! clears them off the stack -- and none of these calls takes a float or a
//! structure by value.

use crate::abi::{Handle, SCARD_E_INVALID_HANDLE, SCARD_E_NO_SERVICE, Status};
use crate::state;
use crate::system::entry;

/// The system's handle for a context, which is what a call being passed on has
/// to be given: the caller may be holding one of ours, and the number in it
/// means nothing to the system library.
fn theirs(context: Handle) -> Option<Handle> {
    match state::context(context) {
        Some(ours) if ours.system == 0 => None,
        Some(ours) => Some(ours.system),
        // Not one of ours, so it is already the system's own.
        None => Some(context),
    }
}

/// The signatures, by how many words they take.
pub type Fn0 = unsafe extern "system" fn() -> Status;
pub type Fn2 = unsafe extern "system" fn(usize, usize) -> Status;
pub type Fn3 = unsafe extern "system" fn(usize, usize, usize) -> Status;
pub type Fn4 = unsafe extern "system" fn(usize, usize, usize, usize) -> Status;
pub type Fn5 = unsafe extern "system" fn(usize, usize, usize, usize, usize) -> Status;
pub type Fn6 = unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> Status;
pub type Fn8 =
    unsafe extern "system" fn(usize, usize, usize, usize, usize, usize, usize, usize) -> Status;

/// Declares a forwarder whose first argument is a context, which is translated
/// on the way through.
macro_rules! on_a_context {
    ($name:ident, $signature:ty $(, $argument:ident)* $(,)?) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name(context: Handle $(, $argument: usize)*) -> Status {
            let Some(theirs) = theirs(context) else {
                return SCARD_E_NO_SERVICE;
            };
            let Some(call): Option<$signature> =
                (unsafe { entry(concat!(stringify!($name), "\0")) })
            else {
                return SCARD_E_NO_SERVICE;
            };
            unsafe { call(theirs $(, $argument)*) }
        }
    };
}

/// Declares a forwarder whose first argument is a card. A card of ours cannot
/// be passed on -- the system library has never heard of it -- and these are
/// calls this library has nothing to answer them with.
macro_rules! on_a_card {
    ($name:ident, $signature:ty $(, $argument:ident)* $(,)?) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name(card: Handle $(, $argument: usize)*) -> Status {
            if state::card(card).is_some() {
                return SCARD_E_INVALID_HANDLE;
            }
            let Some(call): Option<$signature> =
                (unsafe { entry(concat!(stringify!($name), "\0")) })
            else {
                return SCARD_E_NO_SERVICE;
            };
            unsafe { call(card $(, $argument)*) }
        }
    };
}

/// Declares a forwarder that names nothing of ours.
macro_rules! straight_on {
    ($name:ident, $signature:ty $(, $argument:ident)* $(,)?) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name($($argument: usize),*) -> Status {
            let Some(call): Option<$signature> =
                (unsafe { entry(concat!(stringify!($name), "\0")) })
            else {
                return SCARD_E_NO_SERVICE;
            };
            unsafe { call($($argument),*) }
        }
    };
}

on_a_context!(SCardAddReaderToGroupA, Fn3, reader, group);
on_a_context!(SCardAddReaderToGroupW, Fn3, reader, group);
on_a_context!(SCardAudit, Fn2, event);
on_a_context!(SCardForgetCardTypeA, Fn2, name);
on_a_context!(SCardForgetCardTypeW, Fn2, name);
on_a_context!(SCardForgetReaderA, Fn2, name);
on_a_context!(SCardForgetReaderW, Fn2, name);
on_a_context!(SCardForgetReaderGroupA, Fn2, name);
on_a_context!(SCardForgetReaderGroupW, Fn2, name);
on_a_context!(
    SCardGetCardTypeProviderNameA,
    Fn5,
    card,
    provider,
    into,
    length
);
on_a_context!(
    SCardGetCardTypeProviderNameW,
    Fn5,
    card,
    provider,
    into,
    length
);
on_a_context!(SCardGetDeviceTypeIdA, Fn3, reader, kind);
on_a_context!(SCardGetDeviceTypeIdW, Fn3, reader, kind);
on_a_context!(SCardGetProviderIdA, Fn3, card, provider);
on_a_context!(SCardGetProviderIdW, Fn3, card, provider);
on_a_context!(SCardGetReaderDeviceInstanceIdA, Fn4, reader, into, length);
on_a_context!(SCardGetReaderDeviceInstanceIdW, Fn4, reader, into, length);
on_a_context!(SCardGetReaderIconA, Fn4, reader, into, length);
on_a_context!(SCardGetReaderIconW, Fn4, reader, into, length);
on_a_context!(
    SCardIntroduceCardTypeA,
    Fn8,
    card,
    primary,
    interfaces,
    count,
    atr,
    mask,
    length
);
on_a_context!(
    SCardIntroduceCardTypeW,
    Fn8,
    card,
    primary,
    interfaces,
    count,
    atr,
    mask,
    length
);
on_a_context!(SCardIntroduceReaderA, Fn3, reader, device);
on_a_context!(SCardIntroduceReaderW, Fn3, reader, device);
on_a_context!(SCardIntroduceReaderGroupA, Fn2, group);
on_a_context!(SCardIntroduceReaderGroupW, Fn2, group);
on_a_context!(SCardListCardsA, Fn6, atr, interfaces, count, into, length);
on_a_context!(SCardListCardsW, Fn6, atr, interfaces, count, into, length);
on_a_context!(SCardListInterfacesA, Fn4, card, interfaces, count);
on_a_context!(SCardListInterfacesW, Fn4, card, interfaces, count);
on_a_context!(
    SCardListReadersWithDeviceInstanceIdA,
    Fn4,
    device,
    into,
    length
);
on_a_context!(
    SCardListReadersWithDeviceInstanceIdW,
    Fn4,
    device,
    into,
    length
);
on_a_context!(SCardLocateCardsA, Fn4, cards, states, count);
on_a_context!(SCardLocateCardsW, Fn4, cards, states, count);
on_a_context!(SCardLocateCardsByATRA, Fn5, masks, masked, states, count);
on_a_context!(SCardLocateCardsByATRW, Fn5, masks, masked, states, count);
on_a_context!(
    SCardReadCacheA,
    Fn6,
    identifier,
    freshness,
    name,
    data,
    length
);
on_a_context!(
    SCardReadCacheW,
    Fn6,
    identifier,
    freshness,
    name,
    data,
    length
);
on_a_context!(
    SCardWriteCacheA,
    Fn6,
    identifier,
    freshness,
    name,
    data,
    length
);
on_a_context!(
    SCardWriteCacheW,
    Fn6,
    identifier,
    freshness,
    name,
    data,
    length
);
on_a_context!(SCardRemoveReaderFromGroupA, Fn3, reader, group);
on_a_context!(SCardRemoveReaderFromGroupW, Fn3, reader, group);
on_a_context!(SCardSetCardTypeProviderNameA, Fn4, card, provider, name);
on_a_context!(SCardSetCardTypeProviderNameW, Fn4, card, provider, name);

on_a_card!(SCardGetTransmitCount, Fn2, count);
on_a_card!(SCardState, Fn5, state, protocol, atr, length);

straight_on!(SCardAccessNewReaderEvent, Fn0);
straight_on!(SCardAccessStartedEvent, Fn0);
straight_on!(SCardReleaseAllEvents, Fn0);
straight_on!(SCardReleaseNewReaderEvent, Fn0);
straight_on!(SCardReleaseStartedEvent, Fn0);
straight_on!(SCardPciRaw, Fn0);
straight_on!(SCardPciT0, Fn0);
straight_on!(SCardPciT1, Fn0);
straight_on!(ClassInstall32, Fn3, function, information, data);

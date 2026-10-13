// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The channels a driver hands out, and the order it hands them out in.
//!
//! A BonDriver offers channels as a space and an index into it, and a caller stores those
//! two numbers. So the numbering is the thing to protect: a channel that moves takes a
//! viewer's saved channel with it.
//!
//!  - A space with no lines in the settings file offers the built-in band plan, always in
//!    the same order and always the same length.
//!  - A space with lines offers exactly the lines, in the order the file gives them, one
//!    channel per stream. Whatever looks for channels appends and never inserts, so the
//!    numbering holds between looks.

/// One channel of one space, as a caller sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// What the host shows and what a channel file records.
    pub name: String,
    /// What the daemon is asked for, in the vocabulary of a channel definition
    /// file. Passed through rather than interpreted: a new delivery system is
    /// then a matter for the daemon alone.
    pub tune: Vec<(String, String)>,
    /// The other carriers this channel is also on, where a multiplexing frame
    /// was split across several. Tuning one of them alone gives a share of the
    /// stream and no more.
    pub carriers: Vec<String>,
    /// The tuners this channel is received on, as patterns, where they are not
    /// the space's own. A BS stream transmodulated onto a cable belongs in BS,
    /// and is received on the cable's tuner and not on the dish's.
    pub tuners: Vec<String>,
    /// The other ways the same stream can be received. One stream, several
    /// routes: the BS a dish receives and the same BS transmodulated onto a
    /// cable are one channel to a viewer. They are one entry with the ways in
    /// the order to try them, so a saved channel goes on working when the
    /// dish's tuner is busy and the cable's is not.
    pub also: Vec<Route>,
}

/// One way of receiving a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Route {
    /// What the daemon is asked for.
    pub tune: Vec<(String, String)>,
    /// The tuners that can do it, as patterns. Empty leaves it to the space's
    /// own.
    pub tuners: Vec<String>,
    /// The other carriers of a split multiplexing frame, as this route
    /// receives it.
    pub carriers: Vec<String>,
}

impl Channel {
    /// A channel that is nothing but its name, for the tests and for entries
    /// nothing has yet worked out how to tune.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tune: Vec::new(),
            carriers: Vec::new(),
            tuners: Vec::new(),
            also: Vec::new(),
        }
    }

    /// A channel of the band plan: a name and what it takes to receive it.
    pub fn tuned(name: &str, tune: Vec<(String, String)>) -> Self {
        Self {
            name: name.to_owned(),
            tune,
            carriers: Vec::new(),
            tuners: Vec::new(),
            also: Vec::new(),
        }
    }

    /// The properties in the form the daemon's query takes.
    pub fn parameters(&self) -> Vec<(&str, String)> {
        self.tune
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect()
    }
}

/// One tuning space: a platform the viewer recognises, such as BS or 地デジ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub name: String,
    /// The tuners this space is received on, as the settings named them:
    /// patterns rather than identifiers, so that a card that comes back on a
    /// different USB port is still the one on this lead. Empty leaves the
    /// choice to the daemon.
    pub tuners: Vec<String>,
    pub channels: Vec<Channel>,
}

/// Every space a driver offers, in the order it offers them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineup {
    spaces: Vec<Space>,
}

impl Lineup {
    pub fn new(spaces: Vec<Space>) -> Self {
        Self { spaces }
    }

    /// A driver with nothing to offer yet, which is what a library that has not
    /// read its configuration has.
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn spaces(&self) -> &[Space] {
        &self.spaces
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, Lineup, Space};

    #[test]
    fn a_lineup_with_nothing_in_it_offers_no_spaces() {
        assert!(Lineup::empty().spaces().is_empty());
    }

    #[test]
    fn spaces_keep_the_order_they_were_given() {
        let lineup = Lineup::new(vec![
            Space {
                name: "地デジ".to_owned(),
                tuners: Vec::new(),
                channels: vec![Channel::named("13")],
            },
            Space {
                name: "BS".to_owned(),
                tuners: Vec::new(),
                channels: vec![],
            },
        ]);
        let names: Vec<&str> = lineup
            .spaces()
            .iter()
            .map(|space| space.name.as_str())
            .collect();
        assert_eq!(names, ["地デジ", "BS"]);
    }
}

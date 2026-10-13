// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Turning a network's own account of a multiplex into a channel to tune.
//!
//! A network table describes every multiplex of its network, each with a
//! delivery system descriptor saying where it is and how it is modulated. That
//! is the general way to find channels; the band plans this sweep walks are the
//! special case, and work because Japan publishes its frequencies and nobody
//! moves them.
//!
//! Nothing here tunes or reads anything: it is the arithmetic between a
//! descriptor and a set of channel-file properties, kept apart so that both
//! readings of the satellite descriptor can be tested against fixed bytes. The
//! same shape as the daemon's own sweep keeps -- `scanner/delivery.go` there.

use coax::plan;
use isdb::nit;

/// Which standard's rules a network table is written by.
///
/// It has to be carried rather than worked out. ARIB STD-B10 and DVB EN 300 468
/// divide one byte of the satellite delivery system descriptor differently, and
/// no bit in the descriptor says which division was meant. What settles it is
/// the multiplex the table was read out of, and that is known because this
/// sweep tuned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    /// How the Japanese networks write their tables.
    Arib,
    /// How everybody else does.
    Dvb,
}

/// Which rules a network table read out of a multiplex of this delivery system
/// follows. A network table describes its own network, so the multiplex it came
/// off and the ones it describes are of a piece.
///
/// Asked of satellite delivery systems and nothing else, because the satellite
/// descriptor is the only one with two readings: a cable descriptor gives its
/// modulation a byte of its own.
#[must_use]
pub fn convention_for(system: &str) -> Convention {
    match coax::delivery::named(system).as_str() {
        coax::delivery::ISDBS | coax::delivery::ISDBS3 => Convention::Arib,
        _ => Convention::Dvb,
    }
}

/// What stands between a downlink frequency and the tuner: the block on the
/// dish, and the switch position the dish is on where there is one.
///
/// A satellite descriptor gives the downlink frequency, which no tuner takes,
/// and only the person who installed the dish knows which oscillator to
/// subtract. The block's name is passed on to the daemon rather than acted on
/// here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dish {
    /// The low-noise block, as a channel file's `LNB` names it. Empty means the
    /// Japanese arrangement, whose two oscillators are picked by the hand of
    /// the polarisation.
    pub block: String,
    /// The DiSEqC port, written where the dish reaches the tuner through a
    /// switch. Empty sends nothing, which is what a dish with one block wants.
    pub position: String,
}

/// One channel as a tune takes it: the properties, in the order they are
/// written.
pub type Channel = Vec<(String, String)>;

/// Turns one multiplex of a satellite network into the properties a tune takes,
/// and reports nothing where it could not be reached at all.
///
/// The two conventions part company over what the descriptor's modulation field
/// holds, and over how the downlink frequency becomes something a tuner takes:
/// Japan's two oscillators are converted here, while everywhere else a named
/// block does it in the daemon.
#[must_use]
pub fn satellite_channel(described: &nit::Satellite, system: &str, dish: &Dish) -> Option<Channel> {
    match convention_for(system) {
        Convention::Arib => arib_satellite_channel(described, system),
        Convention::Dvb => dvb_satellite_channel(described, dish),
    }
}

/// The Japanese arrangement: one dish, one block, and two oscillators picked by
/// the hand of the circular polarisation. The frequency is converted here
/// because there is no block to name -- every receiver in the country has the
/// same one.
///
/// The modulation field is left where it is: ARIB numbers it its own way and
/// the Japanese demodulators are told the standard rather than the modulation.
fn arib_satellite_channel(described: &nit::Satellite, system: &str) -> Option<Channel> {
    let intermediate =
        plan::intermediate_frequency_khz(described.downlink_khz, !described.right_hand())?;
    Some(vec![
        ("DELIVERY_SYSTEM".to_owned(), system.to_owned()),
        ("FREQUENCY".to_owned(), intermediate.to_string()),
    ])
}

/// Everybody else's: the downlink frequency as the descriptor gave it, with the
/// block on the dish named beside it so that the daemon subtracts the right
/// oscillator.
///
/// The delivery system comes from the descriptor rather than from what was
/// tuned. One network carries both generations on neighbouring transponders, so
/// a DVB-S multiplex found from a DVB-S2 one has to be tuned as DVB-S.
fn dvb_satellite_channel(described: &nit::Satellite, dish: &Dish) -> Option<Channel> {
    if described.downlink_khz == 0 {
        return None;
    }
    let system = if described.dvb_second_generation() {
        coax::delivery::DVBS2
    } else {
        coax::delivery::DVBS
    };
    let mut channel = vec![
        ("DELIVERY_SYSTEM".to_owned(), system.to_owned()),
        // The downlink, which is what the descriptor holds and what a channel
        // file naming a block writes. The daemon converts it.
        ("FREQUENCY".to_owned(), described.downlink_khz.to_string()),
    ];
    if !dish.block.is_empty() {
        channel.push(("LNB".to_owned(), dish.block.clone()));
    }
    if !dish.position.is_empty() {
        channel.push(("SAT_NUMBER".to_owned(), dish.position.clone()));
    }
    // H, V, L and R are what a channel file writes and what the daemon reads,
    // so the descriptor's own letter goes through as it stands.
    channel.push((
        "POLARIZATION".to_owned(),
        described.polarisation.to_string(),
    ));
    if described.symbol_rate != 0 {
        channel.push(("SYMBOL_RATE".to_owned(), described.symbol_rate.to_string()));
    }
    // Each of these is left out where the descriptor declined to say, rather
    // than written as a guess: a demodulator told nothing works it out, one
    // told the wrong thing does not.
    if let Some(modulation) = described.dvb_modulation() {
        channel.push(("MODULATION".to_owned(), modulation.to_owned()));
    }
    if let Some(roll_off) = described.dvb_roll_off() {
        channel.push(("ROLLOFF".to_owned(), roll_off.to_owned()));
    }
    if let Some(correction) = described.inner_fec() {
        channel.push(("INNER_FEC".to_owned(), correction.to_owned()));
    }
    Some(channel)
}

/// Turns one multiplex of a cable network into the properties a tune takes. One
/// reading rather than two: the cable descriptor gives its modulation a byte of
/// its own. The delivery system is what was tuned, because the descriptor does
/// not carry one.
#[must_use]
pub fn cable_channel(described: &nit::Cable, system: &str) -> Option<Channel> {
    if described.frequency_hz == 0 {
        return None;
    }
    let mut channel = vec![
        ("DELIVERY_SYSTEM".to_owned(), system.to_owned()),
        // Hertz, which is what a cable channel file writes: there is no
        // oscillator between the carrier and the receiver.
        ("FREQUENCY".to_owned(), described.frequency_hz.to_string()),
    ];
    if described.symbol_rate != 0 {
        channel.push(("SYMBOL_RATE".to_owned(), described.symbol_rate.to_string()));
    }
    if let Some(modulation) = described.modulation_name() {
        channel.push(("MODULATION".to_owned(), modulation.to_owned()));
    }
    if let Some(correction) = described.inner_fec() {
        channel.push(("INNER_FEC".to_owned(), correction.to_owned()));
    }
    Some(channel)
}

/// How far apart two figures for one carrier may be and still be that carrier.
/// A band's channels are megahertz apart, and the two accounts of one differ by
/// the offset a plan carries and a table rounds off.
const CARRIER_MARGIN_HZ: u32 = 1_000_000;

/// Puts what a cable operator said over the band plan.
///
/// The plan decides which carriers are walked, and the table decides how each
/// is tuned. Neither is the whole answer: a table that names three carriers has
/// not said the other hundred are empty, and a plan cannot say what symbol rate
/// an operator runs.
///
/// A carrier the plan already has keeps the plan's frequency and takes the
/// table's settings: a Japanese cable carries the plan's offset frequency,
/// which an operator's table rounds to the nominal figure. A carrier the plan
/// does not have is left alone.
#[must_use]
/// Nothing calls this today: the walk covers the whole band at the plan's
/// rates, and the cable tables seen here name only the carrier they came off.
/// It is kept, with `cable_channel` beside it, for the feed whose table does
/// name its carriers. The daemon's scanner keeps the same pair for the same
/// reason.
pub fn merge_carriers(planned: Vec<Channel>, named: &[Channel]) -> Vec<Channel> {
    let mut taken = vec![false; named.len()];
    let mut merged: Vec<Channel> = Vec::with_capacity(planned.len());
    for carrier in planned {
        let at = frequency_of(&carrier);
        let matched = named
            .iter()
            .enumerate()
            .position(|(index, spoken)| !taken[index] && nearby(at, frequency_of(spoken)));
        match matched {
            Some(index) => {
                taken[index] = true;
                merged.push(with_frequency(
                    named[index].clone(),
                    &frequency_written(&carrier),
                ));
            }
            None => merged.push(carrier),
        }
    }
    merged
}

/// Whether two figures name one carrier. Two that cannot be compared are not
/// the same one.
fn nearby(one: u32, other: u32) -> bool {
    one != 0 && other != 0 && one.abs_diff(other) < CARRIER_MARGIN_HZ
}

/// A channel's frequency as a number, or zero where it has none this can read.
#[must_use]
pub fn frequency_of(channel: &Channel) -> u32 {
    frequency_written(channel).parse().unwrap_or_default()
}

fn frequency_written(channel: &Channel) -> String {
    channel
        .iter()
        .find(|(name, _)| name == "FREQUENCY")
        .map_or_else(String::new, |(_, value)| value.clone())
}

/// One carrier's settings at the frequency the plan puts it.
fn with_frequency(mut channel: Channel, frequency: &str) -> Channel {
    for (name, value) in &mut channel {
        if name == "FREQUENCY" {
            value.clear();
            value.push_str(frequency);
        }
    }
    channel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(channel: &Channel, key: &str) -> Option<String> {
        channel
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    fn described(modulation: u8) -> nit::Satellite {
        nit::Satellite {
            downlink_khz: 11_727_480,
            orbital_position: 1100,
            east: true,
            polarisation: 'R',
            modulation,
            symbol_rate: 28_860_000,
            inner_correction: 3,
        }
    }

    /// The whole reason a convention has to be carried. ARIB gives the low five
    /// bits of the satellite descriptor's sixth byte to one modulation field;
    /// DVB divides the same five bits into a roll-off, a modulation system and
    /// a modulation type.
    #[test]
    fn one_byte_read_two_ways() {
        // Low bits 01110: ARIB reads a modulation of 14, DVB reads roll-off 25%
        // (bits 4-3 = 01), second generation (bit 2), and 8PSK (bits 1-0 = 10).
        let sent = described(0x0e);

        let arib = satellite_channel(&sent, "ISDBS", &Dish::default())
            .expect("the Japanese reading takes a Japanese transponder");
        // Nothing of the modulation byte reaches the channel: the demodulators
        // are told the standard, and ARIB's numbering is not one a channel file
        // spells.
        for unwanted in [
            "MODULATION",
            "ROLLOFF",
            "SYMBOL_RATE",
            "POLARIZATION",
            "INNER_FEC",
        ] {
            assert_eq!(at(&arib, unwanted), None, "{unwanted} was written");
        }
        assert_eq!(at(&arib, "DELIVERY_SYSTEM").as_deref(), Some("ISDBS"));
        assert_eq!(at(&arib, "FREQUENCY").as_deref(), Some("1049480"));

        let dish = Dish {
            block: "UNIVERSAL".to_owned(),
            position: String::new(),
        };
        let dvb = satellite_channel(&sent, "DVBS2", &dish).expect("the DVB reading takes it too");
        for (key, want) in [
            ("DELIVERY_SYSTEM", "DVBS2"),
            // The downlink as the descriptor gave it: the daemon subtracts the
            // named block's oscillator.
            ("FREQUENCY", "11727480"),
            ("LNB", "UNIVERSAL"),
            ("POLARIZATION", "R"),
            ("SYMBOL_RATE", "28860000"),
            ("MODULATION", "PSK/8"),
            ("ROLLOFF", "25"),
            ("INNER_FEC", "3/4"),
        ] {
            assert_eq!(at(&dvb, key).as_deref(), Some(want), "{key}");
        }
    }

    /// One network carries both generations on neighbouring transponders, so a
    /// DVB-S multiplex found from a DVB-S2 one has to be tuned as DVB-S.
    #[test]
    fn the_delivery_system_comes_from_the_descriptor() {
        // Bit 2 clear: first generation, QPSK.
        let channel = satellite_channel(&described(0x01), "DVBS2", &Dish::default())
            .expect("a placed multiplex");
        assert_eq!(at(&channel, "DELIVERY_SYSTEM").as_deref(), Some("DVBS"));
        // The roll-off field means nothing on DVB-S, so nothing is written.
        assert_eq!(at(&channel, "ROLLOFF"), None);
        assert_eq!(at(&channel, "MODULATION").as_deref(), Some("QPSK"));
    }

    /// A demodulator told nothing works it out; one told the wrong thing does
    /// not. Every field the descriptor declined to fill in is left out.
    #[test]
    fn nothing_is_guessed() {
        let mut silent = described(0x04);
        silent.symbol_rate = 0;
        silent.inner_correction = 0;
        silent.polarisation = 'V';
        let channel =
            satellite_channel(&silent, "DVBS2", &Dish::default()).expect("a placed multiplex");
        for unwanted in [
            "MODULATION",
            "SYMBOL_RATE",
            "INNER_FEC",
            "LNB",
            "SAT_NUMBER",
        ] {
            assert_eq!(at(&channel, unwanted), None, "{unwanted} was written");
        }
        assert_eq!(at(&channel, "POLARIZATION").as_deref(), Some("V"));
        assert_eq!(at(&channel, "ROLLOFF").as_deref(), Some("35"));
    }

    /// A dish reached through a switch needs the position sent with every
    /// tune, and only the person who installed it knows.
    #[test]
    fn a_dish_with_a_switch_says_which_port() {
        let dish = Dish {
            block: "UNIVERSAL".to_owned(),
            position: "2".to_owned(),
        };
        let channel = satellite_channel(&described(0x05), "DVBS2", &dish).expect("a placed one");
        assert_eq!(at(&channel, "SAT_NUMBER").as_deref(), Some("2"));
    }

    /// A descriptor that named nowhere, or nowhere this installation can
    /// reach, is not a channel.
    #[test]
    fn nothing_reachable_is_refused() {
        // Outside what the Japanese block converts.
        let mut outside = described(0);
        outside.downlink_khz = 10_700_000;
        assert!(satellite_channel(&outside, "ISDBS", &Dish::default()).is_none());
        // And a descriptor with no frequency at all says nothing either way.
        let mut nowhere = described(0);
        nowhere.downlink_khz = 0;
        assert!(satellite_channel(&nowhere, "DVBS2", &Dish::default()).is_none());
        assert!(satellite_channel(&nowhere, "ISDBS", &Dish::default()).is_none());
    }

    /// The 4K and 8K pass tunes as ISDB-S3 and what it finds has to say so: a
    /// channel recorded as ISDB-S is tuned as ISDB-S afterwards, which is not
    /// what it is.
    #[test]
    fn the_advanced_services_keep_their_own_system() {
        let mut left = described(0);
        left.downlink_khz = 11_976_820;
        left.polarisation = 'L';
        let channel =
            satellite_channel(&left, "ISDBS3", &Dish::default()).expect("a placed multiplex");
        assert_eq!(at(&channel, "DELIVERY_SYSTEM").as_deref(), Some("ISDBS3"));
        assert_eq!(at(&channel, "FREQUENCY").as_deref(), Some("2471820"));
    }

    /// A network table describes its own network, so the multiplex it was read
    /// out of and the multiplexes it describes are of a piece.
    #[test]
    fn the_convention_follows_what_was_tuned() {
        for (system, want) in [
            ("ISDBS", Convention::Arib),
            ("ISDBS3", Convention::Arib),
            ("DVBS", Convention::Dvb),
            ("DVBS2", Convention::Dvb),
            // Spelled as a settings file might, which is the same system.
            ("ISDB-S", Convention::Arib),
            ("DVB-S2", Convention::Dvb),
        ] {
            assert_eq!(convention_for(system), want, "{system}");
        }
    }

    /// A cable's frequencies are whoever runs it, so the network table is the
    /// only account there is. Its modulation has a byte of its own, so there is
    /// one reading.
    #[test]
    fn a_cable_reads_the_operators_own_account() {
        let sent = nit::Cable {
            frequency_hz: 474_000_000,
            modulation: 5,
            symbol_rate: 6_900_000,
            outer_correction: 0,
            inner_correction: 15,
            ..nit::Cable::default()
        };
        let channel = cable_channel(&sent, "DVBC/ANNEX_A").expect("a placed multiplex");
        for (key, want) in [
            ("DELIVERY_SYSTEM", "DVBC/ANNEX_A"),
            ("FREQUENCY", "474000000"),
            ("MODULATION", "QAM/256"),
            ("SYMBOL_RATE", "6900000"),
            // Fifteen is "none", which is a real answer and not the same as
            // the descriptor declining to say.
            ("INNER_FEC", "NONE"),
        ] {
            assert_eq!(at(&channel, key).as_deref(), Some(want), "{key}");
        }

        let silent = nit::Cable {
            frequency_hz: 474_000_000,
            ..nit::Cable::default()
        };
        let channel = cable_channel(&silent, "ISDBC").expect("a placed multiplex");
        for unwanted in ["MODULATION", "SYMBOL_RATE", "INNER_FEC"] {
            assert_eq!(at(&channel, unwanted), None, "{unwanted} was written");
        }
        assert!(cable_channel(&nit::Cable::default(), "DVBC/ANNEX_A").is_none());
    }

    /// The plan is the band and the table is how each carrier of it is
    /// modulated. Neither is the whole answer.
    #[test]
    fn the_plan_keeps_its_frequency_and_takes_the_operators_settings() {
        let planned = vec![
            // C23, at the offset a Japanese cable actually carries.
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_A".to_owned()),
                ("FREQUENCY".to_owned(), "225142857".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
            ],
            // C24, which the operator says nothing about.
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_A".to_owned()),
                ("FREQUENCY".to_owned(), "231142857".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
            ],
        ];
        let named = vec![
            // The same carrier, rounded to the nominal figure as a table does,
            // and running something other than the plan's guess.
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_A".to_owned()),
                ("FREQUENCY".to_owned(), "225000000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "6900000".to_owned()),
            ],
            // And one outside the band the plan covers, which this walk was
            // not asked to look at.
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_A".to_owned()),
                ("FREQUENCY".to_owned(), "770000000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "6900000".to_owned()),
            ],
        ];
        let merged = merge_carriers(planned, &named);
        // One carrier in, one carrier out: the plan decides what is walked.
        assert_eq!(merged.len(), 2);
        // The matched one keeps the plan's frequency -- tuning the rounded
        // figure asks the demodulator to find its way back -- and takes the
        // operator's settings.
        assert_eq!(at(&merged[0], "FREQUENCY").as_deref(), Some("225142857"));
        assert_eq!(at(&merged[0], "SYMBOL_RATE").as_deref(), Some("6900000"));
        // Untouched where the operator said nothing.
        assert_eq!(at(&merged[1], "FREQUENCY").as_deref(), Some("231142857"));
        assert_eq!(at(&merged[1], "SYMBOL_RATE").as_deref(), Some("5274000"));
    }

    /// A table that names two carriers a plan puts in one place must not have
    /// both folded onto it, and a figure that cannot be read is nobody's.
    /// Neither leftover is walked.
    #[test]
    fn one_carrier_is_not_taken_twice() {
        let planned = vec![vec![("FREQUENCY".to_owned(), "225142857".to_owned())]];
        let named = vec![
            vec![
                ("FREQUENCY".to_owned(), "225000000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "1".to_owned()),
            ],
            vec![
                ("FREQUENCY".to_owned(), "225100000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "2".to_owned()),
            ],
        ];
        let merged = merge_carriers(planned, &named);
        assert_eq!(merged.len(), 1);
        assert_eq!(at(&merged[0], "SYMBOL_RATE").as_deref(), Some("1"));

        let planned = vec![vec![("FREQUENCY".to_owned(), "225142857".to_owned())]];
        let named = vec![vec![("FREQUENCY".to_owned(), String::new())]];
        let merged = merge_carriers(planned, &named);
        assert_eq!(merged.len(), 1);
        assert_eq!(at(&merged[0], "FREQUENCY").as_deref(), Some("225142857"));
        assert_eq!(at(&merged[0], "SYMBOL_RATE"), None);
    }

    /// A carrier the plan already covers must not be added a second time: it
    /// would be walked twice and counted twice.
    #[test]
    fn a_carrier_the_plan_already_has_is_not_added_again() {
        // The whole cable band at its spacing, as the plan gives it.
        let planned: Vec<Channel> = (0..113)
            .map(|step| {
                vec![(
                    "FREQUENCY".to_owned(),
                    (93_000_000u32 + step * 6_000_000).to_string(),
                )]
            })
            .collect();
        // What an operator's table says about carriers of that same band,
        // rounded as a table rounds them.
        let named: Vec<Channel> = [111_000_000u32, 225_000_000, 471_000_000]
            .iter()
            .map(|hz| {
                vec![
                    ("FREQUENCY".to_owned(), hz.to_string()),
                    ("SYMBOL_RATE".to_owned(), "6900000".to_owned()),
                ]
            })
            .collect();
        let merged = merge_carriers(planned.clone(), &named);
        assert_eq!(
            merged.len(),
            planned.len(),
            "the walk is no longer the plan's own length"
        );
        // And the settings still landed on the carriers they belong to.
        let taken = merged
            .iter()
            .filter(|channel| channel.iter().any(|(name, _)| name == "SYMBOL_RATE"))
            .count();
        assert_eq!(taken, 3);
    }
}

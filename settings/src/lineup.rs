// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Building the list of channels a host is offered.
//!
//! Two things it can be built from: the band plan, which is the same on every
//! machine and needs nothing looked at, and the channels the settings file
//! defines. A space takes one or the other and never both: a space with lines
//! is exactly what the lines say. See [`crate::channels`] for the numbering and
//! [`crate::defined`] for the lines.

use crate::channels::{Channel, Lineup, Route, Space};
use crate::config::{self, Config, SpaceSetting};
use crate::defined::{Defined, Definition, Slot};
use coax::delivery::{self, Family};
use coax::plan::{self, Carrier};
use coax::tuning;

/// The spaces the settings ask for, filled from the plan alone.
pub fn build(config: &Config) -> Lineup {
    build_with(config, &Defined::none())
}

/// The same, with each space that has lines in the file filled from them
/// instead of from the plan.
pub fn build_with(config: &Config, defined: &Defined) -> Lineup {
    Lineup::new(
        config
            .spaces()
            .iter()
            .map(|setting| {
                let lines = defined.of(&setting.name);
                let channels = if lines.is_empty() {
                    planned(setting)
                } else {
                    from_lines(config, lines)
                };
                Space {
                    name: setting.name.clone(),
                    tuners: setting.tuners(),
                    channels,
                }
            })
            .collect(),
    )
}

/// A space's channels as its lines define them: one channel per stream, at the
/// index its first line's place gives it, with every later line for the same
/// stream as another way of receiving it.
fn from_lines(config: &Config, lines: &[Definition]) -> Vec<Channel> {
    Defined::seats(lines)
        .into_iter()
        .map(|seat| {
            let also = lines
                .iter()
                .filter(|line| line.key() == seat.key() && !std::ptr::eq(*line, seat))
                .map(|line| Route {
                    tune: tune_of(line),
                    tuners: config.tuners_for(&line.tuners),
                    carriers: carriers_of(line),
                })
                .collect();
            Channel {
                name: seat.name.clone(),
                tune: tune_of(seat),
                carriers: carriers_of(seat),
                tuners: config.tuners_for(&seat.tuners),
                also,
            }
        })
        .collect()
}

/// What the daemon is asked for, from a line. The line writes only what the
/// plan would not have: the delivery system's own figures are filled in here
/// the way they are for a planned carrier, and a satellite's stream is the
/// stream the line is about.
pub fn tune_of(line: &Definition) -> Vec<(String, String)> {
    let carrier = Carrier {
        name: line.name.clone(),
        frequency_hz: line.frequency().parse().unwrap_or_default(),
    };
    let mut tune = properties(&line.system, &carrier);
    // The frequency as written, whatever unit the system counts in: a
    // satellite's is kilohertz, and a parse into hertz above would have lost
    // nothing but is not what is sent.
    if let Some(frequency) = tune.iter_mut().find(|(name, _)| name == "FREQUENCY") {
        frequency.1 = line.frequency().to_owned();
    }
    match line.slot {
        Slot::Whole => {}
        Slot::Transport(number) => tune.push(("TSMF_REL_TS".to_owned(), number.to_string())),
        Slot::Tlv(number) => tune.push(("TSMF_REL_TLV".to_owned(), number.to_string())),
    }
    if delivery::family(&line.system) == Family::Satellite
        && !line.extras.iter().any(|(key, _)| key == "STREAM_ID")
    {
        tune.push(("STREAM_ID".to_owned(), line.tsid.to_string()));
    }
    for (key, value) in &line.extras {
        match tune.iter_mut().find(|(name, _)| name == key) {
            Some(had) => had.1 = value.clone(),
            None => tune.push((key.clone(), value.clone())),
        }
    }
    tune
}

/// A line written from what a tune turned out to be: the tune's own figures,
/// less the ones the plan would have filled in the same way.
pub fn line_of(
    onid: u16,
    tsid: u16,
    name: &str,
    tuners: &str,
    tune: &[(String, String)],
    carriers: &[String],
) -> Definition {
    let value = |key: &str| {
        tune.iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
    };
    // Written under the settings' name: the tune said what the daemon was
    // told, and a viewer reading the file is not the daemon.
    let system = delivery::named(&value("DELIVERY_SYSTEM").unwrap_or_default());
    let frequency = value("FREQUENCY").unwrap_or_default();
    let carriers: Vec<String> = if carriers.len() > 1 {
        carriers.to_vec()
    } else {
        vec![frequency.clone()]
    };
    let slot = match (value("TSMF_REL_TS"), value("TSMF_REL_TLV")) {
        (Some(number), _) => number.parse().map_or(Slot::Whole, Slot::Transport),
        (_, Some(number)) => number.parse().map_or(Slot::Whole, Slot::Tlv),
        _ => Slot::Whole,
    };
    let planned = properties(
        &system,
        &Carrier {
            name: name.to_owned(),
            frequency_hz: frequency.parse().unwrap_or_default(),
        },
    );
    let extras = tune
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.to_ascii_uppercase().as_str(),
                "DELIVERY_SYSTEM" | "FREQUENCY" | "TSMF_REL_TS" | "TSMF_REL_TLV" | "TUNER"
            )
        })
        .filter(|(key, value)| {
            // A satellite line is about its stream, so the stream needs no
            // saying; a figure the plan would have written is the plan's.
            !(key.eq_ignore_ascii_case("STREAM_ID") && *value == tsid.to_string())
                && !planned
                    .iter()
                    .any(|(had, was)| had.eq_ignore_ascii_case(key) && was == value)
        })
        .map(|(key, value)| (key.to_ascii_uppercase(), value.clone()))
        .collect();
    Definition {
        onid,
        tsid,
        name: name.to_owned(),
        tuners: tuners.to_owned(),
        system,
        carriers,
        slot,
        extras,
    }
}

/// The carriers a channel is on, where its frame is split across several.
fn carriers_of(line: &Definition) -> Vec<String> {
    if line.carriers.len() > 1 {
        line.carriers.clone()
    } else {
        Vec::new()
    }
}

/// A space's channels from the plan alone: each route's plan in turn, every
/// channel on the tuners of the route it came from. One space, two plans, is
/// what a platform on two leads looks like before anything has been discovered.
fn planned(setting: &SpaceSetting) -> Vec<Channel> {
    setting
        .routes
        .iter()
        .filter(|route| setting.offers(route))
        .flat_map(plan_of)
        .collect()
}

/// One route's plan as channels, each on the tuners of the route: what a
/// walk of the route asks about, and what a space of the plan's own platform
/// offers before anything has been discovered.
pub fn plan_of(route: &config::Route) -> Vec<Channel> {
    let channels = if delivery::family(&route.system) == Family::Sky {
        sky_channels(&route.system)
    } else {
        carriers(route)
            .into_iter()
            .map(|carrier| Channel::tuned(&carrier.name, properties(&route.system, &carrier)))
            .collect()
    };
    channels
        .into_iter()
        .map(|channel| Channel {
            tuners: route.tuners.clone(),
            ..channel
        })
        .collect()
}

/// The 124/128 degree east plan as channels. Everything a tune needs is in the
/// published table, so this space is usable with nothing discovered. The grid
/// carries two generations on the same frequencies, and a space that named one
/// offers only that one.
fn sky_channels(system: &str) -> Vec<Channel> {
    let wanted = delivery::named(system);
    plan::sky()
        .into_iter()
        .filter(|channel| wanted == delivery::DVBS2 || channel.system == wanted)
        .map(|channel| Channel::tuned(&channel.name, tuning::sky_words(&channel)))
        .collect()
}

/// Which walk of the plan fills a space. A space whose carriers the plan cannot
/// name -- the satellites, whose streams are found by reading a network's own
/// table -- starts empty and is filled by discovery.
fn carriers(route: &config::Route) -> Vec<Carrier> {
    match delivery::family(&route.system) {
        Family::Terrestrial => plan::terrestrial(&route.bands),
        Family::Cable => plan::cable(&route.bands),
        // The grid is what it is: it has no bands to choose between.
        Family::Optical => plan::optical(),
        // The sky plan fills its channels itself: every carrier of it takes a
        // different block, polarisation and symbol rate, so there is nothing
        // one set of properties would cover.
        Family::Sky | Family::Satellite | Family::Elsewhere => Vec::new(),
    }
}

/// What the daemon is asked for a carrier of the plan, under the name it
/// knows the system by. The sky plan writes its own, carrier by carrier; a
/// satellite space is filled by discovery, which writes what the network said.
fn properties(system: &str, carrier: &Carrier) -> Vec<(String, String)> {
    tuning::carrier_words(system, carrier)
}

#[cfg(test)]
mod tests {
    use super::{build, build_with, line_of, tune_of};
    use crate::config::Config;
    use crate::defined::{Defined, Definition, Slot};

    const LINES: &str = "
[Tuners]
sat = ISDBS, pt3:*
cable = ISDBC, tbs:*, bands=UHF

[Spaces]
0 = BS, sat, cable
1 = CATV, cable

[Channels BS]
4, 16625, BS15/16625, -, ISDBS, 1049480, -
4, 16625, BS15/16625, cable, ISDBC, 471142857, ts:2
4, 16626, BS15/16626, -, ISDBS, 1049480, -
";

    /// A space with lines in the file offers the lines and nothing else; one
    /// without is offered the plan. The two are never mixed.
    #[test]
    fn a_space_with_lines_offers_the_lines_and_not_the_plan() {
        let config = Config::parse(LINES);
        let lineup = build_with(&config, &Defined::parse(LINES));
        let bs = &lineup.spaces()[0].channels;
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].name, "BS15/16625");
        assert_eq!(bs[1].name, "BS15/16626");
        // The cable space has no lines, so it is the plan.
        assert_eq!(lineup.spaces()[1].channels.len(), 40);
    }

    /// The first line of a stream is the channel; the later ones are the other
    /// ways of receiving it, each on the tuners it names.
    #[test]
    fn the_first_line_of_a_stream_is_the_channel_and_the_rest_are_ways() {
        let config = Config::parse(LINES);
        let lineup = build_with(&config, &Defined::parse(LINES));
        let channel = &lineup.spaces()[0].channels[0];
        assert!(channel.tuners.is_empty(), "the line named none");
        assert_eq!(channel.also.len(), 1);
        assert_eq!(channel.also[0].tuners, vec!["tbs:*".to_owned()]);
        assert_eq!(
            channel.also[0].tune,
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "ISDBC".to_owned()),
                ("FREQUENCY".to_owned(), "471142857".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
                ("MODULATION".to_owned(), "QAM/AUTO".to_owned()),
                ("TSMF_REL_TS".to_owned(), "2".to_owned()),
            ]
        );
    }

    /// A line's tune is filled in the way a planned carrier's is: what the
    /// delivery system implies is not written on the line, and a satellite
    /// line is about its own stream.
    #[test]
    fn a_line_is_tuned_the_way_a_planned_carrier_is() {
        let line = Definition::parse("4, 16625, BS15/16625, -, ISDBS, 1049480, -").unwrap();
        assert_eq!(
            tune_of(&line),
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "ISDBS".to_owned()),
                ("FREQUENCY".to_owned(), "1049480".to_owned()),
                ("STREAM_ID".to_owned(), "16625".to_owned()),
            ]
        );
        // What the line does say wins over what the plan would have.
        let line = Definition::parse(
            "65534, 45281, C20/45281, -, ISDBC, 153000000+159000000, tlv:1, MODULATION=QAM/256",
        )
        .unwrap();
        let tune = tune_of(&line);
        assert!(tune.contains(&("MODULATION".to_owned(), "QAM/256".to_owned())));
        assert!(tune.contains(&("TSMF_REL_TLV".to_owned(), "1".to_owned())));
        assert!(tune.contains(&("SYMBOL_RATE".to_owned(), "5274000".to_owned())));
    }

    /// A line written from a tune says only what the plan would not, under
    /// the settings' name for the system, and reads back as the tune it was
    /// written from.
    #[test]
    fn a_line_written_from_a_tune_reads_back_as_that_tune() {
        let tune = vec![
            ("DELIVERY_SYSTEM".to_owned(), "ISDBC".to_owned()),
            ("FREQUENCY".to_owned(), "471142857".to_owned()),
            ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
            ("MODULATION".to_owned(), "QAM/AUTO".to_owned()),
            ("TSMF_REL_TS".to_owned(), "2".to_owned()),
        ];
        let line = line_of(4, 16625, "C23/16625", "cable", &tune, &[]);
        assert_eq!(line.slot, Slot::Transport(2));
        assert!(line.extras.is_empty(), "{:?}", line.extras);
        assert_eq!(
            line.render(),
            "4, 16625, C23/16625, cable, ISDBC, 471142857, ts:2"
        );
        assert_eq!(tune_of(&line), tune);
        // A satellite's stream is the line's own, and goes unsaid.
        let tune = vec![
            ("DELIVERY_SYSTEM".to_owned(), "ISDBS".to_owned()),
            ("FREQUENCY".to_owned(), "1049480".to_owned()),
            ("STREAM_ID".to_owned(), "16625".to_owned()),
        ];
        let line = line_of(4, 16625, "BS15/16625", "", &tune, &[]);
        assert_eq!(line.render(), "4, 16625, BS15/16625, -, ISDBS, 1049480, -");
        assert_eq!(tune_of(&line), tune);
    }

    /// A file that says nothing is a machine with one aerial: one space, the
    /// default bands, and no tuner named.
    #[test]
    fn a_driver_with_no_settings_offers_the_terrestrial_plan() {
        let lineup = build(&Config::default());
        let spaces = lineup.spaces();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].name, "地デジ");
        assert_eq!(spaces[0].channels.len(), 101);
        assert_eq!(spaces[0].channels[0].name, "13");
        assert!(spaces[0].tuners.is_empty());
    }

    #[test]
    fn a_terrestrial_channel_carries_its_frequency_and_bandwidth() {
        let lineup = build(&Config::parse(
            "[Tuners]\nter = ISDBT, pt3:*, bands=UHF\n[Spaces]\n0 = 地デジ, ter",
        ));
        let first = &lineup.spaces()[0].channels[0];
        assert_eq!(
            first.tune,
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "ISDBT".to_owned()),
                ("FREQUENCY".to_owned(), "473142857".to_owned()),
                ("BANDWIDTH_HZ".to_owned(), "6000000".to_owned()),
            ]
        );
        assert_eq!(lineup.spaces()[0].channels.len(), 40);
    }

    /// The cable plan is the same channels a seventh of a megahertz lower,
    /// with what a cable takes beyond a frequency.
    #[test]
    fn a_cable_channel_carries_its_symbol_rate() {
        let lineup = build(&Config::parse(
            "[Spaces]\n0 = CATV, cable\n[Tuners]\ncable = ISDBC, tbs:*, bands=UHF",
        ));
        let space = &lineup.spaces()[0];
        assert_eq!(space.tuners, vec!["tbs:*".to_owned()]);
        assert_eq!(space.channels[0].tuners, vec!["tbs:*".to_owned()]);
        assert_eq!(
            space.channels[0].tune,
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "ISDBC".to_owned()),
                ("FREQUENCY".to_owned(), "473000000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
                ("MODULATION".to_owned(), "QAM/AUTO".to_owned()),
            ]
        );
    }

    /// The cable is asked for under its own name, whichever way the settings
    /// spell it: the daemon takes ISDB-C for J.83 Annex A itself, and a line
    /// written from a scan then says what the band is.
    #[test]
    fn a_cable_route_is_told_to_the_daemon_under_the_name_it_knows() {
        for system in ["ISDBC", "isdb-c", "DVBC/ANNEX_A", "DVBC/ANNEX_C"] {
            let lineup = build(&Config::parse(&format!(
                "[Spaces]\n0 = CATV, cable\n[Tuners]\ncable = {system}, tbs:*, bands=UHF"
            )));
            let tune = &lineup.spaces()[0].channels[0].tune;
            assert!(
                tune.contains(&("DELIVERY_SYSTEM".to_owned(), "ISDBC".to_owned())),
                "{system}: {tune:?}"
            );
            assert!(tune.contains(&("SYMBOL_RATE".to_owned(), "5274000".to_owned())));
        }
        let lineup = build(&Config::parse(
            "[Tuners]\nj83b_5_6 = J83B_5_6, tbs:*\n[Spaces]\n0 = SKP, j83b_5_6",
        ));
        let tune = &lineup.spaces()[0].channels[0].tune;
        assert!(tune.contains(&("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_B".to_owned())));
        assert!(tune.contains(&("SYMBOL_RATE".to_owned(), "5600000".to_owned())));
    }

    /// The tuner named for the whole driver stands in for a space that names
    /// none, which is what a settings file written before there were spaces
    /// says.
    #[test]
    fn a_space_that_names_no_tuner_takes_the_drivers_own() {
        let lineup = build(&Config::parse("[coax]\nTuner = tbs:abc:0"));
        assert_eq!(lineup.spaces()[0].tuners, vec!["tbs:abc:0".to_owned()]);
    }

    /// The optical distribution is a grid of its own, with a rate and a
    /// modulation of its own: the cable's would not lock on it.
    #[test]
    fn an_optical_space_is_filled_from_its_own_grid() {
        let lineup = build(&Config::parse(
            "[Tuners]\nhikari = DVBC/ANNEX_B, tbs:*\n[Spaces]\n0 = 光, hikari",
        ));
        let channels = &lineup.spaces()[0].channels;
        assert_eq!(channels.len(), 108);
        assert_eq!(
            channels[0].tune,
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_B".to_owned()),
                ("FREQUENCY".to_owned(), "73750000".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5600000".to_owned()),
                ("MODULATION".to_owned(), "QAM/256".to_owned()),
            ]
        );
    }

    /// The 124/128 degree east service is a plan, not a network to ask: every
    /// carrier of it can be tuned from the published table alone, so the space
    /// is usable before anything has been discovered.
    #[test]
    fn a_sky_space_is_filled_from_the_published_table() {
        let lineup = build(&Config::parse(
            "[Tuners]\nsat = DVBS2, tbs:*\n[Spaces]\n0 = SKY, sat",
        ));
        let channels = &lineup.spaces()[0].channels;
        assert_eq!(channels.len(), 120);
        assert_eq!(channels[0].name, "JCSAT3A/JD1");
        assert_eq!(
            channels[0].tune,
            vec![
                ("DELIVERY_SYSTEM".to_owned(), "DVBS2".to_owned()),
                ("FREQUENCY".to_owned(), "12508000".to_owned()),
                ("LNB".to_owned(), "JCSAT3A".to_owned()),
                ("POLARIZATION".to_owned(), "VERTICAL".to_owned()),
                ("SYMBOL_RATE".to_owned(), "23303000".to_owned()),
                ("MODULATION".to_owned(), "PSK/8".to_owned()),
            ]
        );
        // A space that names the older generation offers only that one.
        let older = build(&Config::parse(
            "[Tuners]\nsat = DVBS, tbs:*\n[Spaces]\n0 = SKY, sat",
        ));
        assert_eq!(older.spaces()[0].channels.len(), 60);
        assert_eq!(older.spaces()[0].channels[0].name, "JCSAT3A/JD1/QPSK");
    }

    /// A satellite space has no carriers of its own until something has read
    /// the network's table. It is still offered, so that the space numbering
    /// does not move once it fills.
    #[test]
    fn a_satellite_space_starts_empty_and_keeps_its_place() {
        let lineup = build(&Config::parse(
            "[Tuners]\nter = ISDBT, pt3:*\nsat = ISDBS, pt3:*\n[Spaces]\n0 = 地デジ, ter\n1 = BS, sat",
        ));
        assert_eq!(lineup.spaces().len(), 2);
        assert_eq!(lineup.spaces()[1].name, "BS");
        assert!(lineup.spaces()[1].channels.is_empty());
    }

    /// A space on two leads is both plans, each channel on the tuners of the
    /// lead it is on and over the bands of that lead.
    #[test]
    fn a_space_on_two_leads_offers_both_plans_each_on_its_own_tuners() {
        let lineup = build(&Config::parse(
            "[Tuners]\nter = ISDBT, pt3:*, bands=UHF\npassthru = ISDBT, tbs:*, bands=UHF,CATV\n\
             [Spaces]\n0 = 地デジ, ter, passthru",
        ));
        let space = &lineup.spaces()[0];
        assert_eq!(space.channels.len(), 40 + 40 + 51);
        assert_eq!(space.channels[0].tuners, vec!["pt3:*".to_owned()]);
        assert_eq!(space.channels[40].tuners, vec!["tbs:*".to_owned()]);
        assert_eq!(space.channels[40].name, "13");
        assert_eq!(space.channels[80].name, "C13");
        assert_eq!(space.tuners, vec!["pt3:*".to_owned(), "tbs:*".to_owned()]);
    }

    /// A space that holds one platform and names another platform's route
    /// offers nothing of that route's plan: BS with the cable it is
    /// transmodulated onto is where the cable's BS goes, not a hundred cable
    /// carriers called BS.
    #[test]
    fn a_space_of_another_platform_offers_none_of_the_cables_plan() {
        let lineup = build(&Config::parse(
            "[Tuners]\nsat = ISDBS, pt3:*\ncable = ISDBC, tbs:*\n\
             [Spaces]\n0 = BS, sat, cable\n1 = 地デジ, cable\n2 = CATV, cable",
        ));
        assert!(lineup.spaces()[0].channels.is_empty());
        assert!(lineup.spaces()[1].channels.is_empty());
        assert_eq!(lineup.spaces()[2].channels.len(), 113);
        // The route is still the space's, for what is found on it.
        assert_eq!(
            lineup.spaces()[0].tuners,
            vec!["pt3:*".to_owned(), "tbs:*".to_owned()]
        );
    }

    /// The spaces come out in the order the file wrote them, because their
    /// numbers are what a host writes into a viewer's channel file.
    #[test]
    fn the_spaces_keep_the_order_the_file_gave_them() {
        let lineup = build(&Config::parse(
            "[Tuners]\nter = ISDBT, pt3:*\ncable = ISDBC, tbs:*\n[Spaces]\n0 = 地デジ, ter\n1 = BS, cable\n2 = CS, cable",
        ));
        let names: Vec<&str> = lineup
            .spaces()
            .iter()
            .map(|space| space.name.as_str())
            .collect();
        assert_eq!(names, ["地デジ", "BS", "CS"]);
    }
}

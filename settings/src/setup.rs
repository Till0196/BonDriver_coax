// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The `[Tuners]`, `[Spaces]` and `[TunerSettings]` sections as something to
//! edit and write back.
//!
//! [`crate::config::Config`] reads the file for tuning, and in doing so
//! resolves what it reads: routes go into a map, a space's labels become the
//! routes they stand for, and a label nothing defines disappears. An editor
//! needs the file as written -- routes in their order, labels as words, the
//! ones that point nowhere included -- and needs to put it back without
//! touching anything else in the file.
//!
//! It also knows what a file that has neither section should start from.

use crate::config::{self, Route};
use crate::tuner_settings::TunerSettings;
use coax::daemon::Tuner;
use coax::delivery::{self, Family};
use coax::group;
use coax::plan::Band;

/// The sections, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Setup {
    /// The routes of `[Tuners]`, in the file's order.
    pub routes: Vec<Route>,
    /// The lines of `[Spaces]`, in the file's order.
    pub spaces: Vec<SpaceLine>,
    /// The lines of `[TunerSettings]`. A file without any has no section.
    pub tuner_settings: TunerSettings,
}

/// One line of `[Spaces]`, as written: the labels are words, whether or not
/// `[Tuners]` defines them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpaceLine {
    pub name: String,
    pub labels: Vec<String>,
    /// The `group=` option, empty where there was none.
    pub group: String,
}

/// The spaces a viewer is offered to switch on, in the order they are
/// usually wanted. Each names a platform, so none needs a `group=`.
pub const KNOWN_SPACES: [&str; 7] = ["地デジ", "BS", "CS", "BS4K", "CS4K", "CATV", "SKP"];

/// Which of the known spaces a file that has none starts with.
const SPACES_ON_AT_FIRST: [&str; 5] = ["地デジ", "BS", "CS", "BS4K", "CATV"];

/// The routes a file that has none starts with: a label and what it is tuned
/// as. One per way of receiving television in this country, labelled after the
/// system; a second way of receiving the same system is a route a viewer adds.
pub const DEFAULT_ROUTES: [(&str, &str); 6] = [
    ("isdbt", delivery::ISDBT),
    ("isdbs", delivery::ISDBS),
    ("isdbs3", delivery::ISDBS3),
    ("isdbc", delivery::ISDBC),
    ("dvbs2", delivery::DVBS2),
    ("j83b_5_6", delivery::J83B_5_6),
];

impl Setup {
    /// Reads the two sections out of the file's text, as written.
    pub fn read(text: &str) -> Self {
        let mut setup = Self::default();
        for (section, name, value) in config::entries(text) {
            match section.as_str() {
                "tuners" => {
                    if let Some(route) = Route::parse(name.trim(), &value) {
                        setup.routes.push(route);
                    }
                }
                "spaces" => {
                    if let Some((space, labels)) = config::SpaceSetting::parse(&value) {
                        setup.spaces.push(SpaceLine {
                            name: space.name,
                            labels,
                            group: space.group,
                        });
                    }
                }
                "tunersettings" => setup.tuner_settings.add_line(&name, &value),
                _ => {}
            }
        }
        setup
    }

    /// Whether the file said nothing in `[Tuners]` or `[Spaces]`, which is when
    /// the suggested setup is what an editor should open with.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.spaces.is_empty()
    }

    /// What a machine that has said nothing starts from: the default routes,
    /// each on the tuners the daemon says receive its system, and the spaces
    /// most viewers want, each holding the routes that belong to it.
    pub fn suggested(tuners: &[Tuner]) -> Self {
        let routes: Vec<Route> = DEFAULT_ROUTES
            .iter()
            .map(|(label, system)| Route {
                label: (*label).to_owned(),
                system: (*system).to_owned(),
                tuners: patterns_for(tuners, system),
                bands: suggested_bands(system),
            })
            .collect();
        let spaces = SPACES_ON_AT_FIRST
            .iter()
            .map(|name| SpaceLine {
                name: (*name).to_owned(),
                labels: routes
                    .iter()
                    .filter(|route| belongs(name, &route.system))
                    .map(|route| route.label.clone())
                    .collect(),
                group: String::new(),
            })
            .collect();
        Self {
            routes,
            spaces,
            tuner_settings: TunerSettings::default(),
        }
    }

    /// What is wrong with it, in words a viewer can act on; nothing when it
    /// can be written.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (index, route) in self.routes.iter().enumerate() {
            let label = route.label.trim();
            if label.is_empty() {
                problems.push(format!("{}番目の経路にラベルがありません", index + 1));
            } else if label.contains(|letter: char| {
                letter.is_whitespace() || matches!(letter, ',' | '=' | ';' | '#' | '[' | ']')
            }) {
                problems.push(format!("ラベル「{label}」に使えない文字があります"));
            } else if self.routes[..index]
                .iter()
                .any(|other| other.label.trim() == label)
            {
                problems.push(format!("ラベル「{label}」が重複しています"));
            }
            if route.system.trim().is_empty() {
                problems.push(format!("経路「{label}」に伝送方式がありません"));
            }
        }
        for space in &self.spaces {
            let name = space.name.trim();
            if name.is_empty() {
                problems.push("名前のない空間があります".to_owned());
                continue;
            }
            if name.contains(',') {
                problems.push(format!("空間の名前「{name}」にコンマは使えません"));
            }
            for label in &space.labels {
                if !self.routes.iter().any(|route| route.label == *label) {
                    problems.push(format!(
                        "空間「{name}」の経路「{label}」は [Tuners] にありません"
                    ));
                }
            }
        }
        problems
    }

    /// The routes no space names, which a sweep would never walk.
    pub fn unplaced(&self) -> Vec<&Route> {
        self.routes
            .iter()
            .filter(|route| {
                !self
                    .spaces
                    .iter()
                    .any(|space| space.labels.contains(&route.label))
            })
            .collect()
    }

    /// The two sections as text, ready to go into the file.
    pub fn render(&self) -> String {
        let mut text = self.render_tuners();
        text.push('\n');
        text.push_str(&self.render_spaces());
        text
    }

    fn render_tuners(&self) -> String {
        let mut text = String::from("[Tuners]\n");
        text.push_str("; ラベル = 伝送方式, チューナー... [, bands=...]\n");
        let label_wide = self
            .routes
            .iter()
            .map(|route| width(&route.label))
            .max()
            .unwrap_or(0);
        let system_wide = self
            .routes
            .iter()
            .map(|route| width(&route.system) + 1)
            .max()
            .unwrap_or(0);
        for route in &self.routes {
            let mut rest: Vec<String> = route.tuners.clone();
            if bands_are_written(&route.system) {
                rest.push(format!(
                    "bands={}",
                    Band::ALL
                        .iter()
                        .filter(|band| route.bands.contains(band))
                        .map(|band| band.name())
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let line = if rest.is_empty() {
                route.system.clone()
            } else {
                format!(
                    "{} {}",
                    pad(&format!("{},", route.system), system_wide),
                    rest.join(", ")
                )
            };
            text.push_str(&format!("{} = {line}\n", pad(&route.label, label_wide)));
        }
        text
    }

    fn render_spaces(&self) -> String {
        let mut text = String::from("[Spaces]\n");
        text.push_str("; 番号 = 名前, 経路... [, group=...]\n");
        let name_wide = self
            .spaces
            .iter()
            .map(|space| width(&space.name) + 1)
            .max()
            .unwrap_or(0);
        for (index, space) in self.spaces.iter().enumerate() {
            let mut fields = space.labels.clone();
            if !space.group.is_empty() {
                fields.push(format!("group={}", space.group));
            }
            let name = format!("{},", space.name);
            let line = if fields.is_empty() {
                space.name.clone()
            } else {
                format!("{} {}", pad(&name, name_wide), fields.join(", "))
            };
            text.push_str(&format!("{index} = {line}\n"));
        }
        text
    }

    /// The file's text with these sections in place of whatever it had.
    ///
    /// The file is the viewer's: only the bodies of `[Tuners]`, `[Spaces]` and
    /// `[TunerSettings]` are replaced. A section the file did not have goes in
    /// before the first `[Channels …]`, or at the end where there is none.
    /// `[TunerSettings]` with nothing in it is no section at all.
    pub fn write_into(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 1024);
        let (mut tuners_done, mut spaces_done) = (false, false);
        // Nothing to write counts as written.
        let mut settings_done = self.tuner_settings.is_empty();
        let mut skipping = false;
        for line in text.lines() {
            if is_header(line) {
                skipping = false;
                match section_named(line) {
                    Some("tuners") => {
                        if !tuners_done {
                            out.push_str(&self.render_tuners());
                            out.push('\n');
                            tuners_done = true;
                        }
                        skipping = true;
                        continue;
                    }
                    Some("spaces") => {
                        if !spaces_done {
                            out.push_str(&self.render_spaces());
                            out.push('\n');
                            spaces_done = true;
                        }
                        skipping = true;
                        continue;
                    }
                    Some("tunersettings") => {
                        if !settings_done {
                            out.push_str(&self.tuner_settings.render());
                            out.push('\n');
                            settings_done = true;
                        }
                        skipping = true;
                        continue;
                    }
                    Some("channels") => {
                        self.fill_missing(
                            &mut out,
                            &mut tuners_done,
                            &mut spaces_done,
                            &mut settings_done,
                        );
                    }
                    _ => {}
                }
            }
            if skipping {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        self.fill_missing(
            &mut out,
            &mut tuners_done,
            &mut spaces_done,
            &mut settings_done,
        );
        out
    }

    fn fill_missing(
        &self,
        out: &mut String,
        tuners_done: &mut bool,
        spaces_done: &mut bool,
        settings_done: &mut bool,
    ) {
        for (done, render) in [
            (tuners_done, Self::render_tuners as fn(&Self) -> String),
            (spaces_done, Self::render_spaces),
            (settings_done, |setup: &Self| setup.tuner_settings.render()),
        ] {
            if *done {
                continue;
            }
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(&render(self));
            out.push('\n');
            *done = true;
        }
    }
}

/// Whether a route of this system belongs in a space of this name, going by
/// what the space's platform is received on: the aerial's platform on the
/// aerial and on the cable that carries it again, the satellites' on the dish
/// and on the cable, the 124/128 degree east service on its dish and its
/// optical distribution, and the cable's own only on the cable. This is where a
/// route goes when nobody has said.
pub fn belongs(space: &str, system: &str) -> bool {
    let family = delivery::family(system);
    let system = delivery::named(system);
    match group::named(space) {
        Some("GR") => family == Family::Terrestrial || family == Family::Cable,
        Some("BS" | "CS") => system == delivery::ISDBS || family == Family::Cable,
        Some("BS4K" | "CS4K") => system == delivery::ISDBS3 || family == Family::Cable,
        Some("CATV") => family == Family::Cable,
        Some("SKY") => family == Family::Sky || family == Family::Optical,
        _ => false,
    }
}

/// What a route of this system walks when nobody has said. The aerial's route
/// walks every band, because the ordinary machine with one lead does not know
/// whether that lead is an aerial or a cable's pass-through. A route without
/// bands keeps what the reader would give it, so that what is suggested reads
/// back as itself.
fn suggested_bands(system: &str) -> Vec<Band> {
    if bands_are_written(system) {
        Band::ALL.to_vec()
    } else {
        config::default_bands(system)
    }
}

/// Whether a route of this system has stretches of a plan to write.
///
/// The aerial and cable plans are walked stretch by stretch, so they have them.
/// The 110 degree east satellites are asked rather than walked, and the 124/128
/// degree east service is walked on a grid of its own.
///
/// A system this does not recognise is one somebody typed in: the stretches
/// stay theirs to set, and are written down so that they read back.
pub fn bands_are_written(system: &str) -> bool {
    matches!(
        delivery::family(system),
        Family::Terrestrial | Family::Cable | Family::Elsewhere
    )
}

/// The patterns naming the tuners that say they receive a system, written
/// the shortest way that names exactly them. A tuner that did not say what
/// it receives is not chosen; it can still be named by hand.
pub fn patterns_for(tuners: &[Tuner], system: &str) -> Vec<String> {
    patterns_naming(tuners, |tuner| {
        !tuner.deliveries.is_empty() && tuner.receives(system)
    })
}

/// The patterns naming the tuners a viewer ticked, written the shortest way
/// that names exactly them.
pub fn patterns_of(tuners: &[Tuner], ids: &[String]) -> Vec<String> {
    patterns_naming(tuners, |tuner| ids.contains(&tuner.id))
}

/// The tuners a route's patterns name, of the ones the daemon has.
pub fn named_by(tuners: &[Tuner], patterns: &[String]) -> Vec<String> {
    tuners
        .iter()
        .filter(|tuner| patterns.iter().any(|pattern| tuner.matches(pattern)))
        .map(|tuner| tuner.id.clone())
        .collect()
}

/// The patterns naming the chosen tuners, written the shortest way that names
/// exactly them.
///
/// A whole card that is chosen is named by its model, which survives the card
/// being moved -- unless another card of the same model is not wholly chosen,
/// in which case the model would name both. A card only some of whose tuners
/// are chosen is named tuner by tuner: a pattern naming the card would declare
/// the other half a receiver of the system too (see [`coax::daemon::declare`]).
fn patterns_naming(tuners: &[Tuner], chosen: impl Fn(&Tuner) -> bool) -> Vec<String> {
    // The cards, in the order their tuners were listed.
    let mut cards: Vec<(String, Vec<&Tuner>)> = Vec::new();
    for tuner in tuners {
        let card = card_of(&tuner.id);
        match cards.iter_mut().find(|(known, _)| *known == card) {
            Some((_, members)) => members.push(tuner),
            None => cards.push((card, vec![tuner])),
        }
    }
    let whole = |members: &[&Tuner]| members.iter().all(|tuner| chosen(tuner));
    let mut patterns: Vec<String> = Vec::new();
    for (_, members) in &cards {
        if whole(members) {
            let model = model_of(&members[0].model);
            let all_of_the_model_are_whole = !model.is_empty()
                && cards
                    .iter()
                    .filter(|(_, others)| model_of(&others[0].model) == model)
                    .all(|(_, others)| whole(others));
            if all_of_the_model_are_whole {
                let pattern = format!("model:{model}*");
                if !patterns.contains(&pattern) {
                    patterns.push(pattern);
                }
                continue;
            }
        }
        patterns.extend(
            members
                .iter()
                .filter(|tuner| chosen(tuner))
                .map(|tuner| tuner.id.clone()),
        );
    }
    patterns
}

/// The card a tuner's identifier belongs to: the identifier less the number
/// of the tuner on the card, which is what follows the last `:` or `/`.
pub fn card_of(id: &str) -> String {
    match id.rfind([':', '/']) {
        Some(at)
            if id[at + 1..].chars().all(|letter| letter.is_ascii_digit()) && at + 1 < id.len() =>
        {
            id[..at].to_owned()
        }
        _ => id.to_owned(),
    }
}

/// The model a card's description names, less the tuner's number where the
/// daemon put one on: "TBS 6205se Tuner 2" is a card called "TBS 6205se".
pub fn model_of(description: &str) -> String {
    let description = description.trim();
    let mut words: Vec<&str> = description.split_whitespace().collect();
    if words.len() >= 2
        && words[words.len() - 1]
            .chars()
            .all(|letter| letter.is_ascii_digit())
        && words[words.len() - 2].eq_ignore_ascii_case("tuner")
    {
        words.truncate(words.len() - 2);
    }
    words.join(" ")
}

/// Which section a header line opens: one of the three this writes,
/// `channels` for any `[Channels …]`, `other` for the rest.
fn section_named(line: &str) -> Option<&'static str> {
    let name = line
        .trim()
        .strip_prefix('[')?
        .strip_suffix(']')?
        .trim()
        .to_ascii_lowercase();
    Some(match name.as_str() {
        "tuners" => "tuners",
        "spaces" => "spaces",
        "tunersettings" => "tunersettings",
        other if other.starts_with("channels") => "channels",
        _ => "other",
    })
}

fn is_header(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('[') && line.ends_with(']')
}

/// How many columns a string takes, counting a wide character as two, so
/// that a name in kana lines up with one in Latin letters.
fn width(text: &str) -> usize {
    text.chars()
        .map(|letter| if letter.is_ascii() { 1 } else { 2 })
        .sum()
}

fn pad(text: &str, to: usize) -> String {
    let mut padded = text.to_owned();
    for _ in width(text)..to {
        padded.push(' ');
    }
    padded
}

#[cfg(test)]
mod tests {
    use super::{
        Setup, SpaceLine, belongs, card_of, model_of, named_by, patterns_for, patterns_of,
    };
    use crate::config::{Config, Route};
    use coax::daemon::Tuner;
    use coax::plan::Band;

    fn tuner(id: &str, model: &str, deliveries: &[&str]) -> Tuner {
        Tuner {
            id: id.to_owned(),
            model: model.to_owned(),
            deliveries: deliveries.iter().map(|one| (*one).to_owned()).collect(),
            leased: false,
            name: String::new(),
        }
    }

    const FILE: &str = "\
; my file
[coax]
Host = 127.0.0.1:40200

[Tuners]
; the old comment
isdbt = ISDBT, pt3:*, bands=UHF
cable = DVBC/ANNEX_A, tbs:*

[Spaces]
0 = 地デジ, isdbt, cable
1 = CATV, cable, nothing

[Channels 地デジ]
; onid, tsid, name, route, system, frequency, slot [, KEY=VALUE ...]
32736, 32736, 27/32736, isdbt, ISDBT, 557142857, -
";

    #[test]
    fn the_sections_are_read_as_written() {
        let setup = Setup::read(FILE);
        assert_eq!(setup.routes.len(), 2);
        assert_eq!(setup.routes[0].label, "isdbt");
        assert_eq!(setup.routes[0].bands, vec![Band::Uhf]);
        assert_eq!(setup.routes[1].system, "DVBC/ANNEX_A");
        assert_eq!(setup.spaces.len(), 2);
        // A label nothing defines is kept: an editor shows it as wrong.
        assert_eq!(setup.spaces[1].labels, ["cable", "nothing"]);
        assert_eq!(
            setup.problems(),
            ["空間「CATV」の経路「nothing」は [Tuners] にありません"]
        );
    }

    #[test]
    fn writing_replaces_only_the_two_sections() {
        let mut setup = Setup::read(FILE);
        setup.spaces[1].labels.pop();
        setup.routes[1].system = "ISDBC".to_owned();
        let written = setup.write_into(FILE);
        assert!(written.starts_with("; my file\n[coax]\nHost = 127.0.0.1:40200\n\n[Tuners]\n"));
        assert!(!written.contains("the old comment"), "{written}");
        assert!(
            written.contains("isdbt = ISDBT, pt3:*, bands=UHF\n"),
            "{written}"
        );
        assert!(
            written.contains("cable = ISDBC, tbs:*, bands=UHF,CATV,EXUHF,VHF\n"),
            "{written}"
        );
        assert!(written.contains("1 = CATV,   cable\n"), "{written}");
        assert!(written.ends_with(
            "[Channels 地デジ]\n; onid, tsid, name, route, system, frequency, slot [, KEY=VALUE ...]\n32736, 32736, 27/32736, isdbt, ISDBT, 557142857, -\n"
        ), "{written}");
        // And what was written reads back as itself, through both readers.
        assert_eq!(Setup::read(&written), setup);
        let config = Config::parse(&written);
        assert_eq!(config.spaces.len(), 2);
        assert_eq!(config.tuners["cable"].system, "ISDBC");
    }

    #[test]
    fn a_file_without_the_sections_gets_them_before_the_channels() {
        let file = "[coax]\nHost = x\n\n[Channels BS]\n4, 1, a, -, ISDBS, 1, -\n";
        let setup = Setup {
            routes: vec![Route {
                label: "isdbs".to_owned(),
                system: "ISDBS".to_owned(),
                tuners: vec!["model:TBS 6205se*".to_owned()],
                bands: Vec::new(),
            }],
            spaces: vec![SpaceLine {
                name: "BS".to_owned(),
                labels: vec!["isdbs".to_owned()],
                group: String::new(),
            }],
            ..Setup::default()
        };
        let written = setup.write_into(file);
        assert_eq!(
            written,
            "[coax]\nHost = x\n\n[Tuners]\n; ラベル = 伝送方式, チューナー... [, bands=...]\nisdbs = ISDBS, model:TBS 6205se*\n\n[Spaces]\n; 番号 = 名前, 経路... [, group=...]\n0 = BS, isdbs\n\n[Channels BS]\n4, 1, a, -, ISDBS, 1, -\n"
        );
        // An empty file gets them at the end, which is the beginning.
        assert!(setup.write_into("").starts_with("[Tuners]\n"));
    }

    /// `[TunerSettings]` is written ahead of the channels, as the other two are,
    /// and reads back as itself; a setup emptied of it loses the section. A
    /// section this does not write is copied as it stands.
    #[test]
    fn the_tuner_settings_are_written_with_the_rest() {
        let file = format!("{FILE}\n[TunerSettings]\npt3:*:0 = lnb=high\n\n[Mine]\nkeep = this\n");
        let mut setup = Setup::read(&file);
        assert_eq!(setup.tuner_settings.lines().len(), 1);
        setup
            .tuner_settings
            .add_line("model:TBS 6205se*", "gain=auto");
        let written = setup.write_into(&file);
        assert!(
            written.contains(
                "[TunerSettings]\n; チューナー = lnb=off|low|high, gain=auto|min|max|+NdB（空はそのチューナーの既定）\npt3:*:0           = lnb=high\nmodel:TBS 6205se* = gain=auto\n"
            ),
            "{written}"
        );
        assert!(written.contains("[Mine]\nkeep = this\n"), "{written}");
        assert!(written.contains("[Channels 地デジ]\n"), "{written}");
        assert_eq!(Setup::read(&written), setup);
        let config = Config::parse(&written);
        assert_eq!(config.tuner_settings, setup.tuner_settings);
        assert_eq!(config.spaces.len(), 2);
        // Before the channels, where the file had none.
        let added = setup.write_into(FILE);
        let settings_at = added.find("[TunerSettings]").expect("the section is added");
        let channels_at = added.find("[Channels").expect("the channels stay");
        assert!(settings_at < channels_at, "{added}");
        // Emptied, the section goes.
        let mut emptied = setup.clone();
        emptied.tuner_settings = Default::default();
        let gone = emptied.write_into(&written);
        assert!(!gone.contains("[TunerSettings]"), "{gone}");
        assert!(gone.contains("[Mine]\nkeep = this\n"), "{gone}");
        // And a file that never had it is not given an empty one.
        assert!(
            !Setup::read(FILE)
                .write_into(FILE)
                .contains("[TunerSettings]")
        );
    }

    #[test]
    fn the_suggested_setup_is_the_ordinary_machine() {
        let tuners = [
            tuner(
                "tbs:aaaa:0",
                "TBS 6205se Tuner 1",
                &["ISDB-T", "ISDB-S", "DVB-C/ANNEX_A"],
            ),
            tuner(
                "tbs:aaaa:1",
                "TBS 6205se Tuner 2",
                &["ISDB-T", "ISDB-S", "DVB-C/ANNEX_A"],
            ),
            tuner("pt3:6-17-0-0:0", "Earthsoft PT3", &["ISDB-S"]),
            tuner("pt3:6-17-0-0:1", "Earthsoft PT3", &["ISDB-S"]),
            tuner("pt3:6-17-0-0:2", "Earthsoft PT3", &["ISDB-T"]),
            tuner("pt3:6-17-0-0:3", "Earthsoft PT3", &["ISDB-T"]),
        ];
        let setup = Setup::suggested(&tuners);
        let route = |label: &str| {
            setup
                .routes
                .iter()
                .find(|route| route.label == label)
                .unwrap()
        };
        assert_eq!(
            route("isdbt").tuners,
            ["model:TBS 6205se*", "pt3:6-17-0-0:2", "pt3:6-17-0-0:3"]
        );
        assert_eq!(route("isdbt").bands, Band::ALL.to_vec());
        assert_eq!(
            route("isdbs").tuners,
            ["model:TBS 6205se*", "pt3:6-17-0-0:0", "pt3:6-17-0-0:1"]
        );
        assert_eq!(route("isdbc").tuners, ["model:TBS 6205se*"]);
        assert!(
            route("isdbs3").tuners.is_empty(),
            "nothing said it receives it"
        );
        assert!(route("dvbs2").tuners.is_empty());
        let names: Vec<&str> = setup
            .spaces
            .iter()
            .map(|space| space.name.as_str())
            .collect();
        assert_eq!(names, ["地デジ", "BS", "CS", "BS4K", "CATV"]);
        let space = |name: &str| {
            setup
                .spaces
                .iter()
                .find(|space| space.name == name)
                .unwrap()
        };
        assert_eq!(space("地デジ").labels, ["isdbt", "isdbc"]);
        assert_eq!(space("BS").labels, ["isdbs", "isdbc"]);
        assert_eq!(space("BS4K").labels, ["isdbs3", "isdbc"]);
        assert_eq!(space("CATV").labels, ["isdbc"]);
        assert!(setup.problems().is_empty());
        let unplaced: Vec<&str> = setup
            .unplaced()
            .iter()
            .map(|route| route.label.as_str())
            .collect();
        assert_eq!(unplaced, ["dvbs2", "j83b_5_6"]);
        // It writes and reads back as itself.
        let written = setup.write_into("");
        assert_eq!(Setup::read(&written), setup, "{written}");
    }

    #[test]
    fn a_model_names_a_card_only_when_every_card_of_it_receives_the_system() {
        let tuners = [
            tuner("bda:1:0", "TBS 6205se Tuner 1", &["ISDB-T"]),
            tuner("bda:2:0", "TBS 6205se Tuner 1", &["DVB-C/ANNEX_A"]),
            tuner("bda:3:0", "", &["ISDB-T"]),
        ];
        assert_eq!(patterns_for(&tuners, "ISDBT"), ["bda:1:0", "bda:3:0"]);
        assert_eq!(patterns_for(&tuners, "ISDBC"), ["bda:2:0"]);
        // A tuner that said nothing is not chosen.
        let silent = [tuner("bda:9:0", "Something", &[])];
        assert!(patterns_for(&silent, "ISDBT").is_empty());
        // What a viewer ticked is written the same way, and read back.
        let ticked = ["bda:1:0".to_owned(), "bda:3:0".to_owned()];
        let patterns = patterns_of(&tuners, &ticked);
        assert_eq!(patterns, ["bda:1:0", "bda:3:0"]);
        assert_eq!(named_by(&tuners, &patterns), ticked);
        assert_eq!(
            named_by(&tuners, &["model:TBS 6205se*".to_owned()]),
            ["bda:1:0", "bda:2:0"]
        );
    }

    #[test]
    fn cards_and_models() {
        assert_eq!(card_of("tbs:1111222233334444:0"), "tbs:1111222233334444");
        assert_eq!(card_of("pt3:3:0.0/2"), "pt3:3:0.0");
        assert_eq!(card_of("usb:1-2"), "usb:1-2");
        assert_eq!(model_of("TBS 6205se Tuner 2"), "TBS 6205se");
        assert_eq!(
            model_of("Earthsoft PT3 satellite tuner 1"),
            "Earthsoft PT3 satellite"
        );
        assert_eq!(
            model_of("US-3POUT (TC90522 + MxL136)"),
            "US-3POUT (TC90522 + MxL136)"
        );
    }

    #[test]
    fn where_a_route_belongs_when_nobody_said() {
        assert!(belongs("地デジ", "ISDBT"));
        assert!(belongs("地デジ", "ISDBC"));
        assert!(!belongs("地デジ", "ISDBS"));
        assert!(belongs("BS", "ISDBS"));
        assert!(belongs("CS", "DVBC/ANNEX_A"));
        assert!(!belongs("BS", "ISDBS3"));
        assert!(belongs("BS4K", "ISDBS3"));
        assert!(belongs("CS4K", "ISDBC"));
        assert!(belongs("CATV", "ISDBC"));
        assert!(!belongs("CATV", "ISDBT"));
        assert!(belongs("SKP", "DVBS2"));
        assert!(belongs("SKP", "J83B_5_6"));
        assert!(!belongs("SKP", "ISDBC"));
        assert!(!belongs("うち", "ISDBT"));
    }

    #[test]
    fn what_is_wrong_is_said() {
        let mut setup = Setup::suggested(&[]);
        setup.routes[1].label = "isdbt".to_owned();
        setup.routes.push(Route {
            label: "bad label".to_owned(),
            system: String::new(),
            tuners: Vec::new(),
            bands: Vec::new(),
        });
        setup.spaces.push(SpaceLine {
            name: String::new(),
            labels: Vec::new(),
            group: String::new(),
        });
        let problems = setup.problems();
        assert!(
            problems.iter().any(|one| one.contains("「isdbt」が重複")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|one| one.contains("「bad label」に使えない")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|one| one.contains("伝送方式がありません")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|one| one == "名前のない空間があります"),
            "{problems:?}"
        );
    }
}

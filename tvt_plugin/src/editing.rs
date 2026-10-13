// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What the settings window edits, kept apart from the window so that it can be
//! reasoned about -- and tested -- where there is no window.
//!
//! The window shows the routes as a list and the spaces as a grid of boxes, one
//! row per space and one column per route. This holds that grid: every space a
//! viewer could switch on, with a box per route, and the routes in the order
//! the file gets them. Beside it, what each tuner is set to beyond the channel
//! -- the supply on a dish's lead and the gain -- which the route window shows
//! against each tuner it lists.

use std::path::PathBuf;

use coax::daemon::{GainControl, LnbControl, Tuner};
use coax::delivery::{self, Family};
use coax::tuning::TunerSetting;
use coax_settings::config::Route;
use coax_settings::setup::{self, Setup, SpaceLine};
use coax_settings::tuner_settings::{self, Listed, TunerSettings};

/// The settings as they are being edited.
#[derive(Debug, Clone)]
pub struct Editing {
    /// The file, and its text as it was read: what the edit is written back
    /// into, so that everything else in it stays.
    pub file: PathBuf,
    pub text: String,
    pub routes: Vec<Route>,
    /// Every space a viewer can switch on, in the order they are shown.
    pub spaces: Vec<SpaceRow>,
    /// The tuners the daemon listed, for choosing from; empty where it did
    /// not answer.
    pub tuners: Vec<Tuner>,
    /// The same listing, with the board each tuner is on and what it lets a
    /// channel set.
    pub listed: Listed,
    pub daemon_answered: bool,
    /// What `[TunerSettings]` says, kept and written back with the rest.
    pub tuner_settings: TunerSettings,
}

/// One row of the grid: a space, whether it is in the file, and a box per
/// route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRow {
    pub name: String,
    pub on: bool,
    /// The `group=` the file gave it, kept as it was.
    pub group: String,
    /// One per route, in the routes' order.
    pub ticks: Vec<bool>,
}

impl Editing {
    /// Opens the file's settings for editing. A file that has neither
    /// section opens on what is suggested for this machine, which is what a
    /// viewer setting it up for the first time wants to start from.
    pub fn open(file: PathBuf, text: String, listed: Option<Listed>) -> Self {
        let daemon_answered = listed.is_some();
        let listed = listed.unwrap_or_default();
        let tuners = listed.tuners.clone();
        let mut read = Setup::read(&text);
        let tuner_settings = read.tuner_settings.clone();
        if read.is_empty() {
            read = Setup::suggested(&tuners);
        }
        let mut spaces: Vec<SpaceRow> = read
            .spaces
            .iter()
            .map(|line| SpaceRow {
                name: line.name.clone(),
                on: true,
                group: line.group.clone(),
                ticks: read
                    .routes
                    .iter()
                    .map(|route| line.labels.contains(&route.label))
                    .collect(),
            })
            .collect();
        // The known spaces the file does not have are added switched off,
        // holding the routes that would go in them if they were on.
        for name in setup::KNOWN_SPACES {
            if spaces.iter().any(|row| row.name == name) {
                continue;
            }
            spaces.push(SpaceRow {
                name: name.to_owned(),
                on: false,
                group: String::new(),
                ticks: read
                    .routes
                    .iter()
                    .map(|route| setup::belongs(name, &route.system))
                    .collect(),
            });
        }
        // In the order the platforms are usually read in -- the aerial, then
        // the satellites, then the cable and the optical distribution -- and
        // not in the order the file happens to name them. A space this does not
        // know keeps its place after them.
        spaces.sort_by_key(|row| {
            setup::KNOWN_SPACES
                .iter()
                .position(|known| *known == row.name)
                .unwrap_or(setup::KNOWN_SPACES.len())
        });
        Self {
            file,
            text,
            routes: read.routes,
            spaces,
            tuners,
            listed,
            daemon_answered,
            tuner_settings,
        }
    }

    /// The two sections as they stand: the routes, and the spaces that are
    /// on, each with the routes ticked for it.
    pub fn setup(&self) -> Setup {
        Setup {
            routes: self.routes.clone(),
            spaces: self
                .spaces
                .iter()
                .filter(|row| row.on)
                .map(|row| SpaceLine {
                    name: row.name.clone(),
                    labels: self
                        .routes
                        .iter()
                        .zip(&row.ticks)
                        .filter(|(_, ticked)| **ticked)
                        .map(|(route, _)| route.label.clone())
                        .collect(),
                    group: row.group.clone(),
                })
                .collect(),
            tuner_settings: self.tuner_settings.clone(),
        }
    }

    /// Adds a route at the end, ticked in the spaces it belongs to.
    pub fn add_route(&mut self, route: Route) {
        for row in &mut self.spaces {
            row.ticks.push(setup::belongs(&row.name, &route.system));
        }
        self.routes.push(route);
    }

    /// Puts a route in place of the one at an index, keeping its boxes.
    pub fn replace_route(&mut self, index: usize, route: Route) {
        if let Some(there) = self.routes.get_mut(index) {
            *there = route;
        }
    }

    pub fn remove_route(&mut self, index: usize) {
        if index >= self.routes.len() {
            return;
        }
        self.routes.remove(index);
        for row in &mut self.spaces {
            row.ticks.remove(index);
        }
    }

    /// Moves a route one place up or down the list, which is one place
    /// earlier or later in the order a space tries them.
    pub fn move_route(&mut self, index: usize, up: bool) {
        let other = if up {
            index.checked_sub(1)
        } else {
            (index + 1 < self.routes.len()).then_some(index + 1)
        };
        let Some(other) = other else {
            return;
        };
        self.routes.swap(index, other);
        for row in &mut self.spaces {
            row.ticks.swap(index, other);
        }
    }

    pub fn tick(&mut self, space: usize, route: usize, on: bool) {
        if let Some(tick) = self
            .spaces
            .get_mut(space)
            .and_then(|row| row.ticks.get_mut(route))
        {
            *tick = on;
        }
    }

    pub fn switch(&mut self, space: usize, on: bool) {
        if let Some(row) = self.spaces.get_mut(space) {
            row.on = on;
        }
    }

    /// What is wrong, in words: what stops it being written first, then
    /// what a viewer would want to know before writing it.
    pub fn problems(&self) -> Vec<String> {
        self.setup().problems()
    }

    pub fn warnings(&self) -> Vec<String> {
        let setup = self.setup();
        let mut warnings: Vec<String> = setup
            .unplaced()
            .iter()
            .map(|route| format!("経路「{}」はどの空間にも入っていません", route.label))
            .collect();
        for line in &setup.spaces {
            if line.labels.is_empty() {
                warnings.push(format!("空間「{}」に経路がありません", line.name));
            }
        }
        warnings
    }

    /// The file's text with the edit in it.
    pub fn written(&self) -> String {
        self.setup().write_into(&self.text)
    }

    /// What a tuner is set to, as the file says: its own line, or a pattern
    /// that names it.
    pub fn setting_of(&self, tuner: usize) -> TunerSetting {
        self.tuners
            .get(tuner)
            .map(|tuner| self.tuner_settings.for_tuner(tuner))
            .unwrap_or_default()
    }

    /// The supply a tuner lets a channel set, where a tune of this system
    /// would use it: only a satellite's lead carries one.
    pub fn lnb_of(&self, tuner: usize, system: &str) -> Option<&LnbControl> {
        if !matches!(delivery::family(system), Family::Satellite | Family::Sky) {
            return None;
        }
        self.listed
            .controls(&self.tuners.get(tuner)?.id)?
            .lnb
            .as_ref()
    }

    /// Whether a tuner's supply is set on its own line, rather than on the
    /// line of the tuner before it on the same board: one supply that feeds
    /// several tuners is one setting, shown once.
    pub fn lnb_is_shown(&self, tuner: usize) -> bool {
        let Some(id) = self.tuners.get(tuner).map(|tuner| tuner.id.as_str()) else {
            return false;
        };
        let Some(lnb) = self
            .listed
            .controls(id)
            .and_then(|controls| controls.lnb.as_ref())
        else {
            return false;
        };
        if lnb.shared.is_empty() {
            return true;
        }
        let board = self.listed.board(id);
        !self.tuners[..tuner].iter().any(|before| {
            self.listed.board(&before.id) == board
                && self
                    .listed
                    .controls(&before.id)
                    .is_some_and(|controls| controls.lnb.is_some())
        })
    }

    /// The gain a tuner lets a channel set, where a tune of this system would
    /// use it: an amplifier in front of one input only is not in front of the
    /// others.
    pub fn gain_of(&self, tuner: usize, system: &str) -> Option<&GainControl> {
        let gain = self
            .listed
            .controls(&self.tuners.get(tuner)?.id)?
            .gain
            .as_ref()?;
        let applies = gain.deliveries.is_empty()
            || gain
                .deliveries
                .iter()
                .any(|one| delivery::same(one, system));
        applies.then_some(gain)
    }

    /// Sets a tuner's supply. A supply its board shares goes on every tuner of
    /// the board that has it, so that whichever of them is tuned asks for the
    /// same.
    pub fn set_lnb(&mut self, tuner: usize, value: &str) {
        let Some(id) = self.tuners.get(tuner).map(|tuner| tuner.id.clone()) else {
            return;
        };
        let shared = self
            .listed
            .controls(&id)
            .and_then(|controls| controls.lnb.as_ref())
            .is_some_and(|lnb| !lnb.shared.is_empty());
        let board = self.listed.board(&id).map(str::to_owned);
        let on: Vec<usize> = (0..self.tuners.len())
            .filter(|at| {
                let other = &self.tuners[*at].id;
                *other == id
                    || (shared
                        && self.listed.board(other).map(str::to_owned) == board
                        && self
                            .listed
                            .controls(other)
                            .is_some_and(|controls| controls.lnb.is_some()))
            })
            .collect();
        for at in on {
            let mut setting = self.setting_of(at);
            setting.lnb = value.to_owned();
            let tuner = self.tuners[at].clone();
            self.tuner_settings.set(&tuner, setting);
        }
    }

    pub fn set_gain(&mut self, tuner: usize, value: &str) {
        let Some(listed) = self.tuners.get(tuner).cloned() else {
            return;
        };
        let mut setting = self.setting_of(tuner);
        setting.gain = value.to_owned();
        self.tuner_settings.set(&listed, setting);
    }

    /// How a route reads in the list: its label, what it is tuned as, and
    /// what it is on -- with where it walks, where that is a question.
    pub fn route_line(route: &Route) -> String {
        let mut line = format!(
            "{}\t{}\t{}",
            route.label,
            delivery::shown(&route.system),
            if route.tuners.is_empty() {
                "(どのチューナーでも)".to_owned()
            } else {
                route.tuners.join(", ")
            }
        );
        let bands = bands_shown(route);
        if !bands.is_empty() {
            line.push_str(&format!("  [{bands}]"));
        }
        line
    }
}

/// What a tuner's supply can be set to, as a value for the file and the words
/// for the list. Nothing written is the tuner's own default, which is no supply
/// where it can be switched off and a supply where it cannot.
pub fn lnb_choices(lnb: &LnbControl) -> Vec<(String, String)> {
    let mut choices = vec![(
        String::new(),
        if lnb.off() {
            "既定（給電しない）".to_owned()
        } else {
            "既定（給電する）".to_owned()
        },
    )];
    if !lnb.low.is_empty() {
        choices.push(("low".to_owned(), format!("給電する（{}）", lnb.low)));
    }
    let high = if lnb.high.is_empty() {
        "給電する".to_owned()
    } else {
        format!("給電する（{}）", lnb.high)
    };
    choices.push(("high".to_owned(), high));
    choices
}

/// What a tuner's gain can be set to: its default, automatic, the two ends,
/// and each step the amplifier has.
pub fn gain_choices(gain: &GainControl) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = [
        ("", "既定"),
        ("auto", "自動"),
        ("min", "最小"),
        ("max", "最大"),
    ]
    .iter()
    .map(|(value, shown)| ((*value).to_owned(), (*shown).to_owned()))
    .collect();
    for db in &gain.decibels {
        let step = tuner_settings::decibels(*db);
        choices.push((step.clone(), step));
    }
    choices
}

/// The stretches of the plan a route walks, as a viewer reads them -- and
/// nothing at all for a route that walks none of them. A satellite route is
/// asked rather than walked, and one on the optical distribution walks a grid
/// of its own, so showing bands for either would name a setting that does not
/// exist (see [`setup::bands_are_written`]).
pub fn bands_shown(route: &Route) -> String {
    if !setup::bands_are_written(&route.system) {
        return String::new();
    }
    coax::plan::Band::ALL
        .iter()
        .filter(|band| route.bands.contains(band))
        .map(|band| band.name())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::{Editing, gain_choices, lnb_choices};
    use coax::daemon::{Board, Controls, GainControl, Held, LnbControl, Tuner};
    use coax::plan::Band;
    use coax_settings::config::Route;
    use coax_settings::tuner_settings::Listed;
    use std::path::PathBuf;

    /// One board of tuners, each with these controls.
    fn board(name: &str, tuners: Vec<Tuner>, controls: &Controls) -> Board {
        Board {
            name: name.to_owned(),
            tuners: tuners
                .into_iter()
                .map(|tuner| Held {
                    tuner,
                    controls: controls.clone(),
                    ..Held::default()
                })
                .collect(),
            ..Board::default()
        }
    }

    fn tuner(id: &str, model: &str, deliveries: &[&str]) -> Tuner {
        Tuner {
            id: id.to_owned(),
            model: model.to_owned(),
            deliveries: deliveries.iter().map(|one| (*one).to_owned()).collect(),
            leased: false,
            name: String::new(),
        }
    }

    fn editing(text: &str) -> Editing {
        Editing::open(
            PathBuf::from("BonDriver_coax.ini"),
            text.to_owned(),
            Some(Listed::of(
                vec![board(
                    "tbs:1",
                    vec![tuner(
                        "tbs:1:0",
                        "TBS 6205se Tuner 1",
                        &["ISDB-T", "ISDB-S"],
                    )],
                    &Controls::default(),
                )],
                &[],
            )),
        )
    }

    #[test]
    fn a_file_with_settings_opens_on_them_and_the_rest_switched_off() {
        let editing = editing(
            "[Tuners]\nisdbt = ISDBT, tbs:*\nisdbs = ISDBS, tbs:*\n\
             [Spaces]\n0 = BS, isdbs\n1 = うち, isdbt, isdbs, group=GR\n",
        );
        assert_eq!(editing.routes.len(), 2);
        let names: Vec<(&str, bool)> = editing
            .spaces
            .iter()
            .map(|row| (row.name.as_str(), row.on))
            .collect();
        // The known spaces are in the order the platforms are read in,
        // whatever order the file names them; a space of the viewer's own
        // keeps its place after them.
        assert_eq!(
            names,
            [
                ("地デジ", false),
                ("BS", true),
                ("CS", false),
                ("BS4K", false),
                ("CS4K", false),
                ("CATV", false),
                ("SKP", false),
                ("うち", true),
            ]
        );
        let row = |name: &str| {
            editing
                .spaces
                .iter()
                .find(|row| row.name == name)
                .expect("the space is there")
        };
        assert_eq!(row("BS").ticks, [false, true]);
        assert_eq!(row("うち").ticks, [true, true]);
        assert_eq!(row("うち").group, "GR");
        // The ones that are off are ready with what belongs in them.
        assert_eq!(row("地デジ").ticks, [true, false]);
        assert_eq!(row("CS").ticks, [false, true]);
        // And only what is on is written, numbered in the order shown.
        let written = editing.written();
        assert!(written.contains("0 = BS,   isdbs\n"), "{written}");
        assert!(
            written.contains("1 = うち, isdbt, isdbs, group=GR\n"),
            "{written}"
        );
        assert!(!written.contains("地デジ"), "{written}");
    }

    #[test]
    fn an_empty_file_opens_on_the_suggestion() {
        let editing = editing("[coax]\nHost = 127.0.0.1:40200\n");
        assert_eq!(editing.routes.len(), 6);
        assert!(editing.spaces[0].on && editing.spaces[0].name == "地デジ");
        assert_eq!(editing.routes[0].tuners, ["model:TBS 6205se*"]);
        assert!(editing.problems().is_empty());
        assert!(
            editing
                .written()
                .starts_with("[coax]\nHost = 127.0.0.1:40200\n\n[Tuners]\n")
        );
    }

    #[test]
    fn routes_are_added_moved_and_removed_with_their_boxes() {
        let mut editing = editing("[Tuners]\nisdbt = ISDBT, tbs:*\n[Spaces]\n0 = 地デジ, isdbt\n");
        editing.add_route(Route {
            label: "isdbc".to_owned(),
            system: "ISDBC".to_owned(),
            tuners: Vec::new(),
            bands: Band::ALL.to_vec(),
        });
        let row = |editing: &Editing, name: &str| {
            editing
                .spaces
                .iter()
                .find(|row| row.name == name)
                .unwrap()
                .clone()
        };
        assert_eq!(row(&editing, "地デジ").ticks, [true, true]);
        assert_eq!(row(&editing, "BS").ticks, [false, true]);
        assert_eq!(row(&editing, "CATV").ticks, [false, true]);
        editing.move_route(1, true);
        assert_eq!(editing.routes[0].label, "isdbc");
        assert_eq!(row(&editing, "BS").ticks, [true, false]);
        editing.switch(1, true);
        assert!(editing.written().contains("1 = BS,     isdbc\n"));
        editing.remove_route(0);
        assert_eq!(editing.routes.len(), 1);
        assert_eq!(row(&editing, "BS").ticks, [false]);
        assert_eq!(editing.warnings(), ["空間「BS」に経路がありません"]);
    }

    #[test]
    fn a_route_reads_as_a_line() {
        let route = Route {
            label: "isdbt".to_owned(),
            system: "ISDBT".to_owned(),
            tuners: vec!["model:TBS 6205se*".to_owned()],
            bands: vec![Band::Uhf],
        };
        assert_eq!(
            Editing::route_line(&route),
            "isdbt\t地上デジタル\tmodel:TBS 6205se*  [UHF]"
        );
        // A route that is asked rather than walked shows no stretches, even
        // where the settings filled some in for it.
        let asked = Route {
            label: "isdbs3".to_owned(),
            system: "ISDBS3".to_owned(),
            tuners: Vec::new(),
            bands: Band::ALL.to_vec(),
        };
        assert_eq!(super::bands_shown(&asked), "");
        assert_eq!(
            Editing::route_line(&asked),
            "isdbs3\tBS4K・CS4K\t(どのチューナーでも)"
        );
    }

    /// A PT3 whose two satellite tuners share one supply, and a card with an
    /// amplifier in front of its terrestrial input only.
    fn with_controls() -> Editing {
        let pt3 = Controls {
            lnb: Some(LnbControl {
                levels: vec!["off".to_owned(), "high".to_owned()],
                high: "15V".to_owned(),
                shared: "board".to_owned(),
                ..LnbControl::default()
            }),
            gain: None,
        };
        let amplified = Controls {
            lnb: Some(LnbControl {
                levels: vec!["low".to_owned(), "high".to_owned()],
                low: "13V".to_owned(),
                high: "18V".to_owned(),
                ..LnbControl::default()
            }),
            gain: Some(GainControl {
                decibels: vec![0.0, 13.0],
                deliveries: vec!["ISDB-T".to_owned()],
            }),
        };
        let listed = Listed::of(
            vec![
                board(
                    "pt3:a",
                    vec![
                        tuner("pt3:a:0", "Earthsoft PT3", &["ISDB-S"]),
                        tuner("pt3:a:1", "Earthsoft PT3", &["ISDB-S"]),
                    ],
                    &pt3,
                ),
                board(
                    "tbs:b",
                    vec![tuner(
                        "tbs:b:0",
                        "TBS 6205se Tuner 1",
                        &["ISDB-T", "ISDB-S"],
                    )],
                    &amplified,
                ),
            ],
            &[],
        );
        Editing::open(
            PathBuf::from("BonDriver_coax.ini"),
            "[TunerSettings]\npt3:* = lnb=high\n".to_owned(),
            Some(listed),
        )
    }

    /// The supply is offered at the levels the tuner has, with a default that
    /// says what the tuner does when nothing is said; the gain at its steps.
    #[test]
    fn a_tuner_is_offered_what_it_can_set() {
        let editing = with_controls();
        let pt3 = editing
            .lnb_of(0, "ISDBS")
            .expect("a satellite tune of a PT3");
        assert_eq!(
            lnb_choices(pt3),
            [
                (String::new(), "既定（給電しない）".to_owned()),
                ("high".to_owned(), "給電する（15V）".to_owned()),
            ]
        );
        let tbs = editing.lnb_of(2, "ISDBS").expect("and of the other card");
        assert_eq!(lnb_choices(tbs)[0].1, "既定（給電する）");
        assert_eq!(
            lnb_choices(tbs)[1],
            ("low".to_owned(), "給電する（13V）".to_owned())
        );
        // No supply on a terrestrial lead.
        assert!(editing.lnb_of(0, "ISDBT").is_none());
        // The gain is in front of the terrestrial input only.
        assert!(editing.gain_of(2, "ISDBS").is_none());
        let gain = editing.gain_of(2, "ISDB-T").expect("the amplifier");
        let values: Vec<String> = gain_choices(gain)
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(values, ["", "auto", "min", "max", "+0dB", "+13dB"]);
        assert!(
            editing.gain_of(0, "ISDBT").is_none(),
            "a PT3 has no amplifier to set"
        );
    }

    /// A supply the board shares is one setting, shown on its first tuner and
    /// written for every tuner of the board.
    #[test]
    fn a_shared_supply_is_set_once_for_the_board() {
        let mut editing = with_controls();
        assert_eq!(editing.setting_of(1).lnb, "high", "from the pattern");
        assert!(editing.lnb_is_shown(0));
        assert!(!editing.lnb_is_shown(1));
        assert!(editing.lnb_is_shown(2));
        editing.set_lnb(0, "");
        assert_eq!(editing.setting_of(0).lnb, "");
        assert_eq!(editing.setting_of(1).lnb, "");
        assert_eq!(editing.setting_of(2).lnb, "");
        editing.set_gain(2, "+13dB");
        editing.set_lnb(2, "low");
        let written = editing.written();
        assert!(
            written.contains("tbs:b:0 = lnb=low, gain=+13dB\n"),
            "{written}"
        );
        assert!(written.contains("pt3:*   = lnb=high\n"), "{written}");
    }
}

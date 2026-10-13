// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The file beside the library, which is where a BonDriver's settings live.
//!
//! Nothing here is required: a driver with no file at all uses the band plan
//! and lets the daemon choose a tuner, which is right for a machine with one
//! aerial and one card. The same file holds the channels something has looked
//! for, in sections of their own -- see [`crate::defined`]. What is read here
//! is the settings.

use std::collections::BTreeMap;

use coax::delivery::{self, Family};
use coax::plan::{Band, CABLE_BANDS, DEFAULT_BANDS};

use crate::tuner_settings::TunerSettings;

/// What the file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where the daemon is: a socket path, or `host:port` for a daemon started
    /// with `--http`. Empty means the socket the daemon opens by itself.
    pub host: String,
    /// Which tuner to ask for. Empty lets the daemon choose; a machine whose
    /// cards are on different aerials has to say.
    pub tuner: String,
    /// Where the daemon's executable is, when it is not beside this library or
    /// on the search path.
    pub daemon: String,
    /// Whether to start the daemon when nothing is answering. On unless the
    /// file says otherwise: a viewer opens their television software, not a
    /// service.
    pub start_daemon: bool,
    /// How much this driver's work is worth against everything else asking the
    /// daemon for a tuner. A higher number takes a tuner from a lower one; the
    /// same number never does, so among equals whoever asked first keeps it.
    ///
    /// One number for the whole driver, because that is the grain the question
    /// has: what wants saying is that the recorder outranks the viewer, not
    /// which of its channels does. Zero when the file says nothing, which is
    /// what every request meant before priorities existed.
    pub priority: i32,
    /// The spaces this driver offers, in the order it offers them. Empty means
    /// the one space a machine with one aerial wants.
    pub spaces: Vec<SpaceSetting>,
    /// The routes `[Tuners]` defines, by label: what each is tuned as, and
    /// which tuners it is received on.
    pub tuners: BTreeMap<String, Route>,
    /// What `[TunerSettings]` sets for each tuner beside the channel: the
    /// supply on a dish's lead, and the gain.
    pub tuner_settings: TunerSettings,
}

/// One line of `[Tuners]`: a way of receiving something, which is a delivery
/// system on a set of tuners.
///
/// The two go together because neither is enough alone. A tuner is on a lead,
/// and what a lead carries is not what the tuner can demodulate: the same card
/// on a dish and on a cable that passes a few BS transponders through receives
/// different things. So a label names both, and a space says which labels it is
/// received by. What a tuner says about itself is not relied on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Route {
    /// The label the file gave it, which is what a space and a channel line
    /// name it by.
    pub label: String,
    /// The delivery system it is tuned as, as the file wrote it. Read through
    /// [`crate::delivery`], which knows every spelling; one this version has
    /// never heard of goes to the daemon as it stands.
    pub system: String,
    /// The tuners it is received on, as patterns. Empty lets the daemon
    /// choose, which is right for a machine with one lead.
    pub tuners: Vec<String>,
    /// Which bands of the plan this lead carries. Only the terrestrial and
    /// cable plans have bands; for anything else this is ignored.
    ///
    /// A fact about the lead rather than the space: an aerial has nothing on
    /// the VHF numbers, a cable may have anything on them, and the two are one
    /// space to a viewer.
    pub bands: Vec<Band>,
}

/// One line of `[Spaces]`: what a viewer sees in the tuning space list, and
/// what it takes to fill it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceSetting {
    /// The name the host shows.
    pub name: String,
    /// The ways it is received, in the order to try them. Never empty: a space
    /// that named none is terrestrial on whatever tuner the daemon finds.
    ///
    /// Several because one platform comes down several leads. BS from a dish
    /// and the same BS transmodulated onto a cable are one space to a viewer:
    /// the same system, a different lead, and a different set of channels on
    /// it.
    pub routes: Vec<Route>,
    /// Which platform this space holds -- BS, CS, BS4K -- when the name does
    /// not say. One lock finds several platforms at once, so where a stream was
    /// found says nothing about which space it belongs in.
    pub group: String,
}

/// Whether a delivery system is a cable one, which decides what a route walks
/// when its settings say nothing.
fn is_cable(system: &str) -> bool {
    delivery::family(system) == Family::Cable
}

/// What a route walks when it was not told: the whole plan on a cable, and the
/// numbers that carry television on an aerial. A cable carries broadcasts on
/// the VHF numbers as well -- measured on one feed, seven of them were in use
/// -- while over the air those numbers have carried nothing since the analogue
/// switch-off.
pub(crate) fn default_bands(system: &str) -> Vec<Band> {
    if is_cable(system) {
        CABLE_BANDS.to_vec()
    } else {
        DEFAULT_BANDS.to_vec()
    }
}

/// The way a space that names none is received: terrestrial, on the tuner
/// named for the whole driver or on whatever the daemon finds.
fn terrestrial_route(tuner: String) -> Route {
    Route {
        label: String::new(),
        system: "ISDBT".to_owned(),
        tuners: if tuner.is_empty() {
            Vec::new()
        } else {
            vec![tuner]
        },
        bands: DEFAULT_BANDS.to_vec(),
    }
}

/// The one space a driver offers when the file says nothing: terrestrial, on
/// the tuner named for the whole driver or whatever tuner the daemon finds.
fn only_terrestrial(tuner: &str) -> SpaceSetting {
    SpaceSetting {
        name: "地デジ".to_owned(),
        routes: vec![terrestrial_route(tuner.to_owned())],
        group: String::new(),
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::new(),
            tuner: String::new(),
            daemon: String::new(),
            start_daemon: true,
            priority: 0,
            spaces: Vec::new(),
            tuners: BTreeMap::new(),
            tuner_settings: TunerSettings::default(),
        }
    }
}

impl Config {
    /// Reads the settings out of an INI file's text. Unknown sections and keys
    /// are ignored rather than refused: a viewer's file outlives the driver
    /// that read it.
    pub fn parse(text: &str) -> Self {
        let mut config = Self::default();
        // The routes each space named, by label. Resolved once the whole file
        // has been read: `[Tuners]` may come after `[Spaces]`.
        let mut named: Vec<Vec<String>> = Vec::new();
        for (section, name, value) in entries(text) {
            match (section.as_str(), name.to_ascii_lowercase().as_str()) {
                ("coax", "host") => config.host = value,
                ("coax", "tuner") => config.tuner = value,
                ("coax", "daemon") => config.daemon = value,
                ("coax", "startdaemon") => config.start_daemon = is_yes(&value),
                // A priority nobody could read is left at zero rather than
                // refused, as everything else in this file is: a mistyped
                // number is no reason to refuse to receive television.
                ("coax", "priority") => {
                    config.priority = value.trim().parse().unwrap_or_default();
                }
                ("spaces", _) => {
                    if let Some((space, labels)) = SpaceSetting::parse(&value) {
                        config.spaces.push(space);
                        named.push(labels);
                    }
                }
                ("tuners", _) => {
                    if let Some(route) = Route::parse(name.trim(), &value) {
                        config.tuners.insert(route.label.clone(), route);
                    }
                }
                ("tunersettings", _) => config.tuner_settings.add_line(&name, &value),
                _ => {}
            }
        }
        for (space, labels) in config.spaces.iter_mut().zip(named) {
            space.resolve(&labels, &config.tuners, &config.tuner);
        }
        config
    }

    /// The spaces to offer, which is what the file says or the one space a
    /// machine with one aerial wants.
    pub fn spaces(&self) -> Vec<SpaceSetting> {
        if self.spaces.is_empty() {
            return vec![only_terrestrial(&self.tuner)];
        }
        self.spaces.clone()
    }

    /// What the file says the tuners receive: for every route, the delivery
    /// system and the tuners it is received on. Handed over rather than applied
    /// here so that whoever holds a list of tuners can put it over them. See
    /// [`coax::daemon::declare`].
    pub fn declared_routes(&self) -> Vec<(String, Vec<String>)> {
        self.tuners
            .values()
            .filter(|route| !route.tuners.is_empty())
            .map(|route| (route.system.clone(), route.tuners.clone()))
            .collect()
    }

    /// The tuners a label stands for. A label nothing defines stands for
    /// itself, so that a channel line naming one tuner outright works without a
    /// `[Tuners]` section.
    pub fn tuners_for(&self, label: &str) -> Vec<String> {
        if label.is_empty() {
            return Vec::new();
        }
        match self.tuners.get(label) {
            Some(route) => route.tuners.clone(),
            None => vec![label.to_owned()],
        }
    }
}

impl Route {
    /// Reads one line of `[Tuners]`: `label = system, tuners... [, bands=...]`.
    /// A line with no system is no route: the system is the half a tuner cannot
    /// be trusted to say.
    pub(crate) fn parse(label: &str, line: &str) -> Option<Self> {
        if label.is_empty() {
            return None;
        }
        let mut fields = line
            .split(',')
            .map(str::trim)
            .filter(|field| !field.is_empty());
        let system = fields.next()?.to_owned();
        // `bands=` is itself a list, and the comma inside `bands=UHF,CATV` has
        // already split the line: once it starts, every bare field to the end
        // is a band.
        let mut tuners = Vec::new();
        let mut bands = Vec::new();
        let mut in_bands = false;
        for field in fields {
            match field.split_once('=') {
                Some(("bands", named)) => {
                    in_bands = true;
                    bands.extend(named.split_whitespace().filter_map(Band::parse));
                }
                Some(_) => in_bands = false,
                None if in_bands => bands.extend(field.split_whitespace().filter_map(Band::parse)),
                None => tuners.push(field.to_owned()),
            }
        }
        if bands.is_empty() {
            bands = default_bands(&system);
        }
        Some(Self {
            label: label.to_owned(),
            system,
            tuners,
            bands,
        })
    }
}

impl SpaceSetting {
    /// Reads one line of `[Spaces]`: `name, routes... [, options]`, giving back
    /// the space and the labels of the routes it named, to be resolved once
    /// `[Tuners]` is read.
    ///
    /// A line with no name is no space. A route is a bare word and an option is
    /// `name=value`, which is what tells the two apart on one comma-separated
    /// list.
    pub(crate) fn parse(line: &str) -> Option<(Self, Vec<String>)> {
        let mut fields = line.split(',').map(str::trim);
        let name = fields.next()?.to_owned();
        if name.is_empty() {
            return None;
        }
        // The options come after the routes: the first `name=value` ends the
        // routes, and a bare field after it is the tail of an option that is
        // itself a list.
        let mut labels = Vec::new();
        let mut group = String::new();
        let mut in_options = false;
        for field in fields {
            match field.split_once('=') {
                Some(("group", named)) => {
                    in_options = true;
                    group = named.trim().to_ascii_uppercase();
                }
                Some(_) => in_options = true,
                None if !in_options && !field.is_empty() => labels.push(field.to_owned()),
                None => {}
            }
        }
        Some((
            Self {
                name,
                routes: Vec::new(),
                group,
            },
            labels,
        ))
    }

    /// Turns the labels a space named into the routes they stand for, and fills
    /// in what the space left unsaid now that it is known what it is tuned as.
    ///
    /// A label `[Tuners]` does not define is dropped: without a system there is
    /// nothing to tune. A space left with no routes is terrestrial on the tuner
    /// named for the whole driver, which is what a file written before there
    /// were routes says.
    fn resolve(&mut self, labels: &[String], defined: &BTreeMap<String, Route>, tuner: &str) {
        self.routes = labels
            .iter()
            .filter_map(|label| defined.get(label).cloned())
            .map(|mut route| {
                // A route that names no tuner takes the one named for the
                // whole driver, where there is one.
                if route.tuners.is_empty() && !tuner.is_empty() {
                    route.tuners = vec![tuner.to_owned()];
                }
                route
            })
            .collect();
        if self.routes.is_empty() {
            self.routes = vec![terrestrial_route(tuner.to_owned())];
        }
    }

    /// Whether this space offers a route's plan, rather than only what was
    /// discovered on it.
    ///
    /// A plan is a platform's: the terrestrial plan is 地デジ, the cable plan the
    /// operator's own carriage. A space that holds a different platform and
    /// names the route all the same -- BS with the cable it is transmodulated
    /// onto -- is saying where what the route finds should go, not asking for a
    /// hundred cable carriers under the name BS. A space that names no platform
    /// offers whatever its routes have.
    pub fn offers(&self, route: &Route) -> bool {
        match (self.platform(), coax::group::of_plan(&route.system)) {
            (Some(held), Some(planned)) => held == planned,
            _ => true,
        }
    }

    /// Which platform this space holds: what it says, or what its name says,
    /// or nothing when it is not a platform at all.
    pub fn platform(&self) -> Option<&str> {
        if !self.group.is_empty() {
            return coax::group::named(&self.group);
        }
        coax::group::named(&self.name)
    }

    /// Whether any of its routes is tuned as this delivery system.
    pub fn receives(&self, system: &str) -> bool {
        self.routes
            .iter()
            .any(|route| route.system.eq_ignore_ascii_case(system))
    }

    /// Whether one of its routes is the one labelled so.
    pub fn has_route(&self, label: &str) -> bool {
        self.routes.iter().any(|route| route.label == label)
    }

    /// Every tuner any route of it names, as patterns, each once.
    pub fn tuners(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for pattern in self.routes.iter().flat_map(|route| route.tuners.iter()) {
            if !all.contains(pattern) {
                all.push(pattern.clone());
            }
        }
        all
    }
}

/// Reads the spellings a viewer might write a switch as. Anything else is read
/// as off: a setting nobody understood should not turn something on.
fn is_yes(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Walks an INI file as (section, name, value), with the section in lower case
/// and comments and blank lines left behind.
pub(crate) fn entries(text: &str) -> Vec<(String, String, String)> {
    let mut section = String::new();
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            found.push((section.clone(), name.trim().to_owned(), trim_comment(value)));
        }
    }
    found
}

/// Takes the value up to a comment. A path can hold a `;`, so only a comment
/// that follows whitespace counts -- which is how the files a viewer already
/// has are written.
fn trim_comment(value: &str) -> String {
    let value = value.trim();
    for (at, letter) in value.char_indices() {
        let starts_a_comment = (letter == ';' || letter == '#')
            && at > 0
            && value[..at].ends_with(char::is_whitespace);
        if starts_a_comment {
            return value[..at].trim().to_owned();
        }
    }
    value.to_owned()
}

/// The settings as a map, for the parts of the file this version does not know
/// about yet.
pub fn sections(text: &str) -> BTreeMap<String, Vec<(String, String)>> {
    let mut sections: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (section, name, value) in entries(text) {
        sections.entry(section).or_default().push((name, value));
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::{Config, sections};
    use coax::plan::{Band, DEFAULT_BANDS};

    const SAMPLE: &str = r#"
; BonDriver_coax.ini
[coax]
Host = 127.0.0.1:40200   ; the daemon
Tuner = tbs:0123456789abcdef:0

[Tuners]
cable = ISDBC, tbs:*

[Spaces]
0 = 地デジ, cable
"#;

    /// The file the redesign was made for: one card that does the three
    /// satellite systems, one that does DVB-S2 alone, a PT3 that does ISDB-S
    /// alone, and a cable that passes a few BS transponders through.
    const ROUTES: &str = "
[Tuners]
isdbt          = ISDBT,        pt3:*:2, pt3:*:3, bands=UHF
isdbt_passthru = ISDBT,        tbs:6205:*, bands=VHF,UHF,EXUHF,CATV
isdbs          = ISDBS,        pt3:*:0, pt3:*:1, tbs:6182:*
isdbs_passthru = ISDBS,        tbs:6205:*
isdbs3         = ISDBS3,       tbs:6182:*
dvbs2          = DVBS2,        tbs:6182:*, tbs:6903:*
cable          = ISDBC,        tbs:6205:*
hikari         = J83B_5_6,     tbs:6205:*

[Spaces]
0 = 地デジ, isdbt, isdbt_passthru, cable
1 = BS,     isdbs, isdbs_passthru, cable
2 = BS4K,   isdbs3, cable
3 = SKP,    dvbs2, hikari
";

    #[test]
    fn a_route_is_a_system_on_a_set_of_tuners() {
        let config = Config::parse(ROUTES);
        let isdbs = &config.tuners["isdbs"];
        assert_eq!(isdbs.system, "ISDBS");
        assert_eq!(
            isdbs.tuners,
            vec![
                "pt3:*:0".to_owned(),
                "pt3:*:1".to_owned(),
                "tbs:6182:*".to_owned()
            ]
        );
        // The same system down another lead is another route, with its own
        // tuners.
        let passthru = &config.tuners["isdbs_passthru"];
        assert_eq!(passthru.system, "ISDBS");
        assert_eq!(passthru.tuners, vec!["tbs:6205:*".to_owned()]);
    }

    /// A space is received by its routes, in the order it named them: the
    /// first is what a channel is tuned by, and the rest are the other ways.
    #[test]
    fn a_space_is_the_routes_it_named_in_order() {
        let config = Config::parse(ROUTES);
        let spaces = config.spaces();
        let labels = |at: usize| -> Vec<&str> {
            spaces[at]
                .routes
                .iter()
                .map(|route| route.label.as_str())
                .collect()
        };
        assert_eq!(labels(0), ["isdbt", "isdbt_passthru", "cable"]);
        assert_eq!(labels(1), ["isdbs", "isdbs_passthru", "cable"]);
        assert_eq!(labels(3), ["dvbs2", "hikari"]);
        assert_eq!(spaces[3].routes[1].system, "J83B_5_6");
        assert!(spaces[1].receives("ISDBS"));
        assert!(!spaces[1].receives("ISDBS3"));
        assert!(spaces[1].has_route("isdbs_passthru"));
    }

    /// What the routes say the tuners receive is every route's system on its
    /// tuners -- which is what stands in for what a tuner says of itself.
    #[test]
    fn the_routes_say_what_the_tuners_receive() {
        let config = Config::parse(ROUTES);
        let mut said = config.declared_routes();
        said.sort();
        assert!(said.contains(&("ISDBS3".to_owned(), vec!["tbs:6182:*".to_owned()])));
        assert!(said.contains(&(
            "DVBS2".to_owned(),
            vec!["tbs:6182:*".to_owned(), "tbs:6903:*".to_owned()]
        )));
        assert_eq!(said.len(), 8);
    }

    /// A route with no system is no route, and a space naming a label nothing
    /// defines has not named a route. A space left with none is terrestrial on
    /// whatever is found.
    #[test]
    fn a_route_without_a_system_and_a_label_nothing_defines_are_nothing() {
        let config = Config::parse("[Tuners]\nbare =\n[Spaces]\n0 = 地デジ, bare, sat\n");
        assert!(config.tuners.is_empty());
        let spaces = config.spaces();
        assert_eq!(spaces[0].routes.len(), 1);
        assert_eq!(spaces[0].routes[0].system, "ISDBT");
        assert!(spaces[0].routes[0].tuners.is_empty());
    }

    /// A route that names no tuner, and a space that names no route, take the
    /// tuner named for the whole driver: that is what a file written before
    /// there were routes says.
    #[test]
    fn the_drivers_own_tuner_stands_in_where_a_route_names_none() {
        let config = Config::parse(
            "[coax]\nTuner = tbs:abc:0\n[Tuners]\nany = ISDBC\n[Spaces]\n0 = CATV, any\n1 = 地デジ\n",
        );
        let spaces = config.spaces();
        assert_eq!(spaces[0].routes[0].tuners, vec!["tbs:abc:0".to_owned()]);
        assert_eq!(spaces[1].routes[0].tuners, vec!["tbs:abc:0".to_owned()]);
        assert_eq!(spaces[1].routes[0].system, "ISDBT");
    }

    #[test]
    fn the_settings_are_read_and_the_comments_are_not() {
        let config = Config::parse(SAMPLE);
        assert_eq!(config.host, "127.0.0.1:40200");
        assert_eq!(config.tuner, "tbs:0123456789abcdef:0");
    }

    #[test]
    fn a_file_that_is_not_there_is_the_same_as_one_that_says_nothing() {
        assert_eq!(Config::parse(""), Config::default());
    }

    /// Starting the daemon is on unless the file turns it off, because a
    /// viewer opens their television software rather than a service.
    #[test]
    fn the_daemon_is_started_unless_the_file_says_not_to() {
        assert!(Config::parse("").start_daemon);
        assert!(!Config::parse("[coax]\nStartDaemon = 0").start_daemon);
        assert!(Config::parse("[coax]\nStartDaemon = yes").start_daemon);
    }

    /// The priority is the whole of what a viewer says about this driver's
    /// standing, so a file that says nothing has to mean what every file meant
    /// before it could be said.
    #[test]
    fn the_priority_is_zero_unless_the_file_says_otherwise() {
        assert_eq!(Config::parse("").priority, 0);
        assert_eq!(Config::parse("[coax]\nPriority = 5").priority, 5);
        assert_eq!(Config::parse("[coax]\npriority = -2").priority, -2);
        assert_eq!(Config::parse("[coax]\nPriority =  7 ").priority, 7);
        // Unreadable is zero, as everything else in this file that cannot be
        // read is.
        assert_eq!(Config::parse("[coax]\nPriority = high").priority, 0);
    }

    /// A section this version has no reading for is kept rather than refused:
    /// a viewer's file outlives the driver that reads it.
    fn spaces_are_kept_for_later() {
        let sections = sections(SAMPLE);
        assert_eq!(
            sections["spaces"],
            vec![("0".to_owned(), "地デジ, cable".to_owned())]
        );
    }

    /// A route is read down to its bands, and a band list the comma-separated
    /// line has already broken up is put back together. The bands are the
    /// lead's: an aerial and a cable carrying the same 地デジ walk different
    /// stretches of one plan.
    #[test]
    fn a_route_is_read_with_its_bands() {
        let config = Config::parse(
            "[Tuners]\n\
             ter = ISDBT, pt3:*, bands=UHF\n\
             passthru = ISDBT, tbs:*, bands=UHF,CATV,VHF\n\
             cable = ISDBC, tbs:*\n\
             [Spaces]\n\
             0 = 地デジ, ter, passthru\n\
             1 = BS, cable\n",
        );
        let spaces = config.spaces();
        assert_eq!(spaces.len(), 2);
        assert_eq!(spaces[0].routes.len(), 2);
        assert_eq!(spaces[0].routes[0].tuners, vec!["pt3:*".to_owned()]);
        assert_eq!(spaces[0].routes[0].bands, vec![Band::Uhf]);
        // What the file spells out is what it gets, and this one does not name
        // the channels the plan gave back. The bands did not become tuners
        // either.
        assert_eq!(spaces[0].routes[1].tuners, vec!["tbs:*".to_owned()]);
        assert_eq!(
            spaces[0].routes[1].bands,
            vec![Band::Uhf, Band::Catv, Band::Vhf]
        );
        assert_eq!(spaces[1].routes[0].system, "ISDBC");
        // Nothing said, and a cable, so the whole plan.
        assert_eq!(
            spaces[1].routes[0].bands,
            vec![Band::Uhf, Band::Catv, Band::ExUhf, Band::Vhf]
        );
    }

    /// A cable route walks the whole plan, VHF numbers included: a cable puts
    /// things on them. An aerial walks the numbers that carry television.
    #[test]
    fn a_cable_route_walks_the_whole_plan_unless_it_is_told_not_to() {
        let config = Config::parse(
            "[Tuners]\ncable = ISDBC, tbs:*\nter = ISDBT, pt3:*\nsome = ISDBC, tbs:*, bands=CATV\n",
        );
        assert_eq!(
            config.tuners["cable"].bands,
            vec![Band::Uhf, Band::Catv, Band::ExUhf, Band::Vhf]
        );
        assert_eq!(config.tuners["ter"].bands, DEFAULT_BANDS.to_vec());
        // What the file says still wins.
        assert_eq!(config.tuners["some"].bands, vec![Band::Catv]);
    }

    /// A space offers the plans of the routes whose platform it holds, and only
    /// says where to put what the others find.
    #[test]
    fn a_space_offers_the_plans_of_its_own_platform() {
        let config = Config::parse(ROUTES);
        let spaces = config.spaces();
        let terrestrial = &spaces[0];
        assert!(terrestrial.offers(&config.tuners["isdbt"]));
        assert!(terrestrial.offers(&config.tuners["isdbt_passthru"]));
        assert!(!terrestrial.offers(&config.tuners["cable"]));
        let bs = &spaces[1];
        assert!(bs.offers(&config.tuners["isdbs"]));
        assert!(!bs.offers(&config.tuners["cable"]));
        let sky = &spaces[3];
        assert!(sky.offers(&config.tuners["dvbs2"]));
        assert!(sky.offers(&config.tuners["hikari"]));
        // A space that is not a platform offers whatever it was given.
        let anything = Config::parse("[Tuners]\ncable = ISDBC, tbs:*\n[Spaces]\n0 = うち, cable\n");
        assert!(anything.spaces()[0].offers(&anything.tuners["cable"]));
    }

    /// A file with no `[Spaces]` is a machine with one aerial, which gets the
    /// one space it wants rather than nothing at all.
    #[test]
    fn a_file_with_no_spaces_still_offers_terrestrial() {
        let spaces = Config::parse("").spaces();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].routes[0].system, "ISDBT");
        assert!(spaces[0].tuners().is_empty());
    }

    #[test]
    fn a_label_stands_for_the_tuners_it_was_given() {
        let config =
            Config::parse("[Tuners]\ncable = ISDBC, tbs:*\nsat = ISDBS, us3pout:*, pt3:*\n");
        assert_eq!(config.tuners_for("cable"), vec!["tbs:*".to_owned()]);
        assert_eq!(
            config.tuners_for("sat"),
            vec!["us3pout:*".to_owned(), "pt3:*".to_owned()]
        );
        // A label nothing defines stands for itself, so a channel line may
        // name one tuner outright.
        assert_eq!(config.tuners_for("tbs:abc:0"), vec!["tbs:abc:0".to_owned()]);
        assert!(config.tuners_for("").is_empty());
    }

    /// `[TunerSettings]` names its tuners the way `[Tuners]` does, and is read
    /// beside the routes without being taken for one -- or for a channel list.
    #[test]
    fn the_tuner_settings_are_read_by_the_tuners_they_name() {
        let config = Config::parse(
            "[Tuners]\nsat = ISDBS, pt3:*\n\
             [TunerSettings]\n\
             pt3:a:0           = lnb=high, gain=auto ; the dish on the roof\n\
             model:TBS 6205se* = gain=+13dB\n\
             broken            = lnb=maybe\n\
             [Spaces]\n0 = BS, sat\n",
        );
        assert_eq!(config.tuners.len(), 1);
        assert_eq!(config.spaces().len(), 1);
        // The line that could not be read says nothing, which is the tuner's
        // default.
        let lines = config.tuner_settings.lines();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[2].1, coax::tuning::TunerSetting::default());
        let pt3 = coax::daemon::Tuner {
            id: "pt3:a:0".to_owned(),
            ..coax::daemon::Tuner::default()
        };
        let setting = config.tuner_settings.for_tuner(&pt3);
        assert_eq!(setting.lnb, "high");
        assert_eq!(setting.gain, "auto");
        let tbs = coax::daemon::Tuner {
            id: "bda:1:0".to_owned(),
            model: "TBS 6205se Tuner 1".to_owned(),
            ..coax::daemon::Tuner::default()
        };
        assert_eq!(config.tuner_settings.for_tuner(&tbs).gain, "+13dB");
        // Nothing said is nothing added.
        assert!(Config::parse("").tuner_settings.is_empty());
    }

    #[test]
    fn a_semicolon_inside_a_value_is_not_a_comment() {
        let config = Config::parse("[coax]\nHost = C:\\path;with\\semicolon.sock");
        assert_eq!(config.host, "C:\\path;with\\semicolon.sock");
    }

    #[test]
    fn unknown_sections_do_not_stop_the_known_ones_being_read() {
        spaces_are_kept_for_later();
    }
}

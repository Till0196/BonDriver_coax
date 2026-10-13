// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The looking the host's own channel scan cannot do.
//!
//! The host scans by asking the driver for every channel in turn and keeping
//! the ones that lock. That is enough where one carrier is one stream, so
//! terrestrial is left to it.
//!
//! A cable is not that. A carrier may hold several transport streams in a
//! multiplexing frame, and which streams those are is written in the frame's
//! own header: a host that tunes the carrier sees the streams laid over one
//! another and shows nothing. So this walks the cable plan, watches every
//! carrier that answers long enough to read a frame header, and writes down
//! what it found.
//!
//! It talks to the daemon rather than through the host: a channel that is not
//! in the host's list yet cannot be tuned through the host. So the tuner is
//! asked for twice over, and where none is free the host is asked to let go of
//! its own for the length of the sweep -- see `look` in the plugin's own
//! module.

use std::time::{Duration, Instant};

use crate::descriptors::{self, Dish};
use coax::daemon::{self, Daemon, Lease, Tuned};
use coax::delivery::{self, Family};
use coax::group;
use coax::plan;
use coax_settings::config::{Config, Route, SpaceSetting};
use coax_settings::defined::{Defined, Definition};
use coax_settings::lineup;
use coax_settings::tuner_settings::{Listed, TunerSettings};
use isdb::nit;
use isdb::tsmf::Survey;

/// How long to hold a carrier that answered, waiting for its tables.
///
/// A carrier is left as soon as it has said everything -- see `settled` -- so
/// this is the bound for the ones that have not. A network table comes round
/// every ten seconds, a thin transponder spends most of those ten transmitting
/// it, and a capture that began part way through has to wait for the next one.
/// A cable takes the satellite's figure because it carries satellite
/// multiplexes remodulated onto it, cadence and all.
const READ_TABLES: Duration = Duration::from_secs(22);

/// The same for a terrestrial carrier, whose tables are its own and quick.
const READ_TERRESTRIAL: Duration = Duration::from_secs(5);

/// The same for one satellite multiplex asked for by its identifier, which is
/// after what the multiplex says of itself: its service description table comes
/// round every two seconds or so.
const READ_STREAM: Duration = Duration::from_secs(8);

/// How much of a carrier to read while watching it.
const READ_SIZE: usize = 32 * 1024;

/// How long to wait before asking again for a tuner that was busy.
const BUSY_AGAIN: Duration = Duration::from_millis(500);

/// How many carriers may be refused for a tuner someone else has before the
/// sweep gives up. A walk with no tuner will find nothing and is minutes long.
/// One refusal is not enough to say so -- a card handed back a moment ago may
/// still be going down -- but three with nothing tuned in between is.
const GIVE_UP: usize = 3;

/// How long to let a carrier settle before asking what the tuner sees.
const MEASURE_AFTER: Duration = Duration::from_millis(300);

/// How long to wait for the demodulator to lock, and how often to ask. The same
/// figures `coax scan` uses, so that a channel found by one is found by the
/// other.
const LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_ASK: Duration = Duration::from_millis(250);

/// The most of a carrier to keep, which bounds the read as surely as the time
/// does: twenty-two seconds of the fastest carrier here, with room to spare.
const MOST_BYTES: usize = 128 * 1024 * 1024;

/// How long to spend reading out the rest of a stream that was left early, and
/// how many reads to spend on it, so that the connection -- and the tuner on it
/// -- can be kept. One that cannot be waited for is let go of instead.
const DRAIN: Duration = Duration::from_millis(200);
const DRAIN_READS: usize = 8;

/// How long a single read of a carrier waits before the walk looks at the clock
/// again: short enough that a carrier is held for what it was given, long
/// enough that a carrier which is merely thin is not cut off.
const PATIENCE: Duration = Duration::from_millis(500);

/// How long to give a carrier before the demodulator's word that it has not
/// locked is taken as final.
const NOT_LOCKED: Duration = Duration::from_millis(1200);

/// How often to say what a carrier is giving while it is being watched. A
/// carrier is held for seconds, and a line that does not move for that long
/// reads as a hang.
const TICK: Duration = Duration::from_millis(200);

/// The shortest a carrier is watched, whatever it gave up.
///
/// A frame header goes past in milliseconds, and leaving as soon as one has
/// been read leaves nothing to measure: the rate over five milliseconds is the
/// speed of the socket, not of the carrier -- measured that way, a 29 Mbit
/// cable read as 262 -- and the tuner has not yet been asked what it sees.
const AT_LEAST: Duration = Duration::from_millis(600);

/// How long the naming has to have been quiet before a carrier read for its
/// network table is let go of.
///
/// The table says which streams the network has, but not what any of them is
/// called: the names are in the service tables, one per multiplex, which come
/// round over several seconds. Leaving the moment the table is whole named five
/// services out of forty-three on the 110 degree east CS. Nothing says they
/// have all been heard, so what is waited for is a few seconds with no new
/// name.
const NAMES_QUIET: Duration = Duration::from_secs(4);
/// How long a slot that has named no services of its own is given before the
/// network table's list of them is taken as everything it will say. A stream
/// sends its own service table every couple of seconds.
const OWN_TABLE: Duration = Duration::from_secs(3);

/// What one carrier turned out to be, told as it happens so that a window can
/// show it the way the host's own channel scan does.
#[derive(Debug, Default)]
pub struct Step<'a> {
    /// The space and the carrier being looked at.
    pub what: &'a str,
    pub done: usize,
    pub total: usize,
    /// What the tuner saw, where the carrier answered: a ratio where the backend
    /// measures one, and a level where that is all it has.
    pub cnr: Option<f32>,
    pub strength: Option<f32>,
    pub mbps: Option<f32>,
    /// What came of it, in a few words.
    pub note: Option<String>,
    /// What the carrier turned out to hold, a line each, to go under the one
    /// the carrier itself is reported on. Separate from the note because the
    /// answer is a list of unknown length. This is `coax scan`'s own shape,
    /// indent and all.
    pub notes: Vec<String>,
}

/// A sweep in progress, as the viewer sees it: something to stop it with.
#[derive(Debug, Default)]
pub struct Stopping {
    asked: std::sync::atomic::AtomicBool,
    /// Whether what the walk found so far is worth keeping. A viewer who has
    /// seen what they were waiting for stops the walk and keeps it; one who
    /// started it by mistake stops it and does not.
    keep: std::sync::atomic::AtomicBool,
}

impl Stopping {
    pub const fn new() -> Self {
        Self {
            asked: std::sync::atomic::AtomicBool::new(false),
            keep: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Stops the walk and throws away what it found.
    pub fn stop(&self) {
        self.keep.store(false, std::sync::atomic::Ordering::SeqCst);
        self.asked.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Stops the walk and keeps what it found.
    pub fn finish(&self) {
        self.keep.store(true, std::sync::atomic::Ordering::SeqCst);
        self.asked.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn asked_to(&self) -> bool {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether what was found is to be written down. A walk that ran to the end
    /// is kept, so this is only about one that was stopped.
    pub fn keeping(&self) -> bool {
        self.keep.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn again(&self) {
        self.keep.store(true, std::sync::atomic::Ordering::SeqCst);
        self.asked.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// How well a carrier came in, for choosing between two carriers of the same
/// stream.
///
/// A stream is often transmitted twice over the same lead -- a main station and
/// a relay of it, or a cable carrying a multiplex it also passes through -- and
/// both answer. The one to keep is the one that comes in best.
///
/// What every tuner can say comes first: a ratio is the demodulator's to report
/// and plenty of hardware reports none, so what is compared before it is the
/// share of the carrier that did not arrive, counted from the packets
/// themselves.
#[derive(Debug, Clone, Copy, Default)]
struct Quality {
    /// The share of the packets that were missed, where enough went past to
    /// say. See `mpegts::continuity`.
    losses: Option<f32>,
    cnr: Option<f32>,
    strength: Option<f32>,
}

impl Quality {
    /// Whether this carrier is the better of the two, on the best evidence both
    /// of them have. Equal evidence is not better: what is already in the file
    /// stands, so that a sweep does not move a channel between two carriers as
    /// good as each other.
    fn better_than(&self, other: &Self) -> bool {
        /// Two carriers whose losses are this close are as good as each other:
        /// about a packet or two of a twenty-thousand-packet second.
        const ALIKE: f32 = 0.0001;
        if let (Some(mine), Some(theirs)) = (self.losses, other.losses) {
            if (mine - theirs).abs() > ALIKE {
                return mine < theirs;
            }
        }
        if let (Some(mine), Some(theirs)) = (self.cnr, other.cnr) {
            return mine > theirs;
        }
        if let (Some(mine), Some(theirs)) = (self.strength, other.strength) {
            return mine > theirs;
        }
        false
    }

    /// What it is worth telling a viewer the choice was made on.
    fn said(&self) -> String {
        let mut said = Vec::new();
        if let Some(losses) = self.losses {
            said.push(format!("欠落 {:.3}%", losses * 100.0));
        }
        if let Some(cnr) = self.cnr {
            said.push(format!("CNR {cnr:.1} dB"));
        } else if let Some(strength) = self.strength {
            said.push(format!("{strength:.1} dBm"));
        }
        said.join("　")
    }
}

/// One stream, as the walk found it: where it belongs, how it was received, and
/// what to call it. Turned into a line of the settings file once the walk is
/// over and the carriers of a split frame are all known.
#[derive(Debug, Clone)]
struct Found {
    onid: u16,
    tsid: u16,
    name: String,
    /// The label of the lead it was received on, where that is not the lead of
    /// the space it is filed under.
    tuners: String,
    tune: Vec<(String, String)>,
    /// Which group of a split multiplexing frame it is on.
    group: Option<u8>,
    carriers: Vec<String>,
    /// How the carrier it was received on came in.
    quality: Quality,
}

/// What a sweep found, as it is reported back to the viewer.
#[derive(Debug, Default)]
pub struct Report {
    pub asked: usize,
    pub answered: usize,
    pub multiplexed: usize,
    pub added: usize,
    /// Carriers that were not listened to because someone else had the tuner.
    pub busy: usize,
    /// Set where the sweep stopped early: nothing could be tuned at all, so
    /// there was nothing to learn by walking the rest of the plan.
    pub gave_up: bool,
}

/// Why a carrier gave nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Missed {
    /// Someone else has the tuner, so the carrier was never listened to.
    Busy,
    /// The tuner was had, and the carrier said nothing.
    Silent,
    /// The demodulator did not lock, so there is nothing on this carrier. The
    /// daemon answers this rather than handing over a stream that would stay
    /// silent.
    NoLock(Option<f32>),
    /// The daemon refused to hand the stream over because what came off the
    /// tuner lined up as neither a transport stream nor TLV. A demodulator
    /// locks to noise and hands it over indefinitely, so this is the answer for
    /// a lead nothing is feeding.
    Noise,
}

/// Which routes to look at. They take very different amounts of time -- a
/// satellite is two or three tunes, a cable is every carrier of the plan -- so
/// a viewer who only wants one should not have to sit through the others. A
/// sweep asked for from the host's own menu looks at everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    Everything,
    Only(Vec<String>),
}

impl What {
    /// Whether a route is one of the chosen.
    fn wants(&self, route: &Route) -> bool {
        match self {
            Self::Everything => true,
            Self::Only(labels) => labels.contains(&route.label),
        }
    }
}

/// Which tuner a viewer chose for each route, by the route's label. A route
/// nobody chose one for is left to what the settings say.
pub type Chosen = std::collections::BTreeMap<String, String>;

/// Whether the sweep has a tuner to work with.
///
/// A machine with one tuner is the ordinary machine, and while the viewer is
/// watching, that tuner is the host's. A sweep started on it would be refused
/// at every carrier, which is why this decides whether the host is asked to let
/// go first.
///
/// Anything unknown answers yes: being refused a carrier is recoverable and is
/// reported; taking a viewer's picture away when there was no need is not.
pub fn a_tuner_is_free(daemon: &Daemon, config: &Config, what: &What, chosen: &Chosen) -> bool {
    let Ok(mut tuners) = daemon.tuners() else {
        return true;
    };
    coax::daemon::declare(&config.declared_routes(), &mut tuners);
    if tuners.is_empty() {
        return true;
    }
    let mut patterns: Vec<String> = Vec::new();
    // A route that names no tuner will take whichever the daemon gives it.
    let mut anything = false;
    for setting in config.spaces() {
        for route in setting.routes.iter().filter(|route| wanted(route, what)) {
            // A viewer who named a tuner for this route gets that one asked
            // about and nothing else.
            if let Some(named) = chosen.get(&route.label) {
                match tuners.iter().find(|tuner| tuner.id == *named) {
                    Some(tuner) if !tuner.leased => return true,
                    Some(_) => {}
                    // A tuner the daemon does not list is not one to take the
                    // viewer's picture away for.
                    None => return true,
                }
            } else if route.tuners.is_empty() {
                anything = true;
            } else {
                patterns.extend(route.tuners.iter().cloned());
            }
        }
    }
    if !anything && patterns.is_empty() {
        return true;
    }
    tuners.iter().any(|tuner| {
        !tuner.leased && (anything || patterns.iter().any(|pattern| tuner.matches(pattern)))
    })
}

/// Walks the spaces that need looking at and writes down what is on them.
///
/// `chosen` names the tuner to use for a route, whatever the settings name.
///
/// `progress` is called with each carrier as it is reached, so that something
/// can be shown to a viewer who is waiting minutes rather than seconds.
// Seven, because a sweep is told seven separate things and none belongs to
// another: where the daemon is, what the settings say, what is known already,
// which routes, which tuner each is to use, how to stop, and where to report.
#[allow(clippy::too_many_arguments)]
pub fn sweep(
    daemon: &Daemon,
    config: &Config,
    known: &Defined,
    what: &What,
    chosen: &Chosen,
    stopping: &Stopping,
    mut progress: impl FnMut(Step),
) -> (Defined, Report) {
    let mut found = known.clone();
    let mut report = Report::default();
    // What each route's carriers turned out to be carrying for which space.
    let mut across: std::collections::BTreeMap<String, Vec<Found>> =
        std::collections::BTreeMap::new();
    // Which carriers each group of a split multiplexing frame turned out to
    // have. A stream on such a frame is on all of them at once, and which they
    // are is only known once the walk has been past every one.
    let mut groups: std::collections::BTreeMap<u8, Vec<String>> = std::collections::BTreeMap::new();
    // Which ways of receiving this sweep looked at, for what a merge may say
    // has gone.
    let mut swept: Vec<(String, String)> = Vec::new();
    // The settings name tuners by pattern; the daemon takes identifiers. The
    // list is read once: a sweep is minutes long.
    // What the routes say a tuner receives goes on top of what it said; what
    // it said is kept beside it, and so is what it lets a channel set.
    let listed = daemon
        .listing()
        .map(|boards| Listed::of(boards, &config.declared_routes()))
        .unwrap_or_default();
    let tuners = &listed.tuners;
    // What the sweep has to ask. The satellite walk corrects it as it goes; see
    // look_at_satellite.
    let total = std::cell::Cell::new(how_much(config, what));
    // How far along the bar is, which is not the same as how many carriers were
    // asked: the satellite walk stops as soon as every network has spoken, and
    // the transponders it did not have to tune are work that no longer has to
    // be done.
    let behind = std::cell::Cell::new(0usize);
    let mut progress = |mut step: Step| {
        step.done += behind.get();
        step.total = total.get();
        progress(step);
    };

    // The satellite is walked once per route, however many spaces are cut out
    // of it. One lock finds several platforms at once, so a space is not a
    // reason to walk again. A route is: a card that has to be told which system
    // to demodulate cannot answer for the other while it is set to one, and a
    // cable that passes a few transponders through is a different lead.
    {
        for route in satellite_routes(config, what) {
            if report.gave_up || stopping.asked_to() {
                break;
            }
            let only = chosen.get(&route.label).map(String::as_str);
            let holders: Vec<SpaceSetting> = config
                .spaces()
                .into_iter()
                .filter(|setting| setting.has_route(&route.label))
                .collect();
            if holders.is_empty() {
                continue;
            }
            // A walk nothing could answer is forty-eight transponders of
            // waiting for a lock that cannot come. A card that says it does not
            // demodulate this is taken at its word; one that says nothing is
            // asked.
            if only.is_none() && !receivable(tuners, &route.tuners, &route.system) {
                total.set(total.get().saturating_sub(satellite_count(&route.system)));
                progress(Step {
                    what: &format!(
                        "{} ({}) を受けられるチューナーがありません",
                        route.label, route.system
                    ),
                    done: 0,
                    ..Step::default()
                });
                continue;
            }
            let (transports, tuned) = look_at_satellite(
                daemon,
                &Tuning {
                    route: &route,
                    listed: &listed,
                    settings: &config.tuner_settings,
                    only,
                },
                &mut report,
                stopping,
                &total,
                &mut progress,
            );
            behind.set(behind.get() + tuned);
            progress(Step {
                what: "衛星のスキャンが終了しました",
                done: 0,
                ..Step::default()
            });
            for space in &holders {
                let streams = for_space(space, &transports, &route, &mut report);
                // Every entry is received the way this route is: that is the
                // one way the satellite walk looked at, and it was looked at
                // whole.
                let swept = [(route.label.clone(), delivery::named(&route.system))];
                let lines = streams.into_iter().map(|one| line(&one, &groups)).collect();
                let merged = merge(found.of(&space.name), lines, &swept);
                found.set(&space.name, in_route_order(space, merged));
            }
        }
    }

    // The rest of the plan is walked once per route as well, however many
    // spaces name the route. What a route's carriers hold goes to the space of
    // the platform each stream declares; what no platform claims stays in the
    // route's own space.
    for route in walked_routes(config, what) {
        if report.gave_up {
            break;
        }
        let lead = route.label.clone();
        let home = home_of(config, &route);
        let only = chosen.get(&route.label).map(String::as_str);
        // The stretches of the plan the route walks.
        let plan = lineup::plan_of(&route);
        if plan.is_empty() {
            continue;
        }
        let walking: Vec<coax_settings::channels::Channel> = plan.clone();
        // One tuner for the whole route, held on one connection: the daemon
        // gives a tuner to the connection that asked for it, so a walk that
        // opened one per carrier may find it taken when it asks again.
        let mut lease =
            daemon.hold(tuner_for(only, tuners, &route.tuners, route.system.as_str()).as_deref());
        let mut said_tuner = false;
        // The operator's own table is not asked before the walk. It was, for a
        // while: on the feeds this has been run against, a cable's table names
        // the carrier it came off and nothing else, so the six tunes bought
        // nothing and cost two minutes.
        for (at, channel) in walking.iter().enumerate() {
            if stopping.asked_to() {
                break;
            }
            report.asked += 1;
            let system = system_of(channel);
            // Named by what it is tuned as, which is how `coax scan` labels a
            // channel: one space can hold a cable's own carriers and the
            // transponders it passes through, and the delivery system tells
            // them apart at a glance.
            let what_it_is = carrier_name(&system, &channel.name);
            progress(Step {
                what: &what_it_is,
                done: at + 1,
                ..Step::default()
            });
            // What the tuner's setting adds goes on what is asked and not on the
            // line: the line is the channel, whichever tuner receives it.
            let asked = listed.asking(lease.tuner(), &config.tuner_settings, &channel.tune);
            let properties = borrowed(&asked);
            // Terrestrial tables are its own and quick; everything else on a
            // cable may be a satellite multiplex remodulated onto it, cadence
            // and all.
            let bound = match delivery::family(&system) {
                Family::Terrestrial => READ_TERRESTRIAL,
                _ => READ_TABLES,
            };
            // The channel is set first and the lock waited for here rather than
            // by the daemon, so that the line in front of a viewer counts up
            // while the demodulator makes up its mind. `coax` scans the same
            // way.
            //
            // The wait is shown on the line that is already there rather than
            // written into the account: a line every quarter second buries what
            // the walk found.
            let mut waiting = |elapsed: Duration, strength: Option<f32>| {
                progress(Step {
                    what: &format!("{what_it_is}　同期待ち {:.1}秒", elapsed.as_secs_f32()),
                    done: at + 1,
                    strength,
                    ..Step::default()
                });
            };
            let mut locked = lock_on(daemon, &mut lease, &properties, &mut waiting);
            // A satellite carrier is asked for again where the demodulator said
            // it was not there. Moving between polarisations and bands moves
            // the supply the block runs on, so a tuner asked the moment after
            // the change can be told there is nothing on a carrier that is
            // about to appear. Nothing else is asked twice: asking every empty
            // cable carrier twice doubles a sweep.
            if matches!(locked, Err(Missed::NoLock(_)))
                && delivery::family(&system) == Family::Satellite
            {
                locked = lock_on(daemon, &mut lease, &properties, &mut waiting);
            }
            // Which tuner this walk is on, said once and as soon as the daemon
            // has chosen one. On a machine with several tuners, "it found
            // nothing" and "that card found nothing" are different reports.
            if !said_tuner {
                if let Some(tuner) = lease.tuner() {
                    said_tuner = true;
                    progress(Step {
                        what: &what_it_is,
                        done: at + 1,
                        notes: vec![format!("チューナー {tuner}")],
                        ..Step::default()
                    });
                }
            }
            let settled = match locked {
                Ok(settled) => settled,
                Err(missed) => {
                    report_missed(missed, &what_it_is, at, &mut report, &mut progress);
                    if report.gave_up {
                        break;
                    }
                    continue;
                }
            };
            let mut looked = watch(
                &mut lease,
                daemon,
                &properties,
                false,
                bound,
                |cnr, strength, mbps| {
                    progress(Step {
                        what: &what_it_is,
                        done: at + 1,
                        cnr,
                        strength,
                        mbps: Some(mbps),
                        ..Step::default()
                    });
                },
            );
            if let Ok(sample) = &mut looked {
                // What the tune measured while it was waiting for the lock
                // stands where the stream had nothing to add.
                sample.cnr = sample.cnr.or(settled.cnr);
                sample.strength = sample.strength.or(settled.strength);
            }
            let sample = match looked {
                Ok(sample) => sample,
                Err(missed) => {
                    report_missed(missed, &what_it_is, at, &mut report, &mut progress);
                    if report.gave_up {
                        break;
                    }
                    continue;
                }
            };
            report.answered += 1;
            // Taken before the sample is picked apart: the same figures go on
            // every line the carrier turns out to hold.
            let quality = quality_of(&sample);
            let streams = sample.tsmf;
            let group = sample.group;
            // What the carrier holds is written under it, a line per stream, so
            // the line itself says only that it was received. A carrier the
            // demodulator never got is not a carrier that was received: saying
            // so beside a figure it measured off noise reads as a channel that
            // is there and broken.
            let note = if sample.locked == Some(false) && sample.bytes == 0 {
                "信号なし".to_owned()
            } else {
                "受信".to_owned()
            };
            // The lines under it: which share of a split frame this carrier is,
            // and then a line for each stream, as `coax scan` writes them.
            let mut notes: Vec<String> = Vec::new();
            if let Some(split) = group.filter(|group| group.carriers > 1) {
                notes.push(format!(
                    "    TSMF 搬送波 {}/{}　グループ{}",
                    split.sequence, split.carriers, split.id
                ));
            }
            // A carrier that answered and said nothing about itself: the
            // account says so, because the alternative is a channel quietly
            // missing from the results.
            if streams.is_empty() && sample.identity.is_none() && sample.locked != Some(false) {
                notes.push(format!(
                    "    内容を読み取れませんでした（{:.1}秒 / {:.1}MB / {}）",
                    sample.watched.as_secs_f32(),
                    sample.bytes as f32 / (1024.0 * 1024.0),
                    // What the daemon's analyser made of the bytes. It reads
                    // every stream it hands out and knows both framings, so a
                    // carrier that lined up as neither is one producing noise.
                    if sample.format.is_empty() {
                        "多重化を認識できません".to_owned()
                    } else {
                        format!("{} {}パケット", sample.format, sample.packets)
                    },
                ));
            }
            if streams.is_empty() {
                // No frame on this carrier, so it is one stream and the plan
                // has it right -- but it may still be another platform's,
                // transmodulated onto the cable, and the stream's own service
                // table is what says so.
                if let Some(identity) = sample.identity {
                    let goes_to =
                        elsewhere_for(config, home.as_deref(), &lead, identity.original_network_id);
                    notes.push(format!(
                        "    TSID {} / ONID {}{}",
                        hex_id(identity.stream_id),
                        hex_id(identity.original_network_id),
                        beside(&sample.network),
                    ));
                    // Only where the carrier holds one stream. The slots of a
                    // multiplexing frame share a packet identifier for their
                    // service tables, so what is read off the carrier whole
                    // does not say which slot it is.
                    if let Some(said) = came_by_cable(channel) {
                        notes.push(format!("    {said}"));
                    }
                    let named: Vec<Named> = sample.services.iter().map(Named::from).collect();
                    notes.extend(service_lines(&named));
                    let written = Found {
                        quality,
                        onid: identity.original_network_id,
                        tsid: identity.stream_id,
                        name: format!("{}/{}", channel.name, identity.stream_id),
                        tuners: lead.clone(),
                        tune: channel.tune.clone(),
                        group: None,
                        carriers: Vec::new(),
                    };
                    match goes_to.or_else(|| home.clone()) {
                        Some(space) => {
                            report.added += 1;
                            across.entry(space).or_default().push(written);
                        }
                        None => notes.push(NOWHERE.to_owned()),
                    }
                }
                progress(Step {
                    what: &what_it_is,
                    done: at + 1,
                    cnr: sample.cnr,
                    strength: sample.strength,
                    mbps: Some(sample.mbps),
                    note: Some(note),
                    notes,
                    ..Step::default()
                });
                continue;
            }
            if streams.len() > 1 {
                report.multiplexed += 1;
            }
            // Which carriers this group has is only known once the walk has
            // been past all of them, so they are noted as they go by.
            if let Some(split) = group.filter(|group| group.carriers > 1) {
                let frequency = channel
                    .tune
                    .iter()
                    .find(|(name, _)| name == "FREQUENCY")
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default();
                let seen = groups.entry(split.id).or_default();
                if !frequency.is_empty() && !seen.contains(&frequency) {
                    seen.push(frequency);
                }
            }
            for stream in &streams {
                let slot = sample
                    .slots
                    .iter()
                    .find(|slot| slot.number == stream.number);
                // What the frame header says, and what the stream in the slot
                // says where the header said nothing. A header that left a
                // slot's identifiers at zero is not a slot without them: taking
                // its word would write down a channel called "C23/0" that
                // belongs to no network.
                let (stream_id, network_id) = identifiers(stream, slot);
                let mut tune = channel.tune.clone();
                // A slot that carries the advanced services is named apart:
                // what comes off it is TLV, and the driver puts it back
                // together.
                let carriage = if stream.tlv {
                    "TSMF_REL_TLV"
                } else {
                    "TSMF_REL_TS"
                };
                tune.push((carriage.to_owned(), stream.number.to_string()));
                let written = Found {
                    quality,
                    onid: network_id,
                    tsid: stream_id,
                    name: format!("{}/{}", channel.name, stream_id),
                    tuners: lead.clone(),
                    tune,
                    group: group.map(|group| group.id),
                    // Filled in once the walk is over: which carriers a group
                    // has is only known when all of them have been seen.
                    carriers: Vec::new(),
                };
                // Which space a stream belongs in is the network's to say and
                // not the carrier's, whichever slot of the frame it came out
                // of. A cable carries BS and 地デジ transmodulated onto it under
                // the identifiers those platforms declare, and a viewer looks
                // for them where the platform is. This is `coax`'s rule -- see
                // its `chsetSpaceName`.
                let goes_to = elsewhere_for(config, home.as_deref(), &lead, network_id);
                notes.push(format!(
                    "    TSMF [#{} {} TSID {} / ONID {}]{}",
                    stream.number,
                    if stream.tlv { "tlv" } else { "ts" },
                    hex_id(stream_id),
                    hex_id(network_id),
                    beside(slot.map_or("", |slot| slot.network.as_str())),
                ));
                match slot.filter(|slot| !slot.services.is_empty()) {
                    Some(slot) => notes.extend(service_lines(&slot.services)),
                    // The network table's word, where the stream's own did not
                    // arrive. Written the same way: a viewer reading the
                    // account wants the services rather than which table they
                    // came out of.
                    None => {
                        if let Some(described) = listed_across(&sample.slots, stream_id, network_id)
                        {
                            let named: Vec<Named> =
                                described.services.iter().map(Named::from).collect();
                            notes.extend(service_lines(&named));
                        }
                    }
                }
                match goes_to.or_else(|| home.clone()) {
                    Some(space) => {
                        report.added += 1;
                        across.entry(space).or_default().push(written);
                    }
                    None => notes.push(NOWHERE.to_owned()),
                }
            }
            progress(Step {
                what: &what_it_is,
                done: at + 1,
                cnr: sample.cnr,
                strength: sample.strength,
                mbps: Some(sample.mbps),
                note: Some(note),
                notes,
                ..Step::default()
            });
        }
        behind.set(behind.get() + plan.len());
        // The way this walk looked at, for what a merge is allowed to say has
        // gone -- and only where the walk got to the end.
        if !stopping.asked_to() {
            swept.push((lead, delivery::named(&route.system)));
        }
    }

    // What the walks found, space by space. A space that named a route the walk
    // got to the end of and was given nothing for has lost what it had on that
    // route; one whose routes were not walked keeps everything.
    for setting in config.spaces() {
        let streams = across.remove(&setting.name).unwrap_or_default();
        let lines = streams.into_iter().map(|one| line(&one, &groups)).collect();
        let merged = merge(found.of(&setting.name), lines, &swept);
        found.set(&setting.name, in_route_order(&setting, merged));
    }
    (found, report)
}

/// What the account says of a stream no space will have: found on a route that
/// no space of its platform names, and no space of the route's own platform
/// either.
const NOWHERE: &str = "    入れる空間がありません";

/// The line of the settings file one stream becomes, now that every carrier of
/// a split frame has been seen and the group it belongs to is known whole.
fn line(
    one: &Found,
    groups: &std::collections::BTreeMap<u8, Vec<String>>,
) -> (Definition, Quality) {
    let carriers = one
        .group
        .and_then(|group| groups.get(&group))
        .filter(|carriers| carriers.len() > 1)
        .cloned()
        .unwrap_or_else(|| one.carriers.clone());
    (
        lineup::line_of(
            one.onid,
            one.tsid,
            &one.name,
            &one.tuners,
            &one.tune,
            &carriers,
        ),
        one.quality,
    )
}

/// Whether a terrestrial multiplex received here can only have come down a
/// cable, and how that is known.
///
/// Only the plan says so, and it says so without a doubt: channels 53 to 62
/// were given back to mobile telephony, and the mid and super high bands and
/// VHF carry no television at all, so a broadcast answering there answers from
/// a cable.
///
/// What the network says about its own transmitters is not used. An operator
/// moving a station lands on the channels that are free, and the ones free
/// where the viewer lives are the ones some other transmitter of that network
/// uses elsewhere -- so falling silent there says "this could be off the air"
/// about a carrier that is nothing of the kind.
fn came_by_cable(channel: &coax_settings::channels::Channel) -> Option<String> {
    use coax::plan::Range;
    // Only a broadcast carrier is being judged. A cable's own carriage is tuned
    // as a cable and is not passing anything through.
    if !channel
        .tune
        .iter()
        .any(|(key, value)| key == "DELIVERY_SYSTEM" && delivery::same(value, delivery::ISDBT))
    {
        return None;
    }
    match coax::plan::range_of(&channel.name) {
        Range::ExUhf => Some("ケーブル（53-62chは地上波の割当なし）".to_owned()),
        Range::Vhf | Range::Catv => Some("ケーブル（この周波数に地上波の割当なし）".to_owned()),
        _ => None,
    }
}

/// What the watch on a carrier says about how it came in.
fn quality_of(sample: &Sample) -> Quality {
    // A demodulator that never got the carrier measured noise. The figure it
    // gives for that is a real number and means nothing, and printed beside a
    // channel's name it reads as a channel that is there: what such a carrier
    // is owed is the level.
    if sample.locked == Some(false) && sample.bytes == 0 {
        return Quality {
            losses: None,
            cnr: None,
            strength: sample.strength,
        };
    }
    Quality {
        losses: sample.losses,
        cnr: sample.cnr,
        strength: sample.strength,
    }
}

/// Which stream a slot holds and which network it belongs to: what the stream
/// says of itself, and what the frame header says where the stream has not
/// said.
///
/// The stream's word comes first. The two are written by different people: the
/// header by the operator multiplexing the carrier, the tables inside a stream
/// by whoever made the stream. Where they disagree it is the operator who has
/// renamed something, and a network identifier is what decides which space a
/// channel belongs in -- taking the header's word puts every re-multiplexed
/// channel into the cable's own space.
fn identifiers(stream: &isdb::tsmf::Relative, slot: Option<&Slot>) -> (u16, u16) {
    let said = slot.and_then(|slot| slot.identity);
    let stream_id = match said.map_or(0, |said| said.stream_id) {
        0 => stream.stream_id,
        id => id,
    };
    let network_id = match said.map_or(0, |said| said.original_network_id) {
        0 => stream.original_network_id,
        id => id,
    };
    (stream_id, network_id)
}

/// A name to put beside something else, or nothing at all where there is no name.
/// The transponder of the plan a table's place falls on, by its name, or the
/// place itself where none does.
fn transponder_at(plan: &[plan::Transponder], khz: u32) -> String {
    plan.iter()
        .find(|transponder| transponder.frequency_khz.abs_diff(khz) < SAME_TRANSPONDER_KHZ)
        .map_or_else(
            || format!("{khz}kHz"),
            |transponder| transponder.name.clone(),
        )
}

/// An identifier the way ZeroWave writes one: `0x4010 (16400)`.
fn hex_id(value: u16) -> String {
    format!("{value:#06x} ({value})")
}

fn beside(name: &str) -> String {
    match name.is_empty() {
        true => String::new(),
        false => format!("　{name}"),
    }
}

/// Whether a stream's own network table has been past at all.
///
/// This is what says a carrier has nothing left to give: the service table
/// arrives in the first couple of seconds and the network table every ten, so
/// the network table is the whole of the wait -- `coax` waits on the same two
/// things, see its `streamSettled`. What is waited for is the table, not the
/// name in it.
fn network_was_read(networks: &isdb::nit::Reader) -> bool {
    !networks.actual().is_empty() || !network_name(networks).is_empty()
}

/// What the stream's own network calls itself, out of the table it carries. A
/// network table describes the network's neighbours as well, and their names
/// are theirs: only the one the table marks as its own is this stream's.
fn network_name(networks: &isdb::nit::Reader) -> String {
    networks
        .named()
        .iter()
        .find(|network| network.actual && !network.name.is_empty())
        .map_or_else(String::new, |network| network.name.clone())
}

/// The same for a TLV stream, whose table names its network by identifier
/// rather than marking one of them as its own.
fn advanced_network_name(state: &isdb::mh::State) -> String {
    state
        .networks
        .iter()
        .find(|network| !network.name.is_empty() && network.id == state.network_id)
        .or_else(|| {
            state
                .networks
                .iter()
                .find(|network| !network.name.is_empty())
        })
        .map_or_else(String::new, |network| network.name.clone())
}

/// A network table's description of one stream, where it described it and said
/// what it carries. The description is weaker than the stream's own table: it
/// says which services the stream should hold, written elsewhere. The names
/// beside them come from whichever service table named the service, so a slot
/// too thin to send its own table can still be named.
fn listed(
    described: &[nit::Transport],
    stream_id: u16,
    network_id: u16,
) -> Option<&nit::Transport> {
    let carrying = |transport: &&nit::Transport| {
        transport.stream_id == stream_id && !transport.services.is_empty()
    };
    // The network first, where the stream said which it belongs to: on a cable
    // the tables of several networks go past together, and each numbers its
    // streams for itself.
    described
        .iter()
        .find(|transport| carrying(transport) && transport.original_network_id == network_id)
        .or_else(|| {
            (network_id == 0)
                .then(|| described.iter().find(carrying))
                .flatten()
        })
}

/// The same, looked for across every slot of a frame: a multiplex's slots are one
/// network's streams, and any of them may be the one carrying the table.
fn listed_across(slots: &[Slot], stream_id: u16, network_id: u16) -> Option<&nit::Transport> {
    slots
        .iter()
        .find_map(|slot| listed(&slot.described, stream_id, network_id))
}

/// The services a stream named, a line each: what selects it, what it calls
/// itself, who runs it, and what kind of thing it is. This is `coax scan`'s own
/// shape -- see its `describeServices` -- so that the two lists read the same.
fn service_lines(services: &[Named]) -> Vec<String> {
    services
        .iter()
        .map(|service| {
            // The sample was too short for the table that names them, or the
            // service came from one that carries no names.
            let name = match service.name.is_empty() {
                true => "???".to_owned(),
                false => isdb::arib::substitute(&service.name),
            };
            let mut line = format!("        SID {} {name}", hex_id(service.id));
            if !service.provider.is_empty() {
                line.push_str(&format!("　{}", isdb::arib::substitute(&service.provider)));
            }
            let mut what: Vec<String> = Vec::new();
            match isdb::sdt::kind_name(service.kind) {
                Some(kind) => what.push(kind.to_owned()),
                // Written the way the host writes a service type it has no word
                // for.
                None if service.kind != 0 => what.push(format!("他({:02x})", service.kind)),
                None => {}
            }
            if service.scrambled {
                what.push("有料".to_owned());
            }
            if !what.is_empty() {
                line.push_str(&format!("　({})", what.join(", ")));
            }
            line
        })
        .collect()
}

/// How much a table said about one transponder, for choosing between two
/// accounts of it: how many of its services were named, and how many it said
/// there were.
fn account(transport: &nit::Transport) -> (usize, usize) {
    let named = transport
        .services
        .iter()
        .filter(|service| !service.name.is_empty())
        .count();
    (named, transport.services.len())
}

/// A carrier of the plan as the account names it, the way `coax scan` and
/// ZeroWave do: the system as its standard spells it, then the carrier --
/// `ISDB-T 13ch`, `ISDB-C C13ch`, `J.83B (5.6 MBaud) 73.75MHz`, `ISDB-S BS01`,
/// `DVB-S2 JCSAT3A/JD1`. The channel's own name -- what the lines and the
/// host's list say -- is left as it is; this is only the heading in front of a
/// viewer. Any other system is the channel's name with the system after it.
fn carrier_name(system: &str, name: &str) -> String {
    match delivery::family(system) {
        Family::Terrestrial | Family::Cable => format!("{} {name}ch", delivery::written(system)),
        Family::Optical => format!("{} {name}MHz", delivery::written(system)),
        Family::Satellite | Family::Sky => format!("{} {name}", delivery::written(system)),
        _ if system.is_empty() => name.to_owned(),
        _ => format!("{name} ({system})"),
    }
}

/// Which other space a stream belongs in, where it is not the one it was found
/// on.
///
/// A cable carries other platforms transmodulated onto it -- BS most of all --
/// and the network identifier is what says so. A stream whose platform has a
/// space of its own goes there; one whose platform is the space it was found on
/// stays where it is.
fn elsewhere_for(
    config: &Config,
    found_on: Option<&str>,
    lead: &str,
    original_network_id: u16,
) -> Option<String> {
    // Read as a transmodulation's, because that is how this arrived: an
    // identifier nobody recognises belongs to the operator carrying it.
    let platform = group::of_network_transmodulated(original_network_id)?;
    let spaces = config.spaces();
    let found_on = found_on.unwrap_or_default();
    let here = spaces.iter().find(|setting| setting.name == found_on);
    if here.and_then(|setting| setting.platform()) == Some(platform) {
        return None;
    }
    // A stream found here is tuned with the parameters of the carrier it was
    // found on, so of two spaces holding the same platform it belongs in the
    // one with this route.
    let named = choose(&spaces, lead, found_on, |setting| {
        setting.platform() == Some(platform)
    });
    named.or_else(|| {
        // A space that did not say which platform it holds, but is set up to
        // receive the one this stream declares. Saying it is optional, and a
        // viewer who left it out has not said their 地デジ space is not for 地デジ.
        // What they did say is the delivery system, which is a fact about the
        // space rather than a name.
        choose(&spaces, lead, found_on, |setting| {
            setting.platform().is_none()
                && setting
                    .routes
                    .iter()
                    .any(|route| receives(platform, &route.system))
        })
    })
}

/// The first space that will have the stream: one with the route it was found
/// on if there is one, and otherwise the first that will take it at all.
fn choose(
    spaces: &[coax_settings::config::SpaceSetting],
    lead: &str,
    found_on: &str,
    wanted: impl Fn(&coax_settings::config::SpaceSetting) -> bool,
) -> Option<String> {
    let mut anywhere = None;
    for setting in spaces {
        // The space the walk is on is not somewhere else.
        if setting.name == found_on || !wanted(setting) {
            continue;
        }
        if !lead.is_empty() && setting.has_route(lead) {
            return Some(setting.name.clone());
        }
        anywhere.get_or_insert_with(|| setting.name.clone());
    }
    anywhere
}

/// Whether a space set up for this delivery system receives this platform.
///
/// Only where the answer is one platform. A satellite space receives BS and CS
/// and the advanced services on the same system, so a space that did not say
/// which of them it holds is not answered by what it is tuned as; a terrestrial
/// one is.
fn receives(platform: &str, system: &str) -> bool {
    match platform {
        "GR" => delivery::family(system) == Family::Terrestrial,
        "CATV" => delivery::family(system) == Family::Cable,
        _ => false,
    }
}

/// How many carriers this sweep will ask about, so that something can say how
/// far along it is. A satellite is counted at its whole plan even though the
/// table usually cuts it short after two: a bar reaching the end early beats
/// one told the total twice.
fn how_much(config: &Config, what: &What) -> usize {
    let mut total = 0;
    // The satellite plan is walked once per route, so it counts once per route.
    total += satellite_routes(config, what)
        .iter()
        .map(|route| satellite_count(&route.system))
        .sum::<usize>();
    for route in walked_routes(config, what) {
        total += lineup::plan_of(&route).len();
    }
    total
}

/// What a walk needs to know about tuning: the route it is on, what the daemon
/// has, what the settings add for each tuner, and what a viewer chose over
/// all of them.
struct Tuning<'a> {
    route: &'a Route,
    listed: &'a Listed,
    settings: &'a TunerSettings,
    only: Option<&'a str>,
}

/// Which tuner to ask for a channel: the one a viewer chose, or the first of
/// the ones the space names that will serve -- and where the space names none,
/// the first the daemon has that will. The walk is named a tuner from its
/// first carrier on: what a tuner's setting adds has to know which tuner it is
/// going to, and a dish whose supply came on only after the first answer
/// would have the first transponders of the walk come back empty.
fn tuner_for(
    only: Option<&str>,
    tuners: &[coax::daemon::Tuner],
    patterns: &[String],
    system: &str,
) -> Option<String> {
    if let Some(only) = only {
        return Some(only.to_owned());
    }
    let patterns = if patterns.is_empty() {
        vec![ANY_TUNER.to_owned()]
    } else {
        patterns.to_vec()
    };
    daemon::candidates(tuners, &patterns, system)
        .into_iter()
        .flatten()
        .next()
}

/// A pattern every tuner matches.
const ANY_TUNER: &str = "*";

/// Words with the names borrowed, as the daemon's query takes them.
fn borrowed(words: &[(String, String)]) -> Vec<(&str, String)> {
    words
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect()
}

/// Whether a satellite route is tuned as ISDB-S3.
fn is_s3(system: &str) -> bool {
    delivery::same(system, delivery::ISDBS3)
}

/// The transponders of the 110 degree east plan a route's system is carried
/// on: ISDB-S on the right hand of BS and CS, ISDB-S3 on both hands of BS and
/// the left hand of CS (see [`plan::carries`]). A transponder its system is
/// never sent on is one more lock that cannot come.
fn satellite_plan(system: &str) -> Vec<plan::Transponder> {
    coax::scan::satellite_transponders(&[], is_s3(system)).unwrap_or_default()
}

/// How many transponders a walk of a satellite route may tune, for the bar.
fn satellite_count(system: &str) -> usize {
    satellite_plan(system).len()
}

/// The two walks a satellite takes -- BS, then the 110 degree east CS -- and
/// how many networks each holds.
///
/// The count is what ends a walk. A network's table names every transponder of
/// that network, so once they have all spoken there is nothing left to ask --
/// without it the walk goes on to every transponder no table mentioned. ISDB-S
/// has two networks on CS that never mention each other; ISDB-S3 has one on
/// each, and waiting for a second CS network that is not there walks every
/// transponder of it.
fn satellite_passes(system: &str) -> [(bool, usize); 2] {
    [(true, 1), (false, if is_s3(system) { 1 } else { 2 })]
}

/// Whether any tuner this space may use says it demodulates this.
///
/// A tuner that lists nothing is counted as able: the list is what a backend
/// knows without opening the card, and several do not know until they have.
/// Only a tuner that named its systems and left this one out is a no.
fn receivable(tuners: &[coax::daemon::Tuner], patterns: &[String], system: &str) -> bool {
    if patterns.is_empty() || tuners.is_empty() {
        return true;
    }
    let mut matched = false;
    for tuner in tuners {
        if !patterns.iter().any(|pattern| tuner.matches(pattern)) {
            continue;
        }
        matched = true;
        if tuner.receives(system) {
            return true;
        }
    }
    // Patterns that named no tuner the daemon listed say nothing about what can
    // be received.
    !matched
}

/// Whether a delivery system is one of the 110 degree east ones, which the plan
/// cannot fill: a transponder alone is not a channel there, because a carrier
/// holds several transport streams and a tune has to name one.
///
/// The 124/128 degree east service is not one of these: it is a grid whose
/// carriers say nothing about their neighbours, so it is walked like a cable.
fn satellite(system: &str) -> bool {
    delivery::family(system) == Family::Satellite
}

/// Whether a delivery system is walked rather than asked, which is what decides
/// what looking at it takes.
///
/// A cable is walked because it cannot be asked, and so is the 124/128 degree
/// east service. Terrestrial is one stream per carrier, so the host's own scan
/// would find it -- but a line in the file is what lets the same stream,
/// received another way, be one channel rather than two.
fn walked(system: &str) -> bool {
    matches!(
        delivery::family(system),
        Family::Optical | Family::Cable | Family::Sky | Family::Terrestrial
    )
}

/// Whether this sweep looks at a route at all.
fn wanted(route: &Route, what: &What) -> bool {
    what.wants(route) && (satellite(&route.system) || walked(&route.system))
}

/// The delivery system a channel is tuned as.
fn system_of(channel: &coax_settings::channels::Channel) -> String {
    let asked = channel
        .tune
        .iter()
        .find(|(name, _)| name == "DELIVERY_SYSTEM")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    if asked.is_empty() {
        return String::new();
    }
    // What the settings call it, not what the daemon is told. A channel is
    // tuned with the name the DVB API knows -- Japanese cable is asked for as
    // DVBC/ANNEX_A -- and putting that in front of a viewer says this band is
    // Europe's, which it is not.
    delivery::named(asked)
}

/// Every route this sweep walks rather than asks, each once, in the order the
/// spaces named them.
fn walked_routes(config: &Config, what: &What) -> Vec<Route> {
    let mut routes: Vec<Route> = Vec::new();
    for setting in config.spaces() {
        for route in &setting.routes {
            if !satellite(&route.system)
                && walked(&route.system)
                && what.wants(route)
                && !routes.iter().any(|known| known.label == route.label)
            {
                routes.push(route.clone());
            }
        }
    }
    routes
}

/// The space a route's own findings go in: the first that names the route and
/// offers its plan. None where every space naming the route holds another
/// platform, as BS with the cable it is transmodulated onto does.
fn home_of(config: &Config, route: &Route) -> Option<String> {
    config
        .spaces()
        .into_iter()
        .find(|setting| setting.has_route(&route.label) && setting.offers(route))
        .map(|setting| setting.name)
}

/// Every satellite route any space is received by, each once, in the order the
/// spaces named them.
fn satellite_routes(config: &Config, what: &What) -> Vec<Route> {
    let mut routes: Vec<Route> = Vec::new();
    for setting in config.spaces() {
        for route in &setting.routes {
            if satellite(&route.system)
                && what.wants(route)
                && !routes.iter().any(|known| known.label == route.label)
            {
                routes.push(route.clone());
            }
        }
    }
    routes
}

/// The entries one space takes out of what the satellite turned out to hold.
///
/// A space that names a platform -- BS, CS -- takes the streams of that
/// platform and no others: a viewer looking in the BS space for a CS channel
/// would not find it. A space that names no platform takes everything.
fn for_space(
    space: &SpaceSetting,
    transports: &[Confirmed],
    route: &Route,
    report: &mut Report,
) -> Vec<Found> {
    let system = route.system.as_str();
    let platform = space.platform().map(str::to_owned);
    let named = plan::satellite();
    let mut extras = Vec::new();
    for (transport, place) in transports {
        if let Some(platform) = &platform {
            if group::of_network(transport.original_network_id) != Some(platform.as_str()) {
                continue;
            }
        }
        // What the network said about where this multiplex is, read under the
        // rules of the standard that was tuned to find it. A multiplex the
        // table gave no place to, or one no receiver here can reach, is not a
        // channel.
        let Some(described) = transport.satellite.as_ref() else {
            continue;
        };
        let Some(mut tune) = descriptors::satellite_channel(described, system, &Dish::default())
        else {
            continue;
        };
        let at = frequency_of(&tune);
        let where_it_is = named
            .iter()
            .find(|transponder| transponder.frequency_khz == at)
            .map_or_else(
                || format!("{at}kHz"),
                |transponder| transponder.name.clone(),
            );
        // The stream by its identifier, or by its place on the transponder for a
        // driver that names it that way.
        let stream = place
            .clone()
            .unwrap_or_else(|| transport.stream_id.to_string());
        tune.push(("STREAM_ID".to_owned(), stream));
        extras.push(Found {
            // Nothing was received: this stream is one the network's own table
            // named.
            quality: Quality::default(),
            onid: transport.original_network_id,
            tsid: transport.stream_id,
            name: format!("{where_it_is}/{}", transport.stream_id),
            tuners: route.label.clone(),
            tune,
            group: None,
            carriers: Vec::new(),
        });
        report.added += 1;
    }
    extras
}

/// How far apart two figures for one transponder may be and still be one
/// transponder. The plan holds nominal frequencies and a table's are worked out
/// from a descriptor; Japan's transponders are tens of megahertz apart.
const SAME_TRANSPONDER_KHZ: u32 = 1_000;

/// A multiplex the walk heard answer to its name, and how a tune names it:
/// nothing for its identifier, or its place on the transponder for a driver
/// that names a stream that way.
type Confirmed = (nit::Transport, Option<String>);

/// One multiplex to ask for once the tables have been read: where it is, what
/// `STREAM_ID` is said, and -- by identifier -- which of the tables' multiplexes
/// it has to say it is. Asked by place, it may turn out to be any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Confirm {
    khz: u32,
    stream: String,
    expected: Option<usize>,
}

/// What to ask for to hear each multiplex the tables named say what it is.
///
/// Only the networks that spoke for themselves: a network's table describes
/// its neighbours too. A driver that takes the stream's identifier is asked
/// for each by it. One that names a stream by its place on the transponder
/// (`places` of them) cannot be asked that way, so every place of every
/// transponder the tables named is asked instead, and the multiplex there says
/// which it is.
fn confirmations(named: &[nit::Transport], heard: &[u16], places: Option<u32>) -> Vec<Confirm> {
    let wanted = named.iter().enumerate().filter(|(_, transport)| {
        heard.contains(&transport.original_network_id) && japanese_intermediate_khz(transport) != 0
    });
    let Some(places) = places else {
        return wanted
            .map(|(at, transport)| Confirm {
                khz: japanese_intermediate_khz(transport),
                stream: transport.stream_id.to_string(),
                expected: Some(at),
            })
            .collect();
    };
    let mut transponders: Vec<u32> = Vec::new();
    for (_, transport) in wanted {
        let khz = japanese_intermediate_khz(transport);
        if !transponders.contains(&khz) {
            transponders.push(khz);
        }
    }
    transponders
        .into_iter()
        .flat_map(|khz| {
            (0..places).map(move |place| Confirm {
                khz,
                stream: place.to_string(),
                expected: None,
            })
        })
        .collect()
}

/// Which of the tables' multiplexes a confirmation heard, if any: the one it
/// asked for by identifier, or -- asked by place -- whichever a network that
/// spoke for itself named under the identity the multiplex gave.
fn heard_as(
    confirm: &Confirm,
    said: Option<isdb::sdt::Identity>,
    named: &[nit::Transport],
    heard: &[u16],
) -> Option<usize> {
    let said = said?;
    let is = |transport: &nit::Transport| {
        transport.stream_id == said.stream_id
            && transport.original_network_id == said.original_network_id
    };
    match confirm.expected {
        Some(at) => named.get(at).filter(|transport| is(transport)).map(|_| at),
        None => named
            .iter()
            .position(|transport| {
                is(transport)
                    && heard.contains(&transport.original_network_id)
                    && japanese_intermediate_khz(transport) == confirm.khz
            })
            .or_else(|| {
                named.iter().position(|transport| {
                    is(transport) && heard.contains(&transport.original_network_id)
                })
            }),
    }
}

/// Reads a satellite out of the signal.
///
/// The plan says where the transponders are; it cannot say what is on them. So
/// this locks one, reads the network's own table, and then skips every
/// transponder the table has already accounted for.
///
/// `total` is the sweep's count of what it has to ask, which this keeps true as
/// the walk learns how much there is: the transponders a pass never had to
/// tune come off it, and the multiplexes the tables named go on it before they
/// are asked for. ZeroWave counts its walk the same way, so the bar never runs
/// past its end. What it returns beside the multiplexes is how many tunes it
/// took, which is how far the bar has gone.
fn look_at_satellite(
    daemon: &Daemon,
    with: &Tuning,
    report: &mut Report,
    stopping: &Stopping,
    total: &std::cell::Cell<usize>,
    progress: &mut impl FnMut(Step),
) -> (Vec<Confirmed>, usize) {
    let route = with.route;
    let system = route.system.clone();
    let named = tuner_for(with.only, &with.listed.tuners, &route.tuners, &system);
    // What one tune asks: the tuner, and what its setting adds.
    let asking = |words: &[(&str, String)]| -> Vec<(String, String)> {
        let words: Vec<(String, String)> = words
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        with.listed.asking(named.as_deref(), with.settings, &words)
    };
    // One tuner for the whole pass, held on one connection: the second tune must
    // not find the first's tuner gone.
    let mut lease = daemon.hold(named.as_deref());
    let mut said_tuner = false;
    let mut confirmed: Vec<Confirmed> = Vec::new();
    // Whether the driver names a stream by its place on the transponder, and
    // how many places it has: learnt from the first tune that asks.
    let mut places: Option<Option<u32>> = None;
    let mut tuned = 0usize;
    let plan = satellite_plan(&system);
    for (bs, networks) in satellite_passes(&system) {
        if report.gave_up {
            break;
        }
        let mut heard: Vec<u16> = Vec::new();
        // What the tables named, every account of every multiplex: a stream two
        // tables place differently is looked for in both places.
        let mut named_streams: Vec<nit::Transport> = Vec::new();
        // Each network's tables, as the places they named, by the transponder
        // they were heard on.
        let mut opinions: Vec<Opinion> = Vec::new();
        let mut visited: Vec<String> = Vec::new();
        let counted = plan
            .iter()
            .filter(|transponder| transponder.is_bs() == bs)
            .count();
        let before = tuned;
        for second in [false, true] {
            for transponder in plan.iter().filter(|transponder| transponder.is_bs() == bs) {
                if stopping.asked_to() || report.gave_up {
                    break;
                }
                if visited.contains(&transponder.name) {
                    continue;
                }
                if second {
                    // A second opinion on each network, and every transponder a
                    // table named where opinions differ.
                    if heard_enough(&opinions, &heard) {
                        break;
                    }
                    if !spoken_for(&named_streams, &heard, transponder) {
                        continue;
                    }
                } else {
                    if heard.len() >= networks {
                        break;
                    }
                    if spoken_for(&named_streams, &heard, transponder) {
                        continue;
                    }
                }
                visited.push(transponder.name.clone());
                report.asked += 1;
                tuned += 1;
                let asked = asking(&[
                    ("DELIVERY_SYSTEM", system.clone()),
                    ("FREQUENCY", transponder.frequency_khz.to_string()),
                ]);
                let properties = borrowed(&asked);
                // Named with the system: a viewer with a dish and a cable that
                // passes a few transponders through sees the same transponder
                // asked twice, as ISDBS and as ISDBC.
                let what = carrier_name(&system, &transponder.name);
                progress(Step {
                    what: &what,
                    done: tuned,
                    ..Step::default()
                });
                if !said_tuner {
                    if let Some(tuner) = lease.tuner() {
                        said_tuner = true;
                        progress(Step {
                            what: &what,
                            done: tuned,
                            notes: vec![format!("チューナー {tuner}")],
                            ..Step::default()
                        });
                    }
                }
                let sample = match watch(
                    &mut lease,
                    daemon,
                    &properties,
                    true,
                    READ_TABLES,
                    |cnr, strength, mbps| {
                        progress(Step {
                            what: &what,
                            done: tuned,
                            cnr,
                            strength,
                            mbps: Some(mbps),
                            ..Step::default()
                        });
                    },
                ) {
                    Ok(sample) => sample,
                    Err(missed) => {
                        let said = if missed == Missed::Busy {
                            report.busy += 1;
                            "チューナーが使用中"
                        } else {
                            "受信できません"
                        };
                        progress(Step {
                            what: &what,
                            done: tuned,
                            note: Some(said.to_owned()),
                            ..Step::default()
                        });
                        if missed == Missed::Busy && report.answered == 0 && report.busy >= GIVE_UP
                        {
                            report.gave_up = true;
                            break;
                        }
                        continue;
                    }
                };
                report.answered += 1;
                let mut notes: Vec<String> = Vec::new();
                for network in &sample.networks {
                    if !heard.contains(network) {
                        heard.push(*network);
                    }
                    let places = places_of(&sample.transports, *network);
                    if let Some((_, from, first)) =
                        opinions.iter().find(|(known, _, _)| known == network)
                    {
                        if *first != places {
                            notes.push(format!(
                                "    ネットワーク {network} のテーブルが {from} で聞いた物と食い違う（{}）",
                                disagreement(first, &places)
                            ));
                        }
                    }
                    opinions.push((*network, transponder.name.clone(), places));
                }
                // A line for each multiplex this lock has just learnt of, in the
                // shape `coax scan` writes a stream in. The ones an earlier lock
                // already named in the same place are left off, unless this lock
                // has a better account of them. The two networks of the 110
                // degree east CS each list the other's transponders with no
                // services on them.
                for transport in &sample.transports {
                    let place = japanese_intermediate_khz(transport);
                    match named_streams.iter().position(|known| {
                        known.stream_id == transport.stream_id
                            && japanese_intermediate_khz(known) == place
                    }) {
                        Some(at) if account(transport) > account(&named_streams[at]) => {
                            named_streams[at] = transport.clone();
                        }
                        Some(_) => continue,
                        None => named_streams.push(transport.clone()),
                    }
                    notes.push(format!(
                        "    TSID {} / ONID {}　{}　サービス {}",
                        hex_id(transport.stream_id),
                        hex_id(transport.original_network_id),
                        transponder_at(&plan, place),
                        transport.services.len()
                    ));
                }
                progress(Step {
                    what: &what,
                    done: tuned,
                    total: 0,
                    cnr: sample.cnr,
                    strength: sample.strength,
                    mbps: Some(sample.mbps),
                    note: Some(format!(
                        "受信　トランスポンダ {} 本",
                        sample.transports.len()
                    )),
                    notes,
                });
            }
        }
        // **Then each multiplex is asked for by its identifier, and kept only
        // where it answers to it.** What the tables named is where to look, not
        // what is there. A driver that names a stream by its place is asked
        // every place instead, and what answers says which it is.
        let first = named_streams.iter().find(|transport| {
            heard.contains(&transport.original_network_id)
                && japanese_intermediate_khz(transport) != 0
        });
        if places.is_none() {
            if let Some(first) = first {
                let asked = asking(&[
                    ("DELIVERY_SYSTEM", system.clone()),
                    ("FREQUENCY", japanese_intermediate_khz(first).to_string()),
                ]);
                places = Some(
                    lease
                        .tune(&borrowed(&asked))
                        .ok()
                        .and_then(|tuned| tuned.relative_streams),
                );
            }
        }
        let confirms = confirmations(&named_streams, &heard, places.flatten());
        let walked = tuned - before;
        total.set(total.get().saturating_sub(counted.saturating_sub(walked)) + confirms.len());
        for confirm in confirms {
            if stopping.asked_to() || report.gave_up {
                break;
            }
            tuned += 1;
            let asked = asking(&[
                ("DELIVERY_SYSTEM", system.clone()),
                ("FREQUENCY", confirm.khz.to_string()),
                ("STREAM_ID", confirm.stream.clone()),
            ]);
            let properties = borrowed(&asked);
            // Named the way ZeroWave names the same step: the transponder, then
            // the stream asked for on it.
            let carrier = carrier_name(&system, &transponder_at(&plan, confirm.khz));
            let what = match confirm.expected.and_then(|at| named_streams.get(at)) {
                Some(transport) => format!("{carrier} TSID {}", hex_id(transport.stream_id)),
                None => format!("{carrier} 相対TS番号 {}", confirm.stream),
            };
            progress(Step {
                what: &what,
                done: tuned,
                ..Step::default()
            });
            let answered = watch(
                &mut lease,
                daemon,
                &properties,
                false,
                READ_STREAM,
                |_, _, _| {},
            );
            let sample = match answered {
                Ok(sample) => sample,
                Err(missed) => {
                    let (note, strength) = match missed {
                        Missed::Busy => ("チューナーが使用中", None),
                        Missed::NoLock(strength) => ("信号なし", strength),
                        Missed::Noise => ("多重化を認識できません", None),
                        Missed::Silent => ("同期しましたが、データを受信できません", None),
                    };
                    progress(Step {
                        what: &what,
                        done: tuned,
                        strength,
                        note: Some(note.to_owned()),
                        ..Step::default()
                    });
                    continue;
                }
            };
            let Some(at) = heard_as(&confirm, sample.identity, &named_streams, &heard) else {
                progress(Step {
                    what: &what,
                    done: tuned,
                    note: Some(match sample.identity {
                        Some(said) => format!("別の TS を受信（TSID {}）", hex_id(said.stream_id)),
                        None => String::from("TSID を読めず"),
                    }),
                    ..Step::default()
                });
                continue;
            };
            let Some(transport) = named_streams.get(at) else {
                continue;
            };
            // What it turned out to hold, as the carriers of the walk are
            // written: the stream, then a line for each of its services.
            let mut notes = vec![format!(
                "    TSID {} / ONID {}{}",
                hex_id(transport.stream_id),
                hex_id(transport.original_network_id),
                beside(&sample.network),
            )];
            let services: Vec<Named> = sample.services.iter().map(Named::from).collect();
            notes.extend(service_lines(&services));
            progress(Step {
                what: &what,
                done: tuned,
                cnr: sample.cnr,
                strength: sample.strength,
                mbps: Some(sample.mbps),
                note: Some("受信".to_owned()),
                notes,
                ..Step::default()
            });
            let mut kept = transport.clone();
            // The services are the ones the multiplex names itself.
            kept.services = sample
                .services
                .iter()
                .map(|service| nit::ListedService {
                    id: service.id,
                    kind: service.kind,
                    name: service.name.clone(),
                    lossy: service.lossy,
                })
                .collect();
            confirmed.retain(|(had, _)| {
                !(had.stream_id == kept.stream_id
                    && had.original_network_id == kept.original_network_id)
            });
            let place = confirm.expected.is_none().then(|| confirm.stream.clone());
            confirmed.push((kept, place));
        }
    }
    (confirmed, tuned)
}

/// One network table as heard: the network, the transponder it was heard on, and
/// the places it named ([`places_of`]).
type Opinion = (u16, String, Vec<(u16, u32)>);

/// The places a network's own table named for its multiplexes, as identifier
/// and intermediate frequency, in order.
fn places_of(transports: &[nit::Transport], network: u16) -> Vec<(u16, u32)> {
    let mut places: Vec<(u16, u32)> = transports
        .iter()
        .filter(|transport| transport.original_network_id == network)
        .map(|transport| (transport.stream_id, japanese_intermediate_khz(transport)))
        .collect();
    places.sort_unstable();
    places.dedup();
    places
}

/// What two tables disagreed on, for the account.
fn disagreement(first: &[(u16, u32)], later: &[(u16, u32)]) -> String {
    let said = |places: Vec<&(u16, u32)>| {
        places
            .iter()
            .map(|(stream, khz)| format!("{stream:#06x}@{khz}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "前のテーブルだけ: {}／このテーブルだけ: {}",
        said(first.iter().filter(|one| !later.contains(one)).collect()),
        said(later.iter().filter(|one| !first.contains(one)).collect())
    )
}

/// Whether every network has been heard from two transponders that agree. One
/// heard to disagree is never enough: the walk then reads every transponder a
/// table named.
fn heard_enough(opinions: &[Opinion], heard: &[u16]) -> bool {
    heard.iter().all(|network| {
        let tables: Vec<&Vec<(u16, u32)>> = opinions
            .iter()
            .filter(|(known, _, _)| known == network)
            .map(|(_, _, places)| places)
            .collect();
        tables.len() >= 2 && tables.iter().all(|one| *one == tables[0])
    })
}

/// Whether a table has already described this transponder, which makes tuning
/// it a way of reading the same network twice.
fn spoken_for(
    transports: &[nit::Transport],
    heard: &[u16],
    transponder: &plan::Transponder,
) -> bool {
    transports.iter().any(|known| {
        // Only a network that has spoken for itself accounts for its
        // transponders. A network table describes its neighbours too -- the 110
        // degree east CS carries two networks, each listing the other's
        // transponders with no services -- and taking that as an account leaves
        // the walk with transponders it never tuned.
        heard.contains(&known.original_network_id)
            && japanese_intermediate_khz(known).abs_diff(transponder.frequency_khz)
                < SAME_TRANSPONDER_KHZ
    })
}

/// Where a multiplex of the 110 degree east plan lands, for comparing what a
/// table said against the plan. This walk is that plan's, so the conversion is
/// that plan's: every installation here has the one block. Zero for a multiplex
/// the table gave no place to, or one outside what the block converts.
fn japanese_intermediate_khz(transport: &nit::Transport) -> u32 {
    transport
        .satellite
        .as_ref()
        .and_then(|described| {
            plan::intermediate_frequency_khz(described.downlink_khz, !described.right_hand())
        })
        .unwrap_or_default()
}

/// A channel's frequency as a number, or zero where it has none this can read.
fn frequency_of(channel: &descriptors::Channel) -> u32 {
    channel
        .iter()
        .find(|(name, _)| name == "FREQUENCY")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or_default()
}

/// What one carrier gave up when it was watched.
#[derive(Debug, Default)]
struct Sample {
    /// How long the carrier was listened to, and how much came off it. Only
    /// worth saying where nothing else came of it: a carrier that answered and
    /// then said nothing about itself is the shape of a stream that went quiet
    /// part way.
    watched: Duration,
    bytes: usize,
    /// Whether the demodulator said it had the carrier. Nothing where the
    /// backend does not answer, which is not a no.
    locked: Option<bool>,
    /// What the daemon's own analyser made of the bytes: the multiplex they
    /// lined up as, and how many packets it found.
    format: String,
    packets: u64,
    /// The streams a multiplexing frame holds, where there is a frame.
    tsmf: Vec<isdb::tsmf::Relative>,
    /// Which group of a split frame the carrier is part of, where the frame is
    /// split.
    group: Option<isdb::tsmf::Group>,
    /// What the carrier says it is, where there is no frame to say it.
    identity: Option<isdb::sdt::Identity>,
    /// The services that table named, which is what says whether the carrier is
    /// worth a viewer's while: a multiplex holds promotional and data services
    /// too.
    services: Vec<isdb::sdt::Service>,
    /// What each slot of the multiplexing frame said about itself.
    slots: Vec<Slot>,
    /// What the network the carrier belongs to calls itself, where its table
    /// went past.
    network: String,
    /// The transport streams a network's own table names, where it was looked
    /// for.
    transports: Vec<nit::Transport>,
    /// The networks this carrier turned out to belong to, which is not the same
    /// as the networks its tables named: a network describes its neighbours
    /// too.
    networks: Vec<u16>,
    cnr: Option<f32>,
    /// Where the backend cannot measure a ratio, the level it can.
    strength: Option<f32>,
    mbps: f32,
    /// The share of the carrier that did not arrive, counted from the packets
    /// themselves rather than asked of the demodulator: the one measure every
    /// tuner gives.
    losses: Option<f32>,
}

/// One slot of a multiplexing frame, read on its own.
#[derive(Debug, Default)]
struct Slot {
    /// The relative stream number the frame header gives it.
    number: u8,
    /// What the stream in the slot says it is, which stands in where the frame
    /// header left the identifiers at zero. A header is written by the operator
    /// multiplexing the carrier; the tables inside a stream by whoever made the
    /// stream.
    identity: Option<isdb::sdt::Identity>,
    services: Vec<Named>,
    /// What the network the stream belongs to calls itself, where its own table
    /// said. Nothing where the table did not go past while the carrier was
    /// held.
    network: String,
    /// The streams the network's own table described, as this slot carried it.
    /// A network describes every stream it has, so a slot carrying the table
    /// says what the other slots hold too.
    described: Vec<nit::Transport>,
}

/// A service as it is written down for a viewer, from whichever kind of stream
/// named it. The two kinds name their services in tables with nothing in common
/// but the questions they answer, and a viewer reading the account wants one
/// shape.
#[derive(Debug, Clone, Default)]
struct Named {
    id: u16,
    kind: u8,
    scrambled: bool,
    name: String,
    provider: String,
}

impl From<&isdb::sdt::Service> for Named {
    fn from(service: &isdb::sdt::Service) -> Self {
        Self {
            id: service.id,
            kind: service.kind,
            scrambled: service.scrambled,
            name: service.name.clone(),
            provider: service.provider.clone(),
        }
    }
}

/// A service the network table listed. The name is what a service table called
/// it, which on a satellite arrives for every multiplex of the network.
impl From<&isdb::nit::ListedService> for Named {
    fn from(service: &isdb::nit::ListedService) -> Self {
        Self {
            id: service.id,
            kind: service.kind,
            name: service.name.clone(),
            ..Named::default()
        }
    }
}

impl From<&isdb::mh::Service> for Named {
    fn from(service: &isdb::mh::Service) -> Self {
        Self {
            id: service.id,
            kind: service.kind,
            scrambled: service.scrambled,
            name: service.name.clone(),
            provider: service.provider.clone(),
        }
    }
}

/// The streams a TLV network table listed, as the transport stream side keeps
/// them. Only what the two have in common is carried over: which stream of
/// which network, and the services it holds with whatever named them.
fn advanced_listing(state: &isdb::mh::State) -> Vec<nit::Transport> {
    state
        .networks
        .iter()
        .flat_map(|network| network.streams.iter())
        .map(|stream| nit::Transport {
            stream_id: stream.tlv_stream_id,
            original_network_id: stream.original_network_id,
            services: stream.services.clone(),
            ..nit::Transport::default()
        })
        .collect()
}

/// A slot being taken apart: what comes out of the carrier for it, and what
/// that turns out to say. Which of the two it is comes from the frame header,
/// which says of each slot whether it holds a transport stream or the TLV the
/// advanced services use.
#[derive(Debug)]
enum Taking {
    Transport {
        number: u8,
        /// The stream identifier the frame header gave this slot, and the
        /// network it said the stream belongs to: what the network table's
        /// description is looked up by.
        said: u16,
        network: u16,
        splitter: isdb::tsmf::Splitter,
        reader: isdb::sdt::Reader,
        networks: isdb::nit::Reader,
    },
    Advanced {
        number: u8,
        splitter: isdb::tsmf::Splitter,
        reader: isdb::mh::Reader,
    },
}

impl Taking {
    /// Puts a chunk of the carrier through, keeping what belongs to this slot.
    fn feed(&mut self, arrived: &[u8]) {
        match self {
            Self::Transport {
                splitter,
                reader,
                networks,
                ..
            } => {
                let mine = splitter.feed(arrived);
                if !mine.is_empty() {
                    reader.feed(&mine);
                    networks.feed(&mine);
                }
            }
            Self::Advanced {
                splitter, reader, ..
            } => {
                let mine = splitter.feed(arrived);
                if !mine.is_empty() {
                    reader.feed(&mine);
                }
            }
        }
    }

    /// Whether the stream in the slot has said what there is to wait for: the
    /// services it carries, and what its network calls itself.
    fn settled(&self, watched: Duration) -> bool {
        match self {
            Self::Transport {
                said,
                network,
                reader,
                networks,
                ..
            } => {
                if reader.services().is_empty() {
                    // A slot too thin to send its own service table often is
                    // described by the network table instead. That description
                    // carries no names, so it stands only once the slot has had
                    // long enough to send a service table of its own.
                    return watched >= OWN_TABLE
                        && listed(networks.described(), *said, *network).is_some()
                        && network_was_read(networks);
                }
                network_was_read(networks)
            }
            // The network table alone lists services and names the network, but
            // only the service table says which stream this is, so a slot is
            // not settled until that has been read.
            Self::Advanced { reader, .. } => {
                reader.identity().is_some()
                    && !reader.state().services.is_empty()
                    && !advanced_network_name(reader.state()).is_empty()
            }
        }
    }

    fn finish(self) -> Slot {
        match self {
            Self::Transport {
                number,
                reader,
                networks,
                ..
            } => Slot {
                number,
                identity: reader.identity(),
                services: reader.services().iter().map(Named::from).collect(),
                network: network_name(&networks),
                described: networks.described().to_vec(),
            },
            Self::Advanced { number, reader, .. } => Slot {
                number,
                // The network table's listing, in the shape the transport
                // stream side keeps it: the advanced services write theirs in a
                // table of their own, but what it says is the same thing, and a
                // slot too thin to have sent its own table is named from it.
                described: advanced_listing(reader.state()),
                identity: reader.identity().map(|(stream_id, original_network_id)| {
                    isdb::sdt::Identity {
                        stream_id,
                        original_network_id,
                    }
                }),
                network: advanced_network_name(reader.state()),
                // What the stream carries rather than what the network defines:
                // the service table of the advanced services describes every
                // service channel of the network, and the package list says
                // which are here.
                services: reader.carried().into_iter().map(Named::from).collect(),
            },
        }
    }
}

/// Locks a carrier and watches it, or says nothing where it did not answer. One
/// shape for both jobs: a cable carrier is watched for its frame header, a
/// satellite transponder for the network's table, and both want the rate and
/// the signal.
fn watch(
    lease: &mut Lease,
    daemon: &Daemon,
    properties: &[(&str, String)],
    table: bool,
    how_long: Duration,
    mut tick: impl FnMut(Option<f32>, Option<f32>, f32),
) -> Result<Sample, Missed> {
    // A carrier that is refused says why, and a walk that threw that away could
    // not tell a carrier with nothing on it from a tuner someone else had.
    // Asked twice where the tuner was busy: the one before it may still have
    // been going back.
    //
    // A read of the stream waits no longer than the carrier is being held for,
    // and the daemon is given the bounds so that it ends the stream where the
    // sample ends: a body that ends can be read to its end, which is what
    // leaves the connection -- and the tuner on it -- for the next carrier.
    let patience = Some(how_long.min(PATIENCE));
    let mut opened = lease.stream(properties, Some(MOST_BYTES), Some(how_long), patience);
    if refused_as_busy(&opened) {
        std::thread::sleep(BUSY_AGAIN);
        opened = lease.stream(properties, Some(MOST_BYTES), Some(how_long), patience);
    }
    let again = refused_as_busy(&opened);
    let refusal = refused_as(&opened);
    let mut answer = match opened {
        Ok(opened) => opened,
        Err(reason) => {
            coax_settings::trace!("plugin: {reason}");
            return Err(match refusal {
                _ if again => Missed::Busy,
                Some(missed) => missed,
                None => Missed::Silent,
            });
        }
    };
    let tuner = answer.header("x-coax-tuner").map(str::to_owned);
    let mut sample = Sample::default();
    let mut survey = Survey::new();
    let mut reader = nit::Reader::new();
    let mut identity = isdb::sdt::Reader::new();
    let mut arriving = mpegts::continuity::Continuity::new();
    // The same question asked of a carrier that turns out to be TLV rather than
    // transport packets. A satellite walk cannot know which it is until it
    // hears it.
    let mut advanced = isdb::mh::Reader::new();
    // Filled in once the frame header has said which slots the carrier holds: a
    // stream joined mid-frame has no way of knowing whose packets it is looking
    // at.
    let mut taking: Vec<Taking> = Vec::new();
    let mut buffer = vec![0u8; READ_SIZE];
    let started = Instant::now();
    // Why the watch ended, for the account of a carrier that gave less than
    // expected.
    let mut left = "the time was up";
    let until = started + how_long;
    let mut read_bytes = 0usize;
    // The rate is counted from the second read on. The first comes back with
    // whatever the daemon buffered while the tuner was locking, which arrives
    // at the speed of the socket -- measured, it made a 29 Mbit cable look like
    // 293.
    let mut counting_from: Option<Instant> = None;
    let mut measured = false;
    let mut told = started;
    // When a service table last named something, for telling a network that has
    // finished naming itself from one still working through its multiplexes.
    // See NAMES_QUIET.
    let mut named_at = started;
    let mut names = 0usize;
    while Instant::now() < until {
        match answer.read_body(&mut buffer) {
            Ok(0) => {
                left = "the stream ended";
                break;
            }
            // Nothing arrived in the time a read waits, which is not the stream
            // ending: an empty carrier gives almost nothing and is held until
            // its time is up.
            Err(reason)
                if matches!(
                    reason.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => {
                left = "the stream broke";
                break;
            }
            Ok(read) => {
                match counting_from {
                    None => counting_from = Some(Instant::now()),
                    Some(_) => read_bytes += read,
                }
                arriving.feed(&buffer[..read]);
                if table {
                    reader.feed(&buffer[..read]);
                    advanced.feed(&buffer[..read]);
                } else {
                    survey.feed(&buffer[..read]);
                    // The carrier read whole, for a carrier that holds one
                    // stream. Where there is a frame, this reads the slots'
                    // tables laid over one another on the same packet
                    // identifier, and is not used.
                    identity.feed(&buffer[..read]);
                    // The network's own table, which is where it says what it
                    // is called. Nothing waits for it: it is sent every few
                    // seconds at best.
                    reader.feed(&buffer[..read]);
                    if taking.is_empty() {
                        taking = start_taking(&survey.streams());
                    }
                    for slot in &mut taking {
                        slot.feed(&buffer[..read]);
                    }
                }
            }
        }
        if table {
            let heard = reader.namings() + advanced.state().namings();
            if heard != names {
                names = heard;
                named_at = Instant::now();
            }
        }
        // The signal is worth one question, once there is a signal to ask
        // about. Asked here because a tuner that has only just locked has
        // nothing to say yet.
        if !measured && started.elapsed() >= MEASURE_AFTER {
            measured = true;
            if let Some(tuner) = &tuner {
                if let Ok(seen) = daemon.seen(tuner) {
                    sample.cnr = seen.cnr;
                    sample.strength = seen.strength;
                    sample.locked = seen.locked;
                    sample.format = seen.format;
                    sample.packets = seen.packets;
                }
            }
        }
        // A carrier the demodulator says it has not got is an empty carrier.
        // Only where the backend says so: one that cannot answer says nothing,
        // and nothing is not a no. Backed by what came off it, because a
        // demodulator that has just been asked may be answering about the tune
        // before this one.
        if measured
            && sample.locked == Some(false)
            && read_bytes == 0
            && started.elapsed() >= NOT_LOCKED
        {
            left = "the demodulator did not lock";
            break;
        }
        // What the carrier is giving, as it gives it. Showing the figures only
        // at the end leaves the line under the carrier's name empty for as long
        // as it is held.
        if measured && told.elapsed() >= TICK {
            told = Instant::now();
            tick(sample.cnr, sample.strength, rate(read_bytes, counting_from));
        }
        // The satellite walk is after the network's own table and nothing else,
        // and a table split into numbered sections arrives one section at a
        // time: stopping at the first lists three transponders of a satellite
        // carrying twelve.
        if table
            && started.elapsed() >= AT_LEAST
            && !reader.actual().is_empty()
            && reader.complete()
            && named_at.elapsed() >= NAMES_QUIET
        {
            left = "the table was whole and the naming had stopped";
            break;
        }
        // The same for a carrier that turned out to be advanced: its network
        // table is whole when it has placed the streams it named.
        if table
            && started.elapsed() >= AT_LEAST
            && named_at.elapsed() >= NAMES_QUIET
            && advanced
                .state()
                .networks
                .iter()
                .any(|network| network.streams.iter().any(|one| one.satellite.is_some()))
        {
            left = "the advanced table named the network";
            break;
        }
        // Left as soon as the carrier has said everything the sweep will write
        // down. What is worth staying for is not the frame header -- that goes
        // past in milliseconds -- but the tables inside each stream.
        if !table
            && started.elapsed() >= AT_LEAST
            && settled(&survey, &taking, &identity, &reader, started.elapsed())
        {
            left = "it had said everything";
            break;
        }
    }
    coax_settings::trace!(
        "plugin: watched {}ms, {} bytes, {} packets, {} missed, frames {}, group {:?}, slots {}, identity {:?}, services {}, left because {left}",
        started.elapsed().as_millis(),
        read_bytes,
        arriving.packets(),
        arriving.errors(),
        survey.frames(),
        survey.group(),
        taking.len(),
        identity.identity(),
        identity.services().len(),
    );
    // Asked once more at the end: the first reading was taken a third of a
    // second in, when a stream that starts slowly has nothing to show.
    if let Some(tuner) = &tuner {
        if let Ok(seen) = daemon.seen(tuner) {
            sample.cnr = seen.cnr.or(sample.cnr);
            sample.strength = seen.strength.or(sample.strength);
            sample.locked = seen.locked.or(sample.locked);
            sample.format = seen.format;
            sample.packets = seen.packets;
        }
    }
    // Read to its end where anything is left, so that the connection -- and the
    // tuner the daemon ties to it -- serves the next carrier. Bounded: waiting
    // out the rest of a sample's bound would give back what leaving early
    // saved.
    if !answer.finished() {
        let until = Instant::now() + DRAIN;
        for _ in 0..DRAIN_READS {
            if Instant::now() >= until {
                break;
            }
            match answer.read_body(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(reason)
                    if matches!(
                        reason.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    }
    lease.recover(answer);
    sample.losses = arriving.losses();
    sample.watched = started.elapsed();
    sample.bytes = read_bytes;
    sample.mbps = rate(read_bytes, counting_from);
    sample.tsmf = survey.streams();
    sample.group = survey.group();
    sample.identity = identity.identity();
    sample.services = identity.services().to_vec();
    sample.slots = taking.into_iter().map(Taking::finish).collect();
    sample.network = network_name(&reader);
    sample.transports = reader.transports();
    sample.networks = reader.actual().to_vec();
    // A carrier of the advanced services answers the same questions from a
    // table of its own. What it says goes in beside the older table's answer
    // rather than instead of it: one carrier is one or the other.
    let named = advanced.transports();
    if !named.is_empty() {
        let network = advanced.state().network_id;
        if network != 0 && !sample.networks.contains(&network) {
            sample.networks.push(network);
        }
        sample.transports.extend(named);
    }
    Ok(sample)
}

/// A splitter and a reader for each slot of a frame, of whichever kind the
/// header says.
fn start_taking(streams: &[isdb::tsmf::Relative]) -> Vec<Taking> {
    streams
        .iter()
        .map(|stream| match stream.tlv {
            false => Taking::Transport {
                number: stream.number,
                said: stream.stream_id,
                network: stream.original_network_id,
                splitter: isdb::tsmf::Splitter::new(stream.number),
                reader: isdb::sdt::Reader::new(),
                networks: isdb::nit::Reader::new(),
            },
            // The driver puts a TLV slot back together rather than handing over
            // the packets it arrived in, and so does this.
            true => Taking::Advanced {
                number: stream.number,
                splitter: isdb::tsmf::Splitter::tlv(stream.number),
                reader: isdb::mh::Reader::new(),
            },
        })
        .collect()
}

/// Whether a carrier has said everything there is to have off it: every stream
/// on it has named its services and its network. This is `coax scan`'s own rule
/// -- see its `analysisSettled`. A carrier that never says one of them is held
/// to the bound.
fn settled(
    survey: &Survey,
    slots: &[Taking],
    identity: &isdb::sdt::Reader,
    networks: &nit::Reader,
    watched: Duration,
) -> bool {
    // A frame header goes past every fifty-three packets, so a carrier that has
    // shown none by now is one stream rather than a multiplex.
    if survey.frames() < 2 {
        return identity.identity().is_some()
            && !identity.services().is_empty()
            && network_was_read(networks);
    }
    // One of several carriers of one multiplex holds a fraction of every stream
    // on it, so no stream can ever be complete from here and the header is all
    // there is to wait for: a carrier of an 8K service split three ways showed
    // its table once in 80s.
    if survey.group().is_some_and(|group| group.carriers > 1) {
        return true;
    }
    !slots.is_empty() && slots.iter().all(|slot| slot.settled(watched))
}

/// How fast the bytes are coming, in megabits a second.
fn rate(bytes: usize, since: Option<Instant>) -> f32 {
    let seconds = since
        .map(|from| from.elapsed().as_secs_f32())
        .unwrap_or_default()
        .max(0.001);
    (bytes as f32 * 8.0) / seconds / 1_000_000.0
}

/// What the daemon refused with, where it is one of the answers a walk acts on.
///
/// The refusal carries a code to branch on and what was measured before it gave
/// up -- see the daemon's `hostapi.Failure` -- so a carrier that did not lock
/// still has a level to show.
fn refused_as<T, E: std::fmt::Display>(opened: &Result<T, E>) -> Option<Missed> {
    let Err(reason) = opened else {
        return None;
    };
    let said = reason.to_string();
    let body = said.find('{').map(|at| &said[at..]);
    let answer = body.and_then(|body| coax::json::parse(body).ok());
    let code = answer
        .as_ref()
        .and_then(|answer| answer.get("error"))
        .and_then(|error| error.text("code"))
        .map(str::to_owned)
        // An answer this could not read is still worth reading the code out of:
        // a walk that shrugged at a body it could not parse would call every
        // refusal silence.
        .unwrap_or_else(|| said.clone());
    let strength = answer
        .as_ref()
        .and_then(|answer| answer.get("status"))
        .and_then(|status| status.get("strength"))
        .and_then(|value| match value {
            coax::json::Value::Number(digits) => digits.parse::<f32>().ok(),
            _ => None,
        });
    if code.contains("no_lock") {
        return Some(Missed::NoLock(strength));
    }
    if code.contains("no_sync") {
        return Some(Missed::Noise);
    }
    None
}

fn refused_as_busy<T, E: std::fmt::Display>(opened: &Result<T, E>) -> bool {
    let Err(reason) = opened else {
        return false;
    };
    let said = reason.to_string();
    said.contains("tuner_busy") || said.contains("503")
}

/// Puts what was just found together with what the file already had.
///
/// The file's lines keep their order, because their order is their numbering
/// and the numbering is what a viewer's channel file points at. A stream found
/// the way a line already receives it brings that line up to date and keeps the
/// line's name; a stream found another way is a new line straight after the
/// last line of that stream; a stream the file has not got is a new line on the
/// end.
///
/// What the sweep looked at and did not find this time is left where it is. A
/// carrier that answered on the last walk and not on this one is usually one
/// that was not listened to properly, and a line taken out on that evidence is
/// a channel a viewer loses without being told.
///
/// `swept` is which ways of receiving were looked at, and is kept for the count
/// of what stood: a walk of the cable says nothing about what a dish receives.
fn merge(
    before: &[Definition],
    now: Vec<(Definition, Quality)>,
    swept: &[(String, String)],
) -> Vec<Definition> {
    let mut merged: Vec<Definition> = before.to_vec();
    let mut refreshed: Vec<bool> = vec![false; merged.len()];
    // How well the carrier behind each line came in, for the lines this sweep
    // wrote. Kept only for the length of the sweep: it says which of two
    // carriers found minutes apart is the better.
    let mut kept: Vec<Option<Quality>> = vec![None; merged.len()];
    for (fresh, quality) in now {
        if let Some(at) = merged
            .iter()
            .position(|line| line.key() == fresh.key() && line.route() == fresh.route())
        {
            // The same stream on a second carrier of the same lead: a main
            // station and a relay of it, or a multiplex a cable passes through
            // as well as carrying. The one to keep is the one that came in
            // better -- not the one found last, which on a walk upwards is
            // whichever is highest in the band.
            if let Some(standing) = kept[at] {
                if !quality.better_than(&standing) {
                    coax_settings::trace!(
                        "plugin: {} was left on {} ({}) rather than moved to {} ({})",
                        merged[at].name,
                        merged[at].frequency(),
                        standing.said(),
                        fresh.frequency(),
                        quality.said(),
                    );
                    continue;
                }
                coax_settings::trace!(
                    "plugin: {} moved to {} ({}) from {} ({})",
                    fresh.name,
                    fresh.frequency(),
                    quality.said(),
                    merged[at].frequency(),
                    standing.said(),
                );
            }
            let name = carried_name(&merged[at], &fresh);
            merged[at] = Definition { name, ..fresh };
            refreshed[at] = true;
            kept[at] = Some(quality);
            continue;
        }
        if let Some(last) = merged.iter().rposition(|line| line.key() == fresh.key()) {
            merged.insert(last + 1, fresh);
            refreshed.insert(last + 1, true);
            kept.insert(last + 1, Some(quality));
            continue;
        }
        merged.push(fresh);
        refreshed.push(true);
        kept.push(Some(quality));
    }
    let stood = merged
        .iter()
        .zip(&refreshed)
        .filter(|(line, refreshed)| !**refreshed && swept.contains(&line.route()))
        .count();
    if stood > 0 {
        coax_settings::trace!(
            "plugin: {stood} line(s) were not found again and were left as they were"
        );
    }
    merged
}

/// Says what became of a carrier that was not read, in the words a viewer
/// reads, and counts it. One place, because the walk gives up on a carrier in
/// two: waiting for the lock, and reading what came off it.
fn report_missed(
    missed: Missed,
    what_it_is: &str,
    at: usize,
    report: &mut Report,
    progress: &mut impl FnMut(Step),
) {
    match missed {
        Missed::Busy => {
            report.busy += 1;
            progress(Step {
                what: what_it_is,
                done: at + 1,
                note: Some("チューナーが使用中".to_owned()),
                ..Step::default()
            });
            if report.answered == 0 && report.busy >= GIVE_UP {
                report.gave_up = true;
            }
        }
        // Worth a line: the demodulator locked, so the carrier is not simply
        // empty, and a viewer whose aerial is unplugged sees this on every
        // carrier of the walk.
        Missed::Noise => progress(Step {
            what: what_it_is,
            done: at + 1,
            note: Some("多重化を認識できません".to_owned()),
            ..Step::default()
        }),
        // Nothing on this carrier. The level is worth showing: it separates a
        // band nothing is broadcast on from a lead that is not plugged in.
        Missed::NoLock(strength) => progress(Step {
            what: what_it_is,
            done: at + 1,
            strength,
            note: Some("信号なし".to_owned()),
            ..Step::default()
        }),
        Missed::Silent => {}
    }
}

/// Sets a tuner to a channel and waits for the demodulator to lock, saying how
/// long it has been waiting as it goes. The daemon is asked not to wait itself:
/// it would hold the answer back for its own timeout, and the walk would have
/// nothing to show for those seconds and no way to give up sooner.
///
/// A driver that does not say whether it has locked is not waited for: the
/// wait would be the whole of [`LOCK_WAIT`] on every carrier and end in "no
/// signal" whatever was there. What comes off the carrier says instead.
fn lock_on(
    daemon: &Daemon,
    lease: &mut Lease,
    properties: &[(&str, String)],
    mut waiting: impl FnMut(Duration, Option<f32>),
) -> Result<Tuned, Missed> {
    let started = Instant::now();
    // Asked twice where the tuner was busy: the carrier before this one may
    // still have been giving it back.
    let mut asked = lease.tune(properties);
    if refused_as_busy(&asked) {
        std::thread::sleep(BUSY_AGAIN);
        asked = lease.tune(properties);
    }
    let busy = refused_as_busy(&asked);
    let mut settled = match asked {
        Ok(settled) => settled,
        Err(reason) => {
            coax_settings::trace!("plugin: {reason}");
            let refused = refused_as(&Err::<(), _>(reason));
            return Err(match refused {
                _ if busy => Missed::Busy,
                Some(missed) => missed,
                None => Missed::Silent,
            });
        }
    };
    while !settled.locked && started.elapsed() < LOCK_WAIT {
        waiting(started.elapsed(), settled.strength);
        std::thread::sleep(LOCK_ASK);
        let Some(tuner) = lease.tuner().map(str::to_owned) else {
            break;
        };
        // Asked on a connection of its own: the one holding the tuner is
        // between requests, and a reader of a status is an observer rather than
        // a second claim.
        let Ok(seen) = daemon.seen(&tuner) else {
            break;
        };
        settled.cnr = seen.cnr.or(settled.cnr);
        settled.strength = seen.strength.or(settled.strength);
        match seen.locked {
            Some(locked) => settled.locked = locked,
            None => return Ok(settled),
        }
    }
    if !settled.locked {
        return Err(Missed::NoLock(settled.strength));
    }
    Ok(settled)
}

/// Puts each stream's ways of being received in the order the space names its
/// routes.
///
/// A stream that comes down two leads has a line for each, and both the name
/// the host shows and the lead a tune reaches for first come from the first of
/// them -- see the driver's `set_channel`. Left in the order they were found, a
/// space whose settings say the aerial first would still show and tune the
/// cable.
///
/// The channels keep the order they were in: only the ways of one stream move
/// among themselves. A way whose lead the space does not name stays after the
/// ones it does.
fn in_route_order(space: &SpaceSetting, lines: Vec<Definition>) -> Vec<Definition> {
    let rank = |line: &Definition| {
        // A line that names no lead is the space's own, which is the first
        // thing it names.
        if line.tuners.is_empty() {
            return 0;
        }
        space
            .routes
            .iter()
            .position(|route| route.label == line.tuners)
            .unwrap_or(usize::MAX)
    };
    let mut groups: Vec<(u16, u16, Vec<Definition>)> = Vec::new();
    for line in lines {
        let key = line.key();
        match groups
            .iter_mut()
            .find(|(onid, tsid, _)| (*onid, *tsid) == key)
        {
            Some((_, _, ways)) => ways.push(line),
            None => groups.push((key.0, key.1, vec![line])),
        }
    }
    let mut ordered = Vec::new();
    for (_, tsid, mut ways) in groups {
        ways.sort_by_key(rank);
        // The name the host shows is the first line's, so a name a viewer wrote
        // on the line that used to be first would be lost to the sweep's own
        // name for the carrier.
        let sweeps_own = format!("/{tsid}");
        if let Some(first) = ways.first() {
            if first.name.ends_with(&sweeps_own) {
                if let Some(named) = ways
                    .iter()
                    .find(|way| !way.name.ends_with(&sweeps_own))
                    .map(|way| way.name.clone())
                {
                    ways[0].name = named;
                }
            }
        }
        ordered.extend(ways);
    }
    ordered
}

/// What to call a line that has just been found again. A name a viewer wrote is
/// theirs and is kept. A name the sweep wrote is the carrier and the stream --
/// `22/31877` -- and has to follow the carrier the line is tuned on.
fn carried_name(standing: &Definition, fresh: &Definition) -> String {
    let sweeps_own = format!("/{}", fresh.tsid);
    if standing.name.ends_with(&sweeps_own) {
        return fresh.name.clone();
    }
    standing.name.clone()
}

#[cfg(test)]
mod tests {
    use super::{Quality, Slot, elsewhere_for, identifiers, in_route_order, merge};
    use super::{disagreement, heard_enough};
    use coax_settings::config::Config;
    use coax_settings::defined::{self, Definition};
    use coax_settings::lineup;
    use isdb::nit;

    /// A network settles on two tables that agree, never on one, and never on
    /// two that disagree (August 2026: one uplink sent stale tables).
    #[test]
    fn a_network_needs_two_tables_that_agree() {
        let right = vec![(0x40f1, 1_318_460), (0x48f3, 1_318_460)];
        let stale = vec![(0x40f1, 1_318_460), (0x40f2, 1_318_460)];
        let heard = [4u16];
        let one = vec![(4u16, String::from("BS01"), right.clone())];
        assert!(
            !heard_enough(&one, &heard),
            "一つのテーブルだけでは決めない"
        );
        let two = vec![
            (4u16, String::from("BS01"), right.clone()),
            (4u16, String::from("BS03"), right.clone()),
        ];
        assert!(heard_enough(&two, &heard));
        let differ = vec![
            (4u16, String::from("BS01"), right.clone()),
            (4u16, String::from("BS15"), stale.clone()),
            (4u16, String::from("BS03"), right.clone()),
        ];
        assert!(!heard_enough(&differ, &heard), "食い違ったら全部回る");
        assert_eq!(
            disagreement(&right, &stale),
            "前のテーブルだけ: 0x48f3@1318460／このテーブルだけ: 0x40f2@1318460"
        );
    }

    fn line(written: &str) -> Definition {
        Definition::parse(written).expect("the line is one")
    }

    /// A line the sweep has just found, on a carrier nothing is known about.
    fn found(written: &str) -> (Definition, Quality) {
        (line(written), Quality::default())
    }

    /// The same, on a carrier that came in as well as this.
    fn found_at(written: &str, losses: f32) -> (Definition, Quality) {
        (
            line(written),
            Quality {
                losses: Some(losses),
                ..Quality::default()
            },
        )
    }

    /// The file's lines keep their order, which is their numbering, and a
    /// stream the file has not got goes on the end.
    #[test]
    fn lines_keep_their_order_and_a_new_stream_goes_on_the_end() {
        let before = vec![
            line("65534, 45281, C20/45281, -, ISDBC, 153000000, ts:1"),
            line("65534, 45282, C20/45282, -, ISDBC, 153000000, ts:2"),
        ];
        let merged = merge(
            &before,
            vec![
                found("65534, 45283, C21/45283, -, ISDBC, 159000000, ts:1"),
                found("65534, 45281, C20/45281, -, ISDBC, 153000000, ts:1"),
                found("65534, 45282, C20/45282, -, ISDBC, 153000000, ts:2"),
            ],
            &[(String::new(), "ISDBC".to_owned())],
        );
        let names: Vec<&str> = merged.iter().map(|line| line.name.as_str()).collect();
        assert_eq!(names, vec!["C20/45281", "C20/45282", "C21/45283"]);
    }

    /// A stream found the way a line already receives it brings the line up to
    /// date: a cable operator moves their carriers about, and a channel that
    /// has moved is the same channel. The name the sweep wrote follows the
    /// carrier.
    #[test]
    fn a_stream_found_the_way_a_line_receives_it_brings_the_line_up_to_date() {
        let before = vec![line("65534, 45281, C20/45281, -, ISDBC, 153000000, ts:1")];
        let merged = merge(
            &before,
            vec![found("65534, 45281, C21/45281, -, ISDBC, 159000000, ts:3")],
            &[(String::new(), "ISDBC".to_owned())],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "C21/45281");
        assert_eq!(merged[0].frequency(), "159000000");
        assert_eq!(merged[0].slot, defined::Slot::Transport(3));
    }

    /// The same stream found another way -- the BS a dish receives, found again
    /// transmodulated onto a cable -- is another line straight after its own.
    #[test]
    fn a_stream_found_another_way_is_a_new_line_after_its_own() {
        let before = vec![
            line("4, 16625, BS15/16625, -, ISDBS, 1049480, -"),
            line("4, 16626, BS15/16626, -, ISDBS, 1049480, -"),
        ];
        let over_the_cable = line("4, 16625, C23/16625, cable, ISDBC, 471142857, ts:2");
        // Only the cable was looked at.
        let swept = [("cable".to_owned(), "ISDBC".to_owned())];
        let merged = merge(
            &before,
            vec![(over_the_cable.clone(), Quality::default())],
            &swept,
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].key(), (4, 16625));
        assert_eq!(merged[1], over_the_cable);
        assert_eq!(merged[2].key(), (4, 16626));
        // Looked at again with the carrier moved, the cable's line is brought
        // up to date.
        let moved = line("4, 16625, C24/16625, cable, ISDBC, 477142857, ts:2");
        let again = merge(&merged, vec![(moved, Quality::default())], &swept);
        assert_eq!(again.len(), 3);
        assert_eq!(again[1].name, "C24/16625");
        assert_eq!(again[1].frequency(), "477142857");
    }

    /// A name a viewer wrote is theirs. Only the name the sweep itself wrote
    /// follows the carrier.
    #[test]
    fn a_name_a_viewer_wrote_is_kept_when_the_line_is_found_again() {
        let before = vec![line(
            "65534, 45281, ケーブルテレビ, -, ISDBC, 153000000, ts:1",
        )];
        let merged = merge(
            &before,
            vec![found("65534, 45281, C21/45281, -, ISDBC, 159000000, ts:3")],
            &[(String::new(), "ISDBC".to_owned())],
        );
        assert_eq!(merged[0].name, "ケーブルテレビ");
        assert_eq!(merged[0].frequency(), "159000000");
    }

    /// One stream on two carriers of the same lead is one line, and the line is
    /// the carrier that came in better. Not the one found last: the walk goes
    /// up the band.
    #[test]
    fn the_better_of_two_carriers_of_one_stream_is_the_one_kept() {
        let swept = [(String::new(), "ISDBT".to_owned())];
        let strong = "31877, 31877, 22/31877, -, ISDBT, 527142857, -";
        let weak = "31877, 31877, 42/31877, -, ISDBT, 647142857, -";
        // Found low first, as a walk up the band finds them.
        let merged = merge(
            &[],
            vec![found_at(strong, 0.0), found_at(weak, 0.02)],
            &swept,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "22/31877");
        assert_eq!(merged[0].frequency(), "527142857");
        // And the same the other way about, so it is the signal deciding, not
        // the order.
        let merged = merge(
            &[],
            vec![found_at(weak, 0.02), found_at(strong, 0.0)],
            &swept,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "22/31877");
        assert_eq!(merged[0].frequency(), "527142857");
    }

    /// Where nothing can be said about either carrier, the first one found
    /// stands, rather than the file changing under a viewer on every sweep.
    #[test]
    fn two_carriers_that_came_in_alike_leave_the_line_where_it_was() {
        let swept = [(String::new(), "ISDBT".to_owned())];
        let merged = merge(
            &[],
            vec![
                found("31877, 31877, 22/31877, -, ISDBT, 527142857, -"),
                found("31877, 31877, 42/31877, -, ISDBT, 647142857, -"),
            ],
            &swept,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].frequency(), "527142857");
    }

    /// A line nothing found again stands. A carrier missed once is not evidence
    /// that the channel has gone, and a viewer who loses a channel to that has
    /// no way of telling what happened.
    #[test]
    fn a_line_that_was_not_found_again_is_left_where_it_is() {
        let before = vec![
            line("4, 16625, BS15/16625, -, ISDBS, 1049480, -"),
            line("4, 16625, C23/16625, cable, ISDBC, 471142857, ts:2"),
            line("4, 16626, BS15/16626, -, ISDBS, 1049480, -"),
        ];
        let swept = [("cable".to_owned(), "ISDBC".to_owned())];
        let merged = merge(&before, Vec::new(), &swept);
        assert_eq!(
            merged.len(),
            3,
            "a walk that found nothing takes nothing away"
        );
        assert_eq!(merged, before);
    }

    /// A line the sweep writes reads back as the tune it was written from,
    /// through the same rules the driver tunes by.
    #[test]
    fn a_line_the_sweep_writes_is_what_the_driver_tunes() {
        let written = lineup::line_of(
            4,
            16625,
            "C23/16625",
            "cable",
            &[
                ("DELIVERY_SYSTEM".to_owned(), "DVBC/ANNEX_A".to_owned()),
                ("FREQUENCY".to_owned(), "471142857".to_owned()),
                ("SYMBOL_RATE".to_owned(), "5274000".to_owned()),
                ("MODULATION".to_owned(), "QAM/AUTO".to_owned()),
                ("TSMF_REL_TS".to_owned(), "2".to_owned()),
            ],
            &[],
        );
        assert_eq!(
            written.render(),
            "4, 16625, C23/16625, cable, ISDBC, 471142857, ts:2"
        );
    }

    const CABLE_AND_THE_PLATFORMS_ON_IT: &str = "
[Tuners]
cable = ISDBC, tbs:*
passthru = ISDBT, tbs:*

[Spaces]
0 = CATV, cable
1 = BS, cable
2 = 地デジ, passthru, group=GR
";

    /// What a cable carries for another platform goes to that platform's space,
    /// and what is left in the cable's own is the operator's own carriage. The
    /// network identifier says which, whichever slot of the frame the stream
    /// came out of.
    #[test]
    fn a_stream_goes_to_the_space_its_network_names() {
        let config = Config::parse(CABLE_AND_THE_PLATFORMS_ON_IT);
        let goes_to = |onid| elsewhere_for(&config, Some("CATV"), "cable", onid);
        // BS transmodulated onto the cable is still BS, and so is the advanced
        // service.
        assert_eq!(goes_to(4).as_deref(), Some("BS"));
        assert_eq!(goes_to(0x7fe0).as_deref(), Some("地デジ"));
        // The operator's own carriage stays where it was found, and so does a
        // network nobody recognises.
        assert_eq!(goes_to(0xfffe), None);
        assert_eq!(goes_to(0x1234), None);
        // A stream that said nothing about its network stays too.
        assert_eq!(goes_to(0), None);
    }

    fn relative(number: u8, stream_id: u16, original_network_id: u16) -> isdb::tsmf::Relative {
        isdb::tsmf::Relative {
            number,
            stream_id,
            original_network_id,
            tlv: false,
            slots: 26,
        }
    }

    /// A stream that comes down two leads is tuned and shown by whichever way
    /// the space names first, whatever order they were found in.
    #[test]
    fn a_streams_ways_follow_the_order_the_space_names_them() {
        let config = Config::parse(
            "[Tuners]\nisdbc = ISDBC, tbs:*\nisdbt = ISDBT, us:*\n\
             [Spaces]\n0 = 地デジ, isdbt, isdbc\n",
        );
        let space = config
            .spaces()
            .into_iter()
            .find(|setting| setting.name == "地デジ")
            .expect("the space is in the settings");
        let way = |name: &str, route: &str, system: &str, frequency: &str| Definition {
            onid: 31795,
            tsid: 31795,
            name: name.to_owned(),
            tuners: route.to_owned(),
            system: system.to_owned(),
            carriers: vec![frequency.to_owned()],
            slot: defined::Slot::Whole,
            extras: Vec::new(),
        };
        let other = Definition {
            onid: 31796,
            ..way("18/31796", "isdbt", "ISDBT", "503142857")
        };
        let lines = vec![
            way("C16/31795", "isdbc", "ISDBC", "129000000"),
            way("26/31795", "isdbt", "ISDBT", "551142857"),
            other.clone(),
        ];
        let ordered = in_route_order(&space, lines);
        assert_eq!(ordered[0].tuners, "isdbt", "the aerial is named first");
        assert_eq!(ordered[1].tuners, "isdbc");
        // The channels keep the order they were in.
        assert_eq!(ordered[2], other);
    }

    /// A name a viewer wrote is what the host shows, whichever way of receiving
    /// ends up first.
    #[test]
    fn a_name_a_viewer_wrote_moves_with_the_order() {
        let config = Config::parse(
            "[Tuners]\nisdbc = ISDBC, tbs:*\nisdbt = ISDBT, us:*\n\
             [Spaces]\n0 = 地デジ, isdbt, isdbc\n",
        );
        let space = config.spaces().into_iter().next().expect("one space");
        let way = |name: &str, route: &str| Definition {
            onid: 31795,
            tsid: 31795,
            name: name.to_owned(),
            tuners: route.to_owned(),
            system: "ISDBT".to_owned(),
            carriers: vec!["551142857".to_owned()],
            slot: defined::Slot::Whole,
            extras: Vec::new(),
        };
        let ordered = in_route_order(
            &space,
            vec![way("わたしのチャンネル", "isdbc"), way("26/31795", "isdbt")],
        );
        assert_eq!(ordered[0].tuners, "isdbt");
        assert_eq!(ordered[0].name, "わたしのチャンネル");
    }

    /// A satellite delivery system descriptor as a Japanese network writes one:
    /// right-hand circular, and the downlink the block converts.
    fn satellite(downlink_khz: u32, symbol_rate: u32) -> isdb::nit::Satellite {
        isdb::nit::Satellite {
            downlink_khz,
            orbital_position: 1100,
            east: true,
            polarisation: 'R',
            modulation: 0,
            symbol_rate,
            inner_correction: 0,
        }
    }

    /// A daemon that is only a socket: a tune is answered with no lock yet,
    /// and a question about the tuner with `seen`.
    fn answering(seen: &'static str) -> coax::daemon::Daemon {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port is free");
        let address = listener
            .local_addr()
            .expect("the port is known")
            .to_string();
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    break;
                };
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    match connection.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => request.push(byte[0]),
                    }
                }
                let body = if request.starts_with(b"GET /tune?") {
                    r#"{"tuner":"t0","locked":false}"#
                } else {
                    seen
                };
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = connection.write_all(answer.as_bytes());
            }
        });
        coax::daemon::Daemon::new(coax::daemon::Endpoint::Network(address))
    }

    /// A driver that does not say whether it has locked is not waited for:
    /// waiting would be the whole wait on every carrier, ending in "no signal"
    /// whatever was there. One that says it has locked is taken at its word.
    #[test]
    fn a_driver_that_does_not_say_whether_it_locked_is_not_waited_for() {
        let asked = [("DELIVERY_SYSTEM", "ISDBT".to_owned())];
        let daemon = answering(r#"{"cnr":21.5}"#);
        let mut lease = daemon.hold(Some("t0"));
        let started = std::time::Instant::now();
        let Ok(settled) = super::lock_on(&daemon, &mut lease, &asked, |_, _| {}) else {
            panic!("a tuner that said nothing is not a carrier with nothing on it");
        };
        assert!(started.elapsed() < super::LOCK_WAIT);
        assert_eq!(settled.cnr, Some(21.5));
        let daemon = answering(r#"{"locked":true}"#);
        let mut lease = daemon.hold(Some("t0"));
        let Ok(settled) = super::lock_on(&daemon, &mut lease, &asked, |_, _| {}) else {
            panic!("a tuner that locked is not refused");
        };
        assert!(settled.locked);
    }

    /// ISDB-S is walked over the right hand of BS and CS, and ISDB-S3 over both
    /// hands of BS and the left hand of CS, with one network on each: waiting
    /// for a second CS network that ISDB-S3 does not have walks every
    /// transponder of it.
    #[test]
    fn each_system_walks_only_the_transponders_it_is_sent_on() {
        let isdbs = super::satellite_plan("ISDBS");
        assert_eq!(isdbs.len(), 24);
        assert!(isdbs.iter().all(|transponder| transponder.right_hand));
        assert_eq!(isdbs[0].name, "BS01");
        let s3 = super::satellite_plan("ISDB-S3");
        assert_eq!(s3.len(), 36);
        assert_eq!(
            s3.iter().filter(|transponder| transponder.is_bs()).count(),
            24
        );
        assert!(
            s3.iter()
                .filter(|transponder| !transponder.is_bs())
                .all(|transponder| !transponder.right_hand)
        );
        assert_eq!(super::satellite_passes("ISDBS"), [(true, 1), (false, 2)]);
        assert_eq!(super::satellite_passes("ISDBS3"), [(true, 1), (false, 1)]);
        assert_eq!(super::satellite_count("ISDBS3"), 36);
    }

    /// The walk names a tuner from its first carrier, even where the route
    /// names none: a setting has to know which tuner it goes to.
    #[test]
    fn a_walk_names_its_tuner_from_the_start() {
        let tuner = |id: &str, deliveries: &[&str], leased: bool| coax::daemon::Tuner {
            id: id.to_owned(),
            deliveries: deliveries.iter().map(|one| (*one).to_owned()).collect(),
            leased,
            ..coax::daemon::Tuner::default()
        };
        let tuners = [
            tuner("pt3:a:0", &["ISDB-S"], true),
            tuner("pt3:a:1", &["ISDB-S"], false),
            tuner("pt3:a:2", &["ISDB-T"], false),
        ];
        assert_eq!(
            super::tuner_for(None, &tuners, &[], "ISDBS").as_deref(),
            Some("pt3:a:1")
        );
        assert_eq!(
            super::tuner_for(None, &tuners, &[], "ISDBT").as_deref(),
            Some("pt3:a:2")
        );
        assert_eq!(super::tuner_for(None, &[], &[], "ISDBT"), None);
        assert_eq!(
            super::tuner_for(Some("chosen"), &tuners, &[], "ISDBT").as_deref(),
            Some("chosen")
        );
    }

    /// A driver that takes a stream's identifier is asked for each multiplex by
    /// it. One that names a stream by its place on the transponder is asked
    /// every place of every transponder the tables named, and the multiplex
    /// that answers says which it is.
    #[test]
    fn a_driver_that_names_streams_by_place_is_asked_every_place() {
        let on_bs1 = |stream_id: u16, original_network_id: u16| nit::Transport {
            stream_id,
            original_network_id,
            satellite: Some(satellite(1_049_480 + 10_678_000, 28_860_000)),
            ..nit::Transport::default()
        };
        let named = vec![on_bs1(16400, 4), on_bs1(16401, 4), on_bs1(24608, 6)];
        let heard = [4u16];
        let by_id = super::confirmations(&named, &heard, None);
        let streams: Vec<&str> = by_id.iter().map(|one| one.stream.as_str()).collect();
        assert_eq!(streams, ["16400", "16401"], "only a network that spoke");
        assert_eq!(by_id[1].expected, Some(1));
        assert_eq!(by_id[0].khz, 1_049_480);
        let by_place = super::confirmations(&named, &heard, Some(3));
        let streams: Vec<&str> = by_place.iter().map(|one| one.stream.as_str()).collect();
        assert_eq!(streams, ["0", "1", "2"], "one transponder, three places");
        assert!(by_place.iter().all(|one| one.expected.is_none()));
        let identity = |stream_id: u16, original_network_id: u16| {
            Some(isdb::sdt::Identity {
                stream_id,
                original_network_id,
            })
        };
        assert_eq!(
            super::heard_as(&by_place[1], identity(16401, 4), &named, &heard),
            Some(1)
        );
        // Nothing the tables named, a network that did not speak, and silence.
        assert_eq!(
            super::heard_as(&by_place[2], identity(9999, 4), &named, &heard),
            None
        );
        assert_eq!(
            super::heard_as(&by_place[2], identity(24608, 6), &named, &heard),
            None
        );
        assert_eq!(super::heard_as(&by_place[0], None, &named, &heard), None);
        // Asked by identifier, only that identifier will do.
        assert_eq!(
            super::heard_as(&by_id[0], identity(16400, 4), &named, &heard),
            Some(0)
        );
        assert_eq!(
            super::heard_as(&by_id[0], identity(16401, 4), &named, &heard),
            None
        );
    }

    /// A multiplex heard by its place is written with that place, which is how
    /// the driver will be asked for it; one heard by identifier with that.
    #[test]
    fn a_stream_heard_by_place_is_written_with_its_place() {
        let config = Config::parse("[Tuners]\nisdbs = ISDBS, sat\n[Spaces]\n0 = BS, isdbs\n");
        let transport = nit::Transport {
            stream_id: 16400,
            original_network_id: 4,
            satellite: Some(satellite(1_049_480 + 10_678_000, 28_860_000)),
            ..nit::Transport::default()
        };
        let route = config.tuners["isdbs"].clone();
        let space = &config.spaces()[0];
        let mut report = super::Report::default();
        let found = super::for_space(
            space,
            &[(transport.clone(), Some("1".to_owned()))],
            &route,
            &mut report,
        );
        assert!(
            found[0]
                .tune
                .contains(&("STREAM_ID".to_owned(), "1".to_owned()))
        );
        let found = super::for_space(space, &[(transport, None)], &route, &mut report);
        assert!(
            found[0]
                .tune
                .contains(&("STREAM_ID".to_owned(), "16400".to_owned()))
        );
    }

    /// Where two locks describe the same transponder, the one that says more is
    /// kept: a neighbour's table lists a transponder with nothing on it, and
    /// the network's own says what its services are.
    #[test]
    fn the_fuller_account_of_a_transponder_is_the_one_kept() {
        let listed = nit::Transport {
            stream_id: 28736,
            original_network_id: 7,
            ..nit::Transport::default()
        };
        let numbered = nit::Transport {
            services: vec![nit::ListedService {
                id: 901,
                kind: 0x01,
                ..nit::ListedService::default()
            }],
            ..listed.clone()
        };
        let told = nit::Transport {
            services: vec![nit::ListedService {
                id: 901,
                kind: 0x01,
                name: "サンプル衛星A".to_owned(),
                lossy: false,
            }],
            ..listed.clone()
        };
        assert!(super::account(&numbered) > super::account(&listed));
        assert!(super::account(&told) > super::account(&numbered));
        assert!(super::account(&listed) == super::account(&listed));
    }

    /// The 110 degree east CS carries two networks, and each one's table lists
    /// the other's transponders with no services. Taking that as an account of
    /// them stops the walk from ever tuning the second network.
    #[test]
    fn a_transponder_is_spoken_for_only_by_a_network_that_has_spoken() {
        let where_it_is = super::plan::satellite()
            .into_iter()
            .find(|transponder| transponder.name == "ND04")
            .expect("the plan has ND04");
        // The entry as the neighbouring network's table writes it: placed, and
        // carrying nothing it will say.
        let listed = nit::Transport {
            stream_id: 28736,
            original_network_id: 7,
            satellite: Some(satellite(
                where_it_is.frequency_khz + 10_678_000,
                28_860_000,
            )),
            ..nit::Transport::default()
        };
        let transports = [listed];
        // Network 6 has spoken; 7 has not, so its transponder is still to be
        // tuned.
        assert!(!super::spoken_for(&transports, &[6], &where_it_is));
        // Once it has, there is nothing left to ask it.
        assert!(super::spoken_for(&transports, &[6, 7], &where_it_is));
    }

    /// A carrier is headed the way `coax scan` names it; the channel's own name
    /// stays what the lines say.
    #[test]
    fn a_carrier_is_headed_by_its_system_and_number() {
        assert_eq!(super::carrier_name("ISDBT", "13"), "ISDB-T 13ch");
        assert_eq!(super::carrier_name("DVBC/ANNEX_A", "C13"), "ISDB-C C13ch");
        assert_eq!(
            super::carrier_name("J83B_5_6", "73.75"),
            "J.83B (5.6 MBaud) 73.75MHz"
        );
        assert_eq!(
            super::carrier_name("DVBS2", "JCSAT3A/JD1"),
            "DVB-S2 JCSAT3A/JD1"
        );
        assert_eq!(super::carrier_name("ISDBS", "BS01"), "ISDB-S BS01");
        assert_eq!(super::carrier_name("ISDBS3", "BS01"), "ISDB-S3 BS01");
    }

    /// A transponder the network table listed, written the way the account of a
    /// satellite bootstrap writes it: the count on the transponder's line, and
    /// a line under it for each service a service table has named. The ones
    /// nothing has named yet are left off rather than written out as unknowns.
    #[test]
    fn a_listed_transponder_shows_the_names_it_has() {
        let services = [
            isdb::nit::ListedService {
                id: 191,
                kind: 0x01,
                name: "サンプル衛星1".to_owned(),
                lossy: false,
            },
            isdb::nit::ListedService {
                id: 791,
                kind: 0xc0,
                ..isdb::nit::ListedService::default()
            },
        ];

        let named: Vec<super::Named> = services
            .iter()
            .filter(|service| !service.name.is_empty())
            .map(super::Named::from)
            .collect();
        let lines = super::service_lines(&named);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("191"), "{lines:?}");
        assert!(lines[0].contains("サンプル衛星1"), "{lines:?}");
    }

    /// The identifiers and kinds of a listing, for comparing with what a test
    /// wrote down.
    fn ids(services: &[nit::ListedService]) -> Vec<(u16, u8)> {
        services
            .iter()
            .map(|service| (service.id, service.kind))
            .collect()
    }

    /// One entry of a network table: which stream, and what it carries.
    fn described(stream_id: u16, services: Vec<(u16, u8)>) -> nit::Transport {
        let services = services
            .into_iter()
            .map(|(id, kind)| nit::ListedService {
                id,
                kind,
                ..nit::ListedService::default()
            })
            .collect();
        nit::Transport {
            stream_id,
            original_network_id: 4,
            // The table named it without saying where it is, which a
            // terrestrial or cable network does: a receiver of one already
            // knows.
            satellite: None,
            services,
            name: String::new(),
            remote_control_key_id: 0,
            partial_reception: Vec::new(),
            channel_bonding: Vec::new(),
            system: None,
            links: Vec::new(),
            emergency: Vec::new(),
            connected: None,
            service_groups: Vec::new(),
            area_broadcasting: Vec::new(),
            cable: None,
            terrestrial: None,
            ..isdb::nit::Transport::default()
        }
    }

    fn slot(number: u8, stream_id: u16, original_network_id: u16) -> Slot {
        Slot {
            number,
            identity: Some(isdb::sdt::Identity {
                stream_id,
                original_network_id,
            }),
            services: Vec::new(),
            network: String::new(),
            described: Vec::new(),
        }
    }

    /// What a stream says of itself is what it is. A cable that re-multiplexes
    /// terrestrial television writes its own identifier over the slot the
    /// stream sits in: taking that word would put the channel in the cable's
    /// own space.
    #[test]
    fn a_slot_is_what_the_stream_in_it_says_it_is() {
        let stream = relative(1, 1, 0xfffe);
        assert_eq!(
            identifiers(&stream, Some(&slot(1, 16625, 0x7fe0))),
            (16625, 0x7fe0)
        );
    }

    /// A slot whose stream said nothing -- a thin one, whose service table did
    /// not arrive in time -- is still the header's to name.
    #[test]
    fn a_slot_that_said_nothing_of_itself_is_the_headers_to_name() {
        let stream = relative(2, 16626, 4);
        assert_eq!(identifiers(&stream, None), (16626, 4));
        // The same where the stream was read but named neither.
        let mut quiet = slot(2, 0, 0);
        quiet.identity = None;
        assert_eq!(identifiers(&stream, Some(&quiet)), (16626, 4));
    }

    /// A slot too thin to send its own service table is still described by the
    /// network table -- which arrives on a slot that is not thin, so it is
    /// looked for across the whole frame.
    #[test]
    fn a_slot_that_named_no_services_is_described_by_the_network_table() {
        let mut carrying = slot(1, 16625, 4);
        carrying.described = vec![
            described(16625, vec![(101, 0x01)]),
            described(16663, vec![(98, 0xa4)]),
        ];
        let thin = slot(9, 16663, 4);
        let slots = [carrying, thin];
        let found = super::listed_across(&slots, 16663, 4).expect("the table described it");
        assert_eq!(ids(&found.services), vec![(98, 0xa4)]);
        // Its own slot said nothing, so nothing is found by looking there.
        assert!(super::listed(&slots[1].described, 16663, 4).is_none());
        // A stream no table described stays undescribed.
        assert!(super::listed_across(&slots, 16626, 4).is_none());
    }

    /// A cable carries several networks' tables at once, and each numbers its
    /// streams for itself. A description belongs to the stream of the network
    /// that named it, and a stream whose network said nothing takes whichever
    /// description names the number.
    #[test]
    fn a_description_belongs_to_the_network_that_named_it() {
        let mut carrying = slot(1, 16625, 4);
        let mut operators = described(16625, vec![(24, 0x01)]);
        operators.original_network_id = 0xfffe;
        carrying.described = vec![operators, described(16625, vec![(101, 0x01)])];
        let slots = [carrying];
        assert_eq!(
            ids(&super::listed_across(&slots, 16625, 4)
                .expect("the platform described it")
                .services),
            vec![(101, 0x01)]
        );
        assert_eq!(
            ids(&super::listed_across(&slots, 16625, 0xfffe)
                .expect("the operator described it")
                .services),
            vec![(24, 0x01)]
        );
        // A network nobody named takes the first description of the stream.
        assert_eq!(
            ids(&super::listed_across(&slots, 16625, 0)
                .expect("something described it")
                .services),
            vec![(24, 0x01)]
        );
        // And one that named a network no table describes takes none.
        assert!(super::listed_across(&slots, 16625, 0x7fe0).is_none());
    }

    /// A header that left the identifiers at zero and a stream that said
    /// nothing leave nothing to go on, which is not the same as zero being an
    /// answer.
    #[test]
    fn a_slot_nobody_named_is_nameless() {
        assert_eq!(identifiers(&relative(3, 0, 0), None), (0, 0));
    }

    /// A space that did not say which platform it holds still says what it
    /// receives, and that is enough for the one platform whose delivery system
    /// means one thing.
    #[test]
    fn a_space_that_did_not_say_what_it_holds_still_takes_what_it_receives() {
        let config = Config::parse(
            "
[Tuners]
passthru = ISDBT, tbs:*
cable = ISDBC, tbs:*

[Spaces]
0 = 地デジ, passthru
1 = CATV, cable
",
        );
        // 31807 is a terrestrial network, transmodulated onto the cable.
        assert_eq!(
            elsewhere_for(&config, Some("CATV"), "cable", 31807).as_deref(),
            Some("地デジ")
        );
    }

    /// A card that says it does not demodulate the advanced services is not
    /// asked to: the walk would be forty-eight transponders of waiting for a
    /// lock that cannot come. One that says nothing about itself is asked.
    #[test]
    fn a_walk_nothing_can_receive_is_not_walked() {
        let tuner = |id: &str, deliveries: &[&str]| coax::daemon::Tuner {
            id: id.to_owned(),
            model: String::new(),
            deliveries: deliveries.iter().map(|one| (*one).to_owned()).collect(),
            leased: false,
            name: String::new(),
        };
        let patterns = vec!["pt3:*".to_owned()];
        let says_isdbs = [tuner("pt3:0:0", &["ISDB-S"])];
        assert!(super::receivable(&says_isdbs, &patterns, "ISDBS"));
        assert!(!super::receivable(&says_isdbs, &patterns, "ISDBS3"));
        // A tuner that named nothing is asked.
        let says_nothing = [tuner("pt3:0:0", &[])];
        assert!(super::receivable(&says_nothing, &patterns, "ISDBS3"));
        // And so is a pattern the daemon listed no tuner for.
        let elsewhere = [tuner("tbs:abc:0", &["ISDB-T"])];
        assert!(super::receivable(&elsewhere, &patterns, "ISDBS3"));
    }

    /// The satellite plan is walked once per route: the advanced services are
    /// the same plan demodulated another way, and a cable that passes a few
    /// transponders through is the same plan on another lead. The progress a
    /// viewer watches has to say so.
    #[test]
    fn a_file_with_two_satellite_routes_walks_the_satellite_plan_twice() {
        let one =
            Config::parse("[Tuners]\nisdbs = ISDBS, sat\n[Spaces]\n0 = BS, isdbs\n1 = CS, isdbs\n");
        let two = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\nisdbs3 = ISDBS3, sat\n\
             [Spaces]\n0 = BS, isdbs\n1 = CS, isdbs\n2 = BS4K, isdbs3\n",
        );
        let passthru = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\nisdbs_passthru = ISDBS, tbs:*\n\
             [Spaces]\n0 = BS, isdbs, isdbs_passthru\n1 = CS, isdbs, isdbs_passthru\n",
        );
        let asked = super::how_much(&one, &super::What::Everything);
        let asked_twice = super::how_much(&two, &super::What::Everything);
        let asked_on_two_leads = super::how_much(&passthru, &super::What::Everything);
        // ISDB-S is the right hand of BS and CS; ISDB-S3 both hands of BS and
        // the left hand of CS.
        assert_eq!(asked, 24);
        assert_eq!(asked_twice, 24 + 36);
        assert_eq!(asked_on_two_leads, asked * 2);
    }

    /// Every satellite route is walked, each once, however many spaces are cut
    /// out of it.
    #[test]
    fn the_satellite_routes_are_each_walked_once() {
        let config = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\nisdbs_passthru = ISDBS, tbs:*\nisdbs3 = ISDBS3, sat\n\
             cable = ISDBC, tbs:*\n\
             [Spaces]\n0 = BS, isdbs, isdbs_passthru\n1 = CS, isdbs, isdbs_passthru\n\
             2 = BS4K, isdbs3, cable\n",
        );
        let labels: Vec<String> = super::satellite_routes(&config, &super::What::Everything)
            .into_iter()
            .map(|route| route.label)
            .collect();
        assert_eq!(labels, ["isdbs", "isdbs_passthru", "isdbs3"]);
    }

    /// What an advanced network said about its own streams becomes lines of the
    /// advanced space. The network identifier is what sends them there: 0x000b
    /// is the advanced services and nothing else.
    #[test]
    fn what_an_advanced_network_named_becomes_lines_of_the_advanced_space() {
        let config = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\nisdbs3 = ISDBS3, sat\n\
             [Spaces]\n0 = BS, isdbs\n1 = BS4K, isdbs3\n",
        );
        let named = vec![
            // One advanced stream, as the TLV network table places it.
            isdb::nit::Transport {
                stream_id: 45168,
                original_network_id: 0x000b,
                // The downlink as the table writes it. BS-13 right-hand, which
                // the block brings down to 1_164_560 kHz.
                satellite: Some(satellite(1_164_560 + 10_678_000, 33_756_100)),
                services: vec![isdb::nit::ListedService {
                    id: 101,
                    kind: 0x01,
                    ..isdb::nit::ListedService::default()
                }],
                name: String::new(),
                remote_control_key_id: 0,
                partial_reception: Vec::new(),
                channel_bonding: Vec::new(),
                system: None,
                links: Vec::new(),
                emergency: Vec::new(),
                connected: None,
                service_groups: Vec::new(),
                area_broadcasting: Vec::new(),
                cable: None,
                terrestrial: None,
                ..isdb::nit::Transport::default()
            },
            // And one of the older services, which belongs elsewhere.
            isdb::nit::Transport {
                stream_id: 16400,
                original_network_id: 4,
                // BS-1, where every Japanese installation starts.
                satellite: Some(satellite(1_049_480 + 10_678_000, 28_860_000)),
                services: vec![isdb::nit::ListedService {
                    id: 101,
                    kind: 0x01,
                    ..isdb::nit::ListedService::default()
                }],
                name: String::new(),
                remote_control_key_id: 0,
                partial_reception: Vec::new(),
                channel_bonding: Vec::new(),
                system: None,
                links: Vec::new(),
                emergency: Vec::new(),
                connected: None,
                service_groups: Vec::new(),
                area_broadcasting: Vec::new(),
                cable: None,
                terrestrial: None,
                ..isdb::nit::Transport::default()
            },
        ];
        let spaces = config.spaces();
        let advanced = spaces
            .iter()
            .find(|space| space.name == "BS4K")
            .expect("the advanced space is there");
        let mut report = super::Report::default();
        let route = coax_settings::config::Route {
            label: "isdbs3".to_owned(),
            system: "ISDBS3".to_owned(),
            tuners: vec!["sat".to_owned()],
            bands: Vec::new(),
        };
        let named: Vec<super::Confirmed> = named
            .into_iter()
            .map(|transport| (transport, None))
            .collect();
        let found = super::for_space(advanced, &named, &route, &mut report);
        assert_eq!(found.len(), 1, "only the advanced network's own stream");
        assert_eq!(found[0].tsid, 45168);
        assert_eq!(found[0].onid, 0x000b);
        // Received the way the walk looked: the row says which route.
        assert_eq!(found[0].tuners, "isdbs3");
        assert!(
            found[0]
                .tune
                .contains(&("DELIVERY_SYSTEM".to_owned(), "ISDBS3".to_owned()))
        );
        assert!(
            found[0]
                .tune
                .contains(&("STREAM_ID".to_owned(), "45168".to_owned()))
        );
        assert!(
            found[0]
                .tune
                .contains(&("FREQUENCY".to_owned(), "1164560".to_owned()))
        );
    }

    /// The guess is only made where the delivery system means one platform. A
    /// satellite space receives BS, CS and the advanced services alike.
    #[test]
    fn a_space_that_receives_several_platforms_is_not_guessed_at() {
        let config = Config::parse(
            "
[Tuners]
isdbs = ISDBS, sat
cable = ISDBC, tbs:*

[Spaces]
0 = 衛星, isdbs
1 = CATV, cable
",
        );
        assert_eq!(elsewhere_for(&config, Some("CATV"), "cable", 4), None);
    }

    /// Two spaces may hold the same platform on different leads. What was found
    /// on the cable is tuned as a cable channel, so it belongs in the space the
    /// cable's tuner receives: sending it to the aerial one would give a viewer
    /// a channel their aerial tuner cannot tune.
    #[test]
    fn a_platform_on_two_leads_takes_the_one_it_was_found_on() {
        let config = Config::parse(
            "
[Tuners]
isdbt = ISDBT, pt3:*
cable = ISDBC, tbs:*
passthru = ISDBT, tbs:*

[Spaces]
0 = 地デジ, isdbt, group=GR
1 = CATV, cable
2 = 地デジ(CATV), cable, passthru, group=GR
",
        );
        assert_eq!(
            elsewhere_for(&config, Some("CATV"), "cable", 0x7fe0).as_deref(),
            Some("地デジ(CATV)")
        );
    }

    /// A cable that carries BS transmodulated is a route of the BS space, and
    /// the BS space is where what it carries for BS goes. Without a space of
    /// its own, the cable's own carriage has no home and is left out.
    #[test]
    fn a_cable_named_by_a_space_of_another_platform_feeds_that_space() {
        let bs_only = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\ncable = ISDBC, tbs:*\n\
             [Spaces]\n0 = BS, isdbs, cable\n",
        );
        let cable = &bs_only.tuners["cable"];
        assert_eq!(super::home_of(&bs_only, cable), None);
        assert_eq!(
            elsewhere_for(&bs_only, None, "cable", 4).as_deref(),
            Some("BS")
        );
        assert_eq!(elsewhere_for(&bs_only, None, "cable", 0xfffe), None);
        // The cable is still walked, once, for the BS space's sake.
        let walked: Vec<String> = super::walked_routes(&bs_only, &super::What::Everything)
            .into_iter()
            .map(|route| route.label)
            .collect();
        assert_eq!(walked, ["cable"]);

        let with_own = Config::parse(
            "[Tuners]\nisdbs = ISDBS, sat\ncable = ISDBC, tbs:*\n\
             [Spaces]\n0 = BS, isdbs, cable\n1 = CATV, cable\n",
        );
        assert_eq!(
            super::home_of(&with_own, &with_own.tuners["cable"]).as_deref(),
            Some("CATV")
        );
        // Named twice, walked once.
        assert_eq!(
            super::walked_routes(&with_own, &super::What::Everything).len(),
            1
        );
    }

    /// A platform with no space of its own has nowhere to be sent, so it stays
    /// where it was found rather than being dropped.
    #[test]
    fn a_platform_with_no_space_stays_where_it_was_found() {
        let config = Config::parse("[Tuners]\ncable = ISDBC, tbs:*\n[Spaces]\n0 = CATV, cable\n");
        assert_eq!(elsewhere_for(&config, Some("CATV"), "cable", 4), None);
    }
}

#[cfg(test)]
mod cable_tests {
    use super::came_by_cable;
    use coax_settings::channels::Channel;

    fn on(name: &str, frequency: u32) -> Channel {
        Channel {
            name: name.to_owned(),
            tune: vec![
                ("DELIVERY_SYSTEM".to_owned(), "ISDBT".to_owned()),
                ("FREQUENCY".to_owned(), frequency.to_string()),
                ("BANDWIDTH_HZ".to_owned(), "6000000".to_owned()),
            ],
            carriers: Vec::new(),
            tuners: Vec::new(),
            also: Vec::new(),
        }
    }

    /// The channels the broadcast plan gave back, and the bands that never
    /// carried television: a broadcast answering there came down a cable.
    #[test]
    fn a_broadcast_where_nothing_is_broadcast_came_down_a_cable() {
        assert!(came_by_cable(&on("53", 713_142_857)).is_some());
        assert!(came_by_cable(&on("62", 767_142_857)).is_some());
        assert!(came_by_cable(&on("C23", 225_142_857)).is_some());
        assert!(came_by_cable(&on("5", 173_142_857)).is_some());
    }

    /// A channel that is broadcast is left alone, whatever the network says
    /// about where else it transmits: an aerial and a cable that left the
    /// carrier where it was look the same from here.
    #[test]
    fn a_carrier_that_could_be_either_is_not_called_one() {
        assert_eq!(came_by_cable(&on("26", 551_142_857)), None);
        assert_eq!(came_by_cable(&on("42", 647_142_857)), None);
    }

    /// A cable's own carriage is not a broadcast passed through, and is not
    /// judged as one.
    #[test]
    fn a_cable_carrier_of_its_own_is_left_alone() {
        let mut cable = on("C23", 225_000_000);
        cable.tune[0].1 = "DVBC/ANNEX_A".to_owned();
        assert_eq!(came_by_cable(&cable), None);
    }
}

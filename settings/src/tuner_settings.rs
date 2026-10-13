// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `[TunerSettings]`: what a viewer set for a tuner beside the channel -- the
//! supply on a dish's lead and the front end's gain -- and what of it a tune
//! takes.
//!
//! A line names its tuners the way a route of `[Tuners]` does, by identifier
//! or by pattern (`model:` included), and says what to add:
//!
//! ```text
//! [TunerSettings]
//! pt3:6-17-0-0:0       = lnb=high
//! model:TBS 6205se*    = gain=auto
//! ```
//!
//! What goes to the daemon is held back to what the tuner says it can set (its
//! `controls`): a word the tuner has no control for is refused every time, and
//! the refusal reads as a channel that will not tune.

use coax::daemon::{Board, Controls, Tuner};
use coax::delivery;
use coax::tuning::{self, TunerSetting};

/// The lines of `[TunerSettings]`, in the file's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunerSettings {
    lines: Vec<(String, TunerSetting)>,
}

impl TunerSettings {
    /// Reads one line, `pattern = lnb=..., gain=...`. A field this does not know,
    /// or a value it cannot read, is left out rather than refused: what is left
    /// is still worth applying. A line left with nothing in it stands for the
    /// tuner's own defaults, over any pattern after it that names the tuner.
    pub fn add_line(&mut self, pattern: &str, line: &str) {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return;
        }
        let mut setting = TunerSetting::default();
        for field in line.split(',') {
            let Some((name, value)) = field.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "lnb" | "lnb_power" => {
                    if let Some(lnb) = lnb_level(value) {
                        setting.lnb = lnb.to_owned();
                    }
                }
                "gain" => {
                    if let Some(gain) = gain_value(value) {
                        setting.gain = gain;
                    }
                }
                _ => {}
            }
        }
        self.lines.retain(|(had, _)| had != pattern);
        self.lines.push((pattern.to_owned(), setting));
    }

    /// Whether no line says anything, which is when nothing is added to any
    /// tune.
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|(_, setting)| said_nothing(setting))
    }

    pub fn lines(&self) -> &[(String, TunerSetting)] {
        &self.lines
    }

    /// The setting for a tuner: the line naming it outright, or else the first
    /// pattern that matches it. Nothing said is the default, which adds
    /// nothing.
    pub fn for_tuner(&self, tuner: &Tuner) -> TunerSetting {
        self.lines
            .iter()
            .find(|(pattern, _)| *pattern == tuner.id)
            .or_else(|| {
                self.lines
                    .iter()
                    .find(|(pattern, _)| tuner.matches(pattern))
            })
            .map(|(_, setting)| setting.clone())
            .unwrap_or_default()
    }

    /// Puts what a viewer chose for one tuner, on a line of its own named by
    /// its identifier. A setting that says nothing takes that line out --
    /// unless a pattern also names the tuner, which the empty line is then
    /// kept to stand in front of.
    pub fn set(&mut self, tuner: &Tuner, setting: TunerSetting) {
        let id = tuner.id.as_str();
        let patterned = self
            .lines
            .iter()
            .any(|(pattern, said)| pattern != id && tuner.matches(pattern) && !said_nothing(said));
        if said_nothing(&setting) && !patterned {
            self.lines.retain(|(pattern, _)| pattern != id);
            return;
        }
        match self.lines.iter_mut().find(|(pattern, _)| pattern == id) {
            Some((_, had)) => *had = setting,
            None => self.lines.push((id.to_owned(), setting)),
        }
    }

    /// The section as text, ready to go into the file.
    pub fn render(&self) -> String {
        let mut text = String::from("[TunerSettings]\n");
        text.push_str(
            "; チューナー = lnb=off|low|high, gain=auto|min|max|+NdB（空はそのチューナーの既定）\n",
        );
        let wide = self
            .lines
            .iter()
            .map(|(pattern, _)| pattern.chars().count())
            .max()
            .unwrap_or(0);
        for (pattern, setting) in &self.lines {
            let mut fields = Vec::new();
            if !setting.lnb.is_empty() {
                fields.push(format!("lnb={}", setting.lnb));
            }
            if !setting.gain.is_empty() {
                fields.push(format!("gain={}", setting.gain));
            }
            let mut padded = pattern.clone();
            for _ in pattern.chars().count()..wide {
                padded.push(' ');
            }
            if fields.is_empty() {
                text.push_str(&format!("{} =\n", padded.trim_end()));
            } else {
                text.push_str(&format!("{padded} = {}\n", fields.join(", ")));
            }
        }
        text
    }
}

fn said_nothing(setting: &TunerSetting) -> bool {
    setting.lnb.trim().is_empty() && setting.gain.trim().is_empty()
}

/// The supply levels `lnb_power` takes.
fn lnb_level(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "off" => Some("off"),
        "low" => Some("low"),
        "high" => Some("high"),
        _ => None,
    }
}

/// A gain as `gain` takes it: `auto`, `min`, `max`, or a step in decibels
/// written the way the daemon lists them (`+13dB`).
fn gain_value(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "auto" | "min" | "max") {
        return Some(lower);
    }
    decibels_of(value).map(decibels)
}

/// A step in decibels as the daemon and the settings write it: `+13dB`,
/// `-2.5dB`, `0dB`.
pub fn decibels(db: f64) -> String {
    format!("{db:+.1}dB").replace(".0dB", "dB")
}

/// Reads a step in decibels, with or without its unit.
fn decibels_of(value: &str) -> Option<f64> {
    let value = value.trim();
    let number = match value.len().checked_sub(2).and_then(|at| value.get(at..)) {
        Some(unit) if unit.eq_ignore_ascii_case("db") => value.get(..value.len() - 2)?,
        _ => value,
    };
    number
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|db| db.is_finite())
}

/// The daemon's tuners as a tune needs them: each with what the routes say it
/// receives on top of what it said, what it said by itself, and what it lets a
/// channel set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listed {
    /// The tuners, with what the routes declared added (see
    /// [`coax::daemon::declare`]).
    pub tuners: Vec<Tuner>,
    /// Per tuner, in the same order: the systems it named itself, the board
    /// it is on, and its controls.
    told: Vec<Vec<String>>,
    boards: Vec<String>,
    controls: Vec<Controls>,
}

impl Listed {
    /// Reads a listing, putting what the routes say each tuner receives over
    /// what it said.
    pub fn of(boards: Vec<Board>, declared: &[(String, Vec<String>)]) -> Self {
        let mut listed = Self::default();
        for board in boards {
            for held in board.tuners {
                listed.told.push(held.tuner.deliveries.clone());
                listed.boards.push(board.name.clone());
                listed.controls.push(held.controls);
                listed.tuners.push(held.tuner);
            }
        }
        coax::daemon::declare(declared, &mut listed.tuners);
        listed
    }

    fn at(&self, id: &str) -> Option<usize> {
        self.tuners.iter().position(|tuner| tuner.id == id)
    }

    /// What a tuner lets a channel set, or nothing for one not listed.
    pub fn controls(&self, id: &str) -> Option<&Controls> {
        self.controls.get(self.at(id)?)
    }

    /// The board a tuner is on.
    pub fn board(&self, id: &str) -> Option<&str> {
        self.boards.get(self.at(id)?).map(String::as_str)
    }

    /// The systems a tuner named itself, before the routes added theirs.
    pub fn told(&self, id: &str) -> Option<&[String]> {
        self.told.get(self.at(id)?).map(Vec::as_slice)
    }

    /// A tune's words as they go to one tuner: named, with ISDB-S3 asked for
    /// as ISDB-S where the tuner does not name it, and with what its setting
    /// adds that it can take. Nothing is added for a tuner this does not
    /// list, or for no tuner at all: the setting would land on whichever the
    /// daemon chose.
    pub fn asking(
        &self,
        tuner: Option<&str>,
        settings: &TunerSettings,
        words: &[(String, String)],
    ) -> Vec<(String, String)> {
        let mut asking: Vec<(String, String)> = words
            .iter()
            .filter(|(name, _)| name != "tuner")
            .cloned()
            .collect();
        let Some(tuner) = tuner else {
            return asking;
        };
        asking.push(("tuner".to_owned(), tuner.to_owned()));
        let Some(at) = self.at(tuner) else {
            return asking;
        };
        let (Some(listed), Some(told), Some(controls)) = (
            self.tuners.get(at),
            self.told.get(at),
            self.controls.get(at),
        ) else {
            return asking;
        };
        told_to(
            &mut asking,
            told,
            &listed.deliveries,
            controls,
            &settings.for_tuner(listed),
        );
        asking
    }
}

/// Adds to a tune what one tuner needs of it: ISDB-S3 under ISDB-S's name
/// where the tuner was only said to receive it (`told` is what it named,
/// `used` what it is used for), and the words of its setting that its controls
/// take. A word the tune already says stays as the tune says it.
pub fn told_to(
    words: &mut Vec<(String, String)>,
    told: &[String],
    used: &[String],
    controls: &Controls,
    setting: &TunerSetting,
) {
    let system = words
        .iter()
        .find(|(name, _)| name == "DELIVERY_SYSTEM")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    if delivery::same(&system, delivery::ISDBS3) && tuning::s3_as_s(told, used) {
        if let Some((name, rate)) = tuning::ask_s3_as_s(words.as_mut_slice()) {
            words.push((name.to_owned(), rate));
        }
    }
    for (name, value) in setting.words(&system) {
        if words.iter().any(|(known, _)| known == name) || !takes(controls, &system, name, &value) {
            continue;
        }
        words.push((name.to_owned(), value));
    }
}

/// Whether a tuner's controls take a word of a setting on a tune of a system.
/// A supply level the tuner did not list -- `off` on one that cannot be
/// switched off -- is not sent, and nor is a gain on a system the amplifier
/// is not in front of.
fn takes(controls: &Controls, system: &str, name: &str, value: &str) -> bool {
    match name {
        "lnb_power" => controls
            .lnb
            .as_ref()
            .is_some_and(|lnb| lnb.levels.iter().any(|level| level == value)),
        "gain" => controls.gain.as_ref().is_some_and(|gain| {
            let on_this = gain.deliveries.is_empty()
                || gain
                    .deliveries
                    .iter()
                    .any(|one| delivery::same(one, system));
            let step = match value {
                "auto" | "min" | "max" => true,
                _ => decibels_of(value).is_some_and(|wanted| {
                    gain.decibels
                        .iter()
                        .any(|step| (step - wanted).abs() < 0.05)
                }),
            };
            on_this && step
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Listed, TunerSettings, decibels, told_to};
    use coax::daemon::{Board, Controls, GainControl, Held, LnbControl, Tuner};
    use coax::tuning::TunerSetting;

    fn tuner(id: &str, model: &str) -> Tuner {
        Tuner {
            id: id.to_owned(),
            model: model.to_owned(),
            ..Tuner::default()
        }
    }

    fn words(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|one| (*one).to_owned()).collect()
    }

    /// A supply that can be switched off, at two levels, and an amplifier with
    /// steps that sits in front of the satellite input only.
    fn controls() -> Controls {
        Controls {
            lnb: Some(LnbControl {
                levels: names(&["off", "low", "high"]),
                low: "11V".to_owned(),
                high: "15V".to_owned(),
                shared: String::new(),
            }),
            gain: Some(GainControl {
                decibels: vec![0.0, 6.5, 13.0],
                deliveries: names(&["ISDB-S"]),
            }),
        }
    }

    #[test]
    fn a_line_is_read_down_to_what_it_says() {
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:*:0", "lnb=High, gain=+13dB");
        settings.add_line("model:TBS 6205se*", " gain = 6.5 ");
        settings.add_line("bad", "lnb=sideways, gain=loud, colour=blue");
        settings.add_line("", "lnb=high");
        assert_eq!(settings.lines().len(), 3, "{settings:?}");
        let first = &settings.lines()[0].1;
        assert_eq!(first.lnb, "high");
        assert_eq!(first.gain, "+13dB");
        assert_eq!(settings.lines()[1].1.gain, "+6.5dB");
        // What could not be read is left unsaid, which is the tuner's default.
        assert_eq!(settings.lines()[2].1, TunerSetting::default());
        assert!(!settings.is_empty());
        let mut nothing = TunerSettings::default();
        nothing.add_line("bad", "lnb=sideways");
        assert!(nothing.is_empty(), "a line that says nothing adds nothing");
        assert_eq!(decibels(-2.0), "-2dB");
    }

    /// A line naming the tuner outright wins over a pattern that also names it,
    /// whatever their order; among patterns the first one does.
    #[test]
    fn a_tuner_named_outright_takes_its_own_line() {
        let mut settings = TunerSettings::default();
        settings.add_line("model:Earthsoft PT3*", "lnb=low");
        settings.add_line("pt3:a:0", "lnb=high");
        settings.add_line("pt3:*", "lnb=off");
        let pt3 = |id: &str| tuner(id, "Earthsoft PT3");
        assert_eq!(settings.for_tuner(&pt3("pt3:a:0")).lnb, "high");
        assert_eq!(settings.for_tuner(&pt3("pt3:a:1")).lnb, "low");
        assert_eq!(settings.for_tuner(&tuner("pt3:b:0", "")).lnb, "off");
        assert_eq!(
            settings.for_tuner(&tuner("tbs:1:0", "")),
            TunerSetting::default()
        );
    }

    #[test]
    fn a_setting_is_put_by_identifier_and_taken_out_when_it_says_nothing() {
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:a:0", "lnb=high");
        let gain = TunerSetting {
            gain: "auto".to_owned(),
            ..TunerSetting::default()
        };
        settings.set(&tuner("pt3:a:1", ""), gain.clone());
        settings.set(&tuner("pt3:a:0", ""), TunerSetting::default());
        assert_eq!(settings.lines(), [("pt3:a:1".to_owned(), gain)]);
        assert_eq!(
            settings.render(),
            "[TunerSettings]\n; チューナー = lnb=off|low|high, gain=auto|min|max|+NdB（空はそのチューナーの既定）\npt3:a:1 = gain=auto\n"
        );
    }

    /// A tuner a pattern names is put back to its defaults by an empty line of
    /// its own, which reads back as itself and leaves the pattern for the
    /// others.
    #[test]
    fn an_empty_line_stands_in_front_of_a_pattern() {
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:*", "lnb=high");
        let one = tuner("pt3:a:0", "");
        settings.set(&one, TunerSetting::default());
        assert_eq!(settings.for_tuner(&one), TunerSetting::default());
        assert_eq!(settings.for_tuner(&tuner("pt3:a:1", "")).lnb, "high");
        let rendered = settings.render();
        assert!(
            rendered.ends_with("pt3:*   = lnb=high\npt3:a:0 =\n"),
            "{rendered}"
        );
        let mut read = TunerSettings::default();
        for line in rendered.lines().skip(2) {
            let (pattern, value) = line.split_once('=').expect("a line");
            read.add_line(pattern, value);
        }
        assert_eq!(read, settings);
    }

    /// The supply goes on a satellite tune, and the gain only where the
    /// amplifier is in front of that system and has the step asked for.
    #[test]
    fn only_what_the_tuner_can_set_is_said() {
        let setting = TunerSetting {
            lnb: "high".to_owned(),
            gain: "+13dB".to_owned(),
            ..TunerSetting::default()
        };
        let told = names(&["ISDB-S"]);
        let mut sky = words(&[("DELIVERY_SYSTEM", "ISDBS"), ("FREQUENCY", "1049480")]);
        told_to(&mut sky, &told, &told, &controls(), &setting);
        assert_eq!(
            sky,
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS"),
                ("FREQUENCY", "1049480"),
                ("lnb_power", "high"),
                ("gain", "+13dB"),
            ])
        );
        // Not the gain on terrestrial, where the amplifier is not.
        let mut air = words(&[("DELIVERY_SYSTEM", "ISDBT")]);
        told_to(&mut air, &told, &told, &controls(), &setting);
        assert_eq!(air, words(&[("DELIVERY_SYSTEM", "ISDBT")]));
        // Nothing to a tuner that has no controls: BDA lists none, and a word
        // it cannot take is refused on every tune.
        let mut bare = words(&[("DELIVERY_SYSTEM", "ISDBS")]);
        told_to(&mut bare, &told, &told, &Controls::default(), &setting);
        assert_eq!(bare, words(&[("DELIVERY_SYSTEM", "ISDBS")]));
        // A step the amplifier does not have, and a supply it cannot cut.
        let fixed = Controls {
            lnb: Some(LnbControl {
                levels: names(&["low", "high"]),
                ..LnbControl::default()
            }),
            ..controls()
        };
        let odd = TunerSetting {
            lnb: "off".to_owned(),
            gain: "+3dB".to_owned(),
            ..TunerSetting::default()
        };
        let mut asked = words(&[("DELIVERY_SYSTEM", "ISDBS")]);
        told_to(&mut asked, &told, &told, &fixed, &odd);
        assert_eq!(asked, words(&[("DELIVERY_SYSTEM", "ISDBS")]));
    }

    /// A tuner the routes said receives ISDB-S3, and that does not name it
    /// itself, is asked for it as ISDB-S at ISDB-S3's rate.
    #[test]
    fn isdb_s3_goes_to_a_tuner_that_does_not_name_it_as_isdb_s() {
        let told = names(&["ISDB-T", "ISDB-S"]);
        let used = names(&["ISDB-T", "ISDB-S", "ISDBS3"]);
        let mut asked = words(&[("DELIVERY_SYSTEM", "ISDBS3"), ("FREQUENCY", "1049480")]);
        told_to(
            &mut asked,
            &told,
            &used,
            &Controls::default(),
            &TunerSetting::default(),
        );
        assert_eq!(
            asked,
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS"),
                ("FREQUENCY", "1049480"),
                ("SYMBOL_RATE", "33756000"),
            ])
        );
        // One that names it is asked for it by its name.
        let native = names(&["ISDB-S", "ISDB-S3"]);
        let mut asked = words(&[("DELIVERY_SYSTEM", "ISDBS3")]);
        told_to(
            &mut asked,
            &native,
            &native,
            &Controls::default(),
            &TunerSetting::default(),
        );
        assert_eq!(asked, words(&[("DELIVERY_SYSTEM", "ISDBS3")]));
    }

    /// The listing keeps what each tuner said apart from what the routes
    /// added, and a tune is named and told what its tuner takes.
    #[test]
    fn a_tune_goes_to_a_listed_tuner_with_its_setting() {
        let boards = vec![Board {
            name: "pt3:a".to_owned(),
            tuners: vec![Held {
                tuner: Tuner {
                    deliveries: names(&["ISDB-S"]),
                    ..tuner("pt3:a:0", "Earthsoft PT3")
                },
                controls: controls(),
                ..Held::default()
            }],
            ..Board::default()
        }];
        let declared = vec![("ISDBS3".to_owned(), names(&["pt3:*"]))];
        let listed = Listed::of(boards, &declared);
        assert_eq!(listed.told("pt3:a:0"), Some(&names(&["ISDB-S"])[..]));
        assert_eq!(listed.tuners[0].deliveries, names(&["ISDB-S", "ISDBS3"]));
        assert_eq!(listed.board("pt3:a:0"), Some("pt3:a"));
        let mut settings = TunerSettings::default();
        settings.add_line("pt3:*", "lnb=high");
        let tune = words(&[
            ("DELIVERY_SYSTEM", "ISDBS3"),
            ("FREQUENCY", "1049480"),
            ("tuner", "elsewhere"),
        ]);
        assert_eq!(
            listed.asking(Some("pt3:a:0"), &settings, &tune),
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS"),
                ("FREQUENCY", "1049480"),
                ("tuner", "pt3:a:0"),
                ("SYMBOL_RATE", "33756000"),
                ("lnb_power", "high"),
            ])
        );
        // No tuner named, or one the daemon did not list: nothing added.
        assert_eq!(
            listed.asking(None, &settings, &tune[..2]),
            tune[..2].to_vec()
        );
        assert_eq!(
            listed.asking(Some("pt3:b:0"), &settings, &tune[..2]),
            words(&[
                ("DELIVERY_SYSTEM", "ISDBS3"),
                ("FREQUENCY", "1049480"),
                ("tuner", "pt3:b:0"),
            ])
        );
    }
}

// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The channels the settings file defines, one line each.
//!
//! The band plan names carriers, and that is all it can do. Which streams a multiplexing
//! frame carries, which identifier a satellite stream answers to, which carrier of a
//! cable holds the same stream a dish receives -- those are in the signal, and something
//! has to have looked. This is where what looked writes down what it found.
//!
//! ```ini
//! [Channels BS]
//! ; onid, tsid, name, route, system, frequency, slot [, KEY=VALUE ...]
//! 4, 16625, BS15/16625, -,     ISDBS, 1049480,   -
//! 4, 16625, BS15/16625, isdbc, ISDBC, 471142857, ts:2
//! ```
//!
//! A line is one way of receiving one stream. What a stream is, is the network it belongs
//! to and its own identifier, so the first line with a pair is the channel -- at the index
//! its place among the firsts gives it -- and every later line with the same pair is
//! another way of receiving it, in the order to try them.
//!
//! A space with no lines is offered the band plan instead. The two are not mixed.

use coax::delivery;

/// Which slot of a multiplexing frame a line names, where it names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The whole carrier: one stream, or a satellite's, which is picked out by
    /// its own identifier.
    Whole,
    /// A slot carrying a transport stream.
    Transport(u8),
    /// A slot carrying the TLV the advanced services use.
    Tlv(u8),
}

/// One line: one way of receiving one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub onid: u16,
    pub tsid: u16,
    /// What the host shows. The first line's name is the channel's.
    pub name: String,
    /// The label in `[Tuners]` this way is received on. Empty for the space's
    /// own: a stream that came down another lead than the rest of its space
    /// names it.
    pub tuners: String,
    /// The delivery system, in the vocabulary of a channel definition file.
    pub system: String,
    /// The carriers, in hertz for cable and terrestrial and kilohertz for a
    /// satellite -- whatever the delivery system's own unit is. More than one
    /// where a multiplexing frame is split across several.
    pub carriers: Vec<String>,
    pub slot: Slot,
    /// What else the tune takes, beyond what the delivery system implies:
    /// written as `KEY=VALUE` on the line, and only where the value is not the
    /// one the plan would have used.
    pub extras: Vec<(String, String)>,
}

impl Definition {
    /// What a line is keyed by.
    pub fn key(&self) -> (u16, u16) {
        (self.onid, self.tsid)
    }

    /// What tells one way of receiving a stream from another: which lead it
    /// comes down, and what it is tuned as. Not the frequency: a cable operator
    /// moves their carriers about, and a channel that has moved is the same way
    /// with a number brought up to date.
    pub fn route(&self) -> (String, String) {
        (self.tuners.clone(), delivery::named(&self.system))
    }

    /// The first carrier, which is the one a tune names.
    pub fn frequency(&self) -> &str {
        self.carriers.first().map_or("", String::as_str)
    }

    /// Reads one line. Nothing for a line that is not one: a comment, or one
    /// with too few columns to name a stream and a carrier.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            return None;
        }
        let mut columns = line.split(',').map(str::trim);
        let onid = number(columns.next()?)?;
        let tsid = number(columns.next()?)?;
        let name = columns.next()?.to_owned();
        let tuners = dash(columns.next()?);
        let system = columns.next()?.to_owned();
        let carriers: Vec<String> = columns
            .next()?
            .split('+')
            .map(str::trim)
            .filter(|carrier| !carrier.is_empty())
            .map(str::to_owned)
            .collect();
        if name.is_empty() || system.is_empty() || carriers.is_empty() {
            return None;
        }
        let slot = match columns.next() {
            None => Slot::Whole,
            Some(written) => slot(written)?,
        };
        let extras = columns
            .filter_map(|column| column.split_once('='))
            .map(|(key, value)| (key.trim().to_ascii_uppercase(), value.trim().to_owned()))
            .filter(|(key, _)| !key.is_empty())
            .collect();
        Some(Self {
            onid,
            tsid,
            name,
            tuners,
            system,
            carriers,
            slot,
            extras,
        })
    }

    /// Writes the line back, the way [`Self::parse`] reads it.
    pub fn render(&self) -> String {
        let mut line = format!(
            "{}, {}, {}, {}, {}, {}, {}",
            self.onid,
            self.tsid,
            self.name,
            if self.tuners.is_empty() {
                "-"
            } else {
                &self.tuners
            },
            self.system,
            self.carriers.join("+"),
            match self.slot {
                Slot::Whole => "-".to_owned(),
                Slot::Transport(number) => format!("ts:{number}"),
                Slot::Tlv(number) => format!("tlv:{number}"),
            }
        );
        for (key, value) in &self.extras {
            line.push_str(&format!(", {key}={value}"));
        }
        line
    }
}

/// A column that may be left empty with a dash.
fn dash(column: &str) -> String {
    match column {
        "-" | "" => String::new(),
        written => written.to_owned(),
    }
}

/// An identifier, written in decimal or as `0x` hexadecimal -- both are how
/// people write them.
fn number(column: &str) -> Option<u16> {
    let column = column.trim();
    match column
        .strip_prefix("0x")
        .or_else(|| column.strip_prefix("0X"))
    {
        Some(digits) => u16::from_str_radix(digits, 16).ok(),
        None => column.parse().ok(),
    }
}

fn slot(column: &str) -> Option<Slot> {
    let column = column.trim().to_ascii_lowercase();
    if column == "-" || column.is_empty() {
        return Some(Slot::Whole);
    }
    if let Some(number) = column.strip_prefix("ts:") {
        return number.parse().ok().map(Slot::Transport);
    }
    if let Some(number) = column.strip_prefix("tlv:") {
        return number.parse().ok().map(Slot::Tlv);
    }
    None
}

/// The lines of every space, in the order the file gives them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defined {
    spaces: Vec<(String, Vec<Definition>)>,
}

/// How a section that holds a space's lines is headed.
const SECTION: &str = "channels";

/// The space a section header names, where the section is one of ours.
fn header(line: &str) -> Option<&str> {
    let inside = line.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    let (kind, space) = inside.split_once(char::is_whitespace)?;
    if !kind.eq_ignore_ascii_case(SECTION) {
        return None;
    }
    let space = space.trim();
    (!space.is_empty()).then_some(space)
}

/// Whether a line opens a section of any kind.
fn is_header(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('[') && line.ends_with(']')
}

impl Defined {
    pub fn none() -> Self {
        Self::default()
    }

    /// Reads the `[Channels ...]` sections out of the settings file's text.
    /// Everything else in the file is somebody else's.
    pub fn parse(text: &str) -> Self {
        let mut defined = Self::default();
        let mut current: Option<String> = None;
        for line in text.lines() {
            if is_header(line) {
                current = header(line).map(str::to_owned);
                if let Some(space) = &current {
                    defined.space_mut(space);
                }
                continue;
            }
            let Some(space) = &current else {
                continue;
            };
            if let Some(definition) = Definition::parse(line) {
                defined.space_mut(space).push(definition);
            }
        }
        defined
    }

    /// The lines of one space, in order. None for a space the file says
    /// nothing about, which is a space that offers the plan.
    pub fn of(&self, space: &str) -> &[Definition] {
        self.spaces
            .iter()
            .find(|(name, _)| name == space)
            .map_or(&[], |(_, lines)| lines.as_slice())
    }

    pub fn spaces(&self) -> impl Iterator<Item = (&str, &[Definition])> {
        self.spaces
            .iter()
            .map(|(name, lines)| (name.as_str(), lines.as_slice()))
    }

    /// Puts a space's lines in, replacing what it had.
    pub fn set(&mut self, space: &str, lines: Vec<Definition>) {
        *self.space_mut(space) = lines;
    }

    fn space_mut(&mut self, space: &str) -> &mut Vec<Definition> {
        if let Some(at) = self.spaces.iter().position(|(name, _)| name == space) {
            return &mut self.spaces[at].1;
        }
        self.spaces.push((space.to_owned(), Vec::new()));
        &mut self.spaces.last_mut().expect("just pushed").1
    }

    /// The seats of a space: the first line of each stream, in order, which is
    /// what the host is offered.
    pub fn seats(lines: &[Definition]) -> Vec<&Definition> {
        let mut seats: Vec<&Definition> = Vec::new();
        for line in lines {
            if !seats.iter().any(|seat| seat.key() == line.key()) {
                seats.push(line);
            }
        }
        seats
    }

    /// Puts these lines into the settings file's text, leaving everything that
    /// is not a `[Channels ...]` section exactly as it was.
    ///
    /// The file is the viewer's: their spaces, their tuners, their comments. A
    /// section for a space with no lines is taken out, which hands the space
    /// back to the plan.
    pub fn write_into(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 1024);
        let mut written: Vec<String> = Vec::new();
        // Which of ours the line being read belongs to, if any: its body is
        // being skipped, having been written afresh under the header.
        let mut skipping = false;
        for line in text.lines() {
            if is_header(line) {
                skipping = false;
                if let Some(space) = header(line) {
                    let lines = self.of(space);
                    skipping = true;
                    if lines.is_empty() {
                        // Taken out, header and all.
                        continue;
                    }
                    if !written.iter().any(|done| done == space) {
                        out.push_str(&section(space, lines));
                        written.push(space.to_owned());
                    }
                    continue;
                }
            }
            if skipping {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        for (space, lines) in &self.spaces {
            if lines.is_empty() || written.iter().any(|done| done == space) {
                continue;
            }
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(&section(space, lines));
        }
        out
    }
}

/// One section, header and lines, ready to go into the file.
fn section(space: &str, lines: &[Definition]) -> String {
    let mut text = format!("[Channels {space}]\n");
    text.push_str("; onid, tsid, name, route, system, frequency, slot [, KEY=VALUE ...]\n");
    for line in lines {
        text.push_str(&line.render());
        text.push('\n');
    }
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::{Defined, Definition, Slot};

    const FILE: &str = "\
; BonDriver_coax.ini
[coax]
Host = 127.0.0.1:40200   ; the daemon

[Tuners]
isdbs = ISDBS, sat
cable = ISDBC, tbs:*

[Spaces]
0 = BS, isdbs, cable
1 = CATV, cable

[Channels BS]
; onid, tsid, name, route, system, frequency, slot
4, 16625, BS15/16625, isdbs, ISDBS, 1049480, -
4, 16625, BS15/16625, cable, ISDBC, 471142857, ts:2

[Channels CATV]
0xfffe, 45281, C20/45281, -, ISDBC, 153000000+159000000, tlv:1, MODULATION=QAM/256
";

    /// A line is read back as it was written, columns and all.
    #[test]
    fn a_line_is_what_it_says() {
        let line = Definition::parse(
            "0xfffe, 45281, C20/45281, -, ISDBC, 153000000+159000000, tlv:1, MODULATION=QAM/256",
        )
        .expect("the line is one");
        assert_eq!(line.key(), (0xfffe, 45281));
        assert_eq!(line.name, "C20/45281");
        assert_eq!(line.tuners, "");
        assert_eq!(line.system, "ISDBC");
        assert_eq!(line.carriers, vec!["153000000", "159000000"]);
        assert_eq!(line.frequency(), "153000000");
        assert_eq!(line.slot, Slot::Tlv(1));
        assert_eq!(
            line.extras,
            vec![("MODULATION".to_owned(), "QAM/256".to_owned())]
        );
        assert_eq!(
            line.render(),
            "65534, 45281, C20/45281, -, ISDBC, 153000000+159000000, tlv:1, MODULATION=QAM/256"
        );
    }

    /// A line that names no stream or no carrier is not a line: a comment, a
    /// blank, or something a person left half written.
    #[test]
    fn what_is_not_a_line_is_passed_over() {
        assert!(Definition::parse("; a comment").is_none());
        assert!(Definition::parse("").is_none());
        assert!(Definition::parse("4, 16625").is_none());
        assert!(Definition::parse("four, 16625, BS15, -, ISDBS, 1049480").is_none());
        // A slot nobody can read is a line nobody should tune by.
        assert!(Definition::parse("4, 16625, BS15, -, ISDBS, 1049480, slot 2").is_none());
    }

    /// The sections that are ours are read, per space, in the order the file
    /// gives them; the rest of the file is left to the settings.
    #[test]
    fn the_sections_of_every_space_are_read_in_order() {
        let defined = Defined::parse(FILE);
        let names: Vec<&str> = defined.spaces().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["BS", "CATV"]);
        assert_eq!(defined.of("BS").len(), 2);
        assert_eq!(defined.of("CATV").len(), 1);
        assert!(defined.of("地デジ").is_empty());
    }

    /// The first line of a stream is the channel; the rest are ways of
    /// receiving it.
    #[test]
    fn the_first_line_of_a_stream_is_its_seat() {
        let defined = Defined::parse(FILE);
        let seats = Defined::seats(defined.of("BS"));
        assert_eq!(seats.len(), 1);
        assert_eq!(seats[0].system, "ISDBS");
    }

    /// Writing the lines back touches nothing but the sections that are ours:
    /// the viewer's settings and comments come through byte for byte.
    #[test]
    fn writing_back_leaves_the_rest_of_the_file_alone() {
        let mut defined = Defined::parse(FILE);
        let mut bs = defined.of("BS").to_vec();
        bs[1].carriers = vec!["477142857".to_owned()];
        defined.set("BS", bs);
        defined.set("CATV", Vec::new());
        defined.set(
            "地デジ",
            vec![Definition::parse("32736, 32736, 27/32736, -, ISDBT, 557142857, -").unwrap()],
        );
        let written = defined.write_into(FILE);
        // The settings, untouched.
        assert!(
            written.starts_with(
                "; BonDriver_coax.ini\n[coax]\nHost = 127.0.0.1:40200   ; the daemon\n"
            )
        );
        assert!(written.contains("[Spaces]\n0 = BS, isdbs, cable\n1 = CATV, cable\n"));
        // Ours, brought up to date.
        assert!(written.contains("4, 16625, BS15/16625, cable, ISDBC, 477142857, ts:2\n"));
        assert!(!written.contains("471142857"));
        // Gone, with its header.
        assert!(!written.contains("[Channels CATV]"));
        assert!(!written.contains("45281"));
        // New, on the end.
        assert!(written.ends_with("[Channels 地デジ]\n; onid, tsid, name, route, system, frequency, slot [, KEY=VALUE ...]\n32736, 32736, 27/32736, -, ISDBT, 557142857, -\n\n"));
        // And what was written is what is read back.
        assert_eq!(Defined::parse(&written), defined_without_empty(&defined));
    }

    fn defined_without_empty(defined: &Defined) -> Defined {
        let mut kept = Defined::none();
        for (space, lines) in defined.spaces() {
            if !lines.is_empty() {
                kept.set(space, lines.to_vec());
            }
        }
        kept
    }
}

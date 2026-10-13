// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The window that says what the tuners are doing.
//!
//! A viewer who cannot start a sweep, or whose channel will not open, wants one
//! question answered: which tuner is busy, and with what. The daemon knows, so
//! this asks it and writes the answer out card by card. What a tuner is tuned
//! to comes back as the properties of the tune, which are matched against the
//! channel definitions beside the driver so that the line reads as the
//! channel's name.

use coax::daemon::{Board, Held, Tuner};
use coax::delivery;
use coax_settings::config::Route;
use coax_settings::defined::Defined;
use coax_settings::setup;

/// The tuners as a viewer reads them: a heading per card, and a line for each
/// tuner under it. The daemon's boards are not cards -- a Windows machine lists
/// a four-tuner card as four boards -- so the cards here are the models, which
/// is what a viewer recognises.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shown {
    /// The heading, or nothing for a line under one.
    pub card: Option<String>,
    pub line: String,
    /// Whether somebody holds this tuner, which is what a viewer came to see.
    pub busy: bool,
}

/// What the tuner window shows, from a listing and the channels beside the
/// driver.
pub fn shown(boards: &[Board], known: &Defined, routes: &[Route]) -> Vec<Shown> {
    let mut cards: Vec<(String, Vec<&Held>)> = Vec::new();
    for board in boards {
        let model = match setup::model_of(&board.model) {
            model if model.is_empty() => board.name.clone(),
            model => model,
        };
        for held in &board.tuners {
            match cards.iter_mut().find(|(known, _)| *known == model) {
                Some((_, members)) => members.push(held),
                None => cards.push((model.clone(), vec![held])),
            }
        }
    }
    // The daemon lists boards in whatever order it found them, which on Windows
    // reads as no order at all: a card's tuners come out 2, 0, 3, 1. Within a
    // card they are put in the order their names number them, which is the
    // order on the bracket.
    for (_, members) in &mut cards {
        members.sort_by_key(|held| numbered(&held.tuner.name, &held.tuner.id));
    }
    let mut lines = Vec::new();
    for (card, members) in cards {
        lines.push(Shown {
            card: Some(card),
            line: String::new(),
            busy: false,
        });
        for held in members {
            lines.push(Shown {
                card: None,
                line: line_of(held, known, routes),
                busy: held.tuner.leased,
            });
        }
    }
    lines
}

/// What a name numbers a tuner by, for putting a card's tuners in order: the
/// last number in the name, and the name itself where it has none. The
/// identifier decides between two of the same.
fn numbered(name: &str, id: &str) -> (u32, String, String) {
    let digits: String = name
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (
        digits.parse().unwrap_or(u32::MAX),
        name.to_owned(),
        id.to_owned(),
    )
}

/// Whether anyone has the tuner, and at what rank where that is worth saying.
///
/// The default rank is left out. Almost every tuner in use is held at it, and
/// spelling it on every line would bury the one line where somebody has said
/// something -- which is what a viewer refused a channel is reading for.
fn holding(held: &Held) -> String {
    if !held.tuner.leased {
        return "  空き".to_owned();
    }
    match held.priority {
        Some(priority) if priority != coax::daemon::DEFAULT_PRIORITY => {
            format!("  使用中(優先度 {priority})")
        }
        _ => "  使用中".to_owned(),
    }
}

/// One tuner's line: what it is called, which routes may use it, whether
/// anyone has it, and -- where somebody has -- what it is doing.
fn line_of(held: &Held, known: &Defined, routes: &[Route]) -> String {
    let mut line = held.tuner.id.clone();
    if !held.tuner.name.is_empty() {
        line.push_str("  ");
        line.push_str(&held.tuner.name);
    }
    line.push_str(&holding(held));
    if let Some(doing) = doing(&held.tuned, known) {
        line.push_str("  ");
        line.push_str(&doing);
    }
    if held.locked == Some(false) {
        line.push_str("  未同期");
    }
    if let Some(cnr) = held.cnr {
        line.push_str(&format!("  CNR {cnr:.1} dB"));
    }
    if let Some(strength) = held.strength {
        line.push_str(&format!("  {strength:.1} dBm"));
    }
    if !held.tuner.leased && !held.tuner.deliveries.is_empty() {
        line.push_str("  ");
        line.push_str(&held.tuner.deliveries.join(", "));
    }
    let named = routes_naming(&held.tuner, routes);
    if !named.is_empty() {
        line.push_str(&format!("  [{}]", named.join(", ")));
    }
    line
}

/// Which routes may use this tuner: the ones whose patterns name it, and the
/// ones that name no tuner at all and would take it if it will demodulate what
/// they are.
///
/// This is what says why a channel came off one card rather than another. Not
/// the whole answer -- a channel definition records the route it was found on
/// -- but it says which tuners each route may reach for.
fn routes_naming(tuner: &Tuner, routes: &[Route]) -> Vec<String> {
    routes
        .iter()
        .filter(|route| {
            if route.tuners.is_empty() {
                tuner.receives(&route.system)
            } else {
                route.tuners.iter().any(|pattern| tuner.matches(pattern))
            }
        })
        .map(|route| route.label.clone())
        .collect()
}

/// What a tuner is doing: what it is tuned as, and which channel that is.
///
/// The delivery system comes first because it is the question a viewer asks of
/// a busy tuner -- which lead it is on -- and the space that follows does not
/// answer it: a cable carries the aerial and satellite platforms
/// transmodulated, and their channels are filed under those platforms' spaces.
/// So a tuner reads `ISDB-C  BS / 23/16626`.
fn doing(tuned: &[(String, String)], known: &Defined) -> Option<String> {
    if tuned.is_empty() {
        return None;
    }
    let value = |wanted: &str| {
        tuned
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.clone())
    };
    let system = delivery::named(&value("DELIVERY_SYSTEM").unwrap_or_default());
    let frequency = value("FREQUENCY").unwrap_or_default();
    let slot = value("TSMF_REL_TS").or_else(|| value("TSMF_REL_TLV"));
    if frequency.is_empty() {
        return None;
    }
    // Written the way the scan's account writes it, which is how a viewer has
    // seen the carrier named.
    let shown = delivery::written(&system);
    match channel_of(&system, &frequency, slot.as_deref(), known) {
        Some(channel) => Some(format!("{shown}  {channel}")),
        // Nothing in the file: the figures themselves, which are what a viewer
        // would have looked the channel up by anyway.
        None => {
            let mut said = format!("{shown} {frequency}");
            if let Some(slot) = slot {
                said.push_str(&format!(" #{slot}"));
            }
            Some(said)
        }
    }
}

/// Which channel a tune names, as the file beside the driver calls it: the
/// space it is shown in and the name it is shown under. Matched on what tells
/// one channel from another -- what it is tuned as, the carrier, and the slot
/// where the tune names one.
fn channel_of(
    system: &str,
    frequency: &str,
    slot: Option<&str>,
    known: &Defined,
) -> Option<String> {
    for (space, lines) in known.spaces() {
        for line in lines {
            if !delivery::same(&line.system, system) || line.frequency() != frequency {
                continue;
            }
            if let Some(slot) = slot {
                let number = match line.slot {
                    coax_settings::defined::Slot::Transport(number)
                    | coax_settings::defined::Slot::Tlv(number) => Some(number.to_string()),
                    coax_settings::defined::Slot::Whole => None,
                };
                if number.as_deref() != Some(slot) {
                    continue;
                }
            }
            return Some(format!("{space} / {}", line.name));
        }
    }
    None
}

#[cfg(windows)]
mod windows {
    use std::sync::Mutex;

    use coax_settings::wide::Wide;

    use crate::win::{
        self, Handle, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_PAINT, WS_TABSTOP, WS_VSCROLL, control,
        set_text, there,
    };

    /// What the window calls to have the listing again.
    pub(crate) type Ask = fn() -> Vec<super::Shown>;

    /// The account is an edit rather than a list so that a viewer can drag
    /// over it and copy what it says.
    const ES_MULTILINE: u32 = 0x0004;
    const ES_READONLY: u32 = 0x0800;
    const ES_AUTOVSCROLL: u32 = 0x0040;

    const WIDE: i32 = 640;
    const ID_LIST: usize = 100;
    const ID_AGAIN: usize = 101;
    const ID_CLOSE: usize = 102;

    static WINDOW: Mutex<usize> = Mutex::new(0);
    /// What to ask when the window wants the listing again: the plugin's, so
    /// that this need not know where the daemon or the settings are.
    static ASK: Mutex<Option<Ask>> = Mutex::new(None);

    fn held() -> Handle {
        *WINDOW.lock().expect("the window is not poisoned") as Handle
    }

    /// Opens the window on what the tuners are doing, or brings it forward
    /// and asks again if it is already open.
    pub fn open(owner: Handle, ask: Ask) {
        *ASK.lock().expect("the question is not poisoned") = Some(ask);
        let existing = held();
        if there(existing) {
            look(existing);
            win::to_the_front(existing);
            return;
        }
        let name = Wide::new("coax_plugin_tuners");
        if !win::class(&name, procedure) {
            return;
        }
        let (width, height) = size();
        let window = win::make(owner, &name, "coax: チューナーの状態", width, height);
        if window.is_null() {
            return;
        }
        *WINDOW.lock().expect("the window is not poisoned") = window as usize;
        unsafe { win::ShowWindow(window, win::SW_SHOW) };
        win::to_the_front(window);
    }

    pub fn look_again() {
        win::redress(held());
    }

    pub fn close() {
        let window = held();
        if there(window) {
            unsafe { win::DestroyWindow(window) };
        }
        *WINDOW.lock().expect("the window is not poisoned") = 0;
    }

    fn size() -> (i32, i32) {
        let m = win::metrics();
        (WIDE, m.margin * 2 + m.line * 20 + m.gap + m.button)
    }

    fn furnish(window: Handle) {
        let m = win::metrics();
        let inner = WIDE - m.margin * 2;
        let high = m.line * 20;
        control(
            window,
            "EDIT",
            "",
            WS_TABSTOP | WS_VSCROLL | ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL,
            (m.margin, m.margin, inner, high),
            ID_LIST,
        );
        let y = m.margin + high + m.gap;
        let wide = m.line * 7;
        control(
            window,
            "BUTTON",
            "更新",
            WS_TABSTOP,
            (m.margin + inner - wide * 2 - m.gap, y, wide, m.button),
            ID_AGAIN,
        );
        control(
            window,
            "BUTTON",
            "閉じる",
            WS_TABSTOP,
            (m.margin + inner - wide, y, wide, m.button),
            ID_CLOSE,
        );
        win::dress(window);
        look(window);
    }

    /// Asks again and writes what came back. The daemon is asked on the
    /// thread that draws the window: it is a question about what it already
    /// holds, and one that will not answer has a message of its own.
    fn look(window: Handle) {
        let ask = *ASK.lock().expect("the question is not poisoned");
        let Some(ask) = ask else {
            return;
        };
        let shown = ask();
        if shown.is_empty() {
            set_text(
                window,
                ID_LIST,
                "coaxdに接続できないか、チューナーがありません。",
            );
            return;
        }
        let mut said = String::new();
        for one in shown {
            match one.card {
                Some(card) => {
                    if !said.is_empty() {
                        said.push_str("\r\n");
                    }
                    said.push_str(&card);
                }
                None => said.push_str(&format!("    {}", one.line)),
            }
            said.push_str("\r\n");
        }
        set_text(window, ID_LIST, &said);
    }

    unsafe extern "system" fn procedure(
        window: Handle,
        message: u32,
        first: usize,
        second: isize,
    ) -> isize {
        let holds = |id: usize| id == ID_LIST;
        if let Some(answer) = win::common(window, message, first, second, &holds) {
            return answer;
        }
        match message {
            WM_CREATE => {
                furnish(window);
                0
            }
            WM_COMMAND => {
                match first & 0xffff {
                    ID_AGAIN => look(window),
                    ID_CLOSE => unsafe {
                        win::DestroyWindow(window);
                    },
                    _ => {}
                }
                0
            }
            WM_PAINT => {
                let mut paint: win::Paint = unsafe { core::mem::zeroed() };
                let context = unsafe { win::BeginPaint(window, &raw mut paint) };
                win::outline(window, context, ID_LIST);
                unsafe { win::EndPaint(window, &raw const paint) };
                0
            }
            WM_DESTROY => {
                if held() == window {
                    *WINDOW.lock().expect("the window is not poisoned") = 0;
                }
                0
            }
            _ => unsafe { win::DefWindowProcW(window, message, first, second) },
        }
    }
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use windows::{close, look_again, open};

#[cfg(not(windows))]
mod elsewhere {
    use core::ffi::c_void;

    pub fn open(_owner: *mut c_void, _ask: fn() -> Vec<super::Shown>) {}
    pub fn look_again() {}
    pub fn close() {}
}

#[cfg(not(windows))]
#[allow(unused_imports)]
pub use elsewhere::{close, look_again, open};

#[cfg(test)]
mod tests {
    use super::*;

    /// The routes the tests read a tuner against: one on the cable, one that
    /// names no tuner at all.
    fn routes() -> Vec<Route> {
        coax_settings::setup::Setup::read("[Tuners]\nisdbc = ISDBC, tbs:*\nisdbs = ISDBS\n").routes
    }

    fn board(name: &str, model: &str, tuners: Vec<Held>) -> Board {
        Board {
            name: name.to_owned(),
            model: model.to_owned(),
            tuners,
            ..Board::default()
        }
    }

    fn held(id: &str, leased: bool, tuned: Vec<(&str, &str)>) -> Held {
        Held {
            tuner: Tuner {
                id: id.to_owned(),
                leased,
                name: format!("tuner {}", id.chars().last().unwrap_or('0')),
                ..Tuner::default()
            },
            tuned: tuned
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            ..Held::default()
        }
    }

    const CHANNELS: &str = "\
[Channels CATV]
4, 16626, BS15/16626, isdbc, DVBC/ANNEX_A, 471142857, ts:2
4, 18675, BS15/18675, isdbc, DVBC/ANNEX_A, 471142857, ts:3
";

    /// The boards of one model are one card in the list, whichever boards the
    /// daemon split them across, and a tuner in use says what it is on.
    #[test]
    fn the_tuners_read_as_cards_with_what_each_is_doing() {
        let known = Defined::parse(CHANNELS);
        let boards = [
            board(
                "tbs:a",
                "Example 1234 Tuner 0",
                vec![held(
                    "tbs:a:0",
                    true,
                    vec![
                        ("DELIVERY_SYSTEM", "DVBC/ANNEX_A"),
                        ("FREQUENCY", "471142857"),
                        ("TSMF_REL_TS", "3"),
                    ],
                )],
            ),
            board(
                "tbs:b",
                "Example 1234 Tuner 1",
                vec![held("tbs:b:0", false, vec![])],
            ),
        ];
        let shown = shown(&boards, &known, &routes());
        assert_eq!(shown.len(), 3, "one heading and two tuners");
        assert_eq!(shown[0].card.as_deref(), Some("Example 1234"));
        assert!(shown[1].busy);
        assert!(shown[1].line.contains("使用中"));
        // What it is tuned as comes first: the space a cable's channel is
        // filed under is the platform's, not the lead's.
        assert!(
            shown[1].line.contains("ISDB-C  CATV / BS15/18675"),
            "the slot picks the line: {}",
            shown[1].line
        );
        assert!(shown[2].line.contains("空き"));
    }

    /// A tune no line in the file matches still says where the tuner is.
    #[test]
    fn a_tune_the_file_does_not_know_is_written_as_it_stands() {
        let known = Defined::parse(CHANNELS);
        let boards = [board(
            "pt3:a",
            "Example PT3 satellite tuner 1",
            vec![held(
                "pt3:a:0",
                true,
                vec![("DELIVERY_SYSTEM", "ISDBS"), ("FREQUENCY", "1049480")],
            )],
        )];
        let shown = shown(&boards, &known, &routes());
        assert_eq!(shown[0].card.as_deref(), Some("Example PT3 satellite"));
        assert!(
            shown[1].line.contains("ISDB-S 1049480"),
            "{}",
            shown[1].line
        );
    }

    /// A card's tuners come out in the order their names number them,
    /// whatever order the daemon listed the boards in, and each line says
    /// which routes may reach for it.
    #[test]
    fn a_cards_tuners_are_in_order_and_say_which_routes_may_use_them() {
        let known = Defined::parse(CHANNELS);
        let boards = [
            board("tbs:c", "Example", vec![held("tbs:c:2", false, vec![])]),
            board("tbs:a", "Example", vec![held("tbs:a:0", false, vec![])]),
            board("tbs:b", "Example", vec![held("tbs:b:1", false, vec![])]),
        ];
        let shown = shown(&boards, &known, &routes());
        let lines: Vec<&str> = shown[1..].iter().map(|one| one.line.as_str()).collect();
        assert!(lines[0].starts_with("tbs:a:0"), "{lines:?}");
        assert!(lines[1].starts_with("tbs:b:1"), "{lines:?}");
        assert!(lines[2].starts_with("tbs:c:2"), "{lines:?}");
        // The cable route names these by pattern; the satellite route names
        // no tuner, and a tuner that said nothing about what it receives is
        // one it would take.
        assert!(lines[0].ends_with("[isdbc, isdbs]"), "{}", lines[0]);
    }

    /// A tuner nobody holds says so and nothing else about a channel.
    #[test]
    fn a_free_tuner_is_on_no_channel() {
        let known = Defined::parse(CHANNELS);
        let boards = [board(
            "tbs:a",
            "Example",
            vec![held("tbs:a:0", false, vec![])],
        )];
        let shown = shown(&boards, &known, &routes());
        assert!(!shown[1].busy);
        assert!(!shown[1].line.contains('/'));
    }

    /// A viewer refused a channel is asking whether they can outrank whoever
    /// has the tuner, and only a rank somebody named has an answer.
    #[test]
    fn a_tuner_says_what_rank_holds_it_where_that_was_said() {
        let mut free = held("tbs:abc:0", false, vec![]);
        free.priority = None;
        assert_eq!(holding(&free), "  空き");

        let mut ordinary = held("tbs:abc:0", true, vec![]);
        ordinary.priority = Some(coax::daemon::DEFAULT_PRIORITY);
        assert_eq!(holding(&ordinary), "  使用中");

        // A daemon too old to say is the same as one saying the default.
        let mut silent = held("tbs:abc:0", true, vec![]);
        silent.priority = None;
        assert_eq!(holding(&silent), "  使用中");

        let mut raised = held("tbs:abc:0", true, vec![]);
        raised.priority = Some(7);
        assert_eq!(holding(&raised), "  使用中(優先度 7)");

        // Below the default is worth saying too: it means the tuner is one
        // anybody can take.
        let mut lowered = held("tbs:abc:0", true, vec![]);
        lowered.priority = Some(-2);
        assert_eq!(holding(&lowered), "  使用中(優先度 -2)");
    }
}

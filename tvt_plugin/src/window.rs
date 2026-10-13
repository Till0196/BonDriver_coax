// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The plugin's own window and the sweep's.
//!
//! The plugin's own is where a viewer starts: the routes the settings name,
//! each with a box so that a sweep can be asked for some of them and not the
//! rest, and the buttons that start a sweep, open the settings window, and open
//! the file itself.
//!
//! The other is the one that matters. Looking at a cable takes minutes, and a
//! viewer watching nothing happen has no way of telling a slow sweep from a
//! broken one, so the sweep puts up a window of its own with what it is tuned
//! to, what the tuner is seeing, how fast the bytes are coming and how far
//! along it is.
//!
//! Both belong to the thread that draws the host's window, because that is the
//! thread whose message loop runs them. The sweep, on a thread of its own,
//! never touches them: it leaves what it has to say behind and knocks.

use std::sync::Mutex;

/// What the sweep leaves behind for its window to pick up.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default, Clone)]
pub struct Progress {
    /// What is being looked at now.
    pub what: String,
    pub done: usize,
    pub total: usize,
    pub cnr: Option<f32>,
    pub strength: Option<f32>,
    pub mbps: Option<f32>,
    /// The lines to add to the account of what has been found.
    pub lines: Vec<String>,
    /// Set when the sweep is over, with what it came to.
    pub finished: Option<String>,
    /// Whether what it found was written down and the host made to read it. A
    /// walk that was thrown away changed nothing, and saying otherwise tells a
    /// viewer their channels have moved when they have not.
    pub applied: bool,
}

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);

/// What the buttons answer. The window hands these back to the plugin, which
/// treats them as it treats a command from the host.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct Buttons {
    pub discover: i32,
    pub settings: i32,
    pub tuners: i32,
    pub open: i32,
}

/// The settings files a viewer may choose between, which of them is in use, and
/// what to call when they pick another.
///
/// A BonDriver is installed by copying it and adding a suffix -- one copy per
/// lead -- and each copy has a settings file of its own. The host has one
/// driver open at a time, so without this the plugin could only ever look at
/// that one's file.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct SettingsFiles {
    /// The names, as a viewer reads them. Empty when the host has no driver
    /// open, and there is then nothing to choose between.
    pub names: Vec<String>,
    /// Which of them is in use.
    pub at: usize,
    /// Called with the place of the file a viewer picked.
    pub pick: fn(usize),
}

/// A route as the window lists it: the label a sweep is asked for it by,
/// and the columns a viewer reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteShown {
    pub label: String,
    pub system: String,
    /// The tuners it is on and, where it has them, the bands it walks.
    pub rest: String,
}

/// Says what is being looked at now, from whichever thread the work is on.
pub fn step(
    what: &str,
    done: usize,
    total: usize,
    cnr: Option<f32>,
    strength: Option<f32>,
    mbps: Option<f32>,
) {
    {
        let mut progress = PROGRESS.lock().expect("the progress is not poisoned");
        let progress = progress.get_or_insert_with(Progress::default);
        // A measurement belongs to the carrier it was taken on. Moving to the
        // next one clears it rather than leaving the last carrier's figures
        // under this one's name.
        if progress.what != what {
            progress.cnr = None;
            progress.strength = None;
            progress.mbps = None;
        }
        progress.what = what.to_owned();
        progress.done = done;
        progress.total = total;
        if cnr.is_some() || strength.is_some() || mbps.is_some() {
            progress.cnr = cnr;
            progress.strength = strength;
            progress.mbps = mbps;
        }
    }
    knock();
}

/// Adds a line to the account of what has been found.
pub fn found(line: impl Into<String>) {
    {
        let mut progress = PROGRESS.lock().expect("the progress is not poisoned");
        let progress = progress.get_or_insert_with(Progress::default);
        progress.lines.push(line.into());
        // A window left open through a long sweep should not hold every line
        // of it.
        const MOST_HELD: usize = 500;
        if progress.lines.len() > MOST_HELD {
            progress.lines.drain(..progress.lines.len() - MOST_HELD);
        }
    }
    knock();
}

/// Says the sweep is over, and what came of it.
pub fn finished(said: impl Into<String>, applied: bool) {
    {
        let mut progress = PROGRESS.lock().expect("the progress is not poisoned");
        let progress = progress.get_or_insert_with(Progress::default);
        progress.finished = Some(said.into());
        progress.applied = applied;
    }
    knock();
}

/// Forgets the last sweep, for the start of the next.
pub fn starting() {
    *PROGRESS.lock().expect("the progress is not poisoned") = Some(Progress::default());
}

#[cfg(windows)]
mod windows {
    use super::{Buttons, PROGRESS, RouteShown, SettingsFiles};
    use std::sync::Mutex;

    use coax_settings::wide::Wide;

    use crate::win::{
        self, BAR_HIGH, BM_SETCHECK, BS_AUTOCHECKBOX, BST_CHECKED, CB_ADDSTRING, CB_GETCURSEL,
        CB_RESETCONTENT, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST, EM_GETSEL, EM_REPLACESEL,
        EM_SCROLLCARET, EM_SETSEL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, Handle, Metrics,
        PBM_SETPOS, PBM_SETRANGE32, SS_CENTERIMAGE, SW_HIDE, SW_SHOW, SendMessageW, WM_ARRIVED,
        WM_COMMAND, WM_CREATE, WM_DESTROY, WM_GETTEXTLENGTH, WM_PAINT, WM_PICKED, WM_RELOAD,
        WS_TABSTOP, WS_VSCROLL, control, item, set_text, there, ticked,
    };

    /// How wide the windows are. The height follows from what is in them.
    const BUTTONS_WIDE: i32 = 560;
    const SWEEP_WIDE: i32 = 540;
    /// A label that is cut short with dots where it does not fit, rather
    /// than run under the next control.
    const SS_ENDELLIPSIS: u32 = 0x4000;

    /// The controls, by the number a message names them with.
    const ID_WHAT: usize = 100;
    const ID_MEASURED: usize = 101;
    const ID_COUNT: usize = 102;
    const ID_BAR: usize = 103;
    const ID_LIST: usize = 104;
    const ID_STOP: usize = 105;
    /// Stops the sweep and keeps what it has found so far.
    const ID_DONE: usize = 109;
    const ID_SAID: usize = 106;
    const ID_ROUTES_LABEL: usize = 119;
    /// The settings file in use, and the label beside it.
    const ID_FILE: usize = 120;
    const ID_FILE_LABEL: usize = 121;
    /// The routes' boxes carry the route's place in the list in their
    /// number, and what stands beside each box the same place again.
    const ID_ROUTE: usize = 200;
    const ID_ROUTE_SYSTEM: usize = 300;
    const ID_ROUTE_REST: usize = 400;
    /// The tuner each route is to be swept on: what the settings say, or one
    /// tuner a viewer named for this sweep.
    const ID_ROUTE_TUNER: usize = 500;
    /// How many routes the window has room to number.
    const MOST_ROUTES: usize = 64;

    /// The buttons that stand for a command carry the command in their number.
    const ID_COMMAND: usize = 1000;

    /// The two windows, and what the plugin asked to be told.
    static BUTTONS_WINDOW: Mutex<usize> = Mutex::new(0);
    static SWEEP_WINDOW: Mutex<usize> = Mutex::new(0);
    static BUTTONS: Mutex<Option<Buttons>> = Mutex::new(None);
    static ACT: Mutex<Option<fn(i32)>> = Mutex::new(None);
    /// The routes the window lists, in the settings' order.
    static ROUTES: Mutex<Vec<RouteShown>> = Mutex::new(Vec::new());
    /// Why there are no routes to list, where it is not simply that the
    /// settings have none: the driver the host has open is not this one, or its
    /// file could not be read. Said in place of the invitation to add one,
    /// which would send a viewer to the wrong file.
    static TROUBLE: Mutex<Option<String>> = Mutex::new(None);
    /// The routes a viewer has unticked, by label: kept apart from the boxes so
    /// that it survives the list being made again when the settings change.
    static UNTICKED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Which tuner a viewer chose for a route, by the route's label. Kept apart
    /// from the drop-downs so that it survives the rows being made again.
    static CHOSEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    /// The tuners the drop-down offers, in the order it offers them: what to
    /// ask the daemon for, and what a viewer reads. The first line is "whatever
    /// the settings say".
    static TUNERS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    /// The settings files to choose between, which one is in use, and what to
    /// call when a viewer picks another.
    static FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static FILE_AT: Mutex<usize> = Mutex::new(0);
    static PICK: Mutex<Option<fn(usize)>> = Mutex::new(None);
    /// The route labels the controls now on the window were made for.
    ///
    /// Not the same list as ROUTES: what a viewer ticked and picked is read
    /// back off the controls before they are taken down, and by then ROUTES may
    /// already be the next file's. Reading the new labels off the old controls
    /// would file one file's choices under another's route names.
    static DRAWN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static STOP: Mutex<Option<fn()>> = Mutex::new(None);
    static DONE: Mutex<Option<fn()>> = Mutex::new(None);

    /// Which window is being made at this moment, so that the one message
    /// that arrives before its handle is known can tell which it is.
    static MAKING: Mutex<u8> = Mutex::new(0);
    const MAKING_BUTTONS: u8 = 1;
    const MAKING_SWEEP: u8 = 2;

    /// Looks at the system's colours again and paints what is open in them.
    /// Called when the host says its own colours changed.
    pub fn look_again() {
        win::forget_colours();
        for which in [&BUTTONS_WINDOW, &SWEEP_WINDOW] {
            win::redress(held(which));
        }
        crate::settings::look_again();
    }

    fn held(which: &Mutex<usize>) -> Handle {
        *which.lock().expect("the window is not poisoned") as Handle
    }

    /// Opens the plugin's own window: the routes, and the buttons.
    pub fn open(
        owner: Handle,
        buttons: Buttons,
        act: fn(i32),
        routes: Vec<RouteShown>,
        trouble: Option<String>,
        tuners: Vec<(String, String)>,
        files: SettingsFiles,
    ) {
        *BUTTONS.lock().expect("the buttons are not poisoned") = Some(buttons);
        *ACT.lock().expect("the action is not poisoned") = Some(act);
        *TUNERS.lock().expect("the tuners are not poisoned") = tuners;
        *ROUTES.lock().expect("the routes are not poisoned") = routes;
        *TROUBLE.lock().expect("the trouble is not poisoned") = trouble;
        *PICK.lock().expect("the pick is not poisoned") = Some(files.pick);
        *FILES.lock().expect("the files are not poisoned") = files.names;
        *FILE_AT.lock().expect("the file is not poisoned") = files.at;

        let existing = held(&BUTTONS_WINDOW);
        if there(existing) {
            // The settings may have changed, the daemon may have been
            // started, or a card plugged in, since this was last opened.
            furnish_buttons(existing);
            win::to_the_front(existing);
            return;
        }
        let name = Wide::new("coax_plugin_window");
        if !win::class(&name, procedure) {
            return;
        }
        *MAKING.lock().expect("the flag is not poisoned") = MAKING_BUTTONS;
        let (width, height) = buttons_size();
        let window = win::make(owner, &name, "coax", width, height);
        *MAKING.lock().expect("the flag is not poisoned") = 0;
        if window.is_null() {
            return;
        }
        *BUTTONS_WINDOW.lock().expect("the window is not poisoned") = window as usize;
        unsafe { win::ShowWindow(window, SW_SHOW) };
        win::to_the_front(window);
    }

    /// Puts a new list of settings files in front of the viewer, for when one
    /// has been written that was not there before.
    pub fn set_settings_files(names: Vec<String>, at: usize) {
        *FILES.lock().expect("the files are not poisoned") = names;
        *FILE_AT.lock().expect("the file is not poisoned") = at;
    }

    /// Puts a new list of routes in front of the viewer, for when the
    /// settings have been written again.
    pub fn set_routes(routes: Vec<RouteShown>, trouble: Option<String>) {
        *ROUTES.lock().expect("the routes are not poisoned") = routes;
        *TROUBLE.lock().expect("the trouble is not poisoned") = trouble;
        let window = held(&BUTTONS_WINDOW);
        if there(window) {
            furnish_buttons(window);
        }
    }

    /// Opens the window a sweep reports into, emptied and ready.
    pub fn open_sweep(owner: Handle, stop: fn(), done: fn()) {
        *STOP.lock().expect("the action is not poisoned") = Some(stop);
        *DONE.lock().expect("the action is not poisoned") = Some(done);
        let existing = held(&SWEEP_WINDOW);
        if there(existing) {
            clear_sweep(existing);
            win::to_the_front(existing);
            return;
        }
        let name = Wide::new("coax_plugin_sweep");
        if !win::class(&name, procedure) {
            return;
        }
        *MAKING.lock().expect("the flag is not poisoned") = MAKING_SWEEP;
        let (width, height) = sweep_size();
        let window = win::make(owner, &name, "coax: チャンネルスキャン中...", width, height);
        *MAKING.lock().expect("the flag is not poisoned") = 0;
        if window.is_null() {
            return;
        }
        *SWEEP_WINDOW.lock().expect("the window is not poisoned") = window as usize;
        unsafe { win::ShowWindow(window, SW_SHOW) };
        win::to_the_front(window);
    }

    fn clear_sweep(window: Handle) {
        set_text(window, ID_LIST, "");
        set_text(window, ID_WHAT, "");
        set_text(window, ID_MEASURED, "");
        set_text(window, ID_COUNT, "");
        set_text(window, ID_SAID, "");
        set_text(window, ID_STOP, "キャンセル");
        set_text(window, ID_DONE, "完了");
        for id in [ID_STOP, ID_DONE] {
            win::enable(window, id, true);
            win::show(window, id, true);
        }
    }

    /// Puts the tuners in one route's drop-down, with what the settings say
    /// at the top and whatever was already chosen for that route kept.
    fn fill_tuners(window: Handle, at: usize, label: &str) {
        let list = item(window, ID_ROUTE_TUNER + at);
        if list.is_null() {
            return;
        }
        unsafe { SendMessageW(list, CB_RESETCONTENT, 0, 0) };
        let tuners = TUNERS.lock().expect("the tuners are not poisoned");
        let names = std::iter::once("設定に従う".to_owned())
            .chain(tuners.iter().map(|(_, shown)| shown.clone()));
        for name in names {
            let text = Wide::new(&name);
            unsafe { SendMessageW(list, CB_ADDSTRING, 0, text.as_ptr() as isize) };
        }
        // What the viewer chose for this route survives the list arriving
        // afterwards, and the row being made again.
        let put_back = CHOSEN
            .lock()
            .expect("the choices are not poisoned")
            .iter()
            .find(|(named, _)| named == label)
            .and_then(|(_, id)| tuners.iter().position(|(known, _)| known == id))
            .map_or(0, |at| at + 1);
        unsafe { SendMessageW(list, CB_SETCURSEL, put_back, 0) };
    }

    /// Writes down which tuner each route's drop-down is showing, so that
    /// the rows can be made again without losing it.
    fn remember_tuners(window: Handle) {
        let drawn = DRAWN.lock().expect("the drawn routes are not poisoned");
        let tuners = TUNERS.lock().expect("the tuners are not poisoned");
        let mut chosen = CHOSEN.lock().expect("the choices are not poisoned");
        for (at, label) in drawn.iter().enumerate().take(MOST_ROUTES) {
            let list = item(window, ID_ROUTE_TUNER + at);
            if list.is_null() {
                continue;
            }
            let picked = unsafe { SendMessageW(list, CB_GETCURSEL, 0, 0) };
            chosen.retain(|(named, _)| named != label);
            if picked > 0 {
                if let Some((id, _)) = tuners.get(picked as usize - 1) {
                    chosen.push((label.clone(), id.clone()));
                }
            }
        }
    }

    /// Puts a list of tuners in front of the viewer, from whichever thread
    /// found them out.
    pub fn set_tuners(names: Vec<(String, String)>) {
        *TUNERS.lock().expect("the tuners are not poisoned") = names;
        let window = held(&BUTTONS_WINDOW);
        if there(window) {
            unsafe { win::PostMessageW(window, WM_ARRIVED, 0, 0) };
        }
    }

    /// Which tuner a viewer chose for each route, by the route's label. Empty
    /// where the window is not open, or where every route was left to the
    /// settings.
    pub fn chosen_tuners() -> crate::discover::Chosen {
        let window = held(&BUTTONS_WINDOW);
        if !there(window) {
            return crate::discover::Chosen::new();
        }
        remember_tuners(window);
        CHOSEN
            .lock()
            .expect("the choices are not poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Which routes a viewer left ticked: everything where the window is not
    /// open, or where nothing was unticked.
    pub fn chosen_routes() -> crate::discover::What {
        let window = held(&BUTTONS_WINDOW);
        if !there(window) {
            return crate::discover::What::Everything;
        }
        let drawn = DRAWN.lock().expect("the drawn routes are not poisoned");
        let labels: Vec<String> = drawn
            .iter()
            .enumerate()
            .filter(|(at, _)| ticked(window, ID_ROUTE + at))
            .map(|(_, label)| label.clone())
            .collect();
        if labels.len() == drawn.len() {
            crate::discover::What::Everything
        } else {
            crate::discover::What::Only(labels)
        }
    }

    /// Writes down which routes are unticked, so that the list can be made
    /// again without losing it.
    fn remember_ticks(window: Handle) {
        let drawn = DRAWN.lock().expect("the drawn routes are not poisoned");
        let mut unticked = UNTICKED.lock().expect("the ticks are not poisoned");
        for (at, label) in drawn.iter().enumerate().take(MOST_ROUTES) {
            let box_ = item(window, ID_ROUTE + at);
            if box_.is_null() {
                continue;
            }
            let on = ticked(window, ID_ROUTE + at);
            unticked.retain(|named| named != label);
            if !on {
                unticked.push(label.clone());
            }
        }
    }

    /// Puts the controls on the plugin's own window, from what it has to show
    /// now. Called again whenever that changes: the controls are taken off and
    /// put back.
    fn furnish_buttons(window: Handle) {
        let buttons = BUTTONS.lock().expect("the buttons are not poisoned");
        let Some(buttons) = *buttons else {
            return;
        };
        remember_ticks(window);
        remember_tuners(window);
        win::strip(window);

        let m = win::metrics();
        let inner = buttons_wide(&m) - m.margin * 2;
        let routes = ROUTES.lock().expect("the routes are not poisoned").clone();
        let unticked = UNTICKED.lock().expect("the ticks are not poisoned").clone();
        let mut y = m.margin;
        // Which file everything below is read from and written to. First,
        // because it settles what the rest of the window means: the routes, the
        // settings button and the sweep all belong to one file.
        let files = FILES.lock().expect("the files are not poisoned").clone();
        if !files.is_empty() {
            let label_wide = m.line * 6;
            control(
                window,
                "STATIC",
                "設定ファイル",
                SS_CENTERIMAGE,
                (m.margin, y, label_wide, m.row),
                ID_FILE_LABEL,
            );
            // Tall, because the height a drop-down is given is the height of
            // the list it drops down, not of the box itself.
            control(
                window,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP,
                (m.margin + label_wide, y, inner - label_wide, m.row * 12),
                ID_FILE,
            );
            let list = item(window, ID_FILE);
            for name in &files {
                let text = Wide::new(name);
                unsafe { SendMessageW(list, CB_ADDSTRING, 0, text.as_ptr() as isize) };
            }
            let at = *FILE_AT.lock().expect("the file is not poisoned");
            unsafe { SendMessageW(list, CB_SETCURSEL, at.min(files.len() - 1), 0) };
            y += m.row + m.gap;
        }
        control(
            window,
            "STATIC",
            &if routes.is_empty() {
                match &*TROUBLE.lock().expect("the trouble is not poisoned") {
                    Some(said) => said.clone(),
                    None => {
                        "経路がありません。「経路と空間の設定...」から追加してください。".to_owned()
                    }
                }
            } else {
                "スキャンする経路と、それに対応するチューナー".to_owned()
            },
            SS_CENTERIMAGE,
            (m.margin, y, inner, m.row),
            ID_ROUTES_LABEL,
        );
        y += m.row;
        let label_wide = m.line * 6;
        let system_wide = m.line * 7;
        let field = m.row + 4;
        // The picker takes the right of the row; what the settings say about
        // the route is written in what is left between them.
        let tuner_wide = m.line * 13;
        let rest_wide = inner - label_wide - system_wide - tuner_wide - m.gap;
        for (at, route) in routes.iter().enumerate().take(MOST_ROUTES) {
            let middle = y + (field - m.row) / 2;
            control(
                window,
                "BUTTON",
                &route.label,
                WS_TABSTOP | BS_AUTOCHECKBOX,
                (m.margin, middle, label_wide, m.row),
                ID_ROUTE + at,
            );
            if !unticked.contains(&route.label) {
                let box_ = item(window, ID_ROUTE + at);
                unsafe { SendMessageW(box_, BM_SETCHECK, BST_CHECKED, 0) };
            }
            control(
                window,
                "STATIC",
                &route.system,
                SS_CENTERIMAGE | SS_ENDELLIPSIS,
                (m.margin + label_wide, middle, system_wide, m.row),
                ID_ROUTE_SYSTEM + at,
            );
            control(
                window,
                "STATIC",
                &route.rest,
                SS_CENTERIMAGE | SS_ENDELLIPSIS,
                (
                    m.margin + label_wide + system_wide,
                    middle,
                    rest_wide,
                    m.row,
                ),
                ID_ROUTE_REST + at,
            );
            // Tall, because the height a drop-down is given is the height of
            // the list it drops down, not of the box itself.
            control(
                window,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP,
                (m.margin + inner - tuner_wide, y, tuner_wide, m.row * 12),
                ID_ROUTE_TUNER + at,
            );
            fill_tuners(window, at, &route.label);
            y += field;
        }
        y += m.gap;

        // The same names the commands are registered under, so that a
        // viewer who found one in the host's own menu recognises it here.
        let row = [
            ("チャンネルスキャン", buttons.discover),
            ("経路と空間の設定...", buttons.settings),
            ("チューナーの状態...", buttons.tuners),
            ("設定ファイルを開く", buttons.open),
        ];
        let wide = (inner - m.gap * 3) / 4;
        let mut x = m.margin;
        for (text, command) in row {
            control(
                window,
                "BUTTON",
                text,
                WS_TABSTOP,
                (x, y, wide, m.button),
                ID_COMMAND + command as usize,
            );
            x += wide + m.gap;
        }
        win::enable(
            window,
            ID_COMMAND + buttons.discover as usize,
            !routes.is_empty(),
        );
        *DRAWN.lock().expect("the drawn routes are not poisoned") = routes
            .iter()
            .take(MOST_ROUTES)
            .map(|route| route.label.clone())
            .collect();
        win::dress(window);
        // The rows are as many as the routes, so the window is a different size
        // every time this runs: a route added leaves the buttons off the bottom
        // of a window that was made for one route fewer.
        let (width, height) = buttons_size();
        win::resize(window, width, height);
    }

    /// How wide the two windows are, and how tall they come out: the room the
    /// controls need, which the frame is added to when the window is made.
    fn buttons_size() -> (i32, i32) {
        let m = win::metrics();
        let routes = ROUTES
            .lock()
            .expect("the routes are not poisoned")
            .len()
            .min(MOST_ROUTES) as i32;
        // A route's picker is a line taller than a label, and the heading of
        // the columns is a line of its own.
        let mut height = m.margin * 2 + m.row * 2 + (m.row + 4) * routes + m.gap + m.button;
        if !FILES.lock().expect("the files are not poisoned").is_empty() {
            height += m.row + m.gap;
        }
        (buttons_wide(&m), height)
    }

    /// How wide the plugin's own window is: the four buttons across the
    /// bottom, and never narrower than a route's row reads well at.
    fn buttons_wide(m: &Metrics) -> i32 {
        (m.margin * 2 + (m.line * 9 + m.gap) * 4).max(BUTTONS_WIDE)
    }

    fn sweep_size() -> (i32, i32) {
        let m = win::metrics();
        let inner = SWEEP_WIDE;
        let height = m.margin * 2 + m.row * 3 + m.gap * 4 + BAR_HIGH + list_high(&m) + m.button;
        (inner, height)
    }

    /// The list is what the window is mostly made of, so it takes whatever is
    /// left over -- counted here as a number of lines so that it grows with the
    /// font.
    fn list_high(m: &Metrics) -> i32 {
        m.line * 13
    }

    fn furnish_sweep(window: Handle) {
        let m = win::metrics();
        let inner = SWEEP_WIDE - m.margin * 2;
        let mut y = m.margin;
        control(
            window,
            "STATIC",
            "",
            0,
            (m.margin, y, inner, m.row),
            ID_WHAT,
        );
        y += m.row;
        control(
            window,
            "STATIC",
            "",
            0,
            (m.margin, y, inner, m.row),
            ID_MEASURED,
        );
        y += m.row + m.gap;
        control(
            window,
            "msctls_progress32",
            "",
            0,
            (m.margin, y, inner, BAR_HIGH),
            ID_BAR,
        );
        y += BAR_HIGH + m.gap;
        control(
            window,
            "STATIC",
            "",
            0,
            (m.margin, y, inner, m.row),
            ID_COUNT,
        );
        y += m.row + m.gap;
        control(
            window,
            "EDIT",
            "",
            WS_VSCROLL | WS_TABSTOP | ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL,
            (m.margin, y, inner, list_high(&m)),
            ID_LIST,
        );
        y += list_high(&m) + m.gap;
        let wide = m.line * 7;
        control(
            window,
            "STATIC",
            "",
            SS_CENTERIMAGE,
            (m.margin, y, inner - (wide + m.gap) * 2, m.button),
            ID_SAID,
        );
        // Finish keeps what has been found so far and cancel throws it away,
        // which is the choice the host's own channel scan offers: a walk is
        // minutes long and a viewer who has seen what they were waiting for
        // should not sit out the rest.
        control(
            window,
            "BUTTON",
            "完了",
            WS_TABSTOP,
            (m.margin + inner - (wide + m.gap) - wide, y, wide, m.button),
            ID_DONE,
        );
        control(
            window,
            "BUTTON",
            "キャンセル",
            WS_TABSTOP,
            (m.margin + inner - wide, y, wide, m.button),
            ID_STOP,
        );
    }

    unsafe extern "system" fn procedure(
        window: Handle,
        message: u32,
        first: usize,
        second: isize,
    ) -> isize {
        if let Some(answer) = win::common(window, message, first, second, &|id| id == ID_LIST) {
            return answer;
        }
        match message {
            WM_CREATE => {
                // The handle is not written down until the call that made the
                // window comes back, so which window this is comes from what
                // was being made.
                let making = *MAKING.lock().expect("the flag is not poisoned");
                if making == MAKING_SWEEP {
                    furnish_sweep(window);
                } else {
                    furnish_buttons(window);
                }
                win::dress(window);
                0
            }
            WM_COMMAND => {
                let id = first & 0xffff;
                // A viewer picked another settings file. Acting on it here
                // would rebuild the controls from inside the drop-down's own
                // notification, so the work is posted and done once this
                // message is out of the way.
                if id == ID_FILE && (first >> 16) as u32 == CBN_SELCHANGE {
                    unsafe { win::PostMessageW(window, WM_PICKED, 0, 0) };
                    return 0;
                }
                match id {
                    ID_STOP => {
                        let stop = *STOP.lock().expect("the action is not poisoned");
                        if let Some(stop) = stop {
                            stop();
                        }
                    }
                    ID_DONE => {
                        let done = *DONE.lock().expect("the action is not poisoned");
                        if let Some(done) = done {
                            done();
                        }
                    }
                    _ if id >= ID_COMMAND => {
                        let act = *ACT.lock().expect("the action is not poisoned");
                        if let Some(act) = act {
                            act((id - ID_COMMAND) as i32);
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_PICKED => {
                // Only the plugin's own window has the picker; the two share
                // this procedure.
                if held(&BUTTONS_WINDOW) != window {
                    return 0;
                }
                let list = item(window, ID_FILE);
                if list.is_null() {
                    return 0;
                }
                let at = unsafe { SendMessageW(list, CB_GETCURSEL, 0, 0) };
                let pick = *PICK.lock().expect("the pick is not poisoned");
                if let (Some(pick), true) = (pick, at >= 0) {
                    // What the viewer had chosen for the routes of the file
                    // they are leaving is not about the file they are going to.
                    remember_tuners(window);
                    pick(at as usize);
                }
                0
            }
            WM_RELOAD => {
                let reload = RELOAD.lock().expect("the action is not poisoned").take();
                if let Some(reload) = reload {
                    reload();
                }
                0
            }
            WM_ARRIVED => {
                if held(&BUTTONS_WINDOW) == window {
                    // The list arrived after the rows were made, so they are
                    // made again: each row's picker is filled from it.
                    furnish_buttons(window);
                } else {
                    draw(window);
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
                if held(&BUTTONS_WINDOW) == window {
                    remember_ticks(window);
                    *BUTTONS_WINDOW.lock().expect("the window is not poisoned") = 0;
                }
                if held(&SWEEP_WINDOW) == window {
                    *SWEEP_WINDOW.lock().expect("the window is not poisoned") = 0;
                }
                0
            }
            _ => unsafe { win::DefWindowProcW(window, message, first, second) },
        }
    }

    /// Puts what the sweep left behind into its window.
    fn draw(window: Handle) {
        let mut progress = PROGRESS.lock().expect("the progress is not poisoned");
        let Some(progress) = progress.as_mut() else {
            return;
        };
        set_text(window, ID_WHAT, &progress.what);
        let mut measured = String::new();
        if let Some(cnr) = progress.cnr {
            measured.push_str(&format!("CNR {cnr:.1} dB"));
        } else if let Some(strength) = progress.strength {
            // A backend that cannot measure a ratio can often still say how
            // strong the carrier is, and that is worth showing.
            measured.push_str(&format!("{strength:.1} dBm"));
        }
        if let Some(mbps) = progress.mbps {
            if !measured.is_empty() {
                measured.push_str("　　");
            }
            measured.push_str(&format!("{mbps:.1} Mbps"));
        }
        set_text(window, ID_MEASURED, &measured);
        if progress.total > 0 {
            set_text(
                window,
                ID_COUNT,
                &format!("{} / {}", progress.done, progress.total),
            );
            let bar = item(window, ID_BAR);
            unsafe {
                SendMessageW(bar, PBM_SETRANGE32, 0, progress.total as isize);
                SendMessageW(bar, PBM_SETPOS, progress.done, 0);
            }
        }
        let account = item(window, ID_LIST);
        let mut added = String::new();
        for line in progress.lines.drain(..) {
            added.push_str(&line);
            added.push_str("\r\n");
        }
        if !added.is_empty() {
            let text = Wide::new(&added);
            // At the end of what is there, which has to be said outright: the
            // insertion point is wherever the viewer last clicked.
            let mut from: u32 = 0;
            let mut to: u32 = 0;
            let length = unsafe { SendMessageW(account, WM_GETTEXTLENGTH, 0, 0) } as usize;
            unsafe {
                SendMessageW(
                    account,
                    EM_GETSEL,
                    (&raw mut from) as usize,
                    (&raw mut to) as isize,
                );
                SendMessageW(account, EM_SETSEL, length, length as isize);
                SendMessageW(account, EM_REPLACESEL, 0, text.as_ptr() as isize);
            }
            if from != to {
                // A viewer part way through selecting something keeps it: the
                // account is written to several times a second while a sweep
                // runs, and a selection taken away that often cannot be copied
                // at all.
                unsafe { SendMessageW(account, EM_SETSEL, from as usize, to as isize) };
            } else {
                unsafe { SendMessageW(account, EM_SCROLLCARET, 0, 0) };
            }
        }
        if let Some(said) = progress.finished.take() {
            set_text(window, ID_WHAT, &said);
            set_text(window, ID_MEASURED, "");
            if progress.applied {
                // The short of it goes beside the buttons, where there is room
                // for a few words; the rest goes into the account, which is as
                // wide as the window.
                set_text(window, ID_SAID, "チューナーを開き直しました");
                let text = Wide::new(
                    "\r\nチューナーを開き直しました。続けてTVTestのチャンネルスキャンを実行してください。\r\n",
                );
                let account = item(window, ID_LIST);
                let length = unsafe { SendMessageW(account, WM_GETTEXTLENGTH, 0, 0) } as usize;
                unsafe {
                    SendMessageW(account, EM_SETSEL, length, length as isize);
                    SendMessageW(account, EM_REPLACESEL, 0, text.as_ptr() as isize);
                    SendMessageW(account, EM_SCROLLCARET, 0, 0);
                }
            } else {
                set_text(window, ID_SAID, "");
            }
            // With nothing left to stop or to finish, one button closes the
            // window and the other has nothing to say.
            set_text(window, ID_STOP, "閉じる");
            unsafe { win::ShowWindow(item(window, ID_DONE), SW_HIDE) };
        }
    }

    /// Knocks on the sweep's window, so that what was left behind is picked
    /// up. Nothing is drawn on the thread that knocks.
    pub fn knock() {
        let window = held(&SWEEP_WINDOW);
        if there(window) {
            unsafe { win::PostMessageW(window, WM_ARRIVED, 0, 0) };
        }
    }

    /// What to run once a reload is asked for, on the thread the windows are
    /// drawn on.
    static RELOAD: Mutex<Option<fn()>> = Mutex::new(None);

    /// Has `reload` run on the thread that draws the windows, which is the
    /// host's own: by way of whichever of this plugin's windows is open. Says
    /// whether one was there to carry it.
    pub fn ask_reload(reload: fn()) -> bool {
        *RELOAD.lock().expect("the action is not poisoned") = Some(reload);
        for which in [&SWEEP_WINDOW, &BUTTONS_WINDOW] {
            let window = held(which);
            if there(window) && unsafe { win::PostMessageW(window, WM_RELOAD, 0, 0) } != 0 {
                return true;
            }
        }
        *RELOAD.lock().expect("the action is not poisoned") = None;
        false
    }

    /// Takes the windows down, which has to happen before this library is
    /// unloaded: their procedure lives here, and a window whose procedure has
    /// gone is a crash.
    pub fn close() {
        crate::settings::close();
        for which in [&BUTTONS_WINDOW, &SWEEP_WINDOW] {
            let window = held(which);
            if there(window) {
                unsafe { win::DestroyWindow(window) };
            }
            *which.lock().expect("the window is not poisoned") = 0;
        }
    }

    /// Closes the sweep's window and leaves the plugin's own where it is.
    pub fn close_sweep() {
        let window = held(&SWEEP_WINDOW);
        if there(window) {
            unsafe { win::DestroyWindow(window) };
        }
        *SWEEP_WINDOW.lock().expect("the window is not poisoned") = 0;
    }
}

#[cfg(windows)]
pub use windows::{
    ask_reload, chosen_routes, chosen_tuners, close, close_sweep, knock, look_again, open,
    open_sweep, set_routes, set_settings_files, set_tuners,
};

#[cfg(not(windows))]
mod elsewhere {
    use super::{Buttons, RouteShown, SettingsFiles};
    use core::ffi::c_void;

    pub fn open(
        _owner: *mut c_void,
        _buttons: Buttons,
        _act: fn(i32),
        _routes: Vec<RouteShown>,
        _trouble: Option<String>,
        _tuners: Vec<(String, String)>,
        _files: SettingsFiles,
    ) {
    }
    pub fn set_settings_files(_names: Vec<String>, _at: usize) {}
    pub fn chosen_tuners() -> crate::discover::Chosen {
        crate::discover::Chosen::new()
    }
    pub fn chosen_routes() -> crate::discover::What {
        crate::discover::What::Everything
    }
    pub fn set_tuners(_names: Vec<(String, String)>) {}
    pub fn set_routes(_routes: Vec<RouteShown>, _trouble: Option<String>) {}
    pub fn open_sweep(_owner: *mut c_void, _stop: fn(), _done: fn()) {}
    pub fn close() {}
    pub fn close_sweep() {}
    pub fn knock() {}
    pub fn ask_reload(_reload: fn()) -> bool {
        false
    }
    pub fn look_again() {}
}

#[cfg(not(windows))]
#[allow(unused_imports)]
pub use elsewhere::{
    ask_reload, chosen_routes, chosen_tuners, close, close_sweep, knock, look_again, open,
    open_sweep, set_routes, set_settings_files, set_tuners,
};

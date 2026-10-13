// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The settings window, and the window a route is added or changed in.
//!
//! The settings are two sections of the file beside the driver: the routes,
//! which say how television reaches this machine and on which tuners, and the
//! spaces, which say what the host shows and which routes each is cut out of.
//! The window shows the routes as a list and the spaces as a grid of boxes, and
//! writes the two sections back without touching the rest of the file. The
//! route window also sets, tuner by tuner, the supply on a dish's lead and the
//! gain, which go in `[TunerSettings]`. What it edits is
//! [`crate::editing::Editing`].
//!
//! Both windows belong to the thread that draws the host's window, and the
//! route window holds the settings window still while it is open.

#[cfg(windows)]
mod windows {
    use std::sync::Mutex;

    use coax::delivery;
    use coax::plan::Band;
    use coax_settings::config::Route;
    use coax_settings::setup;
    use coax_settings::wide::Wide;

    use crate::app::{ask, tell};
    use crate::editing::{self, Editing};
    use crate::win::{
        self, BS_AUTOCHECKBOX, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, Handle,
        SS_CENTERIMAGE, SendMessageW, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_PAINT, WS_TABSTOP,
        WS_VSCROLL, control, item, set_text, text_of, there, tick, ticked,
    };

    /// A list a viewer picks one line of, laid out on tab stops so that the
    /// columns line up, telling the window when a line is picked or
    /// double-clicked.
    const LBS_NOTIFY: u32 = 0x0001;
    const LBS_USETABSTOPS: u32 = 0x0080;
    const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
    const LB_ADDSTRING: u32 = 0x0180;
    const LB_RESETCONTENT: u32 = 0x0184;
    const LB_SETCURSEL: u32 = 0x0186;
    const LB_GETCURSEL: u32 = 0x0188;
    const LB_SETTABSTOPS: u32 = 0x0192;
    const LBN_DBLCLK: usize = 2;
    const CBN_SELCHANGE: usize = 1;
    const EN_CHANGE: usize = 0x0300;
    /// A one-line field a viewer types into, and one they cannot.
    const ES_AUTOHSCROLL: u32 = 0x0080;
    const WS_BORDER: u32 = 0x0080_0000;
    const SS_ENDELLIPSIS: u32 = 0x4000;
    const SS_LEFT: u32 = 0x0000;

    /// How wide the windows are. The settings window grows with the number
    /// of routes, which are its columns.
    const SETTINGS_WIDE: i32 = 600;
    const ROUTE_WIDE: i32 = 560;
    /// How many routes and spaces the windows have room to number.
    const MOST_ROUTES: usize = 32;
    const MOST_SPACES: usize = 32;
    const MOST_TUNERS: usize = 64;

    /// The settings window's controls.
    const ID_ROUTES_LABEL: usize = 100;
    const ID_LIST: usize = 101;
    const ID_ADD: usize = 102;
    const ID_EDIT: usize = 103;
    const ID_REMOVE: usize = 104;
    const ID_UP: usize = 105;
    const ID_DOWN: usize = 106;
    const ID_SPACES_LABEL: usize = 107;
    const ID_NOTE: usize = 108;
    const ID_OPEN: usize = 109;
    const ID_SAVE: usize = 110;
    const ID_CANCEL: usize = 111;
    /// A column's heading carries the route's place; a row's own box the
    /// space's place; and a box in the grid both.
    const ID_HEADING: usize = 200;
    const ID_SPACE: usize = 300;
    const ID_TICK: usize = 1000;

    /// The route window's controls.
    const ID_R_LABEL: usize = 500;
    const ID_R_SYSTEM: usize = 501;
    const ID_R_OTHER: usize = 502;
    const ID_R_TUNERS_LABEL: usize = 503;
    const ID_R_PATTERN: usize = 504;
    const ID_R_BANDS_LABEL: usize = 505;
    const ID_R_OK: usize = 507;
    const ID_R_CANCEL: usize = 508;
    const ID_R_NONE: usize = 509;
    const ID_R_BAND: usize = 520;
    const ID_R_CARD: usize = 600;
    const ID_R_TUNER: usize = 700;
    /// A tuner's supply and gain, beside its line.
    const ID_R_LNB: usize = 800;
    const ID_R_GAIN: usize = 900;
    const ID_R_LNB_LABEL: usize = 510;
    const ID_R_GAIN_LABEL: usize = 511;
    /// The bands, in the order they are shown and the order they are
    /// written, with what each is called.
    const BANDS: [(Band, &str); 4] = [
        (Band::Uhf, "UHF"),
        (Band::ExUhf, "旧UHF"),
        (Band::Vhf, "VHF"),
        (Band::Catv, "CATV"),
    ];
    /// The last line of the list of delivery systems, for one that is not
    /// offered.
    const OTHER: &str = "その他...";

    static SETTINGS_WINDOW: Mutex<usize> = Mutex::new(0);
    static ROUTE_WINDOW: Mutex<usize> = Mutex::new(0);
    static EDITING: Mutex<Option<Editing>> = Mutex::new(None);
    /// Which route the route window is changing; nothing while it is adding
    /// one.
    static CHANGING: Mutex<Option<usize>> = Mutex::new(None);
    /// What the plugin asked to be told: that the file was written, and
    /// that a viewer wants it opened.
    static SAVED: Mutex<Option<fn()>> = Mutex::new(None);
    static OPEN_FILE: Mutex<Option<fn()>> = Mutex::new(None);
    /// Set while the pattern field is being written by this code rather than
    /// the viewer, so that the change is not read back as theirs.
    static WRITING_PATTERN: Mutex<bool> = Mutex::new(false);
    /// Which line each of the route window's supply and gain lists was opened
    /// on, by its control: only one a viewer moved is written, so that a value
    /// the list does not offer is not lost by opening the window.
    static OPENED_ON: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

    static MAKING: Mutex<u8> = Mutex::new(0);
    const MAKING_SETTINGS: u8 = 1;
    const MAKING_ROUTE: u8 = 2;

    fn held(which: &Mutex<usize>) -> Handle {
        *which.lock().expect("the window is not poisoned") as Handle
    }

    /// Opens the settings window on what the file says.
    pub fn open(owner: Handle, editing: Editing, saved: fn(), open_file: fn()) {
        *SAVED.lock().expect("the action is not poisoned") = Some(saved);
        *OPEN_FILE.lock().expect("the action is not poisoned") = Some(open_file);
        let existing = held(&SETTINGS_WINDOW);
        if there(existing) {
            // An edit in progress is not thrown away for a second click on
            // the button that opened it.
            win::to_the_front(existing);
            return;
        }
        // Which file is being edited, because there may be several: a window
        // that did not say would leave a viewer editing a lead they had stopped
        // looking at.
        let title = match editing.file.file_name() {
            Some(name) => format!("coax: 経路と空間の設定 [{}]", name.to_string_lossy()),
            None => "coax: 経路と空間の設定".to_owned(),
        };
        *EDITING.lock().expect("the edit is not poisoned") = Some(editing);
        let name = Wide::new("coax_plugin_settings");
        if !win::class(&name, procedure) {
            return;
        }
        *MAKING.lock().expect("the flag is not poisoned") = MAKING_SETTINGS;
        let (width, height) = settings_size();
        let window = win::make(owner, &name, &title, width, height);
        *MAKING.lock().expect("the flag is not poisoned") = 0;
        if window.is_null() {
            return;
        }
        *SETTINGS_WINDOW.lock().expect("the window is not poisoned") = window as usize;
        unsafe { win::ShowWindow(window, win::SW_SHOW) };
        win::to_the_front(window);
    }

    pub fn look_again() {
        for which in [&SETTINGS_WINDOW, &ROUTE_WINDOW] {
            win::redress(held(which));
        }
    }

    /// Takes both windows down, for when the library is about to go.
    pub fn close() {
        for which in [&ROUTE_WINDOW, &SETTINGS_WINDOW] {
            let window = held(which);
            if there(window) {
                unsafe { win::DestroyWindow(window) };
            }
            *which.lock().expect("the window is not poisoned") = 0;
        }
    }

    /// Runs something on the edit, and says whether there was one.
    fn with_editing<T>(act: impl FnOnce(&mut Editing) -> T) -> Option<T> {
        let mut editing = EDITING.lock().expect("the edit is not poisoned");
        editing.as_mut().map(act)
    }

    /// How wide each column of the grid is: enough for the route's label,
    /// and never less than a box with room round it.
    fn column_widths(m: &win::Metrics, editing: &Editing) -> (i32, Vec<i32>) {
        let name = m.line * 7;
        let columns = editing
            .routes
            .iter()
            .map(|route| (m.line * route.label.chars().count() as i32 / 2 + m.line).max(m.line * 3))
            .collect();
        (name, columns)
    }

    fn settings_size() -> (i32, i32) {
        let m = win::metrics();
        let editing = EDITING.lock().expect("the edit is not poisoned");
        let Some(editing) = editing.as_ref() else {
            return (SETTINGS_WIDE, m.row * 10);
        };
        let (name, columns) = column_widths(&m, editing);
        let grid = name + columns.iter().sum::<i32>();
        let width = (grid + m.margin * 2).max(SETTINGS_WIDE);
        let spaces = editing.spaces.len().min(MOST_SPACES) as i32;
        let height = m.margin * 2
            + m.row
            + list_high(&m, editing)
            + m.gap
            + m.row
            + m.row
            + m.row * spaces
            + m.gap
            + m.row * 2
            + m.gap
            + m.button;
        (width, height)
    }

    /// The list of routes is as tall as the buttons beside it, or as its
    /// lines where there are more of them than that.
    fn list_high(m: &win::Metrics, editing: &Editing) -> i32 {
        let buttons = m.button * 5 + m.gap * 4;
        let lines = (m.line + 2) * (editing.routes.len().clamp(3, 10) as i32) + 6;
        buttons.max(lines)
    }

    /// Puts the controls on the settings window from the edit as it stands.
    /// Called again whenever the routes change, which changes the columns.
    fn furnish_settings(window: Handle) {
        win::strip(window);
        let m = win::metrics();
        let guard = EDITING.lock().expect("the edit is not poisoned");
        let Some(editing) = guard.as_ref() else {
            return;
        };
        let (width, height) = {
            let (name, columns) = column_widths(&m, editing);
            let grid = name + columns.iter().sum::<i32>();
            let width = (grid + m.margin * 2).max(SETTINGS_WIDE);
            let spaces = editing.spaces.len().min(MOST_SPACES) as i32;
            let height = m.margin * 2
                + m.row
                + list_high(&m, editing)
                + m.gap
                + m.row
                + m.row
                + m.row * spaces
                + m.gap
                + m.row * 2
                + m.gap
                + m.button;
            (width, height)
        };
        let inner = width - m.margin * 2;
        let mut y = m.margin;
        control(
            window,
            "STATIC",
            "経路（空間は上から順に試します）",
            SS_CENTERIMAGE,
            (m.margin, y, inner, m.row),
            ID_ROUTES_LABEL,
        );
        y += m.row;
        let button_wide = m.line * 6;
        let list_wide = inner - button_wide - m.gap;
        let high = list_high(&m, editing);
        control(
            window,
            "LISTBOX",
            "",
            WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY | LBS_USETABSTOPS | LBS_NOINTEGRALHEIGHT,
            (m.margin, y, list_wide, high),
            ID_LIST,
        );
        fill_list(window, editing, None);
        let column = m.margin + list_wide + m.gap;
        for (at, (text, id)) in [
            ("追加...", ID_ADD),
            ("編集...", ID_EDIT),
            ("削除", ID_REMOVE),
            ("上へ", ID_UP),
            ("下へ", ID_DOWN),
        ]
        .into_iter()
        .enumerate()
        {
            control(
                window,
                "BUTTON",
                text,
                WS_TABSTOP,
                (
                    column,
                    y + (m.button + m.gap) * at as i32,
                    button_wide,
                    m.button,
                ),
                id,
            );
        }
        y += high + m.gap;

        control(
            window,
            "STATIC",
            "空間（TVTestに出すチューニング空間と、それぞれに入れる経路）",
            SS_CENTERIMAGE,
            (m.margin, y, inner, m.row),
            ID_SPACES_LABEL,
        );
        y += m.row;
        let (name, columns) = column_widths(&m, editing);
        let mut x = m.margin + name;
        for (at, (route, wide)) in editing.routes.iter().zip(&columns).enumerate() {
            control(
                window,
                "STATIC",
                &route.label,
                SS_CENTERIMAGE | SS_ENDELLIPSIS,
                (x, y, *wide, m.row),
                ID_HEADING + at,
            );
            x += wide;
        }
        y += m.row;
        for (row, space) in editing.spaces.iter().enumerate().take(MOST_SPACES) {
            control(
                window,
                "BUTTON",
                &space.name,
                WS_TABSTOP | BS_AUTOCHECKBOX,
                (m.margin, y, name, m.row),
                ID_SPACE + row,
            );
            tick(window, ID_SPACE + row, space.on);
            let mut x = m.margin + name;
            for (at, wide) in columns.iter().enumerate().take(MOST_ROUTES) {
                let id = ID_TICK + row * MOST_ROUTES + at;
                control(
                    window,
                    "BUTTON",
                    "",
                    WS_TABSTOP | BS_AUTOCHECKBOX,
                    (x + m.line / 2, y, m.line * 2, m.row),
                    id,
                );
                tick(window, id, space.ticks.get(at).copied().unwrap_or(false));
                win::enable(window, id, space.on);
                x += wide;
            }
            y += m.row;
        }
        y += m.gap;
        control(
            window,
            "STATIC",
            "",
            SS_LEFT,
            (m.margin, y, inner, m.row * 2),
            ID_NOTE,
        );
        y += m.row * 2 + m.gap;
        let wide = m.line * 7;
        control(
            window,
            "BUTTON",
            "設定ファイルを開く",
            WS_TABSTOP,
            (m.margin, y, m.line * 9, m.button),
            ID_OPEN,
        );
        control(
            window,
            "BUTTON",
            "保存",
            WS_TABSTOP,
            (m.margin + inner - wide * 2 - m.gap, y, wide, m.button),
            ID_SAVE,
        );
        control(
            window,
            "BUTTON",
            "キャンセル",
            WS_TABSTOP,
            (m.margin + inner - wide, y, wide, m.button),
            ID_CANCEL,
        );
        // Let go of the edit before the note reads it again.
        drop(guard);
        note(window);
        win::dress(window);
        win::resize(window, width, height);
    }

    /// Puts the routes in the list, and picks one.
    fn fill_list(window: Handle, editing: &Editing, pick: Option<usize>) {
        let list = item(window, ID_LIST);
        if list.is_null() {
            return;
        }
        // The columns, in dialogue units: a quarter of the average character.
        let stops: [i32; 2] = [52, 116];
        unsafe {
            SendMessageW(list, LB_SETTABSTOPS, stops.len(), stops.as_ptr() as isize);
            SendMessageW(list, LB_RESETCONTENT, 0, 0);
        }
        for route in &editing.routes {
            let text = Wide::new(&Editing::route_line(route));
            unsafe { SendMessageW(list, LB_ADDSTRING, 0, text.as_ptr() as isize) };
        }
        if let Some(pick) = pick {
            unsafe { SendMessageW(list, LB_SETCURSEL, pick, 0) };
        }
    }

    /// Which route is picked in the list.
    fn picked(window: Handle) -> Option<usize> {
        let list = item(window, ID_LIST);
        if list.is_null() {
            return None;
        }
        let at = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) };
        (at >= 0).then_some(at as usize)
    }

    /// Says what is wrong with the edit as it stands, under the grid.
    fn note(window: Handle) {
        let said = with_editing(|editing| {
            let mut lines = editing.problems();
            lines.extend(editing.warnings());
            lines.truncate(2);
            lines.join("\r\n")
        })
        .unwrap_or_default();
        set_text(window, ID_NOTE, &said);
    }

    /// Writes the file, tells the plugin, and closes.
    fn save(window: Handle) {
        let outcome = with_editing(|editing| {
            let problems = editing.problems();
            if !problems.is_empty() {
                return Err(problems.join("\n"));
            }
            std::fs::write(&editing.file, editing.written()).map_err(|reason| {
                format!("{} を保存できません。({reason})", editing.file.display())
            })
        });
        match outcome {
            Some(Ok(())) => {
                let saved = *SAVED.lock().expect("the action is not poisoned");
                unsafe { win::DestroyWindow(window) };
                if let Some(saved) = saved {
                    saved();
                }
            }
            Some(Err(reason)) => tell(window, "coax", &reason),
            None => {}
        }
    }

    /// Takes a route out, after asking: the channels found on it stay in the
    /// file, and stop tuning until a route of that label is back.
    fn remove(window: Handle) {
        let Some(at) = picked(window) else {
            return;
        };
        let asked = with_editing(|editing| {
            editing.routes.get(at).map(|route| {
                let defined = coax_settings::defined::Defined::parse(&editing.text);
                let rows = defined
                    .spaces()
                    .flat_map(|(_, lines)| lines.iter())
                    .filter(|line| line.route().0 == route.label)
                    .count();
                let mut text = format!("経路「{}」を削除しますか？", route.label);
                if rows > 0 {
                    text.push_str(&format!(
                        "\n\nこの経路でスキャンした {rows} 件のチャンネル定義は残りますが、同じラベルの経路を作るまで選局できません。"
                    ));
                }
                text
            })
        })
        .flatten();
        let Some(text) = asked else {
            return;
        };
        if !ask(window, "coax", &text) {
            return;
        }
        with_editing(|editing| editing.remove_route(at));
        furnish_settings(window);
    }

    fn shift(window: Handle, up: bool) {
        let Some(at) = picked(window) else {
            return;
        };
        let moved = with_editing(|editing| {
            let to = if up { at.checked_sub(1) } else { Some(at + 1) };
            let to = to.filter(|to| *to < editing.routes.len());
            if to.is_some() {
                editing.move_route(at, up);
            }
            to
        })
        .flatten();
        if let Some(to) = moved {
            furnish_settings(window);
            with_editing(|editing| fill_list(window, editing, Some(to)));
        }
    }

    // ----- the route window -----

    /// Opens the route window, to add a route or to change the one at an
    /// index. The settings window is held still until it closes.
    fn open_route(settings: Handle, changing: Option<usize>) {
        if there(held(&ROUTE_WINDOW)) {
            win::to_the_front(held(&ROUTE_WINDOW));
            return;
        }
        *CHANGING.lock().expect("the index is not poisoned") = changing;
        let name = Wide::new("coax_plugin_route");
        if !win::class(&name, procedure) {
            return;
        }
        *MAKING.lock().expect("the flag is not poisoned") = MAKING_ROUTE;
        let (width, height) = route_size();
        let title = if changing.is_some() {
            "coax: 経路の編集"
        } else {
            "coax: 経路の追加"
        };
        let window = win::make(settings, &name, title, width, height);
        *MAKING.lock().expect("the flag is not poisoned") = 0;
        if window.is_null() {
            return;
        }
        *ROUTE_WINDOW.lock().expect("the window is not poisoned") = window as usize;
        unsafe {
            win::EnableWindow(settings, 0);
            win::ShowWindow(window, win::SW_SHOW);
        }
        win::to_the_front(window);
    }

    /// The tuners as the route window lists them: a heading per model of card,
    /// and a line per tuner under it. The heading is the model rather than the
    /// board, because a board is not a card -- a Windows machine lists one
    /// four-tuner card as four boards -- and the identifier on each line is
    /// what tells two cards of a model apart.
    fn tuner_lines(editing: &Editing) -> Vec<(Option<usize>, String)> {
        // The models in the order they first appear, each with its tuners, so
        // that two cards of a model listed apart still read as one group.
        let mut models: Vec<(String, Vec<usize>)> = Vec::new();
        for (at, tuner) in editing.tuners.iter().enumerate().take(MOST_TUNERS) {
            let model = setup::model_of(&tuner.model);
            let model = if model.is_empty() {
                setup::card_of(&tuner.id)
            } else {
                model
            };
            match models.iter_mut().find(|(known, _)| *known == model) {
                Some((_, members)) => members.push(at),
                None => models.push((model, vec![at])),
            }
        }
        let mut lines: Vec<(Option<usize>, String)> = Vec::new();
        for (model, members) in models {
            lines.push((None, model));
            for at in members {
                let tuner = &editing.tuners[at];
                let mut line = tuner.id.clone();
                if !tuner.name.is_empty() {
                    line.push_str("  ");
                    line.push_str(&tuner.name);
                }
                if !tuner.deliveries.is_empty() {
                    line.push_str("  ");
                    line.push_str(&tuner.deliveries.join(", "));
                }
                lines.push((Some(at), line));
            }
        }
        lines
    }

    /// Whether any tuner has a supply or a gain to set, which is what gives
    /// the route window its two extra columns.
    fn has_controls(editing: &Editing) -> bool {
        editing.tuners.iter().any(|tuner| {
            editing
                .listed
                .controls(&tuner.id)
                .is_some_and(|controls| controls.lnb.is_some() || controls.gain.is_some())
        })
    }

    /// How wide the supply and gain columns are.
    fn control_columns(m: &win::Metrics) -> (i32, i32) {
        (m.line * 9, m.line * 6)
    }

    /// How wide the route window is: wider by the two columns where a tuner
    /// has something to set.
    fn route_wide(m: &win::Metrics, editing: &Editing) -> i32 {
        if has_controls(editing) {
            let (lnb, gain) = control_columns(m);
            ROUTE_WIDE + lnb + gain + m.gap * 2
        } else {
            ROUTE_WIDE
        }
    }

    /// How tall a tuner's line is: tall enough for a list beside it where
    /// there are lists.
    fn tuner_high(m: &win::Metrics, editing: &Editing) -> i32 {
        if has_controls(editing) {
            m.row + 4
        } else {
            m.row
        }
    }

    fn route_size() -> (i32, i32) {
        let m = win::metrics();
        let editing = EDITING.lock().expect("the edit is not poisoned");
        let Some(editing) = editing.as_ref() else {
            return (ROUTE_WIDE, m.row * 10);
        };
        let lines = tuner_lines(editing);
        let tuner_rows: i32 = if lines.is_empty() {
            m.row
        } else {
            lines
                .iter()
                .map(|(tuner, _)| match tuner {
                    Some(_) => tuner_high(&m, editing),
                    None => m.row,
                })
                .sum()
        };
        let field = m.row + 4;
        let height = m.margin * 2
            + field
            + m.gap
            + field
            + m.gap
            + m.row
            + tuner_rows
            + m.gap
            + field
            + m.gap
            + m.row
            + m.gap
            + m.button;
        (route_wide(&m, editing), height)
    }

    /// Puts a drop-down list on a window, opened on one line, and notes which.
    fn choice(
        window: Handle,
        at: (i32, i32, i32, i32),
        id: usize,
        choices: &[(String, String)],
        current: &str,
    ) {
        control(
            window,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP,
            (at.0, at.1, at.2, at.3 * 8),
            id,
        );
        let combo = item(window, id);
        if combo.is_null() {
            return;
        }
        for (_, shown) in choices {
            let text = Wide::new(shown);
            unsafe { SendMessageW(combo, CB_ADDSTRING, 0, text.as_ptr() as isize) };
        }
        // A value the list does not offer opens on the default, and is kept
        // unless the viewer moves the list.
        let pick = choices
            .iter()
            .position(|(value, _)| value == current)
            .unwrap_or(0);
        unsafe { SendMessageW(combo, CB_SETCURSEL, pick, 0) };
        OPENED_ON
            .lock()
            .expect("the lines are not poisoned")
            .push((id, pick));
    }

    /// The line a viewer moved a list to, or nothing where it is where it was
    /// opened.
    fn moved(window: Handle, id: usize) -> Option<usize> {
        let combo = item(window, id);
        if combo.is_null() {
            return None;
        }
        let at = unsafe { SendMessageW(combo, CB_GETCURSEL, 0, 0) };
        let at = usize::try_from(at).ok()?;
        let opened = OPENED_ON
            .lock()
            .expect("the lines are not poisoned")
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, line)| *line);
        (opened != Some(at)).then_some(at)
    }

    fn furnish_route(window: Handle) {
        let m = win::metrics();
        let label = m.line * 5;
        let field = m.row + 4;
        let guard = EDITING.lock().expect("the edit is not poisoned");
        let Some(editing) = guard.as_ref() else {
            return;
        };
        let inner = route_wide(&m, editing) - m.margin * 2;
        let (lnb_wide, gain_wide) = control_columns(&m);
        let controls = has_controls(editing);
        // Where the supply and gain columns start, at the right.
        let lnb_x = m.margin + inner - lnb_wide - gain_wide - m.gap;
        let gain_x = m.margin + inner - gain_wide;
        let line_high = tuner_high(&m, editing);
        OPENED_ON
            .lock()
            .expect("the lines are not poisoned")
            .clear();
        let changing = *CHANGING.lock().expect("the index is not poisoned");
        let route = changing.and_then(|at| editing.routes.get(at)).cloned();

        let mut y = m.margin;
        control(
            window,
            "STATIC",
            "ラベル",
            SS_CENTERIMAGE,
            (m.margin, y, label, field),
            0,
        );
        control(
            window,
            "EDIT",
            route.as_ref().map_or("", |route| route.label.as_str()),
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
            (m.margin + label + m.gap, y, m.line * 10, field),
            ID_R_LABEL,
        );
        y += field + m.gap;
        control(
            window,
            "STATIC",
            "伝送方式",
            SS_CENTERIMAGE,
            (m.margin, y, label, field),
            0,
        );
        let combo_wide = m.line * 12;
        control(
            window,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP,
            (m.margin + label + m.gap, y, combo_wide, m.row * 10),
            ID_R_SYSTEM,
        );
        let combo = item(window, ID_R_SYSTEM);
        for (system, shown) in delivery::OFFERED {
            let text = Wide::new(&format!("{shown} ({system})"));
            unsafe { SendMessageW(combo, CB_ADDSTRING, 0, text.as_ptr() as isize) };
        }
        let other = Wide::new(OTHER);
        unsafe { SendMessageW(combo, CB_ADDSTRING, 0, other.as_ptr() as isize) };
        let system = route.as_ref().map(|route| delivery::named(&route.system));
        let offered = system.as_deref().and_then(|system| {
            delivery::OFFERED
                .iter()
                .position(|(name, _)| *name == system)
        });
        let pick = match (&system, offered) {
            (None, _) => 0,
            (Some(_), Some(at)) => at,
            (Some(_), None) => delivery::OFFERED.len(),
        };
        unsafe { SendMessageW(combo, CB_SETCURSEL, pick, 0) };
        control(
            window,
            "EDIT",
            if offered.is_none() {
                system.as_deref().unwrap_or("")
            } else {
                ""
            },
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
            (
                m.margin + label + m.gap + combo_wide + m.gap,
                y,
                inner - label - m.gap - combo_wide - m.gap,
                field,
            ),
            ID_R_OTHER,
        );
        y += field + m.gap;

        control(
            window,
            "STATIC",
            if editing.daemon_answered {
                "チューナー"
            } else {
                "チューナー（coaxdに接続できないため一覧を出せません。パターンを直接入力してください）"
            },
            SS_CENTERIMAGE,
            (
                m.margin,
                y,
                if controls {
                    lnb_x - m.margin - m.gap
                } else {
                    inner
                },
                m.row,
            ),
            ID_R_TUNERS_LABEL,
        );
        if controls {
            control(
                window,
                "STATIC",
                "LNB 給電",
                SS_CENTERIMAGE,
                (lnb_x, y, lnb_wide, m.row),
                ID_R_LNB_LABEL,
            );
            control(
                window,
                "STATIC",
                "利得",
                SS_CENTERIMAGE,
                (gain_x, y, gain_wide, m.row),
                ID_R_GAIN_LABEL,
            );
        }
        y += m.row;
        let lines = tuner_lines(editing);
        let named: Vec<String> = route
            .as_ref()
            .map(|route| setup::named_by(&editing.tuners, &route.tuners))
            .unwrap_or_default();
        if lines.is_empty() {
            control(
                window,
                "STATIC",
                if editing.daemon_answered {
                    "（チューナーがありません）"
                } else {
                    ""
                },
                SS_CENTERIMAGE,
                (m.margin + m.line, y, inner - m.line, m.row),
                ID_R_NONE,
            );
            y += m.row;
        }
        for (at, (tuner, text)) in lines.iter().enumerate() {
            match tuner {
                None => control(
                    window,
                    "STATIC",
                    text,
                    SS_CENTERIMAGE | SS_ENDELLIPSIS,
                    (m.margin + m.line, y, inner - m.line, m.row),
                    ID_R_CARD + at,
                ),
                Some(index) => {
                    let wide = if controls {
                        lnb_x - m.gap - m.margin - m.line * 2
                    } else {
                        inner - m.line * 2
                    };
                    control(
                        window,
                        "BUTTON",
                        text,
                        WS_TABSTOP | BS_AUTOCHECKBOX,
                        (m.margin + m.line * 2, y, wide, line_high),
                        ID_R_TUNER + index,
                    );
                    let id = &editing.tuners[*index].id;
                    tick(window, ID_R_TUNER + index, named.contains(id));
                    furnish_tuner(window, editing, *index, (lnb_x, gain_x, y), &m);
                }
            }
            y += if tuner.is_some() { line_high } else { m.row };
        }
        y += m.gap;
        control(
            window,
            "STATIC",
            "パターン",
            SS_CENTERIMAGE,
            (m.margin, y, label, field),
            0,
        );
        *WRITING_PATTERN.lock().expect("the flag is not poisoned") = true;
        control(
            window,
            "EDIT",
            &route
                .as_ref()
                .map(|route| route.tuners.join(", "))
                .unwrap_or_default(),
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
            (m.margin + label + m.gap, y, inner - label - m.gap, field),
            ID_R_PATTERN,
        );
        *WRITING_PATTERN.lock().expect("the flag is not poisoned") = false;
        y += field + m.gap;

        control(
            window,
            "STATIC",
            "受信帯域",
            SS_CENTERIMAGE,
            (m.margin, y, label, m.row),
            ID_R_BANDS_LABEL,
        );
        let mut x = m.margin + label + m.gap;
        let bands: Vec<Band> = route
            .as_ref()
            .map_or(Band::ALL.to_vec(), |route| route.bands.clone());
        for (at, (band, text)) in BANDS.iter().enumerate() {
            let wide = m.line * (text.chars().count() as i32) / 2 + m.line * 2;
            control(
                window,
                "BUTTON",
                text,
                WS_TABSTOP | BS_AUTOCHECKBOX,
                (x, y, wide, m.row),
                ID_R_BAND + at,
            );
            tick(window, ID_R_BAND + at, bands.contains(band));
            x += wide + m.gap;
        }
        y += m.row + m.gap;
        let wide = m.line * 7;
        control(
            window,
            "BUTTON",
            "OK",
            WS_TABSTOP,
            (m.margin + inner - wide * 2 - m.gap, y, wide, m.button),
            ID_R_OK,
        );
        control(
            window,
            "BUTTON",
            "キャンセル",
            WS_TABSTOP,
            (m.margin + inner - wide, y, wide, m.button),
            ID_R_CANCEL,
        );
        drop(guard);
        follow_system(window);
    }

    /// Puts a tuner's supply and gain beside its line, where it has them. A
    /// supply its board shares is shown on the board's first tuner only.
    fn furnish_tuner(
        window: Handle,
        editing: &Editing,
        index: usize,
        (lnb_x, gain_x, y): (i32, i32, i32),
        m: &win::Metrics,
    ) {
        let (lnb_wide, gain_wide) = control_columns(m);
        let Some(tuner) = editing.tuners.get(index) else {
            return;
        };
        let Some(controls) = editing.listed.controls(&tuner.id) else {
            return;
        };
        let setting = editing.setting_of(index);
        if let Some(lnb) = &controls.lnb {
            if editing.lnb_is_shown(index) {
                choice(
                    window,
                    (lnb_x, y, lnb_wide, m.row + 4),
                    ID_R_LNB + index,
                    &editing::lnb_choices(lnb),
                    &setting.lnb,
                );
            } else {
                control(
                    window,
                    "STATIC",
                    "（上と共通）",
                    SS_CENTERIMAGE,
                    (lnb_x, y, lnb_wide, m.row + 4),
                    0,
                );
            }
        }
        if let Some(gain) = &controls.gain {
            choice(
                window,
                (gain_x, y, gain_wide, m.row + 4),
                ID_R_GAIN + index,
                &editing::gain_choices(gain),
                &setting.gain,
            );
        }
    }

    /// Puts what the route window's supply and gain lists say into the edit,
    /// for the lists a viewer moved.
    fn take_tuner_settings(window: Handle) {
        with_editing(|editing| {
            for index in 0..editing.tuners.len().min(MOST_TUNERS) {
                let Some(controls) = editing.listed.controls(&editing.tuners[index].id).cloned()
                else {
                    continue;
                };
                if let (Some(lnb), Some(at)) = (&controls.lnb, moved(window, ID_R_LNB + index)) {
                    if let Some((value, _)) = editing::lnb_choices(lnb).get(at) {
                        editing.set_lnb(index, value);
                    }
                }
                if let (Some(gain), Some(at)) = (&controls.gain, moved(window, ID_R_GAIN + index)) {
                    if let Some((value, _)) = editing::gain_choices(gain).get(at) {
                        editing.set_gain(index, value);
                    }
                }
            }
        });
    }

    /// The delivery system the route window has chosen, as the settings
    /// name it.
    fn system_chosen(window: Handle) -> String {
        let combo = item(window, ID_R_SYSTEM);
        let at = unsafe { SendMessageW(combo, CB_GETCURSEL, 0, 0) };
        match delivery::OFFERED.get(at.max(0) as usize) {
            Some((system, _)) => (*system).to_owned(),
            None => delivery::named(&text_of(window, ID_R_OTHER)),
        }
    }

    /// Greys out what the chosen delivery system has no use for: the field
    /// for a system that is not offered, and the bands of one that is asked
    /// rather than walked.
    fn follow_system(window: Handle) {
        let combo = item(window, ID_R_SYSTEM);
        let at = unsafe { SendMessageW(combo, CB_GETCURSEL, 0, 0) };
        let other = at as usize >= delivery::OFFERED.len();
        win::enable(window, ID_R_OTHER, other);
        let system = system_chosen(window);
        let walked = setup::bands_are_written(&system);
        for at in 0..BANDS.len() {
            win::enable(window, ID_R_BAND + at, walked);
        }
        win::enable(window, ID_R_BANDS_LABEL, walked);
        // A supply only on a satellite's lead, and a gain only where the
        // amplifier is in front of this system.
        with_editing(|editing| {
            for at in 0..editing.tuners.len().min(MOST_TUNERS) {
                win::enable(window, ID_R_LNB + at, editing.lnb_of(at, &system).is_some());
                win::enable(
                    window,
                    ID_R_GAIN + at,
                    editing.gain_of(at, &system).is_some(),
                );
            }
        });
    }

    /// Writes the ticked tuners into the pattern field, the shortest way
    /// that names exactly them.
    fn ticks_to_pattern(window: Handle) {
        let patterns = with_editing(|editing| {
            let ids: Vec<String> = editing
                .tuners
                .iter()
                .enumerate()
                .filter(|(at, _)| ticked(window, ID_R_TUNER + at))
                .map(|(_, tuner)| tuner.id.clone())
                .collect();
            setup::patterns_of(&editing.tuners, &ids)
        })
        .unwrap_or_default();
        *WRITING_PATTERN.lock().expect("the flag is not poisoned") = true;
        set_text(window, ID_R_PATTERN, &patterns.join(", "));
        *WRITING_PATTERN.lock().expect("the flag is not poisoned") = false;
    }

    /// Ticks the tuners the pattern field names, for a viewer typing one.
    fn pattern_to_ticks(window: Handle) {
        if *WRITING_PATTERN.lock().expect("the flag is not poisoned") {
            return;
        }
        let patterns = patterns_typed(window);
        with_editing(|editing| {
            let named = setup::named_by(&editing.tuners, &patterns);
            for (at, tuner) in editing.tuners.iter().enumerate() {
                tick(window, ID_R_TUNER + at, named.contains(&tuner.id));
            }
        });
    }

    fn patterns_typed(window: Handle) -> Vec<String> {
        text_of(window, ID_R_PATTERN)
            .split(',')
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Takes what the route window says and puts it in the edit, or says
    /// what is wrong with it.
    fn finish_route(window: Handle) {
        let label = text_of(window, ID_R_LABEL).trim().to_owned();
        let system = system_chosen(window);
        let changing = *CHANGING.lock().expect("the index is not poisoned");
        let wrong = with_editing(|editing| {
            if label.is_empty() {
                return Some("ラベルを入力してください。".to_owned());
            }
            if label.contains(|letter: char| {
                letter.is_whitespace() || matches!(letter, ',' | '=' | ';' | '#' | '[' | ']')
            }) {
                return Some("ラベルに空白やコンマ、= ; # [ ] は使えません。".to_owned());
            }
            if editing
                .routes
                .iter()
                .enumerate()
                .any(|(at, route)| Some(at) != changing && route.label == label)
            {
                return Some(format!("ラベル「{label}」は既にあります。"));
            }
            if system.is_empty() {
                return Some("伝送方式を入力してください。".to_owned());
            }
            None
        })
        .flatten();
        if let Some(wrong) = wrong {
            tell(window, "coax", &wrong);
            return;
        }
        let bands: Vec<Band> = if setup::bands_are_written(&system) {
            BANDS
                .iter()
                .enumerate()
                .filter(|(at, _)| ticked(window, ID_R_BAND + at))
                .map(|(_, (band, _))| *band)
                .collect()
        } else {
            Vec::new()
        };
        let route = Route {
            label,
            system,
            tuners: patterns_typed(window),
            bands,
        };
        with_editing(|editing| match changing {
            Some(at) => editing.replace_route(at, route),
            None => editing.add_route(route),
        });
        take_tuner_settings(window);
        unsafe { win::DestroyWindow(window) };
        let settings = held(&SETTINGS_WINDOW);
        if there(settings) {
            furnish_settings(settings);
        }
    }

    unsafe extern "system" fn procedure(
        window: Handle,
        message: u32,
        first: usize,
        second: isize,
    ) -> isize {
        let holds = |id: usize| matches!(id, ID_LIST | ID_R_LABEL | ID_R_OTHER | ID_R_PATTERN);
        if let Some(answer) = win::common(window, message, first, second, &holds) {
            return answer;
        }
        let is_route = held(&ROUTE_WINDOW) == window
            || *MAKING.lock().expect("the flag is not poisoned") == MAKING_ROUTE;
        match message {
            WM_CREATE => {
                if is_route {
                    furnish_route(window);
                } else {
                    furnish_settings(window);
                }
                win::dress(window);
                0
            }
            WM_COMMAND if is_route => {
                let id = first & 0xffff;
                let code = first >> 16;
                match id {
                    ID_R_OK => finish_route(window),
                    ID_R_CANCEL => unsafe {
                        win::DestroyWindow(window);
                    },
                    ID_R_SYSTEM if code == CBN_SELCHANGE => follow_system(window),
                    ID_R_OTHER if code == EN_CHANGE => follow_system(window),
                    ID_R_PATTERN if code == EN_CHANGE => pattern_to_ticks(window),
                    _ if (ID_R_TUNER..ID_R_TUNER + MOST_TUNERS).contains(&id) => {
                        ticks_to_pattern(window);
                    }
                    _ => {}
                }
                0
            }
            WM_COMMAND => {
                let id = first & 0xffff;
                let code = first >> 16;
                match id {
                    ID_ADD => open_route(window, None),
                    ID_EDIT => {
                        if let Some(at) = picked(window) {
                            open_route(window, Some(at));
                        }
                    }
                    ID_LIST if code == LBN_DBLCLK => {
                        if let Some(at) = picked(window) {
                            open_route(window, Some(at));
                        }
                    }
                    ID_REMOVE => remove(window),
                    ID_UP => shift(window, true),
                    ID_DOWN => shift(window, false),
                    ID_OPEN => {
                        let open_file = *OPEN_FILE.lock().expect("the action is not poisoned");
                        if let Some(open_file) = open_file {
                            open_file();
                        }
                    }
                    ID_SAVE => save(window),
                    ID_CANCEL => unsafe {
                        win::DestroyWindow(window);
                    },
                    _ if id >= ID_TICK => {
                        let row = (id - ID_TICK) / MOST_ROUTES;
                        let at = (id - ID_TICK) % MOST_ROUTES;
                        let on = ticked(window, id);
                        with_editing(|editing| editing.tick(row, at, on));
                        note(window);
                    }
                    _ if (ID_SPACE..ID_SPACE + MOST_SPACES).contains(&id) => {
                        let row = id - ID_SPACE;
                        let on = ticked(window, id);
                        with_editing(|editing| editing.switch(row, on));
                        for at in 0..MOST_ROUTES {
                            win::enable(window, ID_TICK + row * MOST_ROUTES + at, on);
                        }
                        note(window);
                    }
                    _ => {}
                }
                0
            }
            WM_PAINT => {
                let mut paint: win::Paint = unsafe { core::mem::zeroed() };
                let context = unsafe { win::BeginPaint(window, &raw mut paint) };
                if !is_route {
                    win::outline(window, context, ID_LIST);
                }
                unsafe { win::EndPaint(window, &raw const paint) };
                0
            }
            WM_DESTROY => {
                if held(&ROUTE_WINDOW) == window {
                    *ROUTE_WINDOW.lock().expect("the window is not poisoned") = 0;
                    let settings = held(&SETTINGS_WINDOW);
                    if there(settings) {
                        unsafe { win::EnableWindow(settings, 1) };
                        win::to_the_front(settings);
                    }
                }
                if held(&SETTINGS_WINDOW) == window {
                    *SETTINGS_WINDOW.lock().expect("the window is not poisoned") = 0;
                    *EDITING.lock().expect("the edit is not poisoned") = None;
                }
                0
            }
            _ => unsafe { win::DefWindowProcW(window, message, first, second) },
        }
    }
}

#[cfg(windows)]
pub use windows::{close, look_again, open};

#[cfg(not(windows))]
mod elsewhere {
    use crate::editing::Editing;
    use core::ffi::c_void;

    pub fn open(_owner: *mut c_void, _editing: Editing, _saved: fn(), _open_file: fn()) {}
    pub fn look_again() {}
    pub fn close() {}
}

#[cfg(not(windows))]
#[allow(unused_imports)]
pub use elsewhere::{close, look_again, open};

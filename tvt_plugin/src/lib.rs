// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The TVTest plugin that fills in what the host's own channel scan cannot find
//! out.
//!
//! Terrestrial television is one stream per carrier, so the host's own scan is
//! enough for it; a cable's carriers may hold several streams each, and which
//! ones is written in the signal. This plugin does the looking that needs,
//! writes it beside the driver, and then asks the host to read its channel list
//! again. A viewer sees one command and one message box.

// A plugin is a Windows thing, and most of what is here is reached only from the
// exported calls a host makes. Elsewhere the crate is built so its own tests run.
#![cfg_attr(not(windows), allow(dead_code))]

mod app;
mod descriptors;
mod discover;
mod editing;
mod sdk;
mod settings;
mod state;
#[cfg(windows)]
mod win;
mod window;

use core::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use coax::daemon::{Daemon, Endpoint};
use coax::launch::Launcher;
use coax_settings::beside;
use coax_settings::config::Config;
use coax_settings::defined::Defined;
use coax_settings::setup::Setup;
use coax_settings::trace;
use coax_settings::wide::Wide;

use crate::app::{Host, tell};
use crate::discover::What;
use crate::editing::Editing;
use crate::sdk::{
    EVENT_COLORCHANGE, EVENT_COMMAND, EVENT_PLUGINENABLE, EVENT_PLUGINSETTINGS,
    PLUGIN_FLAG_HASSETTINGS, PLUGIN_TYPE_NORMAL, PLUGIN_VERSION, PluginInfo, PluginParam,
};

/// The commands this plugin has. The first opens a menu of the others; the rest
/// are there so that a keyboard shortcut or the host's plugin menu can go
/// straight to one.
const MENU: i32 = 1;
const DISCOVER: i32 = 2;
const OPEN_CHANNELS: i32 = 5;
const SETTINGS: i32 = 9;
/// What the tuners are doing, which is what a viewer looks at when a sweep or
/// a channel is refused.
const TUNER_STATE: i32 = 10;
/// Not registered with the host: these are the sweep window's own buttons.
const STOP: i32 = 6;
const DONE: i32 = 8;

/// What the plugin holds between the host's calls: the way back to the host,
/// and whether a sweep is already running.
static STATE: Mutex<Option<Host>> = Mutex::new(None);
static SWEEPING: Mutex<bool> = Mutex::new(false);
/// The settings file, once the host has said where the driver is. See
/// [`settings_file`].
static SETTINGS_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The file a viewer picked in the plugin's own window, which stands in for the
/// one the driver's name points at. An installation holds a file per lead --
/// see `beside::settings_family` -- and the host has one driver open.
static PICKED_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// What a viewer presses "キャンセル" to set, and what the sweep reads between one
/// carrier and the next.
static STOPPING: discover::Stopping = discover::Stopping::new();
/// The plugin's own threads: the sweep, and the one asking the daemon for its
/// tuners. Held so that taking the plugin down can wait for them -- each holds
/// the way back to the host, and runs code in this library.
static SWEEPER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static LISTER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// How long a question asked on the thread that draws the host's window may
/// wait. A daemon that is running answers a listing at once.
const ASKED_WHILE_DRAWING: Duration = Duration::from_secs(3);

/// How long taking the plugin down waits for its threads. A sweep notices
/// it has been stopped between one carrier and the next, which is seconds at
/// the most; a thread that is still going after this is waiting on a daemon
/// that has stopped answering.
const FINISHING: Duration = Duration::from_secs(10);

/// The strings the host is handed pointers to. They have to outlive the call,
/// so they are made once and kept.
struct Text {
    name: Wide,
    copyright: Wide,
    description: Wide,
    /// Each command as the host takes it: the identifier it is stored under,
    /// and the name a viewer reads.
    commands: Vec<(i32, Wide, Wide)>,
}

static TEXT: Mutex<Option<Text>> = Mutex::new(None);

/// The version of the plugin interface this is written against.
#[unsafe(no_mangle)]
pub extern "system" fn TVTGetVersion() -> u32 {
    trace::line("plugin: TVTGetVersion");
    PLUGIN_VERSION
}

/// What the plugin says about itself, in the host's plugin list.
///
/// # Safety
///
/// The host passes a structure of its own to fill in.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn TVTGetPluginInfo(info: *mut PluginInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    let mut text = TEXT.lock().expect("the strings are not poisoned");
    let text = text.get_or_insert_with(|| Text {
        name: Wide::new("coax"),
        copyright: Wide::new("Copyright 2026 Till0196"),
        description: Wide::new(
            "TVTestのチャンネルスキャンでは分からないチャンネルを、coaxdのチューナーでスキャンします。",
        ),
        // The text is the identifier the host stores and a viewer types after
        // the plugin's file name; the name is what a viewer reads in a menu.
        commands: vec![
            (MENU, Wide::new("Menu"), Wide::new("coaxのメニュー")),
            (
                DISCOVER,
                Wide::new("Discover"),
                Wide::new("チャンネルをスキャン"),
            ),
            (
                SETTINGS,
                Wide::new("Settings"),
                Wide::new("経路と空間の設定"),
            ),
            (
                TUNER_STATE,
                Wide::new("Tuners"),
                Wide::new("チューナーの状態"),
            ),
            (
                OPEN_CHANNELS,
                Wide::new("OpenChannels"),
                Wide::new("設定ファイルを開く"),
            ),
        ],
    });
    unsafe {
        (*info).kind = PLUGIN_TYPE_NORMAL;
        // Switchable, because that is what puts the plugin in the host's own
        // plugin menu: a plugin with no on and off is left out of it entirely.
        // Switching it on opens the window. It says it has settings too,
        // because the settings button of the plugin list is the other place a
        // viewer goes looking.
        (*info).flags = PLUGIN_FLAG_HASSETTINGS;
        (*info).name = text.name.as_ptr();
        (*info).copyright = text.copyright.as_ptr();
        (*info).description = text.description.as_ptr();
    }
    1
}

/// # Safety
///
/// The host passes its own parameter block, which is good until the plugin is
/// taken down.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn TVTInitialize(param: *mut PluginParam) -> i32 {
    if param.is_null() {
        return 0;
    }
    trace::line("plugin: TVTInitialize");
    app::remember();
    let host = unsafe { Host::new(param) };
    let listening = host.listen(on_event);
    let text = TEXT.lock().expect("the strings are not poisoned");
    let registered = text.as_ref().map_or(0, |text| {
        text.commands
            .iter()
            .filter(|(id, command, name)| host.register_command(*id, command, name))
            .count()
    });
    trace!("plugin: listening {listening}, {registered} command(s) registered");
    follow_colours(&host);
    *STATE.lock().expect("the state is not poisoned") = Some(host);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn TVTFinalize() -> i32 {
    // The threads hold the way back to the host, which is not good after this
    // call, and run code in this library, which is about to be unloaded. So
    // they are stopped, and cut off from the host first: one of them calling
    // into it while this thread waits could be waiting on this thread.
    STOPPING.stop();
    app::forget();
    // The window's procedure lives in this library too, so the window goes.
    window::close();
    state::close();
    #[cfg(windows)]
    win::unregister_classes();
    *STATE.lock().expect("the state is not poisoned") = None;
    let deadline = Instant::now() + FINISHING;
    let mut all_finished = true;
    for held in [&SWEEPER, &LISTER] {
        let thread = held.lock().expect("the thread is not poisoned").take();
        if let Some(thread) = thread {
            all_finished &= finish(thread, deadline);
        }
    }
    if !all_finished {
        // Still waiting on a daemon that does not answer. Better the library
        // stays in memory than the thread comes back to code that has gone.
        trace::line("plugin: a thread did not finish; staying loaded");
        beside::stay_loaded();
    }
    1
}

/// Waits for a thread until a deadline, and says whether it finished.
fn finish(thread: JoinHandle<()>, deadline: Instant) -> bool {
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = thread.join();
    true
}

/// Starts one of the plugin's threads where it can be found again, unless the
/// last one started there is still going.
fn spawn_into(
    held: &Mutex<Option<JoinHandle<()>>>,
    name: &str,
    work: impl FnOnce() + Send + 'static,
) -> std::io::Result<bool> {
    let mut held = held.lock().expect("the thread is not poisoned");
    if let Some(running) = held.take() {
        if !running.is_finished() {
            *held = Some(running);
            return Ok(false);
        }
        let _ = running.join();
    }
    *held = Some(thread::Builder::new().name(name.to_owned()).spawn(work)?);
    Ok(true)
}

/// What the host sends the plugin. Only one event matters here.
unsafe extern "system" fn on_event(
    event: u32,
    first: isize,
    _second: isize,
    _data: *mut c_void,
) -> isize {
    trace!("plugin: event {event} {first}");
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return 0;
    };
    match event {
        EVENT_COMMAND => chose(host, first as i32),
        // The tick in the host's plugin menu, which is the only reason this
        // plugin has an on and an off. Nothing else hangs on it -- a window
        // that came and went with the tick would close itself the moment a
        // viewer unticked it.
        EVENT_PLUGINENABLE => 1,
        // A viewer switched theme, or turned the dark one on. The windows are
        // painted in the host's colours, so they follow it.
        EVENT_COLORCHANGE => {
            follow_colours(&host);
            1
        }
        // The settings button of the host's plugin list. There is nothing here
        // that a dialog of fields would hold, so it opens the plugin's own
        // window, hung on the window the host passes: the list is modal, and a
        // window owned by one behind it cannot be reached.
        EVENT_PLUGINSETTINGS => {
            let owner = first as *mut c_void;
            let owner = if owner.is_null() {
                host.window()
            } else {
                owner
            };
            show_window(owner);
            1
        }
        _ => 0,
    }
}

/// Looks at the colours again. The windows follow the system's own dark mode,
/// and a viewer who changed the host's theme has usually changed the system's
/// as well.
fn follow_colours(_host: &Host) {
    window::look_again();
    state::look_again();
}

/// Acts on one of the plugin's commands.
fn chose(host: Host, command: i32) -> isize {
    match command {
        MENU => {
            show_window(host.window());
            1
        }
        // From the host's own menu everything is looked for; from the
        // plugin's window, what is ticked there.
        DISCOVER => start(host, window::chosen_routes()),
        SETTINGS => {
            show_settings(host);
            1
        }
        TUNER_STATE => {
            show_tuners(host);
            1
        }
        OPEN_CHANNELS => {
            match where_things_are(host) {
                Ok((_, file, _)) => open(host.window(), &file),
                Err(reason) => tell(host.window(), "coax", &reason),
            }
            1
        }
        STOP => {
            // While there is a sweep this stops it and throws away what it
            // found; with none, the button is the one that closes the window.
            if *SWEEPING.lock().expect("the flag is not poisoned") {
                STOPPING.stop();
            } else {
                window::close_sweep();
            }
            1
        }
        // Stops the sweep and keeps what it has found so far, which is what a
        // viewer who has seen their channels go by wants.
        DONE => {
            if *SWEEPING.lock().expect("the flag is not poisoned") {
                STOPPING.finish();
            }
            1
        }
        _ => 0,
    }
}

/// Opens the plugin's own window: the buttons, and nothing else. What there is
/// to say about the state of things is said by the sweep's own window while it
/// is working.
fn show_window(owner: *mut c_void) {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return;
    };
    let routes = routes_shown(host);
    let (files, at) = settings_choices(host);
    window::open(
        owner,
        window::Buttons {
            discover: DISCOVER,
            settings: SETTINGS,
            tuners: TUNER_STATE,
            open: OPEN_CHANNELS,
        },
        on_button,
        routes.0,
        routes.1,
        Vec::new(),
        window::SettingsFiles {
            names: shown_files(&files),
            at,
            pick: on_picked,
        },
    );
    // The list is asked for on a thread of its own: asking here would mean
    // waiting for a daemon that may have to be started first, on the thread
    // that draws the host's window. One already asking will answer this window
    // too.
    let _ = spawn_into(&LISTER, "coax tuners", || window::set_tuners(tuners()));
}

/// The routes as the plugin's window lists them, from the settings as they
/// stand.
fn routes_shown(host: Host) -> (Vec<window::RouteShown>, Option<String>) {
    let text = match where_things_are(host) {
        Ok((_, _, text)) => text,
        Err(reason) => return (Vec::new(), Some(reason)),
    };
    let routes = Setup::read(&text)
        .routes
        .into_iter()
        .map(|route| {
            let mut rest = if route.tuners.is_empty() {
                "(どのチューナーでも)".to_owned()
            } else {
                route.tuners.join(", ")
            };
            let bands = editing::bands_shown(&route);
            if !bands.is_empty() {
                if !rest.is_empty() {
                    rest.push_str("  ");
                }
                rest.push_str(&bands);
            }
            window::RouteShown {
                label: route.label,
                system: coax::delivery::shown(&route.system),
                rest,
            }
        })
        .collect();
    (routes, None)
}

/// Opens the settings window on the file as it stands, with the daemon's tuners
/// where the daemon answers.
///
/// The daemon is asked on this thread, which draws the host's window: a daemon
/// that is running answers at once. One that is not is not started for this --
/// the window says the list could not be had, and a pattern can be typed. One
/// that has stopped answering is given up on quickly, for the same reason.
fn show_settings(host: Host) {
    let (config, file, text) = match where_things_are(host) {
        Ok(things) => things,
        Err(reason) => {
            tell(host.window(), "coax", &reason);
            return;
        }
    };
    let daemon = Daemon::new(Endpoint::parse(&config.host))
        .with_priority(config.priority)
        .answering_within(ASKED_WHILE_DRAWING);
    // As the daemon lists them, with what each lets a channel set: the window
    // offers a tuner's supply and gain only where the tuner has them.
    let listed = daemon
        .listing()
        .ok()
        .map(|boards| coax_settings::tuner_settings::Listed::of(boards, &[]));
    let editing = Editing::open(file, text, listed);
    settings::open(host.window(), editing, on_saved, on_open_file);
}

/// What the settings window does once it has written the file: the driver
/// reads it again, and the plugin's own window shows the new routes.
fn on_saved() {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return;
    };
    reload();
    // Writing a file that was not there is what puts it in the list, so the
    // choices are read again beside the routes.
    let (files, at) = settings_choices(host);
    window::set_settings_files(shown_files(&files), at);
    let (routes, trouble) = routes_shown(host);
    window::set_routes(routes, trouble);
}

fn on_open_file() {
    on_button(OPEN_CHANNELS);
}

/// Opens the window that says what the tuners are doing.
fn show_tuners(host: Host) {
    state::open(host.window(), tuner_state);
}

/// The tuners and what each is doing, for that window: what the daemon says
/// now, read against the channel definitions beside the driver so that a tuner
/// in use says which channel rather than which frequency. Asked on the thread
/// that draws the window, so a daemon that does not answer is given up on
/// quickly.
fn tuner_state() -> Vec<state::Shown> {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return Vec::new();
    };
    let Ok((config, _, text)) = where_things_are(host) else {
        return Vec::new();
    };
    let daemon = Daemon::new(Endpoint::parse(&config.host))
        .with_priority(config.priority)
        .answering_within(ASKED_WHILE_DRAWING);
    let Ok(mut boards) = daemon.listing() else {
        return Vec::new();
    };
    // What the routes say a tuner receives goes on top of what it said, as it
    // does for a sweep: a card that does not declare a system is still on the
    // routes that name it.
    for board in &mut boards {
        let mut tuners: Vec<coax::daemon::Tuner> =
            board.tuners.iter().map(|held| held.tuner.clone()).collect();
        coax::daemon::declare(&config.declared_routes(), &mut tuners);
        for (held, tuner) in board.tuners.iter_mut().zip(tuners) {
            held.tuner = tuner;
        }
    }
    state::shown(&boards, &Defined::parse(&text), &Setup::read(&text).routes)
}

/// What the daemon has, for the drop-down. The daemon is started if it is not
/// answering and the settings allow it: a viewer opening this window is about
/// to ask for a sweep.
fn tuners() -> Vec<(String, String)> {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return Vec::new();
    };
    let Ok((config, file, _)) = where_things_are(host) else {
        return Vec::new();
    };
    let daemon = Daemon::new(Endpoint::parse(&config.host)).with_priority(config.priority);
    if config.start_daemon && daemon.version().is_err() {
        let launcher = Launcher {
            named: (!config.daemon.is_empty()).then(|| PathBuf::from(&config.daemon)),
            beside: file.parent().map(PathBuf::from),
        };
        let _ = daemon.ensure_running(&launcher);
    }
    daemon
        .tuners()
        .map(|tuners| tuners.into_iter().map(named).collect())
        .unwrap_or_default()
}

/// A tuner as a viewer reads it: what the daemon calls it, and what it
/// receives. The identifier alone says which card only to whoever set the
/// machine up.
fn named(tuner: coax::daemon::Tuner) -> (String, String) {
    /// How many delivery systems to spell out. A card that receives everything
    /// would fill the line and say nothing by it.
    const MOST_SHOWN: usize = 4;

    let mut shown = tuner.id.clone();
    if !tuner.deliveries.is_empty() {
        shown.push_str(" / ");
        shown.push_str(
            &tuner
                .deliveries
                .iter()
                .take(MOST_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
        if tuner.deliveries.len() > MOST_SHOWN {
            shown.push_str(" …");
        }
    }
    (tuner.id, shown)
}

/// What a button in either window means: the same as the command of that name.
fn on_button(command: i32) {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return;
    };
    chose(host, command);
}

/// The button of the sweep's window, which is the same command whichever of
/// its two jobs it is doing.
fn on_stop() {
    on_button(STOP);
}

/// The sweep's other button: stop, and keep what has been found so far.
fn on_done() {
    on_button(DONE);
}

/// Makes the host read the channel list from the driver again, which is the
/// only way what the sweep has just written reaches a viewer.
///
/// The host builds that list when a tuner is opened and at no other time, and
/// it will not open the tuner it already has. So the tuner is put down first
/// and then picked up again, which loads the driver afresh and has it read the
/// file beside it.
///
/// This runs on the thread that draws the host's window, because that is the
/// thread the host builds its filter graph on. Called from the sweep's own
/// thread it would fail, so the sweep has it posted there: see
/// `window::ask_reload`.
fn reload() {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return;
    };
    let Some(name) = host.driver_file_name() else {
        return;
    };
    host.close_tuner();
    // The host registers a command per driver, under the driver's own file
    // name, and it is the one its menu runs: it opens the tuner and puts the
    // viewer back on the channel they were watching. Naming the driver outright
    // does the opening but not the channel.
    if host.command(&name) || host.set_driver(&name) {
        host.log("coax: チャンネル定義を読み込み直しました。");
    } else {
        host.log("coax: チューナーを開き直せません。");
    }
}

/// Opens a file the way the viewer's own settings say to.
fn open(window: *mut c_void, file: &std::path::Path) {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                window: *mut c_void,
                verb: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> *mut c_void;
        }
        const SHOW_NORMAL: i32 = 1;
        if !file.is_file() {
            tell(
                window,
                "coax",
                "設定ファイルがありません。\n先にチャンネルスキャンを実行してください。",
            );
            return;
        }
        let verb = Wide::new("open");
        let path = Wide::new(&file.display().to_string());
        unsafe {
            ShellExecuteW(
                window,
                verb.as_ptr(),
                path.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                SHOW_NORMAL,
            )
        };
    }
    #[cfg(not(windows))]
    {
        let _ = (window, file);
    }
}

/// Runs the sweep on a thread of its own. The host calls the plugin on the
/// thread that draws its window, and a sweep is minutes of tuning: doing it
/// here would freeze the television.
fn start(host: Host, what: What) -> isize {
    {
        let mut sweeping = SWEEPING.lock().expect("the flag is not poisoned");
        if *sweeping {
            // Already running, so the window it is reporting into is brought
            // forward rather than a second sweep started.
            window::open_sweep(host.window(), on_stop, on_done);
            return 1;
        }
        *sweeping = true;
    }
    STOPPING.again();
    window::starting();
    window::open_sweep(host.window(), on_stop, on_done);
    let started = spawn_into(&SWEEPER, "coax discovery", move || {
        let outcome = look(host, &what);
        *SWEEPING.lock().expect("the flag is not poisoned") = false;
        // Said in the sweep's own window rather than in a message box: a
        // box put up from this thread and owned by another's window can
        // leave the host hung.
        let wrote = outcome.is_ok() && !(STOPPING.asked_to() && !STOPPING.keeping());
        match outcome {
            Ok(said) => window::finished(said, wrote),
            Err(reason) => window::finished(reason, false),
        }
        // Nothing was written, so there is nothing for the host to read
        // again -- and picking the tuner up and putting it down would take
        // the picture away.
        if wrote && !window::ask_reload(reload) {
            host.log("coax: チューナーを開き直すと、新しいチャンネル定義が読み込まれます。");
        }
    });
    if !matches!(started, Ok(true)) {
        *SWEEPING.lock().expect("the flag is not poisoned") = false;
        return 0;
    }
    host.log("coax: チャンネルスキャンを開始しました。");
    1
}

/// How long to give the daemon to notice that the host has let go of a tuner.
/// The host closing it and the connection carrying it going away are not the
/// same instant.
const TUNER_LET_GO: std::time::Duration = std::time::Duration::from_millis(500);

/// The settings beside the driver: what they say, where they are, and the
/// text as it stands -- which is what the findings are written back into.
fn where_things_are(host: Host) -> Result<(Config, PathBuf, String), String> {
    let file = match settings_file(host) {
        Some(file) => file,
        None => {
            return Err(
                match &*SETTINGS_FILE.lock().expect("the path is not poisoned") {
                    Some(_) => "BonDriverの設定ファイルを読めません。".to_owned(),
                    None => "BonDriverが選択されていません。".to_owned(),
                },
            );
        }
    };
    // In whichever encoding the viewer's editor wrote it, as the driver reads
    // it. A file that cannot be read stops here rather than being taken for
    // an empty one, which a sweep or the settings window would then write
    // over the viewer's own.
    let text = beside::read_for_writing(&file)
        .map_err(|reason| format!("{} を読めません。({reason})", file.display()))?;
    Ok((Config::parse(&text), file, text))
}

/// Where the settings beside the driver are.
///
/// The host is asked first and its answer remembered, because it stops
/// answering as soon as its tuner is closed -- which is what a sweep asks it to
/// do.
///
/// Which file that is follows the driver's own name: a host with
/// `BonDriver_coax-S.dll` open is looking at `BonDriver_coax-S.ini`. A viewer
/// may pick one of its neighbours instead, and that pick wins for as long as
/// the driver is one of the same family.
fn settings_file(host: Host) -> Option<PathBuf> {
    let driver = host.driver_path();
    let found = driver.as_deref().map(beside::settings_for);
    let picked = picked_file(driver.as_deref());
    let mut remembered = SETTINGS_FILE.lock().expect("the path is not poisoned");
    if let Some(found) = found {
        *remembered = Some(found.clone());
        return Some(picked.unwrap_or(found));
    }
    // The host has stopped answering. What it last said still stands, and so
    // does a pick made against it.
    picked.or_else(|| remembered.clone())
}

/// The file a viewer picked, when it still belongs beside the driver the host
/// has open. Forgotten when it does not.
fn picked_file(driver: Option<&std::path::Path>) -> Option<PathBuf> {
    let mut picked = PICKED_FILE.lock().expect("the pick is not poisoned");
    let chosen = picked.clone()?;
    if let Some(directory) = driver.and_then(|driver| driver.parent()) {
        if chosen.parent() != Some(directory) {
            *picked = None;
            return None;
        }
    }
    Some(chosen)
}

/// The files a viewer may choose between, and which of them is in use. The
/// family is read off the file in use rather than off the driver: the naming
/// rule is the same either way, and the file in use is known even while the
/// host has stopped answering, which is most of a sweep.
fn settings_choices(host: Host) -> (Vec<PathBuf>, usize) {
    let Some(in_use) = settings_file(host) else {
        // No driver has been open yet, so there is nothing to list.
        return (Vec::new(), 0);
    };
    let files = beside::settings_family(&in_use);
    let at = files
        .iter()
        .position(|known| *known == in_use)
        .unwrap_or_default();
    (files, at)
}

/// What the window does when a viewer picks a different file: it becomes the one
/// everything here reads and writes, and the routes are shown again from it.
fn on_picked(at: usize) {
    let Some(host) = *STATE.lock().expect("the state is not poisoned") else {
        return;
    };
    let (files, _) = settings_choices(host);
    let Some(chosen) = files.get(at) else {
        return;
    };
    *PICKED_FILE.lock().expect("the pick is not poisoned") = Some(chosen.clone());
    trace::line(&format!("plugin: settings now {chosen:?}"));
    // The driver is left alone: the host has the one it has, and this is the
    // plugin being pointed at a file, not the host being told to change tuner.
    let (routes, trouble) = routes_shown(host);
    let (files, at) = settings_choices(host);
    window::set_settings_files(shown_files(&files), at);
    window::set_routes(routes, trouble);
}

/// The files as a viewer reads them: the name alone, since they are all in one
/// directory and the path would be the same words repeated.
fn shown_files(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            file.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string_lossy().into_owned())
        })
        .collect()
}

/// The sweep itself, from the driver's settings to the file beside it.
fn look(host: Host, what: &What) -> Result<String, String> {
    trace::line("plugin: looking");
    let (config, file, text) = where_things_are(host)?;
    let directory = file
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "BonDriverのパスを取得できません。".to_owned())?;
    let known = Defined::parse(&text);

    let daemon = Daemon::new(Endpoint::parse(&config.host)).with_priority(config.priority);
    if config.start_daemon {
        let launcher = Launcher {
            named: (!config.daemon.is_empty()).then(|| PathBuf::from(&config.daemon)),
            beside: Some(directory.clone()),
        };
        daemon
            .ensure_running(&launcher)
            .map_err(|reason| format!("coaxdに接続できません。({reason})"))?;
    }

    // What a viewer chose in the drop-down, which stands in for what the
    // settings say about every space.
    let chosen = window::chosen_tuners();
    // The ordinary machine has one tuner and the host is holding it to show the
    // picture. A sweep that will not get a tuner is minutes of being refused,
    // so the host is asked to let go of it first -- and picks it up again when
    // the sweep is over.
    if !discover::a_tuner_is_free(&daemon, &config, what, &chosen) {
        if host.close_tuner() {
            host.log("coax: チャンネルスキャンをするためチューナーを閉じました。");
            // The daemon lets go of a tuner when the connection carrying it
            // does, which is not the same instant the host was told to close.
            std::thread::sleep(TUNER_LET_GO);
        } else {
            host.log("coax: チューナーを閉じられません。");
        }
    }

    let (found, report) =
        discover::sweep(&daemon, &config, &known, what, &chosen, &STOPPING, |step| {
            trace!("plugin: {}", step.what);
            window::step(
                step.what,
                step.done,
                step.total,
                step.cnr,
                step.strength,
                step.mbps,
            );
            if let Some(note) = step.note {
                // The measurement goes into the account as well as onto the
                // line above, so that it is still there once the sweep has
                // moved on.
                let mut said = format!("{}: {note}", step.what);
                if let Some(cnr) = step.cnr {
                    said.push_str(&format!("　CNR {cnr:.1} dB"));
                } else if let Some(strength) = step.strength {
                    said.push_str(&format!("　{strength:.1} dBm"));
                }
                if let Some(mbps) = step.mbps {
                    said.push_str(&format!("　{mbps:.1} Mbps"));
                }
                window::found(said);
            }
            // What the carrier turned out to hold goes under the line the
            // carrier was reported on, indented as it arrived: `coax scan`'s
            // own shape.
            for line in step.notes {
                window::found(line);
            }
        });
    trace!(
        "plugin: asked {}, answered {}, multiplexed {}, added {}, busy {}",
        report.asked, report.answered, report.multiplexed, report.added, report.busy
    );

    // Nothing was tuned and the tuner was somebody's throughout: the file is
    // left as it was, because a sweep that heard nothing has not found out that
    // there is nothing there.
    if report.gave_up {
        return Err("チューナーが使用中のため、チャンネルをスキャンできませんでした。\n他のソフトウェアがチューナーを使用していないか確認してください。".to_owned());
    }

    // A walk stopped with the button that throws it away writes nothing: half a
    // walk over the file a viewer already had is worse than nothing.
    if STOPPING.asked_to() && !STOPPING.keeping() {
        return Ok("チャンネルスキャンを中止しました。".to_owned());
    }

    // Written back into the settings file, into the sections that are the
    // sweep's own: the viewer's settings and comments are left as they were.
    std::fs::write(&file, found.write_into(&text))
        .map_err(|reason| format!("{} を保存できません。({reason})", file.display()))?;

    // The host is made to read what was just written by the sweep's window,
    // on the thread that window is drawn on: see `reload`.
    let mut said = format!(
        "チャンネルスキャンが終了しました。(走査 {} / 受信 {} / 多重 {} / 追加 {}",
        report.asked, report.answered, report.multiplexed, report.added
    );
    // A carrier nobody listened to is not a carrier with nothing on it, and a
    // viewer reading the account is entitled to know which it was.
    if report.busy > 0 {
        said.push_str(&format!(" / 使用中 {}", report.busy));
    }
    said.push(')');
    Ok(said)
}

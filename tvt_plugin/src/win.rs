// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! What every window of this plugin is made of: the system calls, the font and
//! the spacing the host's own dialogues use, the colours the system asks for,
//! and the drawing that has to be done by hand for a dark window.
//!
//! Nothing here knows which window it is working on. Each window keeps its own
//! controls and its own procedure, and comes here for what they have in common.

use core::ffi::c_void;
use std::sync::Mutex;

use coax_settings::wide::Wide;

pub(crate) type Handle = *mut c_void;

pub(crate) const WS_CHILD: u32 = 0x4000_0000;
pub(crate) const WS_VISIBLE: u32 = 0x1000_0000;
pub(crate) const WS_VSCROLL: u32 = 0x0020_0000;
pub(crate) const WS_TABSTOP: u32 = 0x0001_0000;
/// A window with a title bar and a close button and nothing else: neither
/// of these has anything to resize by, nor a reason to be minimised.
pub(crate) const WS_FIXED: u32 = 0x00C8_0000;
/// The dialogue frame, which is what keeps an icon out of the title bar.
pub(crate) const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub(crate) const SW_SHOW: i32 = 5;
pub(crate) const SW_RESTORE: i32 = 9;
pub(crate) const SW_HIDE: i32 = 0;

pub(crate) const WM_CREATE: u32 = 0x0001;
pub(crate) const WM_DESTROY: u32 = 0x0002;
pub(crate) const WM_CLOSE: u32 = 0x0010;
pub(crate) const WM_COMMAND: u32 = 0x0111;
pub(crate) const WM_SETFONT: u32 = 0x0030;
pub(crate) const WM_SETTEXT: u32 = 0x000C;
/// The sweep knocking to say it has left something behind.
pub(crate) const WM_ARRIVED: u32 = 0x0400 + 1;
/// A settings file was picked. Posted rather than acted on where the message
/// arrives, because acting on it rebuilds the very control that sent it.
pub(crate) const WM_PICKED: u32 = 0x0400 + 2;
/// Posted from a sweep's thread to have the host read the channel list again
/// on the thread that draws its window, which is the one that can.
pub(crate) const WM_RELOAD: u32 = 0x0400 + 3;
pub(crate) const WM_PAINT: u32 = 0x000F;
pub(crate) const WM_SETTINGCHANGE: u32 = 0x001A;
pub(crate) const WM_THEMECHANGED: u32 = 0x031A;
pub(crate) const WM_ERASEBKGND: u32 = 0x0014;
pub(crate) const WM_CTLCOLOREDIT: u32 = 0x0133;
pub(crate) const WM_CTLCOLORLISTBOX: u32 = 0x0134;
pub(crate) const WM_CTLCOLORBTN: u32 = 0x0135;
pub(crate) const WM_CTLCOLORSTATIC: u32 = 0x0138;

pub(crate) const CB_ADDSTRING: u32 = 0x0143;
pub(crate) const CB_RESETCONTENT: u32 = 0x014B;
pub(crate) const CB_GETCURSEL: u32 = 0x0147;
pub(crate) const CB_SETCURSEL: u32 = 0x014E;
/// A list that drops down and cannot be typed into: the names are the
/// daemon's, and a viewer typing one of them by hand is a viewer making a
/// mistake.
pub(crate) const CBS_DROPDOWNLIST: u32 = 0x0003;
/// What a drop-down says in the high half of a WM_COMMAND when the selection
/// moved.
pub(crate) const CBN_SELCHANGE: u32 = 1;

/// The messages the account of what was found is written with. It is an
/// edit rather than a list so that a viewer can drag over it and copy what
/// it says: a list box hands out its lines only to a program.
pub(crate) const EM_GETSEL: u32 = 0x00B0;
pub(crate) const EM_SETSEL: u32 = 0x00B1;
pub(crate) const WM_GETTEXTLENGTH: u32 = 0x000E;
pub(crate) const WM_GETTEXT: u32 = 0x000D;
pub(crate) const EM_REPLACESEL: u32 = 0x00C2;
pub(crate) const EM_SCROLLCARET: u32 = 0x00B7;
/// Several lines, no typing, and only as wide as the box: what is written
/// here is read, selected and copied, and nothing else.
pub(crate) const ES_MULTILINE: u32 = 0x0004;
pub(crate) const ES_READONLY: u32 = 0x0800;
pub(crate) const ES_AUTOVSCROLL: u32 = 0x0040;
pub(crate) const PBM_SETRANGE32: u32 = 0x0400 + 6;
pub(crate) const PBM_SETPOS: u32 = 0x0400 + 2;

/// The window frame follows the colours too: a dark window under a white
/// title bar looks like a mistake.
pub(crate) const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// What the common controls are asked to draw themselves as. The dark one
/// is what the system's own windows ask for, and a host at a theme this
/// does not know is left with the ordinary one.
pub(crate) const THEME_DARK: &str = "DarkMode_Explorer";
/// What a field is asked for instead: the box a drop-down list shows the
/// chosen line in, and a one-line field. The ordinary dark theme leaves
/// both of them white.
pub(crate) const THEME_DARK_FIELD: &str = "DarkMode_CFD";
/// The metrics the system keeps its window fonts in. Only the message font
/// is wanted -- what a dialogue is written in, and so what these windows
/// should be written in rather than the ancient stock font.
pub(crate) const SPI_GETNONCLIENTMETRICS: u32 = 0x0029;

/// The system's own colours, for a host that will not say what it uses.
pub(crate) const COLOR_BTNTEXT: i32 = 18;
pub(crate) const COLOR_BTNFACE: i32 = 15;
pub(crate) const COLOR_WINDOW: i32 = 5;
pub(crate) const COLOR_3DSHADOW: i32 = 16;

pub(crate) const DEFAULT_GUI_FONT: i32 = 17;
pub(crate) const IDC_ARROW: usize = 32512;
/// The dialogue face, so the windows look like the host's own.
pub(crate) const COLOR_3DFACE: usize = 15;

pub(crate) const SWP_NOSIZE: u32 = 0x0001;
pub(crate) const SWP_NOMOVE: u32 = 0x0002;
pub(crate) const SWP_SHOWWINDOW: u32 = 0x0040;
pub(crate) const SWP_NOZORDER: u32 = 0x0004;

/// The progress bar, which has no text to take a height from.
pub(crate) const BAR_HIGH: i32 = 16;
/// A label drawn down the middle of the box it is in, so that it lines up
/// with the field beside it.
pub(crate) const SS_CENTERIMAGE: u32 = 0x0200;
/// A box that ticks itself when it is clicked.
pub(crate) const BS_AUTOCHECKBOX: u32 = 0x0003;
pub(crate) const BM_GETCHECK: u32 = 0x00F0;
pub(crate) const BM_SETCHECK: u32 = 0x00F1;
pub(crate) const BST_CHECKED: usize = 1;
/// The kinds of button that draw a box and a label beside it rather than
/// a face. Those are the ones whose label has to be drawn here: a themed
/// button writes its own text in the theme's colour, which is black.
pub(crate) const BS_TYPEMASK: u32 = 0x000F;
pub(crate) const BS_CHECKBOX: u32 = 0x0002;
pub(crate) const BS_3STATE: u32 = 0x0005;
pub(crate) const BS_AUTO3STATE: u32 = 0x0006;
pub(crate) const BS_RADIOBUTTON: u32 = 0x0004;
pub(crate) const BS_AUTORADIOBUTTON: u32 = 0x0009;
pub(crate) const BS_PUSHLIKE: u32 = 0x1000;
pub(crate) const BS_MULTILINE: u32 = 0x2000;
pub(crate) const GWL_STYLE: i32 = -16;

pub(crate) const WM_NOTIFY: u32 = 0x004E;
pub(crate) const WM_QUERYUISTATE: u32 = 0x0129;
pub(crate) const UISF_HIDEACCEL: usize = 0x0002;
pub(crate) const UISF_HIDEFOCUS: usize = 0x0001;
/// A control saying it is about to draw itself, and the answer that says
/// it has been drawn already.
pub(crate) const NM_CUSTOMDRAW: u32 = (-12i32) as u32;
pub(crate) const CDDS_PREPAINT: u32 = 0x0001;
pub(crate) const CDRF_DODEFAULT: isize = 0x0000;
pub(crate) const CDRF_SKIPDEFAULT: isize = 0x0004;
pub(crate) const CDIS_SELECTED: u32 = 0x0001;
pub(crate) const CDIS_DISABLED: u32 = 0x0004;
pub(crate) const CDIS_FOCUS: u32 = 0x0010;
pub(crate) const CDIS_HOT: u32 = 0x0040;
pub(crate) const CDIS_INDETERMINATE: u32 = 0x0100;
/// The parts of the button theme, and the states each is drawn in.
pub(crate) const BP_CHECKBOX: i32 = 3;
pub(crate) const BP_RADIOBUTTON: i32 = 2;
pub(crate) const TS_TRUE: i32 = 1;
pub(crate) const DT_LEFT: u32 = 0x0000;
pub(crate) const DT_SINGLELINE: u32 = 0x0020;
pub(crate) const DT_VCENTER: u32 = 0x0004;
pub(crate) const DT_WORDBREAK: u32 = 0x0010;
pub(crate) const DT_HIDEPREFIX: u32 = 0x0010_0000;
pub(crate) const DT_CALCRECT: u32 = 0x0400;
pub(crate) const COLOR_GRAYTEXT: i32 = 17;

#[repr(C)]
pub(crate) struct WindowClass {
    pub(crate) size: u32,
    pub(crate) style: u32,
    pub(crate) procedure: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
    pub(crate) class_extra: i32,
    pub(crate) window_extra: i32,
    pub(crate) instance: Handle,
    pub(crate) icon: Handle,
    pub(crate) cursor: Handle,
    pub(crate) background: Handle,
    pub(crate) menu_name: *const u16,
    pub(crate) class_name: *const u16,
    pub(crate) small_icon: Handle,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LogFont {
    pub(crate) height: i32,
    pub(crate) width: i32,
    pub(crate) escapement: i32,
    pub(crate) orientation: i32,
    pub(crate) weight: i32,
    pub(crate) italic: u8,
    pub(crate) underline: u8,
    pub(crate) strike_out: u8,
    pub(crate) char_set: u8,
    pub(crate) out_precision: u8,
    pub(crate) clip_precision: u8,
    pub(crate) quality: u8,
    pub(crate) pitch_and_family: u8,
    pub(crate) face_name: [u16; 32],
}

#[repr(C)]
pub(crate) struct NonClientMetrics {
    pub(crate) size: u32,
    pub(crate) border_width: i32,
    pub(crate) scroll_width: i32,
    pub(crate) scroll_height: i32,
    pub(crate) caption_width: i32,
    pub(crate) caption_height: i32,
    pub(crate) caption_font: LogFont,
    pub(crate) small_caption_width: i32,
    pub(crate) small_caption_height: i32,
    pub(crate) small_caption_font: LogFont,
    pub(crate) menu_width: i32,
    pub(crate) menu_height: i32,
    pub(crate) menu_font: LogFont,
    pub(crate) status_font: LogFont,
    pub(crate) message_font: LogFont,
    pub(crate) padded_border_width: i32,
}

#[repr(C)]
pub(crate) struct Rect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(crate) fn RegisterClassExW(class: *const WindowClass) -> u16;
    pub(crate) fn CreateWindowExW(
        extended: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: usize,
        instance: Handle,
        parameter: *mut c_void,
    ) -> Handle;
    pub(crate) fn DefWindowProcW(
        window: Handle,
        message: u32,
        first: usize,
        second: isize,
    ) -> isize;
    pub(crate) fn DestroyWindow(window: Handle) -> i32;
    pub(crate) fn ShowWindow(window: Handle, how: i32) -> i32;
    pub(crate) fn SetForegroundWindow(window: Handle) -> i32;
    pub(crate) fn BringWindowToTop(window: Handle) -> i32;
    pub(crate) fn SetWindowPos(
        window: Handle,
        after: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    pub(crate) fn IsWindow(window: Handle) -> i32;
    pub(crate) fn SendMessageW(window: Handle, message: u32, first: usize, second: isize) -> isize;
    pub(crate) fn PostMessageW(window: Handle, message: u32, first: usize, second: isize) -> i32;
    pub(crate) fn GetDlgItem(window: Handle, id: i32) -> Handle;
    pub(crate) fn EnableWindow(window: Handle, enabled: i32) -> i32;
    pub(crate) fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
    pub(crate) fn LoadCursorW(instance: Handle, name: usize) -> Handle;
    pub(crate) fn GetSysColor(index: i32) -> u32;
    pub(crate) fn GetClientRect(window: Handle, rect: *mut Rect) -> i32;
    pub(crate) fn FillRect(context: Handle, rect: *const Rect, brush: Handle) -> i32;
    pub(crate) fn InvalidateRect(window: Handle, rect: *const Rect, erase: i32) -> i32;
    pub(crate) fn EnumChildWindows(
        window: Handle,
        each: Option<unsafe extern "system" fn(Handle, isize) -> i32>,
        with: isize,
    ) -> i32;
    pub(crate) fn GetDlgCtrlID(window: Handle) -> i32;
    pub(crate) fn GetClassNameW(window: Handle, into: *mut u16, length: i32) -> i32;
    pub(crate) fn GetWindowLongW(window: Handle, index: i32) -> i32;
    pub(crate) fn GetComboBoxInfo(window: Handle, about: *mut ComboBoxInfo) -> i32;
    pub(crate) fn DrawTextW(
        context: Handle,
        text: *const u16,
        length: i32,
        rect: *mut Rect,
        how: u32,
    ) -> i32;
    pub(crate) fn DrawFocusRect(context: Handle, rect: *const Rect) -> i32;
    pub(crate) fn SystemParametersInfoW(
        action: u32,
        size: u32,
        data: *mut c_void,
        write: u32,
    ) -> i32;
    pub(crate) fn BeginPaint(window: Handle, paint: *mut Paint) -> Handle;
    pub(crate) fn EndPaint(window: Handle, paint: *const Paint) -> i32;
    pub(crate) fn FrameRect(context: Handle, rect: *const Rect, brush: Handle) -> i32;
    pub(crate) fn ScreenToClient(window: Handle, point: *mut Point) -> i32;
    pub(crate) fn GetDC(window: Handle) -> Handle;
    pub(crate) fn ReleaseDC(window: Handle, context: Handle) -> i32;
    pub(crate) fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, extended: u32) -> i32;
}

#[repr(C)]
pub(crate) struct Point {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// What `BeginPaint` fills in. Only the device context is wanted, but the
/// whole of it has to be there for the call to write into.
#[repr(C)]
pub(crate) struct Paint {
    pub(crate) context: Handle,
    pub(crate) erase: i32,
    pub(crate) rect: Rect,
    pub(crate) restore: i32,
    pub(crate) update: i32,
    pub(crate) reserved: [u8; 32],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub(crate) fn GetModuleHandleW(name: *const u16) -> Handle;
    pub(crate) fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Handle) -> i32;
    pub(crate) fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    pub(crate) fn GetLastError() -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn UnregisterClassW(class_name: *const u16, instance: Handle) -> i32;
}

/// This library's own module, which is what its window classes and windows
/// are registered under. Not the host's: a class registered under the host's
/// module outlives this library, and the next copy of it loaded would find
/// the class already there pointing at a procedure that has gone.
pub(crate) fn own_instance() -> Handle {
    const FROM_ADDRESS: u32 = 0x0000_0004;
    const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
    let mut module: Handle = core::ptr::null_mut();
    let anchor = (own_instance as *const ()).cast::<u16>();
    unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &raw mut module) };
    module
}

/// The classes registered so far, to be taken away again before the library
/// is unloaded.
static REGISTERED: Mutex<Vec<Wide>> = Mutex::new(Vec::new());

/// Takes away every class this library registered. Its windows have to be gone
/// first.
pub(crate) fn unregister_classes() {
    let instance = own_instance();
    let mut registered = REGISTERED.lock().unwrap_or_else(|held| held.into_inner());
    for name in registered.drain(..) {
        unsafe { UnregisterClassW(name.as_ptr(), instance) };
    }
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub(crate) fn GetStockObject(which: i32) -> Handle;
    pub(crate) fn CreateSolidBrush(colour: u32) -> Handle;
    pub(crate) fn DeleteObject(object: Handle) -> i32;
    pub(crate) fn SetTextColor(context: Handle, colour: u32) -> u32;
    pub(crate) fn SetBkColor(context: Handle, colour: u32) -> u32;
    pub(crate) fn CreateFontIndirectW(font: *const LogFont) -> Handle;
    pub(crate) fn SelectObject(context: Handle, object: Handle) -> Handle;
    pub(crate) fn GetTextMetricsW(context: Handle, metrics: *mut TextMetrics) -> i32;
}

/// Only the height is wanted, but the call fills in the whole of it.
#[repr(C)]
pub(crate) struct TextMetrics {
    pub(crate) height: i32,
    pub(crate) ascent: i32,
    pub(crate) descent: i32,
    pub(crate) internal_leading: i32,
    pub(crate) external_leading: i32,
    pub(crate) average_width: i32,
    pub(crate) max_width: i32,
    pub(crate) weight: i32,
    pub(crate) overhang: i32,
    pub(crate) digitized_aspect_x: i32,
    pub(crate) digitized_aspect_y: i32,
    pub(crate) first_char: u16,
    pub(crate) last_char: u16,
    pub(crate) default_char: u16,
    pub(crate) break_char: u16,
    pub(crate) italic: u8,
    pub(crate) underlined: u8,
    pub(crate) struck_out: u8,
    pub(crate) pitch_and_family: u8,
    pub(crate) char_set: u8,
}

#[link(name = "uxtheme")]
unsafe extern "system" {
    pub(crate) fn SetWindowTheme(window: Handle, application: *const u16, id: *const u16) -> i32;
    pub(crate) fn OpenThemeData(window: Handle, classes: *const u16) -> Handle;
    pub(crate) fn CloseThemeData(theme: Handle) -> i32;
    pub(crate) fn DrawThemeBackground(
        theme: Handle,
        context: Handle,
        part: i32,
        state: i32,
        rect: *const Rect,
        clip: *const Rect,
    ) -> i32;
    pub(crate) fn GetThemePartSize(
        theme: Handle,
        context: Handle,
        part: i32,
        state: i32,
        rect: *const Rect,
        kind: i32,
        size: *mut Size,
    ) -> i32;
}

/// What a drop-down list keeps: only the window its list drops into is
/// wanted, but the call fills in the whole of it.
#[repr(C)]
pub(crate) struct ComboBoxInfo {
    pub(crate) size: u32,
    pub(crate) item: Rect,
    pub(crate) button: Rect,
    pub(crate) button_state: u32,
    pub(crate) combo: Handle,
    pub(crate) field: Handle,
    pub(crate) list: Handle,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// What a control that draws itself is told, and what it is drawing.
#[repr(C)]
pub(crate) struct Notify {
    pub(crate) from: Handle,
    pub(crate) id: usize,
    pub(crate) code: u32,
}

#[repr(C)]
pub(crate) struct CustomDraw {
    pub(crate) header: Notify,
    pub(crate) stage: u32,
    pub(crate) context: Handle,
    pub(crate) rect: Rect,
    pub(crate) item: usize,
    pub(crate) state: u32,
    pub(crate) with: isize,
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    pub(crate) fn DwmSetWindowAttribute(
        window: Handle,
        attribute: u32,
        value: *const c_void,
        size: u32,
    ) -> i32;
}

#[link(name = "comctl32")]
unsafe extern "system" {
    pub(crate) fn InitCommonControls();
}

/// The brushes the surfaces are painted with, kept rather than made per
/// message: they are asked for on every control of every window, several times
/// a second while a sweep runs. Thrown away when the system's colours change.
pub(crate) static BRUSH: Mutex<usize> = Mutex::new(0);
pub(crate) static SUNKEN: Mutex<usize> = Mutex::new(0);

/// The font the host's own windows are written in: the system's message
/// font, not the stock font, which is a bitmap face from another century
/// and the loudest thing that made these windows look unlike the host's.
pub(crate) static FONT: Mutex<usize> = Mutex::new(0);

pub(crate) fn ui_font() -> Handle {
    let mut held = FONT.lock().expect("the font is not poisoned");
    if *held == 0 {
        let mut metrics: NonClientMetrics = unsafe { core::mem::zeroed() };
        metrics.size = size_of::<NonClientMetrics>() as u32;
        let asked = unsafe {
            SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                metrics.size,
                (&raw mut metrics).cast(),
                0,
            )
        };
        let font = if asked != 0 {
            unsafe { CreateFontIndirectW(&raw const metrics.message_font) }
        } else {
            core::ptr::null_mut()
        };
        *held = if font.is_null() {
            unsafe { GetStockObject(DEFAULT_GUI_FONT) as usize }
        } else {
            font as usize
        };
    }
    *held as Handle
}

/// The spacing everything is laid out on, taken from the font rather than
/// written down in pixels: the host's windows grow with the message font
/// and its DPI, and one that does not ends up cramped beside them.
pub(crate) struct Metrics {
    /// A line of text.
    pub(crate) line: i32,
    /// A control that holds one line.
    pub(crate) row: i32,
    /// A push button.
    pub(crate) button: i32,
    /// The space round the edge, and between one control and the next.
    pub(crate) margin: i32,
    pub(crate) gap: i32,
}

pub(crate) fn metrics() -> Metrics {
    let mut text: TextMetrics = unsafe { core::mem::zeroed() };
    let context = unsafe { GetDC(core::ptr::null_mut()) };
    let line = if context.is_null() {
        16
    } else {
        let previous = unsafe { SelectObject(context, ui_font()) };
        let asked = unsafe { GetTextMetricsW(context, &raw mut text) };
        unsafe {
            SelectObject(context, previous);
            ReleaseDC(core::ptr::null_mut(), context);
        }
        if asked != 0 { text.height } else { 16 }
    };
    Metrics {
        line,
        row: line + 6,
        button: line + 14,
        margin: line - 3,
        gap: line / 2,
    }
}

/// What these windows are painted with.
///
/// The system's own dark mode decides, not the host's theme -- that is what the
/// host's own dialogues follow. The dark values are the host's own dialogue
/// colours, so that a window of this plugin standing beside one of the host's
/// is the same colour.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Colours {
    /// The window itself, and what a label or a button sits on.
    pub(crate) face: u32,
    /// What holds something rather than says something: the account of
    /// what was found, and any field.
    pub(crate) content: u32,
    pub(crate) text: u32,
    /// The line round the account.
    pub(crate) edge: u32,
}

pub(crate) fn colours() -> Colours {
    if dark_mode() {
        return Colours {
            face: 0x38_38_38,
            content: 0x20_20_20,
            text: 0xff_ff_ff,
            edge: 0x66_66_66,
        };
    }
    Colours {
        face: unsafe { GetSysColor(COLOR_BTNFACE) },
        content: unsafe { GetSysColor(COLOR_WINDOW) },
        text: unsafe { GetSysColor(COLOR_BTNTEXT) },
        edge: unsafe { GetSysColor(COLOR_3DSHADOW) },
    }
}

/// Whether the system is set to draw applications the dark way. High
/// contrast wins over it: a viewer who asked for that asked for their own
/// colours, not for anyone's idea of dark.
pub(crate) fn dark_mode() -> bool {
    /// The theme engine's unnamed export that answers it.
    const SHOULD_APPS_USE_DARK_MODE: usize = 132;
    const SPI_GETHIGHCONTRAST: u32 = 0x0042;
    const HCF_HIGHCONTRASTON: u32 = 0x0001;

    #[repr(C)]
    struct HighContrast {
        size: u32,
        flags: u32,
        scheme: *mut u16,
    }
    let mut contrast = HighContrast {
        size: size_of::<HighContrast>() as u32,
        flags: 0,
        scheme: core::ptr::null_mut(),
    };
    let asked = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.size,
            (&raw mut contrast).cast(),
            0,
        )
    };
    if asked != 0 && contrast.flags & HCF_HIGHCONTRASTON != 0 {
        return false;
    }
    let uxtheme = Wide::new("uxtheme.dll");
    let module = unsafe { GetModuleHandleW(uxtheme.as_ptr()) };
    if module.is_null() {
        return false;
    }
    let found = unsafe { GetProcAddress(module, SHOULD_APPS_USE_DARK_MODE as *const u8) };
    let Some(found) = std::ptr::NonNull::new(found) else {
        return false;
    };
    let should: unsafe extern "system" fn() -> u8 = unsafe { core::mem::transmute(found.as_ptr()) };
    unsafe { should() != 0 }
}

/// The brush the background is painted with, made once per set of colours.
pub(crate) fn background() -> Handle {
    let mut held = BRUSH.lock().expect("the brush is not poisoned");
    if *held == 0 {
        *held = unsafe { CreateSolidBrush(colours().face) } as usize;
    }
    *held as Handle
}

/// The same for what the list sits on.
pub(crate) fn sunken() -> Handle {
    let mut held = SUNKEN.lock().expect("the brush is not poisoned");
    if *held == 0 {
        *held = unsafe { CreateSolidBrush(colours().content) } as usize;
    }
    *held as Handle
}

/// Tells the theme engine a window is dark: a themed control on an untold
/// window comes out with a black label. What tells it is an export of `uxtheme`
/// that carries no name; the host calls the same one for its own dialogues, and
/// where it is not there -- an older Windows -- nothing is dark and nothing is
/// asked for.
pub(crate) fn allow_dark(window: Handle, dark: bool) {
    /// The per-window one. The process-wide one is the host's to set, and
    /// it does: a plugin that set it too would be overruling what the
    /// viewer chose for the whole program.
    const DARK_MODE_FOR_WINDOW: usize = 133;

    let uxtheme = Wide::new("uxtheme.dll");
    let module = unsafe { GetModuleHandleW(uxtheme.as_ptr()) };
    if module.is_null() {
        return;
    }
    let found = unsafe { GetProcAddress(module, DARK_MODE_FOR_WINDOW as *const u8) };
    let Some(found) = std::ptr::NonNull::new(found) else {
        return;
    };
    let allow: unsafe extern "system" fn(Handle, i32) -> i32 =
        unsafe { core::mem::transmute(found.as_ptr()) };
    unsafe { allow(window, i32::from(dark)) };
}

/// Puts a window and its controls into the colours the system asked for:
/// the frame, then every control, which the common controls draw from a
/// theme name rather than from anything this can hand them.
pub(crate) fn dress(window: Handle) {
    let dark = dark_mode();
    allow_dark(window, dark);
    let immersive: i32 = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const immersive).cast(),
            size_of::<i32>() as u32,
        )
    };
    // Every control, whatever it is and whenever it was added: a list of
    // identifiers written down here goes stale the next time the window
    // grows something.
    unsafe { EnumChildWindows(window, Some(dress_child), isize::from(dark)) };
}

/// Puts one control into the look the window is in. The dark names are the ones
/// the host gives its own controls; the light one is no name at all, which is
/// the control's own default rather than a theme of its own.
pub(crate) unsafe extern "system" fn dress_child(control: Handle, dark: isize) -> i32 {
    let dark = dark != 0;
    let class = class_of(control);
    let style = unsafe { GetWindowLongW(control, GWL_STYLE) } as u32;
    let field = class.eq_ignore_ascii_case("COMBOBOX")
        || (class.eq_ignore_ascii_case("EDIT") && style & ES_MULTILINE == 0);
    let wanted = Wide::new(if field { THEME_DARK_FIELD } else { THEME_DARK });
    let name = if dark {
        wanted.as_ptr()
    } else {
        core::ptr::null()
    };
    unsafe { SetWindowTheme(control, name, core::ptr::null()) };
    if class.eq_ignore_ascii_case("COMBOBOX") {
        // The list a drop-down drops is a window of its own and is not a
        // child of this one, so it is never walked to; without this the
        // lines stay white under a dark box.
        let mut about: ComboBoxInfo = unsafe { core::mem::zeroed() };
        about.size = size_of::<ComboBoxInfo>() as u32;
        if unsafe { GetComboBoxInfo(control, &raw mut about) } != 0 && !about.list.is_null() {
            let ordinary = Wide::new(THEME_DARK);
            let name = if dark {
                ordinary.as_ptr()
            } else {
                core::ptr::null()
            };
            unsafe { SetWindowTheme(about.list, name, core::ptr::null()) };
        }
    }
    1
}

/// What kind of control this is, by the name its class was made with.
pub(crate) fn class_of(control: Handle) -> String {
    let mut name = [0u16; 32];
    let length = unsafe { GetClassNameW(control, name.as_mut_ptr(), name.len() as i32) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&name[..length as usize])
}

/// Draws a tick box or a radio button the dark way.
///
/// A themed button writes its own label in the colour its theme says, so on a
/// dark window the label comes out black. When the system is dark the box is
/// drawn from the theme -- which does know the dark look -- and the label is
/// drawn here. The light look needs none of this.
pub(crate) fn draw_tick(drawing: &CustomDraw) -> isize {
    let control = drawing.header.from;
    let style = unsafe { GetWindowLongW(control, GWL_STYLE) } as u32;
    let kind = style & BS_TYPEMASK;
    let ticking = matches!(
        kind,
        BS_CHECKBOX | BS_AUTOCHECKBOX | BS_3STATE | BS_AUTO3STATE
    );
    let choosing = matches!(kind, BS_RADIOBUTTON | BS_AUTORADIOBUTTON);
    if (!ticking && !choosing) || style & BS_PUSHLIKE != 0 || drawing.stage != CDDS_PREPAINT {
        return CDRF_DODEFAULT;
    }

    let context = drawing.context;
    let theme = {
        let button = Wide::new("BUTTON");
        unsafe { OpenThemeData(control, button.as_ptr()) }
    };
    if theme.is_null() {
        return CDRF_DODEFAULT;
    }

    let checked = unsafe { SendMessageW(control, BM_GETCHECK, 0, 0) } as usize == BST_CHECKED;
    let middling = drawing.state & CDIS_INDETERMINATE != 0;
    let disabled = drawing.state & CDIS_DISABLED != 0;
    // The four the theme draws each of its boxes in, in the order the
    // theme numbers them: plain, hovered, held down, and greyed.
    let within = if drawing.state & CDIS_SELECTED != 0 {
        3
    } else if disabled {
        4
    } else if drawing.state & CDIS_HOT != 0 {
        2
    } else {
        1
    };
    let (part, state) = if choosing {
        (BP_RADIOBUTTON, if checked { 4 + within } else { within })
    } else if checked {
        (BP_CHECKBOX, 4 + within)
    } else if middling {
        (BP_CHECKBOX, 8 + within)
    } else {
        (BP_CHECKBOX, within)
    };

    let colours = colours();
    unsafe { FillRect(context, &raw const drawing.rect, background()) };

    let mut size = Size {
        width: 0,
        height: 0,
    };
    unsafe {
        GetThemePartSize(
            theme,
            context,
            part,
            state,
            core::ptr::null(),
            TS_TRUE,
            &raw mut size,
        )
    };
    let whole = &drawing.rect;
    let box_ = Rect {
        left: whole.left,
        top: whole.top + ((whole.bottom - whole.top) - size.height) / 2,
        right: whole.left + size.width,
        bottom: whole.top + ((whole.bottom - whole.top) - size.height) / 2 + size.height,
    };
    unsafe {
        DrawThemeBackground(
            theme,
            context,
            part,
            state,
            &raw const box_,
            core::ptr::null(),
        );
        CloseThemeData(theme);
    }

    let text = window_text(control);
    if !text.is_empty() {
        // Half a character between the box and what it says, which is
        // what the host leaves.
        let gap = size.width / 4;
        let mut where_ = Rect {
            left: box_.right + gap,
            top: whole.top,
            right: whole.right,
            bottom: whole.bottom,
        };
        let told = unsafe { SendMessageW(control, WM_QUERYUISTATE, 0, 0) } as usize;
        let mut how = DT_LEFT;
        if style & BS_MULTILINE != 0 {
            how |= DT_WORDBREAK;
        } else {
            how |= DT_SINGLELINE | DT_VCENTER;
        }
        if told & UISF_HIDEACCEL != 0 {
            how |= DT_HIDEPREFIX;
        }
        let wide = Wide::new(&text);
        let ink = if disabled {
            unsafe { GetSysColor(COLOR_GRAYTEXT) }
        } else {
            colours.text
        };
        unsafe {
            SetTextColor(context, ink);
            SetBkColor(context, colours.face);
            DrawTextW(
                context,
                wide.as_ptr(),
                text.chars().count() as i32,
                &raw mut where_,
                how,
            );
        }
        if drawing.state & CDIS_FOCUS != 0 && told & UISF_HIDEFOCUS == 0 {
            let mut around = Rect {
                left: box_.right + gap,
                top: whole.top,
                right: whole.right,
                bottom: whole.bottom,
            };
            unsafe {
                DrawTextW(
                    context,
                    wide.as_ptr(),
                    text.chars().count() as i32,
                    &raw mut around,
                    how | DT_CALCRECT,
                );
                DrawFocusRect(context, &raw const around);
            }
        }
    }

    CDRF_SKIPDEFAULT
}

/// What a control says, as it says it.
pub(crate) fn window_text(control: Handle) -> String {
    let length = unsafe { SendMessageW(control, WM_GETTEXTLENGTH, 0, 0) };
    if length <= 0 {
        return String::new();
    }
    let mut room = vec![0u16; length as usize + 1];
    let written =
        unsafe { SendMessageW(control, WM_GETTEXT, room.len(), room.as_mut_ptr() as isize) };
    if written <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&room[..written as usize])
}

pub(crate) fn there(window: Handle) -> bool {
    !window.is_null() && unsafe { IsWindow(window) != 0 }
}

/// Brings a window to the front, which is where a window a viewer has just
/// asked for belongs.
pub(crate) fn to_the_front(window: Handle) {
    unsafe {
        ShowWindow(window, SW_RESTORE);
        SetWindowPos(
            window,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        BringWindowToTop(window);
        SetForegroundWindow(window);
    }
}

/// Registers a class once, and says whether there is one to make windows
/// from.
pub(crate) fn class(
    name: &Wide,
    procedure: unsafe extern "system" fn(Handle, u32, usize, isize) -> isize,
) -> bool {
    unsafe { InitCommonControls() };
    let cursor = unsafe { LoadCursorW(core::ptr::null_mut(), IDC_ARROW) };
    let registered = WindowClass {
        size: core::mem::size_of::<WindowClass>() as u32,
        style: 0,
        procedure: Some(procedure),
        class_extra: 0,
        window_extra: 0,
        instance: own_instance(),
        icon: core::ptr::null_mut(),
        cursor,
        background: (COLOR_3DFACE + 1) as Handle,
        menu_name: core::ptr::null(),
        class_name: name.as_ptr(),
        small_icon: core::ptr::null_mut(),
    };
    if unsafe { RegisterClassExW(&raw const registered) } != 0 {
        REGISTERED
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(name.clone());
        return true;
    }
    // A class that is already registered is not a failure: the window was
    // opened before and closed again.
    const ALREADY: u32 = 1410;
    let why = unsafe { GetLastError() };
    if why == ALREADY {
        return true;
    }
    coax_settings::trace!("plugin: the window class would not register: {why}");
    false
}

/// Makes a window, owned by the host's so that it stays in front of it.
pub(crate) fn make(
    owner: Handle,
    class_name: &Wide,
    title: &str,
    width: i32,
    height: i32,
) -> Handle {
    let title = Wide::new(title);
    let (x, y) = beside(owner);
    // The size asked for is the room the controls need; what a window is
    // made with includes its frame and title bar.
    let mut wanted = Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe { AdjustWindowRectEx(&raw mut wanted, WS_FIXED, 0, WS_EX_DLGMODALFRAME) };
    let (width, height) = (wanted.right - wanted.left, wanted.bottom - wanted.top);
    let window = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_FIXED,
            x,
            y,
            width,
            height,
            owner,
            0,
            own_instance(),
            core::ptr::null_mut(),
        )
    };
    if window.is_null() {
        coax_settings::trace!("plugin: no window: {}", unsafe { GetLastError() });
    }
    window
}

/// Where to put a window: at the top left of the one it was opened from,
/// moved in a little so that it does not sit exactly over it.
pub(crate) fn beside(owner: Handle) -> (i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if owner.is_null() || unsafe { GetWindowRect(owner, &raw mut rect) } == 0 {
        /// Let the system choose where to put it.
        const DEFAULT: i32 = i32::MIN;
        return (DEFAULT, DEFAULT);
    }
    (rect.left + 48, rect.top + 48)
}

pub(crate) fn control(
    window: Handle,
    class_name: &str,
    text: &str,
    style: u32,
    at: (i32, i32, i32, i32),
    id: usize,
) {
    let class_name = Wide::new(class_name);
    let text = Wide::new(text);
    let control = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            at.0,
            at.1,
            at.2,
            at.3,
            window,
            id,
            own_instance(),
            core::ptr::null_mut(),
        )
    };
    unsafe { SendMessageW(control, WM_SETFONT, ui_font() as usize, 1) };
}

pub(crate) fn set_text(window: Handle, id: usize, text: &str) {
    let control = unsafe { GetDlgItem(window, id as i32) };
    if control.is_null() {
        return;
    }
    let text = Wide::new(text);
    unsafe { SendMessageW(control, WM_SETTEXT, 0, text.as_ptr() as isize) };
}

/// Draws a line round a control, in a colour a little way from the background.
/// A list of what was found wants an edge to read as a list rather than as
/// words on the window, and the edge the control draws for itself comes from
/// the theme, which leaves it light.
pub(crate) fn outline(window: Handle, context: Handle, id: usize) {
    let control = unsafe { GetDlgItem(window, id as i32) };
    if control.is_null() || context.is_null() {
        return;
    }
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(control, &raw mut rect) } == 0 {
        return;
    }
    let mut top_left = Point {
        x: rect.left,
        y: rect.top,
    };
    let mut bottom_right = Point {
        x: rect.right,
        y: rect.bottom,
    };
    unsafe {
        ScreenToClient(window, &raw mut top_left);
        ScreenToClient(window, &raw mut bottom_right);
    }
    let around = Rect {
        left: top_left.x - 1,
        top: top_left.y - 1,
        right: bottom_right.x + 1,
        bottom: bottom_right.y + 1,
    };
    let brush = unsafe { CreateSolidBrush(colours().edge) };
    if brush.is_null() {
        return;
    }
    unsafe {
        FrameRect(context, &raw const around, brush);
        DeleteObject(brush);
    }
}

/// Throws the brushes away, for when the system's colours have changed.
/// Whatever is open is dressed again by whoever holds it.
pub(crate) fn forget_colours() {
    for which in [&BRUSH, &SUNKEN] {
        let mut brush = which.lock().expect("the brush is not poisoned");
        if *brush != 0 {
            unsafe { DeleteObject(*brush as Handle) };
            *brush = 0;
        }
    }
}

/// Puts a window into the colours the system asks for now, and has it
/// painted again.
pub(crate) fn redress(window: Handle) {
    if there(window) {
        dress(window);
        unsafe { InvalidateRect(window, core::ptr::null(), 1) };
    }
}

/// Gives a window the room its controls need; the frame is added here, as
/// it was when the window was made.
pub(crate) fn resize(window: Handle, width: i32, height: i32) {
    let mut wanted = Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe { AdjustWindowRectEx(&raw mut wanted, WS_FIXED, 0, WS_EX_DLGMODALFRAME) };
    unsafe {
        SetWindowPos(
            window,
            core::ptr::null_mut(),
            0,
            0,
            wanted.right - wanted.left,
            wanted.bottom - wanted.top,
            SWP_NOMOVE | SWP_NOZORDER,
        )
    };
}

pub(crate) fn item(window: Handle, id: usize) -> Handle {
    unsafe { GetDlgItem(window, id as i32) }
}

/// Whether a tick box is ticked. One that is not there is not.
pub(crate) fn ticked(window: Handle, id: usize) -> bool {
    let box_ = item(window, id);
    !box_.is_null() && unsafe { SendMessageW(box_, BM_GETCHECK, 0, 0) } as usize == BST_CHECKED
}

pub(crate) fn tick(window: Handle, id: usize, on: bool) {
    let box_ = item(window, id);
    if !box_.is_null() {
        unsafe { SendMessageW(box_, BM_SETCHECK, usize::from(on), 0) };
    }
}

pub(crate) fn enable(window: Handle, id: usize, on: bool) {
    let control = item(window, id);
    if !control.is_null() {
        unsafe { EnableWindow(control, i32::from(on)) };
    }
}

pub(crate) fn show(window: Handle, id: usize, on: bool) {
    let control = item(window, id);
    if !control.is_null() {
        unsafe { ShowWindow(control, if on { SW_SHOW } else { SW_HIDE }) };
    }
}

/// What a control says, by its number.
pub(crate) fn text_of(window: Handle, id: usize) -> String {
    let control = item(window, id);
    if control.is_null() {
        return String::new();
    }
    window_text(control)
}

/// Takes every control off a window, for one that is about to be furnished
/// again from what it now has to show.
pub(crate) fn strip(window: Handle) {
    unsafe extern "system" fn take_down(control: Handle, _: isize) -> i32 {
        unsafe { DestroyWindow(control) };
        1
    }
    unsafe { EnumChildWindows(window, Some(take_down), 0) };
}

/// Answers what every window of this plugin answers the same way: the
/// background, the colours a control asks for, the boxes that have to be drawn
/// by hand when the system is dark, a change of the system's colours, and
/// closing. `holds` says which controls hold something rather than say
/// something. Nothing where the window answers for itself.
pub(crate) fn common(
    window: Handle,
    message: u32,
    first: usize,
    second: isize,
    holds: &dyn Fn(usize) -> bool,
) -> Option<isize> {
    match message {
        WM_ERASEBKGND => {
            let mut rect = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            unsafe {
                GetClientRect(window, &raw mut rect);
                FillRect(first as Handle, &raw const rect, background());
            }
            Some(1)
        }
        // Every control that draws its own text asks the window it is in what
        // to draw it with. What holds something sits on the content colour and
        // what says something on the window's own; which is which cannot be
        // told from the message.
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT => {
            let colours = colours();
            let id = unsafe { GetDlgCtrlID(second as Handle) } as usize;
            let holds = holds(id);
            let back = if holds { colours.content } else { colours.face };
            unsafe {
                SetTextColor(first as Handle, colours.text);
                SetBkColor(first as Handle, back);
            }
            let brush = if holds { sunken() } else { background() };
            Some(brush as isize)
        }
        // A control saying it is about to draw itself. Only the boxes that
        // draw a label want anything doing, and only when the system is dark.
        WM_NOTIFY if dark_mode() => {
            let notice = second as *const Notify;
            if notice.is_null() {
                return None;
            }
            let code = unsafe { (*notice).code };
            let from = unsafe { (*notice).from };
            if code != NM_CUSTOMDRAW || !class_of(from).eq_ignore_ascii_case("BUTTON") {
                return None;
            }
            Some(draw_tick(unsafe { &*(second as *const CustomDraw) }))
        }
        // The system's colours changed under the window, or its theme did.
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            forget_colours();
            redress(window);
            Some(0)
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            Some(0)
        }
        _ => None,
    }
}

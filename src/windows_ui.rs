//! Win32 pieces of the Windows screensaver build: the `/c` settings dialog and the single-instance guard.
//! This is the only module of the crate that allows `unsafe` (it calls into the Win32 API).
#![allow(unsafe_code)]

use std::num::NonZeroIsize;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle};
use winit::window::{Window, WindowAttributes};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DEFAULT_GUI_FONT, DeleteObject, GetStockObject, HBRUSH, HGDIOBJ,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, MonitorFromWindow, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext, SystemParametersInfoForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, GetClientRect, GetDlgItem, GetMessageW, GetSystemMetrics,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW, IDCANCEL, IDOK, IsDialogMessageW, IsWindow,
    KillTimer, LoadCursorW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MSG, MessageBoxW, NONCLIENTMETRICSW, PostMessageW,
    PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETNONCLIENTMETRICS, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowTextW, ShowWindow, TranslateMessage, UnregisterClassW, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::overlay::{Place, search_places};

const TITLE: &str = "Johnny Castaway – ustawienia";
const CLASS: &str = "OpenScranticSettings";
/// Number of checkboxes in [`Settings::checks`].
pub const CHECKS: usize = 9;
/// In the order of [`Settings::checks`]: the first four in the left column, the next four in the right one, and
/// the last one below the left column.
const LABELS: [&str; CHECKS] = [
    "Tło",
    "Chmury",
    "Fale",
    "Wygładzanie obrazu",
    "Dźwięki",
    "Lepszy dźwięk",
    "Zegar",
    "Pogoda",
    "Wszystkie monitory",
];
const COLUMNS: [&str; 2] = ["Obraz", "Dźwięk i nakładka"];
/// Control id of the first checkbox; the others follow.
const FIRST_CHECKBOX: i32 = 101;
/// Control id of the weather place field.
const LOCATION_FIELD: i32 = 120;
/// Longest place the field accepts (the INI keeps at most this many characters too).
const LOCATION_MAX: usize = 80;
/// Control id of the list of places matching the field.
const PLACE_LIST: i32 = 121;
/// The search runs this long after the last keystroke in the field.
const SEARCH_TIMER: usize = 1;
const SEARCH_DELAY_MS: u32 = 400;
/// Posted by the search thread when its results are in [`PLACES`]: `wparam` is the search generation, `lparam` 1
/// when nothing was found.
const WM_PLACES: u32 = WM_APP + 1;
/// Set in [`RESULT`] when the user pressed OK; the low bits hold the checkboxes.
const ACCEPTED: u32 = 1 << CHECKS;
const CHECKED: usize = 1;

const CLIENT_W: i32 = 420;
const CLIENT_H: i32 = 452;
/// `SS_CENTER`: static text centered in its box; `SS_LEFT` (0) is the default.
const SS_CENTER: u32 = 1;
// Edit and list box messages, notifications and styles (in `Win32_UI_Controls`, a feature this crate does not
// otherwise need).
const EM_LIMITTEXT: u32 = 0x00C5;
const EN_CHANGE: u32 = 0x0300;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_GETCURSEL: u32 = 0x0188;
const LBN_SELCHANGE: u32 = 1;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const MARGIN: i32 = 24;
const COLUMN_W: i32 = 180;
const BUTTON_W: i32 = 84;
const BUTTON_GAP: i32 = 12;

/// Outcome of the dialog, shared with the window procedure (there is one dialog per process).
static RESULT: AtomicU32 = AtomicU32::new(0);
/// Text of the place field when OK was pressed.
static LOCATION: Mutex<String> = Mutex::new(String::new());
/// `lat,lon` of the place picked from the list; cleared when the field is edited by hand.
static COORDS: Mutex<String> = Mutex::new(String::new());
/// Places in the list, in its order.
static PLACES: Mutex<Vec<Place>> = Mutex::new(Vec::new());
/// Bumped by every search: only the latest one may fill the list.
static SEARCH_GENERATION: AtomicU32 = AtomicU32::new(0);
/// Set while the dialog itself writes the field, so that its change notification does not start a search.
static SETTING_FIELD: AtomicBool = AtomicBool::new(false);

/// What the settings dialog edits: the checkboxes (Background, Clouds, Waves, Dedither, Sounds, SoftAudio, Clock,
/// Weather, AllMonitors) and the place of the weather report.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub checks: [bool; CHECKS],
    pub location: String,
    /// `lat,lon` of `location` when it was picked from the search, else empty.
    pub coords: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `v`, a length at 96 DPI, at `dpi`, rounded to the nearest pixel.
fn scaled(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// DPI of the monitor `owner` is on, or of the primary monitor without an owner; 96 if it cannot be told.
/// (The monitor's, not the window's: a window of a DPI-unaware process reports 96 wherever it is.)
unsafe fn monitor_dpi(owner: HWND) -> u32 {
    // SAFETY: the monitor handle comes from the calls just before; both out values are valid for the call.
    unsafe {
        let monitor = if owner.is_null() {
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
        } else {
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST)
        };
        let (mut x, mut y) = (0, 0);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) == 0 && x != 0 {
            x
        } else {
            96
        }
    }
}

/// The font Windows uses for its own dialogs (Segoe UI 9 pt) at `dpi`, or null if it cannot be queried. The
/// caller deletes it.
unsafe fn system_message_font(dpi: u32) -> HGDIOBJ {
    // SAFETY: `metrics` is valid for the call and `cbSize` tells Windows how much of it to fill.
    unsafe {
        let mut metrics: NONCLIENTMETRICSW = std::mem::zeroed();
        metrics.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&raw mut metrics).cast(),
            0,
            dpi,
        );
        if ok == 0 {
            return null_mut();
        }
        CreateFontIndirectW(&metrics.lfMessageFont)
    }
}

/// Creates one child control of `parent`.
#[allow(clippy::too_many_arguments)]
unsafe fn child(parent: HWND, class: &str, text: &str, style: u32, rect: [i32; 4], id: i32, font: usize) -> HWND {
    let (class, text) = (wide(class), wide(text));
    let [x, y, w, h] = rect;
    // SAFETY: `class` and `text` are NUL-terminated and outlive the call; `parent` is a live window.
    unsafe {
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            parent,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font, 1);
        hwnd
    }
}

/// Bit `i` is set when checkbox `i` is checked.
unsafe fn read_checks(hwnd: HWND) -> u32 {
    (0..LABELS.len() as i32).fold(0, |bits, i| {
        // SAFETY: the checkboxes are children of `hwnd`, created before the message loop starts.
        let checked = unsafe { SendMessageW(GetDlgItem(hwnd, FIRST_CHECKBOX + i), BM_GETCHECK, 0, 0) };
        bits | (u32::from(checked as usize == CHECKED) << i)
    })
}

/// Text of the place field.
unsafe fn read_location(hwnd: HWND) -> String {
    // SAFETY: the field is a child of `hwnd`; `buf` has room for the text and its NUL.
    unsafe {
        let field = GetDlgItem(hwnd, LOCATION_FIELD);
        let len = GetWindowTextLengthW(field).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(field, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n.min(len)])
    }
}

/// Searches for the text of the place field on a worker thread (the dialog stays responsive while curl runs);
/// the result arrives as [`WM_PLACES`].
unsafe fn start_search(hwnd: HWND) {
    // SAFETY: `hwnd` is the live dialog.
    let query = unsafe { read_location(hwnd) }.trim().to_string();
    let generation = SEARCH_GENERATION.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if query.chars().count() < 2 {
        if let Ok(mut places) = PLACES.lock() {
            places.clear();
        }
        // SAFETY: as above.
        unsafe { show_places(hwnd, generation, false) };
        return;
    }
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let found = search_places(&query);
        if SEARCH_GENERATION.load(Ordering::Relaxed) != generation {
            return;
        }
        let nothing = found.is_empty();
        if let Ok(mut places) = PLACES.lock() {
            *places = found;
        }
        // SAFETY: posting to a window that is gone by now only fails.
        unsafe {
            PostMessageW(target as HWND, WM_PLACES, generation as usize, LPARAM::from(nothing));
        }
    });
}

/// Fills the list with [`PLACES`], if `generation` is still the latest search.
unsafe fn show_places(hwnd: HWND, generation: u32, nothing_found: bool) {
    if generation != SEARCH_GENERATION.load(Ordering::Relaxed) {
        return;
    }
    let places = PLACES.lock().map(|p| p.clone()).unwrap_or_default();
    let mut lines: Vec<String> = places
        .iter()
        .map(|p| {
            if p.description.is_empty() {
                p.name.clone()
            } else {
                format!("{}  —  {}", p.name, p.description)
            }
        })
        .collect();
    if nothing_found {
        // Not a place: picking it does nothing, as its index is past the end of PLACES.
        lines.push("(nie znaleziono takiej miejscowości)".to_string());
    }
    // SAFETY: the list is a child of `hwnd`; every string is NUL-terminated and outlives its call.
    unsafe {
        let list = GetDlgItem(hwnd, PLACE_LIST);
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        for line in lines {
            let text = wide(&line);
            SendMessageW(list, LB_ADDSTRING, 0, text.as_ptr() as LPARAM);
        }
    }
}

/// The place selected in the list goes into the field, with its coordinates.
unsafe fn pick_place(hwnd: HWND) {
    // SAFETY: the list and the field are children of `hwnd`; the text outlives the call.
    unsafe {
        let index = SendMessageW(GetDlgItem(hwnd, PLACE_LIST), LB_GETCURSEL, 0, 0);
        let picked = usize::try_from(index)
            .ok()
            .and_then(|i| PLACES.lock().ok().and_then(|places| places.get(i).cloned()));
        let Some(place) = picked else { return };
        let name = wide(&place.name);
        SETTING_FIELD.store(true, Ordering::Relaxed);
        SetWindowTextW(GetDlgItem(hwnd, LOCATION_FIELD), name.as_ptr());
        SETTING_FIELD.store(false, Ordering::Relaxed);
        if let Ok(mut coords) = COORDS.lock() {
            *coords = place.coords();
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: called by the system with the arguments of a message for `hwnd`.
    unsafe {
        match msg {
            WM_COMMAND => {
                let (id, code) = ((wparam & 0xFFFF) as i32, (wparam >> 16 & 0xFFFF) as u32);
                match (id, code) {
                    (IDOK, _) => {
                        if let Ok(mut place) = LOCATION.lock() {
                            *place = read_location(hwnd);
                        }
                        RESULT.store(read_checks(hwnd) | ACCEPTED, Ordering::Relaxed);
                        DestroyWindow(hwnd);
                    }
                    (IDCANCEL, _) => {
                        DestroyWindow(hwnd);
                    }
                    (LOCATION_FIELD, EN_CHANGE) if !SETTING_FIELD.load(Ordering::Relaxed) => {
                        // Typed by hand: the coordinates of a place picked earlier no longer apply.
                        if let Ok(mut coords) = COORDS.lock() {
                            coords.clear();
                        }
                        SetTimer(hwnd, SEARCH_TIMER, SEARCH_DELAY_MS, None);
                    }
                    (PLACE_LIST, LBN_SELCHANGE) => pick_place(hwnd),
                    _ => {}
                }
                0
            }
            WM_TIMER if wparam == SEARCH_TIMER => {
                KillTimer(hwnd, SEARCH_TIMER);
                start_search(hwnd);
                0
            }
            WM_PLACES => {
                show_places(hwnd, wparam as u32, lparam != 0);
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Shows the modal settings dialog preset to `initial`. Returns the new values on OK, `None` on Cancel (or if the
/// dialog cannot be created). `owner` is the window handle Windows passes after `/c:`; it is disabled while the
/// dialog is open.
pub fn settings_dialog(initial: &Settings, owner: Option<isize>) -> Option<Settings> {
    RESULT.store(0, Ordering::Relaxed);
    if let Ok(mut coords) = COORDS.lock() {
        coords.clone_from(&initial.coords);
    }
    let class = wide(CLASS);
    // SAFETY: plain Win32 calls; every pointer passed is valid for the duration of its call.
    unsafe {
        // Without this Windows stretches the dialog on screens scaled above 100%, which blurs it. This process
        // shows nothing else; a run gets the same awareness from winit.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return None;
        }
        let owner = owner.map_or(null_mut(), |h| h as usize as HWND);
        let owner = if !owner.is_null() && IsWindow(owner) != 0 {
            owner
        } else {
            null_mut()
        };

        let dpi = monitor_dpi(owner);
        let px = |v: i32| scaled(v, dpi);
        let (cw, ch) = (px(CLIENT_W), px(CLIENT_H));
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        };
        AdjustWindowRectExForDpi(&mut frame, style, 0, WS_EX_DLGMODALFRAME, dpi);
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let title = wide(TITLE);
        // Centered over the owner (Screen Saver Settings) when there is one, else on the screen.
        let mut area = RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        };
        if !owner.is_null() {
            GetWindowRect(owner, &mut area);
        }
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class.as_ptr(),
            title.as_ptr(),
            style,
            (area.left + area.right - w) / 2,
            (area.top + area.bottom - h) / 2,
            w,
            h,
            owner,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            UnregisterClassW(class.as_ptr(), instance);
            return None;
        }

        let message_font = system_message_font(dpi);
        let font = if message_font.is_null() {
            GetStockObject(DEFAULT_GUI_FONT)
        } else {
            message_font
        } as usize;
        let margin = px(MARGIN);
        child(
            hwnd,
            "STATIC",
            "Wybierz, co ma być włączone:",
            SS_CENTER,
            [margin, px(14), cw - 2 * margin, px(18)],
            -1,
            font,
        );
        let (column_w, row) = (px(COLUMN_W), px(24));
        let column_x = |c: i32| margin + c * (cw - 2 * margin) / 2;
        for (c, header) in COLUMNS.iter().enumerate() {
            child(
                hwnd,
                "STATIC",
                header,
                0,
                [column_x(c as i32), px(42), column_w, px(18)],
                -1,
                font,
            );
        }
        for (i, label) in LABELS.iter().enumerate() {
            let id = FIRST_CHECKBOX + i as i32;
            let (c, r) = if i < 8 {
                ((i / 4) as i32, (i % 4) as i32)
            } else {
                (0, 4)
            };
            let style = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
            let check = child(
                hwnd,
                "BUTTON",
                label,
                style,
                [column_x(c) + px(8), px(64) + row * r, column_w, px(20)],
                id,
                font,
            );
            SendMessageW(check, BM_SETCHECK, usize::from(initial.checks[i]), 0);
        }
        child(
            hwnd,
            "STATIC",
            "Miejscowość dla pogody (puste = według adresu IP):",
            0,
            [margin, px(194), cw - 2 * margin, px(18)],
            -1,
            font,
        );
        let field = child(
            hwnd,
            "EDIT",
            &initial.location,
            WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            [margin, px(216), cw - 2 * margin, px(24)],
            LOCATION_FIELD,
            font,
        );
        SendMessageW(field, EM_LIMITTEXT, LOCATION_MAX, 0);
        child(
            hwnd,
            "LISTBOX",
            "",
            WS_TABSTOP | WS_BORDER | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT,
            [margin, px(246), cw - 2 * margin, px(132)],
            PLACE_LIST,
            font,
        );
        child(
            hwnd,
            "STATIC",
            "Wpisz nazwę i wybierz miejscowość z listy (z powiatem i województwem).",
            0,
            [margin, px(382), cw - 2 * margin, px(18)],
            -1,
            font,
        );
        let (button_w, gap, y, button_h) = (px(BUTTON_W), px(BUTTON_GAP), px(410), px(28));
        let x = (cw - 2 * button_w - gap) / 2;
        let ok = WS_TABSTOP | BS_DEFPUSHBUTTON as u32;
        child(hwnd, "BUTTON", "OK", ok, [x, y, button_w, button_h], IDOK, font);
        let cancel = WS_TABSTOP | BS_PUSHBUTTON as u32;
        child(
            hwnd,
            "BUTTON",
            "Anuluj",
            cancel,
            [x + button_w + gap, y, button_w, button_h],
            IDCANCEL,
            font,
        );

        if !owner.is_null() {
            EnableWindow(owner, 0);
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(GetDlgItem(hwnd, FIRST_CHECKBOX));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            // Tab, Enter and Esc are handled here, which sends IDOK and IDCANCEL to `wnd_proc`.
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if !owner.is_null() {
            EnableWindow(owner, 1);
            SetForegroundWindow(owner);
        }
        UnregisterClassW(class.as_ptr(), instance);
        if !message_font.is_null() {
            DeleteObject(message_font);
        }
    }
    let bits = RESULT.load(Ordering::Relaxed);
    (bits & ACCEPTED != 0).then(|| Settings {
        checks: std::array::from_fn(|i| bits >> i & 1 != 0),
        location: LOCATION.lock().map(|place| place.clone()).unwrap_or_default(),
        coords: COORDS.lock().map(|coords| coords.clone()).unwrap_or_default(),
    })
}

/// `true` if another screensaver instance is already running in this session. The first caller
/// holds the named mutex until the process exits.
pub fn already_running() -> bool {
    let name = wide("Local\\OpenScranticSaver");
    // SAFETY: `name` is NUL-terminated; the handle is deliberately never closed.
    unsafe {
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS
    }
}

/// Attributes of a borderless child window that fills `parent`, the preview pane of the Screen Saver
/// Settings window (the handle Windows passes after `/p`).
pub fn preview_attributes(parent: isize) -> WindowAttributes {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 1,
        bottom: 1,
    };
    // SAFETY: `rect` is valid for the call; a stale handle only makes it fail and leave the 1x1 default.
    unsafe {
        GetClientRect(parent as usize as HWND, &mut rect);
    }
    let size = PhysicalSize::new(
        (rect.right - rect.left).max(1) as u32,
        (rect.bottom - rect.top).max(1) as u32,
    );
    let attrs = Window::default_attributes()
        .with_title("Open Scrantic")
        .with_decorations(false)
        .with_inner_size(size)
        .with_position(PhysicalPosition::new(0, 0));
    match NonZeroIsize::new(parent) {
        // SAFETY: `parent` is the window Windows created for the preview and keeps alive while it runs.
        Some(handle) => unsafe {
            attrs.with_parent_window(Some(RawWindowHandle::Win32(Win32WindowHandle::new(handle))))
        },
        None => attrs,
    }
}

/// `false` once the window with this handle is gone: the preview ends with its pane.
pub fn window_exists(handle: isize) -> bool {
    // SAFETY: `IsWindow` accepts any handle value.
    unsafe { IsWindow(handle as usize as HWND) != 0 }
}

/// Shows a fatal error. The screensaver build has no console, so a message box is the only place for it.
pub fn error_box(text: &str) {
    let (text, title) = (wide(text), wide("Open Scrantic"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_scale_with_the_dpi_and_round_to_the_nearest_pixel() {
        assert_eq!(scaled(280, 96), 280);
        assert_eq!(scaled(280, 144), 420);
        assert_eq!(scaled(188, 120), 235);
        assert_eq!(scaled(1, 120), 1); // 1.25
        assert_eq!(scaled(3, 120), 4); // 3.75
        assert_eq!(scaled(0, 192), 0);
    }
}

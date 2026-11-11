//! About window: version, developer and licence details.

use super::form::{self, Row};
use super::{app, hinstance, theme, APP_NAME};
use std::cell::Cell;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("KeepStickyNotes_About");
const DEVELOPER: &str = "Dr Sifat";
const GITHUB: &str = "https://github.com/sifatrhmn";
const LICENCE: &str = include_str!("../../LICENSE");
const ID_NAME: i32 = 1;
const ID_GITHUB: i32 = 2;
const ID_LICENCE: i32 = 3;
const ID_CLOSE: i32 = 4;
const ID_VERSION: i32 = 100;
const ID_DEVELOPER: i32 = 101;
const ID_PROFILE: i32 = 102;
const ID_FREE: i32 = 103;
const ID_CREDITS: i32 = 104;

const ROWS: &[Row] = &[
    Row::Title(ID_NAME),
    Row::Text(ID_VERSION),
    Row::Text(ID_DEVELOPER),
    Row::Text(ID_PROFILE),
    Row::Text(ID_FREE),
    Row::Text(ID_CREDITS),
    Row::Buttons(&[ID_GITHUB, ID_LICENCE, ID_CLOSE]),
];

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

pub fn handle() -> HWND {
    WINDOW.with(|w| w.get())
}

pub fn register() {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinstance()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

unsafe fn text(h: HWND, id: i32, s: &str) {
    let text = HSTRING::from(s);
    form::control(h, w!("STATIC"), PCWSTR(text.as_ptr()), 0, 0, id);
}

pub fn show() {
    unsafe {
        if !handle().is_invalid() {
            let _ = ShowWindow(handle(), SW_RESTORE);
            let _ = SetForegroundWindow(handle());
            return;
        }
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(format!("About {APP_NAME}")),
            WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            300,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            return;
        };
        WINDOW.with(|w| w.set(h));
        text(h, ID_NAME, APP_NAME);
        text(h, ID_VERSION, &format!("Version {}", env!("CARGO_PKG_VERSION")));
        text(h, ID_DEVELOPER, &format!("Developer: {DEVELOPER}"));
        text(h, ID_PROFILE, &format!("GitHub: {GITHUB}"));
        text(
            h,
            ID_FREE,
            "Free and open-source software, released under the MIT Licence. You may use, copy, modify and share it at no cost. It comes with no warranty.",
        );
        text(
            h,
            ID_CREDITS,
            "Built with Rust, SQLite, windows-rs, webview2-com, ureq, native-tls, serde_json, chrono and fastrand, under their own open-source licences. Not affiliated with or endorsed by Google.",
        );
        let tab = WS_TABSTOP.0 | 0xB;
        form::control(h, w!("BUTTON"), w!("GitHub Profile"), tab, 0, ID_GITHUB);
        form::control(h, w!("BUTTON"), w!("View Licence"), tab, 0, ID_LICENCE);
        form::control(h, w!("BUTTON"), w!("Close"), tab, 0, ID_CLOSE);
        form::attach(h, ROWS);
        form::refresh_fonts(h);
        theme::apply_chrome(h);
        form::fit_form(h, 460);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
    }
}

extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(r) = theme::chrome_message(h, msg, wp, lp) {
            return r;
        }
        if let Some(r) = form::handle(h, msg, wp, lp) {
            return r;
        }
        match msg {
            WM_COMMAND => {
                match (wp.0 & 0xFFFF) as i32 {
                    ID_GITHUB => {
                        ShellExecuteW(None, w!("open"), &HSTRING::from(GITHUB), None, None, SW_SHOWNORMAL);
                    }
                    ID_LICENCE => {
                        MessageBoxW(Some(h), &HSTRING::from(LICENCE), w!("MIT Licence"), MB_OK);
                    }
                    ID_CLOSE => {
                        let _ = DestroyWindow(h);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                WINDOW.with(|w| w.set(HWND::default()));
                let _ = app();
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

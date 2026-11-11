//! Account Settings and the Google sign-in window.
//! The password is entered only into Google's page inside WebView2. The application polls
//! for the `oauth_token` cookie that page issues, then clears the browser session.

use super::form::{self, Row};
use super::{app, backups_dir, data_dir, hinstance, report_error, SyncCmd, WM_APP_SIGNED_IN, APP_NAME};
use keepsticky::auth::{exchange_cookie, COOKIE_NAME, SIGN_IN_URL};
use keepsticky::creds;
use keepsticky::sync::KEY_TWO_WAY;
use std::cell::RefCell;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    GetCookiesCompletedHandler,
};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("KeepStickyNotes_Account");
const SIGN_IN_CLASS: PCWSTR = w!("KeepStickyNotes_SignIn");
const ID_EMAIL: i32 = 1;
const ID_SIGN_IN: i32 = 2;
const ID_SYNC: i32 = 3;
const ID_SIGN_OUT: i32 = 4;
const ID_STATUS: i32 = 5;
const ID_ACCOUNT: i32 = 6;
const ID_TWO_WAY: i32 = 7;
const ID_BACKUPS: i32 = 8;
const ID_EMAIL_LABEL: i32 = 100;
const ID_WARNING: i32 = 101;
const ROWS: &[Row] = &[
    Row::Title(ID_ACCOUNT),
    Row::Text(ID_EMAIL_LABEL),
    Row::Field(ID_EMAIL),
    Row::Buttons(&[ID_SIGN_IN, ID_SYNC, ID_SIGN_OUT]),
    Row::Buttons(&[ID_TWO_WAY, ID_BACKUPS]),
    Row::Text(ID_STATUS),
    Row::Text(ID_WARNING),
];
const DEVICES_URL: &str = "https://myaccount.google.com/device-activity";
const TIMER_COOKIE: usize = 1;

struct SignIn {
    hwnd: HWND,
    email: String,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    done: bool,
}

thread_local! {
    static SIGN_IN: RefCell<Option<SignIn>> = const { RefCell::new(None) };
}

pub fn register() {
    unsafe {
        let icon = LoadIconW(Some(hinstance()), PCWSTR(1 as *const u16)).unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: hinstance(),
            hCursor: cursor,
            hIcon: icon,
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let wc2 = WNDCLASSW { lpfnWndProc: Some(sign_in_proc), lpszClassName: SIGN_IN_CLASS, ..wc };
        RegisterClassW(&wc2);
    }
}

pub fn show() {
    let a = app();
    unsafe {
        if !a.account.get().is_invalid() {
            let _ = ShowWindow(a.account.get(), SW_RESTORE);
            let _ = SetForegroundWindow(a.account.get());
            return;
        }
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(format!("{APP_NAME}: Account Settings")),
            WS_OVERLAPPEDWINDOW,
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
        a.account.set(h);
        let tab = WS_TABSTOP.0;
        let b = tab | 0xB;
        form::control(h, w!("STATIC"), w!(""), 0, 0, ID_ACCOUNT);
        form::control(h, w!("STATIC"), w!("Google account email address:"), 0, 0, ID_EMAIL_LABEL);
        let edit = form::control(h, w!("EDIT"), PCWSTR::null(), tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE.0, ID_EMAIL);
        if let Some((email, _)) = creds::load() {
            let _ = SetWindowTextW(edit, &HSTRING::from(email));
        }
        form::control(h, w!("BUTTON"), w!("Sign In with Google"), b, 0, ID_SIGN_IN);
        form::control(h, w!("BUTTON"), w!("Synchronise Now"), b, 0, ID_SYNC);
        form::control(h, w!("BUTTON"), w!("Sign Out"), b, 0, ID_SIGN_OUT);
        form::control(h, w!("BUTTON"), w!("Enable Two-Way Sync"), b, 0, ID_TWO_WAY);
        form::control(h, w!("BUTTON"), w!("Open Backups Folder"), b, 0, ID_BACKUPS);
        form::control(h, w!("STATIC"), w!(""), 0, 0, ID_STATUS);
        form::control(
            h,
            w!("STATIC"),
            w!("Important: Google offers no public Keep API for personal accounts, so this application uses an unofficial protocol that may stop working or may breach Google's terms. Sign-in stores a powerful account token in Windows Credential Manager. Your password goes only to Google's page."),
            0,
            0,
            ID_WARNING,
        );
        form::attach(h, ROWS);
        form::refresh_fonts(h);
        super::theme::apply_chrome(h);
        update_status();
        form::fit_form(h, 520);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
    }
}

pub fn update_status() {
    let a = app();
    let h = a.account.get();
    if h.is_invalid() {
        return;
    }
    unsafe {
        let _ = SetWindowTextW(GetDlgItem(Some(h), ID_STATUS).unwrap_or_default(), &HSTRING::from(a.status_text()));
        let who = match creds::load() {
            Some((email, _)) => format!("Signed in as {email}"),
            None => "Not signed in".to_string(),
        };
        let _ = SetWindowTextW(GetDlgItem(Some(h), ID_ACCOUNT).unwrap_or_default(), &HSTRING::from(who));
        let label = if two_way_on() { w!("Switch to Read-Only") } else { w!("Enable Two-Way Sync") };
        let _ = SetWindowTextW(GetDlgItem(Some(h), ID_TWO_WAY).unwrap_or_default(), label);
        form::relayout(h);
    }
}

fn two_way_on() -> bool {
    app().store.kv_get(KEY_TWO_WAY).ok().flatten().as_deref() == Some("1")
}

fn ask(text: &str, flags: MESSAGEBOX_STYLE) -> bool {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(APP_NAME), MB_YESNO | flags) == IDYES }
}

fn toggle_two_way() {
    let a = app();
    if two_way_on() {
        let _ = a.store.kv_del(KEY_TWO_WAY);
        a.request_sync(SyncCmd::Now);
        return;
    }
    if creds::load().is_none() {
        report_error("Please sign in first.");
        return;
    }
    let question = "Two-way sync lets this application change your real Google Keep notes.\n\nFirst confirm that your notes look correct in the Note Manager, and that you have exported them with Google Takeout.\n\nEnable two-way sync?";
    if !ask(question, MB_ICONWARNING | MB_DEFBUTTON2) {
        return;
    }
    if !a.backup_now() {
        report_error("Two-way sync was not enabled because a backup could not be written.");
        return;
    }
    let _ = a.store.kv_set(KEY_TWO_WAY, "1");
    a.request_sync(SyncCmd::Now);
}

/// Local notes belong to one Google account and must never upload into another.
fn prepare_for_account(email: &str) -> bool {
    let a = app();
    let bound = a.store.kv_get("bound_email").ok().flatten();
    if let Some(b) = bound.filter(|b| !b.eq_ignore_ascii_case(email)) {
        if !a.backup_now() {
            report_error("Sign-in was stopped because a backup of the other account's notes could not be written.");
            creds::delete();
            return false;
        }
        for n in a.snapshot_notes() {
            n.destroy(false);
        }
        if let Err(e) = a.store.reset_account_data() {
            report_error(&format!("The previous account's notes could not be cleared: {e}"));
            creds::delete();
            return false;
        }
        report_error(&format!("Notes from {b} were saved to the Backups folder and removed from this view."));
    }
    let _ = a.store.kv_set("bound_email", email);
    true
}

unsafe fn read_email(h: HWND) -> String {
    let e = GetDlgItem(Some(h), ID_EMAIL).unwrap_or_default();
    let mut buf = vec![0u16; GetWindowTextLengthW(e) as usize + 1];
    let n = GetWindowTextW(e, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..n]).trim().to_string()
}

pub fn on_signed_in() {
    let a = app();
    let result = a.sign_in_result.lock().unwrap().take();
    match result {
        Some(Ok(email)) => {
            if prepare_for_account(&email) {
                *a.status.lock().unwrap() = format!("Signed in as {email}. Downloading in read-only mode...");
                a.request_sync(SyncCmd::AccountChanged);
                super::hub::refresh();
            }
        }
        Some(Err(e)) => report_error(&format!("Sign-in failed: {e}")),
        None => {}
    }
    update_status();
}

fn start_sign_in(email: String) {
    unsafe {
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            SIGN_IN_CLASS,
            w!("Sign in to Google"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            720,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            return;
        };
        form::fit_client(hwnd, 520, form::scale(hwnd, 720));
        SIGN_IN.with(|s| *s.borrow_mut() = Some(SignIn { hwnd, email, controller: None, webview: None, done: false }));

        let folder = HSTRING::from(data_dir().join("SignInSession").as_os_str());
        let raw = hwnd.0 as isize;
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |code, env| {
            code?;
            let env = env.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
            let controller_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |code, controller| {
                code?;
                let controller = controller.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
                let hwnd = HWND(raw as *mut _);
                let mut c = RECT::default();
                let _ = GetClientRect(hwnd, &mut c);
                controller.SetBounds(c)?;
                controller.SetIsVisible(true)?;
                let view = controller.CoreWebView2()?;
                view.Navigate(&HSTRING::from(SIGN_IN_URL))?;
                SIGN_IN.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        st.controller = Some(controller);
                        st.webview = Some(view);
                    }
                });
                SetTimer(Some(hwnd), TIMER_COOKIE, 1000, None);
                Ok(())
            }));
            env.CreateCoreWebView2Controller(HWND(raw as *mut _), &controller_handler)
        }));
        let created = CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &folder, None, &handler);
        if let Err(e) = created {
            report_error(&format!("Microsoft Edge WebView2 could not be started: {e}"));
            let _ = DestroyWindow(hwnd);
        }
    }
}

unsafe fn poll_cookie() -> windows::core::Result<()> {
    let view = SIGN_IN.with(|s| s.borrow().as_ref().and_then(|st| st.webview.clone()));
    let Some(view) = view else { return Ok(()) };
    let manager = view.cast::<ICoreWebView2_2>()?.CookieManager()?;
    let handler = GetCookiesCompletedHandler::create(Box::new(move |code, list| {
        code?;
        let Some(list) = list else { return Ok(()) };
        let mut count = 0u32;
        list.Count(&mut count)?;
        for i in 0..count {
            let cookie = list.GetValueAtIndex(i)?;
            let mut name = PWSTR::null();
            cookie.Name(&mut name)?;
            if CoTaskMemPWSTR::from(name).to_string() == COOKIE_NAME {
                let mut value = PWSTR::null();
                cookie.Value(&mut value)?;
                finish_sign_in(CoTaskMemPWSTR::from(value).to_string());
                break;
            }
        }
        Ok(())
    }));
    manager.GetCookies(&HSTRING::from("https://accounts.google.com"), &handler)
}

fn finish_sign_in(cookie: String) {
    let details = SIGN_IN.with(|s| {
        let mut b = s.borrow_mut();
        let st = b.as_mut()?;
        if st.done {
            return None;
        }
        st.done = true;
        Some((st.hwnd, st.email.clone(), st.webview.clone()))
    });
    let Some((hwnd, email, view)) = details else { return };
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_COOKIE);
        if let Some(v) = view {
            if let Ok(m) = v.cast::<ICoreWebView2_2>().and_then(|x| x.CookieManager()) {
                let _ = m.DeleteAllCookies(); // Google's session must not outlive the sign-in.
            }
        }
        let _ = DestroyWindow(hwnd);
    }
    let a = app();
    let android_id = a.android_id();
    let slot = a.sign_in_result.clone();
    let main = a.main.get().0 as isize;
    std::thread::spawn(move || {
        let outcome = exchange_cookie(&email, &cookie, &android_id).map_err(|e| e.to_string()).and_then(|master| {
            creds::save(&email, &master).map_err(|e| format!("Windows Credential Manager refused the token: {e}"))?;
            Ok(email.clone())
        });
        *slot.lock().unwrap() = Some(outcome);
        unsafe {
            let _ = PostMessageW(Some(HWND(main as *mut _)), WM_APP_SIGNED_IN, WPARAM(0), LPARAM(0));
        }
    });
}

extern "system" fn sign_in_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SIZE => {
                let controller = SIGN_IN.with(|s| s.borrow().as_ref().and_then(|st| st.controller.clone()));
                if let Some(c) = controller {
                    let mut r = RECT::default();
                    let _ = GetClientRect(h, &mut r);
                    let _ = c.SetBounds(r);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wp.0 == TIMER_COOKIE {
                    let _ = poll_cookie();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(h), TIMER_COOKIE);
                let state = SIGN_IN.with(|s| s.borrow_mut().take());
                if let Some(c) = state.and_then(|st| st.controller) {
                    let _ = c.Close();
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if !app().account.get().is_invalid() {
            if let Some(r) = super::theme::chrome_message(h, msg, wp, lp) {
                return r;
            }
        }
        if let Some(r) = form::handle(h, msg, wp, lp) {
            return r;
        }
        match msg {
            WM_COMMAND => {
                if (wp.0 >> 16) & 0xFFFF == 0 {
                    let a = app();
                    match (wp.0 & 0xFFFF) as i32 {
                        ID_SIGN_IN => {
                            let email = read_email(h);
                            if creds::load().is_some() {
                                report_error("Please sign out before signing in again.");
                            } else if !email.contains('@') {
                                report_error("Please enter your Google account email address first.");
                            } else if SIGN_IN.with(|s| s.borrow().is_none()) {
                                start_sign_in(email);
                            }
                        }
                        ID_SYNC => a.request_sync(SyncCmd::Now),
                        ID_SIGN_OUT => {
                            creds::delete();
                            let _ = a.store.kv_del(KEY_TWO_WAY);
                            a.request_sync(SyncCmd::AccountChanged);
                            update_status();
                            let q = "Signed out. The token has been removed from this computer, but Google may still list this sign-in.\n\nOpen Google's device page to remove it there as well?";
                            if ask(q, MB_ICONINFORMATION) {
                                ShellExecuteW(None, w!("open"), &HSTRING::from(DEVICES_URL), None, None, SW_SHOWNORMAL);
                            }
                        }
                        ID_TWO_WAY => {
                            toggle_two_way();
                            update_status();
                        }
                        ID_BACKUPS => {
                            let dir = backups_dir();
                            let _ = std::fs::create_dir_all(&dir);
                            ShellExecuteW(None, w!("open"), &HSTRING::from(dir.as_os_str()), None, None, SW_SHOWNORMAL);
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                app().account.set(HWND::default());
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

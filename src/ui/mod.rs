//! Application shell: single instance, tray icon, global hotkey, background synchronisation.

mod about;
mod account;
mod form;
mod hub;
mod note;
mod theme;

use keepsticky::auth::{new_android_id, Account};
use keepsticky::creds;
use keepsticky::keep::{KeepError, UnofficialKeep};
use keepsticky::store::{Store, TYPE_NOTE};
use keepsticky::sync::{SyncEngine, SyncError, KEY_BULK_APPROVED, KEY_TWO_WAY};
use note::NoteWin;
use theme::{Theme, THEMES};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APP_NAME: &str = "Keep Sticky Notes";
const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_SYNCED: u32 = WM_APP + 2;
pub const WM_APP_SIGNED_IN: u32 = WM_APP + 3;
const WM_APP_SHOW_HUB: u32 = WM_APP + 4;
const WM_APP_BULK_HELD: u32 = WM_APP + 5;
const BACKUPS_KEPT: usize = 14;
const TRAY_THEME: usize = 20;
const TRAY_TOGGLE: usize = 8;
const TRAY_ABOUT: usize = 9;
const TRAY_TOUCH: usize = 10;
const HOTKEY_NEW: i32 = 1;
const POLL: Duration = Duration::from_secs(120);
const MAX_BACKOFF: Duration = Duration::from_secs(1800);
const MAIN_CLASS: PCWSTR = w!("KeepStickyNotes_Main");

const TRAY_NEW: usize = 1;
const TRAY_SYNC: usize = 2;
const TRAY_SHOW: usize = 3;
const TRAY_HIDE: usize = 4;
const TRAY_HUB: usize = 5;
const TRAY_ACCOUNT: usize = 6;
const TRAY_EXIT: usize = 7;

/// API colour value and British display name.
pub const COLOURS: [(&str, &str); 12] = [
    ("DEFAULT", "Default"),
    ("RED", "Coral"),
    ("ORANGE", "Peach"),
    ("YELLOW", "Sand"),
    ("GREEN", "Mint"),
    ("TEAL", "Sage"),
    ("BLUE", "Fog"),
    ("CERULEAN", "Storm"),
    ("PURPLE", "Dusk"),
    ("PINK", "Blossom"),
    ("BROWN", "Clay"),
    ("GRAY", "Chalk"),
];

pub fn rgb(v: u32) -> COLORREF {
    COLORREF(((v >> 16) & 0xFF) | (v & 0xFF00) | ((v & 0xFF) << 16))
}

pub enum SyncCmd {
    Now,
    Soon,
    AccountChanged,
    Quit,
}

#[derive(Clone, Copy)]
pub struct Fonts {
    pub ui: HFONT,
    pub bold: HFONT,
    pub italic: HFONT,
    pub underline: HFONT,
    pub strike: HFONT,
    pub icon: HFONT,
    pub tiny: HFONT,
}

pub struct App {
    pub store: Store,
    pub main: Cell<HWND>,
    pub notes: RefCell<HashMap<isize, Rc<NoteWin>>>,
    pub hub: Cell<HWND>,
    pub account: Cell<HWND>,
    pub sync_tx: Sender<SyncCmd>,
    pub status: Arc<Mutex<String>>,
    pub sign_in_result: Arc<Mutex<Option<Result<String, String>>>>,
    pub hidden_all: Cell<bool>,
    pub cascade: Cell<i32>,
    pub theme: Cell<Theme>,
    pub chrome_brush: Cell<HBRUSH>,
    pub field_brush: Cell<HBRUSH>,
    pub touch: Cell<bool>,
    fonts: RefCell<HashMap<u32, Fonts>>,
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

pub fn app() -> Rc<App> {
    APP.with(|a| a.get().expect("application initialised").clone())
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default() }
}

impl App {
    pub fn fonts(&self, dpi: u32) -> Fonts {
        if let Some(f) = self.fonts.borrow().get(&dpi) {
            return *f;
        }
        let make = |size: i32, weight: i32, italic: u32, under: u32, strike: u32, face: PCWSTR| unsafe {
            CreateFontW(
                -(size * dpi as i32 / 96),
                0,
                0,
                0,
                weight,
                italic,
                under,
                strike,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                face,
            )
        };
        let f = Fonts {
            ui: make(15, 400, 0, 0, 0, w!("Segoe UI")),
            bold: make(15, 600, 0, 0, 0, w!("Segoe UI")),
            italic: make(15, 400, 1, 0, 0, w!("Segoe UI")),
            underline: make(15, 400, 0, 1, 0, w!("Segoe UI")),
            strike: make(15, 400, 0, 0, 1, w!("Segoe UI")),
            icon: make(13, 400, 0, 0, 0, w!("Segoe MDL2 Assets")),
            tiny: make(8, 700, 0, 0, 0, w!("Segoe MDL2 Assets")),
        };
        self.fonts.borrow_mut().insert(dpi, f);
        f
    }

    /// Font scale: touch mode enlarges text by 15% on top of the monitor's DPI.
    pub fn font_dpi(&self, dpi: u32) -> u32 {
        let dpi = dpi.max(96);
        if self.touch.get() {
            dpi * 115 / 100
        } else {
            dpi
        }
    }

    pub fn fonts_for(&self, hwnd: HWND) -> Fonts {
        self.fonts(self.font_dpi(unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }))
    }

    pub fn set_touch(&self, on: bool) {
        self.touch.set(on);
        let _ = self.store.kv_set("touch_mode", if on { "1" } else { "0" });
        for n in self.snapshot_notes() {
            n.apply_touch();
        }
        hub::apply_touch();
        unsafe {
            form::refresh_fonts(self.account.get());
            form::refresh_fonts(about::handle());
        }
    }

    pub fn request_sync(&self, cmd: SyncCmd) {
        let _ = self.sync_tx.send(cmd);
    }

    pub fn status_text(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    pub fn open_note(&self, id: &str) {
        let existing = self.notes.borrow().values().find(|n| *n.id.borrow() == id).cloned();
        match existing {
            Some(n) => n.bring_forward(),
            None => NoteWin::open(id),
        }
    }

    pub fn new_note(&self, kind: &str) {
        match self.store.create_note(kind) {
            Ok(id) => {
                NoteWin::open(&id);
                self.request_sync(SyncCmd::Soon);
                hub::refresh();
            }
            Err(e) => report_error(&format!("The note could not be created: {e}")),
        }
    }

    pub fn snapshot_notes(&self) -> Vec<Rc<NoteWin>> {
        self.notes.borrow().values().cloned().collect()
    }

    fn set_all_visible(&self, visible: bool) {
        self.hidden_all.set(!visible);
        for n in self.snapshot_notes() {
            n.set_visible(visible);
        }
    }

    pub fn set_theme(&self, t: Theme) {
        self.theme.set(t);
        let _ = self.store.kv_set("theme", t.key());
        unsafe {
            let old = self.chrome_brush.get();
            self.chrome_brush.set(CreateSolidBrush(rgb(theme::chrome_bg(t))));
            if !old.is_invalid() {
                let _ = DeleteObject(old.into());
            }
            let old_field = self.field_brush.get();
            self.field_brush.set(CreateSolidBrush(rgb(theme::field_bg(t))));
            if !old_field.is_invalid() {
                let _ = DeleteObject(old_field.into());
            }
            hub::refresh();
            theme::apply_chrome(self.hub.get());
            theme::apply_chrome(self.account.get());
            theme::apply_chrome(about::handle());
        }
        for n in self.snapshot_notes() {
            n.apply_theme();
        }
    }

    /// Switches between the last light theme and the last dark theme used.
    pub fn toggle_light(&self) {
        let current = self.theme.get();
        if current.is_dark() {
            let _ = self.store.kv_set("last_dark_theme", current.key());
            let last = self.store.kv_get("last_light_theme").ok().flatten();
            let light = Theme::from_key(last.as_deref());
            self.set_theme(if light.is_dark() { Theme::Classic } else { light });
        } else {
            let _ = self.store.kv_set("last_light_theme", current.key());
            let last = self.store.kv_get("last_dark_theme").ok().flatten();
            let dark = Theme::from_key(last.as_deref());
            self.set_theme(if dark.is_dark() { dark } else { Theme::Black });
        }
    }

    pub fn toggle_label(&self) -> PCWSTR {
        if self.theme.get().is_dark() {
            w!("Light Mode")
        } else {
            w!("Dark Mode")
        }
    }

    pub fn backup_now(&self) -> bool {
        self.store.backup_to(&backups_dir(), BACKUPS_KEPT).is_ok()
    }

    pub fn android_id(&self) -> String {
        if let Ok(Some(id)) = self.store.kv_get("android_id") {
            return id;
        }
        let id = new_android_id();
        let _ = self.store.kv_set("android_id", &id);
        id
    }
}

pub fn report_error(msg: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(msg), &HSTRING::from(APP_NAME), MB_OK | MB_ICONWARNING);
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("KEEP_STICKY_DATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("KeepStickyNotes")))
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&base);
    base
}

pub fn backups_dir() -> PathBuf {
    data_dir().join("Backups")
}

fn load_account(store: &Store) -> Option<Account> {
    let (email, master_token) = creds::load()?;
    let android_id = store.kv_get("android_id").ok().flatten()?;
    Some(Account { email, master_token, android_id })
}

fn sync_thread(db: PathBuf, rx: std::sync::mpsc::Receiver<SyncCmd>, hwnd: isize, status: Arc<Mutex<String>>) {
    let store = match Store::open(&db) {
        Ok(s) => s,
        Err(e) => {
            *status.lock().unwrap() = format!("Local database error: {e}");
            return;
        }
    };
    let mut backend: Option<UnofficialKeep> = None;
    let mut next = Instant::now() + Duration::from_secs(2);
    let mut delay = POLL;
    let mut paused = false;
    let notify = |changed: bool| unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_SYNCED, WPARAM(changed as usize), LPARAM(0));
    };
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(SyncCmd::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(SyncCmd::Soon) => {
                if !paused {
                    next = next.min(Instant::now() + Duration::from_secs(5));
                }
                continue;
            }
            Ok(SyncCmd::AccountChanged) => {
                backend = None;
                paused = false;
                delay = POLL;
            }
            Ok(SyncCmd::Now) => {
                paused = false;
                delay = POLL;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if paused {
            next = Instant::now() + Duration::from_secs(86_400);
            continue;
        }
        if backend.is_none() {
            let Some(account) = load_account(&store) else {
                *status.lock().unwrap() = "Not signed in. Notes are saved on this computer only.".into();
                notify(false);
                paused = true;
                continue;
            };
            match UnofficialKeep::new(account) {
                Ok(b) => backend = Some(b),
                Err(e) => {
                    *status.lock().unwrap() = e.to_string();
                    notify(false);
                    next = Instant::now() + delay;
                    continue;
                }
            }
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if store.kv_get("last_backup_day").ok().flatten().as_deref() != Some(today.as_str()) {
            if store.backup_to(&backups_dir(), BACKUPS_KEPT).is_err() {
                *status.lock().unwrap() = "Synchronisation is waiting: the daily backup could not be written.".into();
                notify(false);
                next = Instant::now() + delay;
                continue;
            }
            let _ = store.kv_set("last_backup_day", &today);
        }
        let allow_push = store.kv_get(KEY_TWO_WAY).ok().flatten().as_deref() == Some("1");
        *status.lock().unwrap() = "Synchronising...".into();
        notify(false);
        let result = SyncEngine { store: &store, backend: backend.as_ref().unwrap(), allow_push }.sync_once();
        match result {
            Ok(report) => {
                delay = POLL;
                let clock = chrono::Local::now().format("%H:%M");
                let mut text = format!("Last synchronised at {clock}.");
                if !allow_push {
                    let waiting = store.dirty_count().unwrap_or(0);
                    text = format!("Read-only: last downloaded at {clock}. Nothing is sent to Google. {waiting} local change(s) waiting.");
                }
                if report.conflicts > 0 {
                    text.push_str(&format!(" {} conflict(s) preserved as copies.", report.conflicts));
                }
                *status.lock().unwrap() = text;
                notify(report.changed_notes);
            }
            Err(SyncError::BulkHeld { outgoing, count }) => {
                *status.lock().unwrap() = "Synchronisation is paused until you confirm a large change.".into();
                notify(false);
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_BULK_HELD, WPARAM(outgoing as usize), LPARAM(count as isize));
                }
                paused = true;
                continue;
            }
            Err(e) => {
                let needs_sign_in = matches!(e, SyncError::Keep(KeepError::Auth(_)));
                *status.lock().unwrap() = format!("Synchronisation failed: {e}");
                notify(false);
                if needs_sign_in || matches!(e, SyncError::UpgradeRecommended) {
                    paused = true;
                    backend = None;
                    continue;
                }
                delay = (delay * 2).min(MAX_BACKOFF);
            }
        }
        let jitter = Duration::from_secs(fastrand::u64(0..15));
        next = Instant::now() + delay + jitter;
    }
}

fn add_tray(hwnd: HWND, op: NOTIFY_ICON_MESSAGE) {
    unsafe {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: LoadIconW(Some(hinstance()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        for (i, c) in APP_NAME.encode_utf16().enumerate() {
            nid.szTip[i] = c;
        }
        let _ = Shell_NotifyIconW(op, &nid);
    }
}

/// The Appearance menu shared by the tray icon and the Note Manager.
pub fn show_appearance_menu(owner: HWND, x: i32, y: i32) {
    let a = app();
    unsafe {
        let menu = CreatePopupMenu().unwrap();
        let _ = AppendMenuW(menu, MF_STRING, TRAY_TOGGLE, a.toggle_label());
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        for (i, t) in THEMES.iter().enumerate() {
            let flag = if a.theme.get() == t.0 { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | flag, TRAY_THEME + i, &HSTRING::from(t.2));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let touch_flag = if a.touch.get() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | touch_flag, TRAY_TOUCH, w!("Touch-Friendly Layout"));
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, x, y, None, owner, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            TRAY_TOGGLE => a.toggle_light(),
            TRAY_TOUCH => a.set_touch(!a.touch.get()),
            c if (TRAY_THEME..TRAY_THEME + THEMES.len()).contains(&c) => a.set_theme(THEMES[c - TRAY_THEME].0),
            _ => {}
        }
    }
}

fn tray_menu(hwnd: HWND) {
    let a = app();
    unsafe {
        let menu = CreatePopupMenu().unwrap();
        let _ = AppendMenuW(menu, MF_STRING, TRAY_NEW, w!("New Sticky Note"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_SYNC, w!("Synchronise Now"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, TRAY_SHOW, w!("Show All Notes"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_HIDE, w!("Hide All Notes"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_HUB, w!("Note Manager"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, TRAY_TOGGLE, a.toggle_label());
        let appearance = CreatePopupMenu().unwrap();
        for (i, t) in THEMES.iter().enumerate() {
            let flag = if a.theme.get() == t.0 { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(appearance, MF_STRING | flag, TRAY_THEME + i, &HSTRING::from(t.2));
        }
        let _ = AppendMenuW(appearance, MF_SEPARATOR, 0, None);
        let touch_flag = if a.touch.get() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(appearance, MF_STRING | touch_flag, TRAY_TOUCH, w!("Touch-Friendly Layout"));
        let _ = AppendMenuW(menu, MF_POPUP, appearance.0 as usize, w!("Appearance"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_ACCOUNT, w!("Account Settings"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_ABOUT, w!("About"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_EXIT, w!("Exit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            TRAY_NEW => a.new_note(TYPE_NOTE),
            TRAY_SYNC => a.request_sync(SyncCmd::Now),
            TRAY_SHOW => a.set_all_visible(true),
            TRAY_HIDE => a.set_all_visible(false),
            TRAY_HUB => hub::show(),
            TRAY_ACCOUNT => account::show(),
            TRAY_TOGGLE => a.toggle_light(),
            TRAY_ABOUT => about::show(),
            TRAY_TOUCH => a.set_touch(!a.touch.get()),
            c if (TRAY_THEME..TRAY_THEME + THEMES.len()).contains(&c) => a.set_theme(THEMES[c - TRAY_THEME].0),
            TRAY_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_APP_TRAY => {
                match (lp.0 as u32) & 0xFFFF {
                    WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(hwnd),
                    WM_LBUTTONDBLCLK => hub::show(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                if wp.0 as i32 == HOTKEY_NEW {
                    app().new_note(TYPE_NOTE);
                }
                LRESULT(0)
            }
            WM_APP_SHOW_HUB => {
                hub::show();
                LRESULT(0)
            }
            WM_APP_SYNCED => {
                if wp.0 != 0 {
                    for n in app().snapshot_notes() {
                        n.reload_if_idle();
                    }
                    hub::refresh();
                }
                hub::update_status();
                account::update_status();
                LRESULT(0)
            }
            WM_APP_BULK_HELD => {
                let what = if wp.0 != 0 {
                    format!("{APP_NAME} is about to send {} changes to Google Keep, which is unusually many.\n\nSend them now?", lp.0)
                } else {
                    format!("Google Keep reports that {} notes were permanently removed, which is unusually many.\n\nRemove them from this computer as well?", lp.0)
                };
                let answer = MessageBoxW(None, &HSTRING::from(what), &HSTRING::from(APP_NAME), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2);
                let a = app();
                if answer == IDYES && a.backup_now() {
                    let _ = a.store.kv_set(KEY_BULK_APPROVED, "1");
                    a.request_sync(SyncCmd::Now);
                }
                LRESULT(0)
            }
            WM_APP_SIGNED_IN => {
                account::on_signed_in();
                LRESULT(0)
            }
            WM_ENDSESSION | WM_DESTROY => {
                let a = app();
                for n in a.snapshot_notes() {
                    n.flush();
                }
                if msg == WM_DESTROY {
                    a.request_sync(SyncCmd::Quit);
                    let nid = NOTIFYICONDATAW {
                        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: hwnd,
                        uID: 1,
                        ..Default::default()
                    };
                    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => {
                if msg == RegisterWindowMessageW(w!("TaskbarCreated")) {
                    add_tray(hwnd, NIM_ADD);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

pub fn run() {
    unsafe {
        let _instance = CreateMutexW(None, true, w!("Local\\KeepStickyNotes_SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(MAIN_CLASS, None) {
                let _ = PostMessageW(Some(existing), WM_APP_SHOW_HUB, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let db_path = data_dir().join("notes.db");
        let store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                report_error(&format!("The note database could not be opened: {e}"));
                return;
            }
        };

        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_proc),
            hInstance: hinstance(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        note::register();
        hub::register();
        account::register();
        about::register();

        let main = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MAIN_CLASS,
            &HSTRING::from(APP_NAME),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .expect("main window");

        let (tx, rx) = channel();
        let status = Arc::new(Mutex::new(String::from("Starting...")));
        let a = Rc::new(App {
            store,
            main: Cell::new(main),
            notes: RefCell::new(HashMap::new()),
            hub: Cell::new(HWND::default()),
            account: Cell::new(HWND::default()),
            sync_tx: tx,
            status: status.clone(),
            sign_in_result: Arc::new(Mutex::new(None)),
            hidden_all: Cell::new(false),
            cascade: Cell::new(0),
            theme: Cell::new(Theme::Black),
            chrome_brush: Cell::new(HBRUSH::default()),
            field_brush: Cell::new(HBRUSH::default()),
            touch: Cell::new(false),
            fonts: RefCell::new(HashMap::new()),
        });
        APP.with(|c| {
            let _ = c.set(a.clone());
        });
        a.android_id();
        a.theme.set(Theme::from_key(a.store.kv_get("theme").ok().flatten().as_deref()));
        a.chrome_brush.set(CreateSolidBrush(rgb(theme::chrome_bg(a.theme.get()))));
        a.field_brush.set(CreateSolidBrush(rgb(theme::field_bg(a.theme.get()))));
        let touch = match a.store.kv_get("touch_mode").ok().flatten().as_deref() {
            Some(v) => v == "1",
            None => GetSystemMetrics(SM_MAXIMUMTOUCHES) > 0, // On by default on touch screens.
        };
        a.touch.set(touch);

        add_tray(main, NIM_ADD);
        if RegisterHotKey(Some(main), HOTKEY_NEW, MOD_WIN | MOD_ALT | MOD_NOREPEAT, 'N' as u32).is_err() {
            *a.status.lock().unwrap() = "The shortcut Win+Alt+N is in use by another application.".into();
        }

        let main_raw = main.0 as isize;
        std::thread::spawn(move || sync_thread(db_path, rx, main_raw, status));

        let open = a.store.open_note_ids().unwrap_or_default();
        if open.is_empty() {
            hub::show();
        }
        for id in open {
            NoteWin::open(&id);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let dlg = [a.hub.get(), a.account.get(), about::handle()]
                .into_iter()
                .find(|h| !h.is_invalid() && IsChild(*h, msg.hwnd).as_bool() || *h == msg.hwnd && !h.is_invalid());
            if let Some(d) = dlg {
                if IsDialogMessageW(d, &msg).as_bool() {
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        CoUninitialize();
    }
}

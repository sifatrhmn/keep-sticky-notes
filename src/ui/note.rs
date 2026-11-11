//! A floating sticky note: frameless layered window with custom-drawn title bar and checklist.

use super::theme::{self, palette, Palette, THEMES};
use super::{app, hinstance, hub, report_error, rgb, SyncCmd, COLOURS};
use keepsticky::format::{self, TextEdit};
use keepsticky::store::{WindowState, TYPE_NOTE};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows::core::{w, IUnknown, Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Controls::RichEdit::{tomResume, tomSuspend, ITextDocument};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("KeepStickyNotes_Note");
const ID_TITLE: isize = 10;
const ID_BODY: isize = 11;
const ID_ITEM: isize = 12;
const TIMER_SAVE: usize = 1;
const SAVE_DELAY: u32 = 500;
const BASE_W: i32 = 300;
const BASE_H: i32 = 300;

const BTN_ADD: usize = 0;
const BTN_LIST: usize = 1;
const BTN_TOP: usize = 2;
const BTN_ROLL: usize = 3;
const BTN_MENU: usize = 4;
const BTN_CLOSE: usize = 5;
const BTN_COUNT: usize = 6;

const CMD_COLOUR: usize = 100;
const CMD_OPACITY: usize = 200;
const CMD_LABEL: usize = 300;
const CMD_PIN: usize = 500;
const CMD_ARCHIVE: usize = 501;
const CMD_DELETE: usize = 502;
const CMD_THEME: usize = 600;

const FMT_COUNT: usize = 6;
const FMT_HOVER_BASE: i32 = 100;
const FMT_MARKERS: [&str; 4] = ["**", "*", "__", "~~"];

const EM_EXGETSEL: u32 = 0x434;
const EM_EXLIMITTEXT: u32 = 0x435;
const EM_EXSETSEL: u32 = 0x437;
const EM_GETOLEINTERFACE: u32 = 0x43C;
const EM_SETBKGNDCOLOR: u32 = 0x443;
const EM_SETCHARFORMAT: u32 = 0x444;
const EM_SETEVENTMASK: u32 = 0x445;
const ENM_CHANGE: isize = 1;
const SCF_DEFAULT: usize = 0;
const SCF_SELECTION: usize = 1;
const SCF_ALL: usize = 4;
const CFE_BOLD: u32 = 1;
const CFE_ITALIC: u32 = 2;
const CFE_UNDERLINE: u32 = 4;
const CFE_STRIKEOUT: u32 = 8;
const CF_MASK: u32 = CFE_BOLD | CFE_ITALIC | CFE_UNDERLINE | CFE_STRIKEOUT | 0x4000_0000 | 0x8000_0000 | 0x2000_0000;
const MAX_STYLED_UNITS: usize = 60_000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CharRange {
    min: i32,
    max: i32,
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct CharFormat2 {
    cb_size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    colour: u32,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
    weight: u16,
    spacing: i16,
    back: u32,
    lcid: u32,
    reserved: u32,
    style: i16,
    kerning: u16,
    underline_type: u8,
    animation: u8,
    author: u8,
    underline_colour: u8,
}

fn char_format(effects: u32, height: i32, colour: u32) -> CharFormat2 {
    let mut face = [0u16; 32];
    for (i, c) in "Segoe UI".encode_utf16().enumerate() {
        face[i] = c;
    }
    CharFormat2 {
        cb_size: std::mem::size_of::<CharFormat2>() as u32,
        mask: CF_MASK,
        effects,
        height,
        offset: 0,
        colour,
        charset: 0,
        pitch: 0,
        face,
        weight: 0,
        spacing: 0,
        back: 0,
        lcid: 0,
        reserved: 0,
        style: 0,
        kerning: 0,
        underline_type: 0,
        animation: 0,
        author: 0,
        underline_colour: 0,
    }
}

unsafe fn set_char_format(h: HWND, scope: usize, f: &CharFormat2) {
    SendMessageW(h, EM_SETCHARFORMAT, Some(WPARAM(scope)), Some(LPARAM(f as *const _ as isize)));
}

unsafe fn get_range(h: HWND) -> CharRange {
    let mut r = CharRange::default();
    SendMessageW(h, EM_EXGETSEL, None, Some(LPARAM(&mut r as *mut _ as isize)));
    r
}

unsafe fn set_range(h: HWND, min: i32, max: i32) {
    let r = CharRange { min, max };
    SendMessageW(h, EM_EXSETSEL, None, Some(LPARAM(&r as *const _ as isize)));
}
const OPACITIES: [i32; 7] = [100, 90, 80, 70, 60, 50, 40];
const MIN_OPACITY: i32 = 30;

#[derive(Clone)]
struct Row {
    id: String,
    text: String,
    checked: bool,
    y: i32,
    h: i32,
}

pub struct NoteWin {
    pub hwnd: Cell<HWND>,
    pub id: RefCell<String>,
    title: Cell<HWND>,
    body: Cell<HWND>,
    item_edit: Cell<HWND>,
    is_list: Cell<bool>,
    api: RefCell<String>,
    pal: Cell<Palette>,
    restyling: Cell<bool>,
    swallow_return: Cell<bool>,
    doc: RefCell<Option<ITextDocument>>,
    brush: Cell<HBRUSH>,
    opacity: Cell<i32>,
    topmost: Cell<bool>,
    rolled: Cell<bool>,
    expanded_h: Cell<i32>,
    rows: RefCell<Vec<Row>>,
    add_row_y: Cell<i32>,
    active: RefCell<Option<String>>,
    scroll: Cell<i32>,
    content_h: Cell<i32>,
    hover: Cell<i32>,
    tracking: Cell<bool>,
    last_title: RefCell<String>,
    last_body: RefCell<String>,
    pending: Cell<bool>,
    loading: Cell<bool>,
    pan_y: Cell<i32>,
}

fn lp_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got]).replace("\r\n", "\n").replace('\r', "\n")
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s.replace('\n', "\r\n")));
    }
}

fn find(hwnd: HWND) -> Option<Rc<NoteWin>> {
    app().notes.borrow().get(&(hwnd.0 as isize)).cloned()
}

pub fn register() {
    unsafe {
        let _ = LoadLibraryW(w!("msftedit.dll"));
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
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

fn child_edit(parent: HWND, id: isize, style: u32) -> HWND {
    child_control(parent, w!("EDIT"), id, style)
}

fn child_control(parent: HWND, class: PCWSTR, id: isize, style: u32) -> HWND {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            None,
            WS_CHILD | WINDOW_STYLE(style),
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as *mut _)),
            Some(hinstance()),
            None,
        )
        .unwrap_or_default()
    }
}

fn body_twips() -> i32 {
    if app().touch.get() {
        250
    } else {
        220
    }
}

fn heading_twips() -> i32 {
    if app().touch.get() {
        340
    } else {
        300
    }
}

fn is_mac() -> bool {
    app().theme.get() == theme::Theme::Mac
}

impl NoteWin {
    fn s(&self, v: i32) -> i32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd.get()) } as i32;
        v * dpi.max(96) / 96
    }

    /// A size that grows in touch mode.
    fn t(&self, normal: i32, touch: i32) -> i32 {
        self.s(if app().touch.get() { touch } else { normal })
    }

    fn title_h(&self) -> i32 {
        self.t(32, 46)
    }

    fn content_top(&self) -> i32 {
        self.title_h() + self.t(32, 42)
    }

    pub fn apply_touch(&self) {
        self.apply_fonts();
        unsafe {
            let body = self.body.get();
            set_char_format(body, SCF_DEFAULT, &char_format(0, body_twips(), rgb(self.pal.get().text).0));
        }
        self.restyle();
        if self.rolled.get() {
            unsafe {
                let mut r = RECT::default();
                let _ = GetWindowRect(self.hwnd.get(), &mut r);
                let _ = SetWindowPos(self.hwnd.get(), None, 0, 0, r.right - r.left, self.title_h(), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.layout();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, true);
        }
    }

    pub fn open(id: &str) {
        let a = app();
        let Ok(Some(doc)) = a.store.doc(id) else { return };
        let saved = a.store.window_state(id).ok().flatten();
        let is_new = saved.is_none();
        let mut state = saved.unwrap_or(WindowState { x: 0, y: 0, w: 0, h: 0, opacity: 100, topmost: false, rolled: false, open: true });
        unsafe {
            let base_dpi = GetDpiForWindow(GetDesktopWindow()).max(96) as i32;
            if is_new {
                let mut wa = RECT::default();
                let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                let step = 36 * base_dpi / 96 * (a.cascade.get() % 8);
                a.cascade.set(a.cascade.get() + 1);
                state.w = BASE_W * base_dpi / 96;
                state.h = BASE_H * base_dpi / 96;
                state.x = wa.left + 80 + step;
                state.y = wa.top + 80 + step;
            } else {
                let rect = RECT { left: state.x, top: state.y, right: state.x + state.w, bottom: state.y + 40 };
                if MonitorFromRect(&rect, MONITOR_DEFAULTTONULL).is_invalid() {
                    state.x = 80;
                    state.y = 80; // The saved monitor is no longer attached.
                }
            }
            let h = if state.rolled { (if a.touch.get() { 46 } else { 32 }) * base_dpi / 96 } else { state.h };
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                CLASS,
                &HSTRING::from(if doc.top.title.is_empty() { "Note" } else { doc.top.title.as_str() }),
                WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
                state.x,
                state.y,
                state.w,
                h,
                None,
                None,
                Some(hinstance()),
                None,
            );
            let Ok(hwnd) = hwnd else {
                report_error("The note window could not be created.");
                return;
            };
            let multiline = (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
            let title = child_edit(hwnd, ID_TITLE, WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32);
            let body = child_control(hwnd, w!("RICHEDIT50W"), ID_BODY, WS_TABSTOP.0 | WS_VSCROLL.0 | multiline);
            let item_edit = child_edit(hwnd, ID_ITEM, multiline);
            SendMessageW(title, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(w!("Title").as_ptr() as isize)));
            SendMessageW(body, EM_EXLIMITTEXT, None, Some(LPARAM(0x7FFF_FFFE)));
            SendMessageW(body, EM_SETEVENTMASK, None, Some(LPARAM(ENM_CHANGE)));
            let mut ole: *mut core::ffi::c_void = std::ptr::null_mut();
            SendMessageW(body, EM_GETOLEINTERFACE, None, Some(LPARAM(&mut ole as *mut _ as isize)));
            let text_doc = if ole.is_null() { None } else { IUnknown::from_raw(ole).cast::<ITextDocument>().ok() };
            let _ = SetWindowSubclass(body, Some(body_subclass), 2, hwnd.0 as usize);
            let _ = SetWindowSubclass(item_edit, Some(item_subclass), 1, hwnd.0 as usize);

            let win = Rc::new(NoteWin {
                hwnd: Cell::new(hwnd),
                id: RefCell::new(id.to_string()),
                title: Cell::new(title),
                body: Cell::new(body),
                item_edit: Cell::new(item_edit),
                is_list: Cell::new(doc.is_list()),
                api: RefCell::new(String::new()),
                pal: Cell::new(palette(a.theme.get(), "DEFAULT")),
                restyling: Cell::new(false),
                swallow_return: Cell::new(false),
                doc: RefCell::new(text_doc),
                brush: Cell::new(HBRUSH::default()),
                opacity: Cell::new(state.opacity.clamp(MIN_OPACITY, 100)),
                topmost: Cell::new(state.topmost),
                rolled: Cell::new(state.rolled),
                expanded_h: Cell::new(state.h),
                rows: RefCell::new(Vec::new()),
                add_row_y: Cell::new(0),
                active: RefCell::new(None),
                scroll: Cell::new(0),
                content_h: Cell::new(0),
                hover: Cell::new(-1),
                tracking: Cell::new(false),
                last_title: RefCell::new(String::new()),
                last_body: RefCell::new(String::new()),
                pending: Cell::new(false),
                loading: Cell::new(false),
                pan_y: Cell::new(0),
            });
            a.notes.borrow_mut().insert(hwnd.0 as isize, win.clone());
            win.apply_fonts();
            let round: i32 = DWMWCP_ROUND.0;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
            win.apply_opacity();
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
            win.load_doc();
            win.layout();
            if win.topmost.get() {
                win.apply_topmost();
            }
            win.save_state(true);
            if !a.hidden_all.get() {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            if is_new && !win.is_list.get() {
                let _ = SetFocus(Some(body));
            }
        }
    }

    fn apply_fonts(&self) {
        let f = app().fonts_for(self.hwnd.get());
        unsafe {
            SendMessageW(self.title.get(), WM_SETFONT, Some(WPARAM(f.bold.0 as usize)), Some(LPARAM(1)));
                        SendMessageW(self.item_edit.get(), WM_SETFONT, Some(WPARAM(f.ui.0 as usize)), Some(LPARAM(1)));
        }
    }

    fn apply_opacity(&self) {
        let alpha = (self.opacity.get() * 255 / 100) as u8;
        unsafe {
            let _ = SetLayeredWindowAttributes(self.hwnd.get(), COLORREF(0), alpha, LWA_ALPHA);
        }
    }

    fn apply_topmost(&self) {
        let after = if self.topmost.get() { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(self.hwnd.get(), Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn set_colour_value(&self, api: &str) {
        if *self.api.borrow() == api && !self.brush.get().is_invalid() {
            return;
        }
        *self.api.borrow_mut() = api.to_string();
        self.refresh_palette();
    }

    fn refresh_palette(&self) {
        let t = app().theme.get();
        let p = palette(t, &self.api.borrow());
        self.pal.set(p);
        unsafe {
            let old = self.brush.get();
            self.brush.set(CreateSolidBrush(rgb(p.bg)));
            if !old.is_invalid() {
                let _ = DeleteObject(old.into());
            }
            let hwnd = self.hwnd.get();
            theme::apply_note_frame(hwnd, t, p);
            let body = self.body.get();
            SendMessageW(body, EM_SETBKGNDCOLOR, Some(WPARAM(0)), Some(LPARAM(rgb(p.bg).0 as isize)));
            set_char_format(body, SCF_DEFAULT, &char_format(0, body_twips(), rgb(p.text).0));
            for c in [self.title.get(), body, self.item_edit.get()] {
                theme::apply_control_theme(c, t.is_dark(), false);
            }
            self.restyle();
            let _ = InvalidateRect(Some(hwnd), None, true);
            let _ = InvalidateRect(Some(self.title.get()), None, true);
            let _ = InvalidateRect(Some(self.item_edit.get()), None, true);
        }
    }

    pub fn apply_theme(&self) {
        self.refresh_palette();
    }

    /// Re-applies formatting from the markers. Excluded from undo so Ctrl+Z only undoes typing.
    fn restyle(&self) {
        if self.is_list.get() {
            return;
        }
        let body = self.body.get();
        let text: Vec<u16> = get_text(body).encode_utf16().collect();
        let p = self.pal.get();
        let (normal, dim) = (rgb(p.text).0, rgb(p.muted).0);
        let doc = self.doc.borrow().clone();
        self.restyling.set(true);
        unsafe {
            if let Some(d) = &doc {
                let _ = d.Undo(tomSuspend.0);
            }
            SendMessageW(body, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(body, EM_SETEVENTMASK, None, Some(LPARAM(0)));
            let sel = get_range(body);
            let first = SendMessageW(body, EM_GETFIRSTVISIBLELINE, None, None).0;
            set_char_format(body, SCF_ALL, &char_format(0, body_twips(), normal));
            if text.len() <= MAX_STYLED_UNITS {
                for run in format::style_runs(&text) {
                    let mut fx = 0;
                    if run.flags & (format::BOLD | format::HEADING) != 0 {
                        fx |= CFE_BOLD;
                    }
                    if run.flags & format::ITALIC != 0 {
                        fx |= CFE_ITALIC;
                    }
                    if run.flags & format::UNDERLINE != 0 {
                        fx |= CFE_UNDERLINE;
                    }
                    if run.flags & format::STRIKE != 0 {
                        fx |= CFE_STRIKEOUT;
                    }
                    let size = if run.flags & format::HEADING != 0 { heading_twips() } else { body_twips() };
                    let colour = if run.flags & format::MARKER != 0 { dim } else { normal };
                    set_range(body, run.start as i32, run.end as i32);
                    set_char_format(body, SCF_SELECTION, &char_format(fx, size, colour));
                }
            }
            set_range(body, sel.min, sel.max);
            let now = SendMessageW(body, EM_GETFIRSTVISIBLELINE, None, None).0;
            if now != first {
                SendMessageW(body, EM_LINESCROLL, None, Some(LPARAM(first - now)));
            }
            SendMessageW(body, EM_SETEVENTMASK, None, Some(LPARAM(ENM_CHANGE)));
            SendMessageW(body, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = InvalidateRect(Some(body), None, true);
            if let Some(d) = &doc {
                let _ = d.Undo(tomResume.0);
            }
        }
        self.restyling.set(false);
    }

    fn perform_edit(&self, e: TextEdit) {
        let body = self.body.get();
        unsafe {
            set_range(body, e.start as i32, e.end as i32);
            let text = HSTRING::from(e.text.replace('\n', "\r"));
            SendMessageW(body, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
            set_range(body, e.sel_start as i32, e.sel_end as i32);
            let _ = SetFocus(Some(body));
        }
    }

    fn apply_format(&self, i: usize) {
        let body = self.body.get();
        let text: Vec<u16> = get_text(body).encode_utf16().collect();
        let sel = unsafe { get_range(body) };
        let (s, e) = (sel.min.max(0) as usize, sel.max.max(0) as usize);
        let edit = match i {
            0..=3 => format::toggle_wrap(&text, s, e, FMT_MARKERS[i]),
            4 => format::toggle_line_prefix(&text, s, e, format::HEADING_PREFIX),
            _ => format::toggle_line_prefix(&text, s, e, format::BULLET_PREFIX),
        };
        self.perform_edit(edit);
    }

    fn fmt_rect(&self, i: usize) -> RECT {
        let mut c = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd.get(), &mut c);
        }
        let size = self.t(30, 44);
        let x = self.s(8) + i as i32 * (size + self.s(2));
        let y = c.bottom - self.t(38, 52);
        RECT { left: x, top: y, right: x + size, bottom: y + size }
    }

    fn fmt_at(&self, x: i32, y: i32) -> Option<usize> {
        if self.is_list.get() || self.rolled.get() {
            return None;
        }
        (0..FMT_COUNT).find(|&i| {
            let r = self.fmt_rect(i);
            x >= r.left && x < r.right && y >= r.top && y < r.bottom
        })
    }

    pub fn bring_forward(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd.get(), SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd.get());
        }
    }

    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd.get(), if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    // ----- data -----

    fn load_doc(&self) {
        let id = self.id.borrow().clone();
        let Ok(Some(doc)) = app().store.doc(&id) else { return };
        self.loading.set(true);
        self.is_list.set(doc.is_list());
        self.set_colour_value(&doc.top.colour);
        if get_text(self.title.get()) != doc.top.title {
            set_text(self.title.get(), &doc.top.title);
        }
        *self.last_title.borrow_mut() = doc.top.title.clone();
        unsafe {
            let _ = SetWindowTextW(self.hwnd.get(), &HSTRING::from(if doc.top.title.is_empty() { "Note" } else { doc.top.title.as_str() }));
        }
        if doc.is_list() {
            let (mut open, mut done): (Vec<_>, Vec<_>) = doc.items.iter().cloned().partition(|i| !i.checked);
            open.append(&mut done);
            let mut rows = self.rows.borrow_mut();
            *rows = open.into_iter().map(|n| Row { id: n.id, text: n.text, checked: n.checked, y: 0, h: 0 }).collect();
        } else {
            let body = doc.body();
            if get_text(self.body.get()) != body {
                let (mut start, mut end) = (0u32, 0u32);
                unsafe {
                    SendMessageW(self.body.get(), EM_GETSEL, Some(WPARAM(&mut start as *mut _ as usize)), Some(LPARAM(&mut end as *mut _ as isize)));
                }
                set_text(self.body.get(), &body);
                unsafe {
                    SendMessageW(self.body.get(), EM_SETSEL, Some(WPARAM(start as usize)), Some(LPARAM(end as isize)));
                }
            }
            *self.last_body.borrow_mut() = body;
            self.restyle();
        }
        self.loading.set(false);
        self.layout_rows();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
    }

    /// Refreshes after a sync, unless the person has unsaved typing in this window.
    pub fn reload_if_idle(&self) {
        if self.pending.get() {
            return;
        }
        let id = self.id.borrow().clone();
        match app().store.doc(&id) {
            Ok(Some(d)) if !d.top.is_trashed() && !d.top.is_deleted() => {
                if self.active.borrow().is_none() {
                    self.load_doc();
                }
            }
            _ => self.destroy(false),
        }
    }

    fn schedule_save(&self) {
        if self.loading.get() {
            return;
        }
        self.pending.set(true);
        unsafe {
            SetTimer(Some(self.hwnd.get()), TIMER_SAVE, SAVE_DELAY, None);
        }
    }

    pub fn flush(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd.get()), TIMER_SAVE);
        }
        if !self.pending.replace(false) {
            return;
        }
        let a = app();
        let id = self.id.borrow().clone();
        let title = get_text(self.title.get());
        if *self.last_title.borrow() != title {
            if let Err(e) = a.store.set_title(&id, &title) {
                report_error(&format!("The note could not be saved: {e}"));
                return;
            }
            unsafe {
                let _ = SetWindowTextW(self.hwnd.get(), &HSTRING::from(if title.is_empty() { "Note" } else { title.as_str() }));
            }
            *self.last_title.borrow_mut() = title;
        }
        if self.is_list.get() {
            self.commit_active_text();
        } else {
            let body = get_text(self.body.get());
            if *self.last_body.borrow() != body {
                if let Err(e) = a.store.set_body(&id, &body) {
                    report_error(&format!("The note could not be saved: {e}"));
                    return;
                }
                *self.last_body.borrow_mut() = body;
            }
        }
        a.request_sync(SyncCmd::Soon);
        hub::refresh();
    }

    fn save_state(&self, open: bool) {
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd.get(), &mut r);
            let rolled = self.rolled.get();
            if !rolled {
                self.expanded_h.set(r.bottom - r.top);
            }
            let st = WindowState {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: self.expanded_h.get(),
                opacity: self.opacity.get(),
                topmost: self.topmost.get(),
                rolled,
                open,
            };
            let _ = app().store.save_window_state(&self.id.borrow(), &st);
        }
    }

    pub fn destroy(&self, keep_open_flag: bool) {
        self.flush();
        self.save_state(keep_open_flag);
        unsafe {
            let _ = DestroyWindow(self.hwnd.get());
        }
    }

    // ----- layout -----

    fn button_rect(&self, i: usize) -> RECT {
        let mut c = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd.get(), &mut c);
        }
        let b = self.title_h();
        if is_mac() {
            // Traffic lights on the left, other controls on the right, as on macOS.
            let d = self.t(22, 38);
            let x0 = self.t(8, 10);
            let (x, w) = match i {
                BTN_CLOSE => (x0, d),
                BTN_ROLL => (x0 + d, d),
                BTN_TOP => (x0 + 2 * d, d),
                BTN_ADD => (c.right - 3 * b, b),
                BTN_LIST => (c.right - 2 * b, b),
                _ => (c.right - b, b),
            };
            return RECT { left: x, top: 0, right: x + w, bottom: b };
        }
        let x = if i == BTN_ADD { 0 } else { c.right - b * (BTN_COUNT - i) as i32 };
        RECT { left: x, top: 0, right: x + b, bottom: b }
    }

    fn button_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..BTN_COUNT).find(|&i| {
            let r = self.button_rect(i);
            x >= r.left && x < r.right && y >= r.top && y < r.bottom
        })
    }

    fn layout(&self) {
        let mut c = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd.get(), &mut c);
            let m = self.s(10);
            let rolled = self.rolled.get();
            let list = self.is_list.get();
            let _ = MoveWindow(self.title.get(), m, self.title_h() + self.s(4), c.right - 2 * m, self.t(24, 32), true);
            let top = self.content_top();
            let _ = MoveWindow(self.body.get(), m, top, c.right - 2 * m, (c.bottom - top - self.t(44, 58)).max(1), true);
            let _ = ShowWindow(self.title.get(), if rolled { SW_HIDE } else { SW_SHOW });
            let _ = ShowWindow(self.body.get(), if rolled || list { SW_HIDE } else { SW_SHOW });
        }
        self.layout_rows();
    }

    fn layout_rows(&self) {
        if !self.is_list.get() {
            unsafe {
                let _ = ShowWindow(self.item_edit.get(), SW_HIDE);
            }
            return;
        }
        let mut c = RECT::default();
        unsafe {
            let hwnd = self.hwnd.get();
            let _ = GetClientRect(hwnd, &mut c);
            let f = app().fonts_for(hwnd);
            let hdc = GetDC(Some(hwnd));
            let old = SelectObject(hdc, f.ui.into());
            let text_w = (c.right - self.text_x() - self.s(10)).max(20);
            let min_h = self.t(26, 42);
            let pad = self.s(6);
            let mut y = 0;
            let mut added = false;
            let mut rows = self.rows.borrow_mut();
            for r in rows.iter_mut() {
                if r.checked && !added {
                    self.add_row_y.set(y);
                    y += min_h + self.s(8);
                    added = true;
                }
                let mut rc = RECT { left: 0, top: 0, right: text_w, bottom: 0 };
                let mut w: Vec<u16> = if r.text.is_empty() { vec![b' ' as u16] } else { r.text.encode_utf16().collect() };
                DrawTextW(hdc, &mut w, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX | DT_EDITCONTROL);
                r.y = y;
                r.h = (rc.bottom + pad).max(min_h);
                y += r.h;
            }
            if !added {
                self.add_row_y.set(y);
                y += min_h;
            }
            self.content_h.set(y + self.s(8));
            drop(rows);
            SelectObject(hdc, old);
            let _ = ReleaseDC(Some(hwnd), hdc);
        }
        self.clamp_scroll();
        self.position_item_edit();
    }

    fn view_h(&self) -> i32 {
        let mut c = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd.get(), &mut c);
        }
        (c.bottom - self.content_top()).max(0)
    }

    fn clamp_scroll(&self) {
        let max = (self.content_h.get() - self.view_h()).max(0);
        self.scroll.set(self.scroll.get().clamp(0, max));
    }

    fn text_x(&self) -> i32 {
        self.t(38, 52)
    }

    fn position_item_edit(&self) {
        let active = self.active.borrow().clone();
        let edit = self.item_edit.get();
        let row = active.and_then(|id| self.rows.borrow().iter().find(|r| r.id == id).cloned());
        unsafe {
            let Some(r) = row.filter(|_| self.is_list.get() && !self.rolled.get()) else {
                let _ = ShowWindow(edit, SW_HIDE);
                return;
            };
            let mut c = RECT::default();
            let _ = GetClientRect(self.hwnd.get(), &mut c);
            let y = self.content_top() + r.y - self.scroll.get();
            let x = self.text_x();
            let _ = MoveWindow(edit, x, y + self.s(3), c.right - x - self.s(10), r.h - self.s(3), true);
            let visible = y >= self.content_top() - r.h / 2 && y < c.bottom;
            let _ = ShowWindow(edit, if visible { SW_SHOW } else { SW_HIDE });
        }
    }

    // ----- checklist editing -----

    fn commit_active_text(&self) {
        let Some(id) = self.active.borrow().clone() else { return };
        let text = get_text(self.item_edit.get());
        if let Err(e) = app().store.set_item_text(&id, &text) {
            report_error(&format!("The item could not be saved: {e}"));
        }
    }

    fn activate(&self, id: Option<String>) {
        self.commit_active_text();
        let text = id.as_ref().and_then(|i| self.rows.borrow().iter().find(|r| &r.id == i).map(|r| r.text.clone()));
        *self.active.borrow_mut() = id;
        self.loading.set(true);
        set_text(self.item_edit.get(), text.as_deref().unwrap_or(""));
        self.loading.set(false);
        self.position_item_edit();
        unsafe {
            if text.is_some() {
                let e = self.item_edit.get();
                let len = GetWindowTextLengthW(e);
                SendMessageW(e, EM_SETSEL, Some(WPARAM(len as usize)), Some(LPARAM(len as isize)));
                let _ = SetFocus(Some(e));
            }
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
    }

    fn reload_rows_and_activate(&self, id: Option<String>) {
        *self.active.borrow_mut() = None;
        self.flush(); // Unsaved title text must be stored before the reload replaces it.
        self.load_doc();
        self.activate(id);
        self.pending.set(false);
        app().request_sync(SyncCmd::Soon);
        hub::refresh();
    }

    fn add_item_after(&self, after: Option<String>) {
        self.commit_active_text();
        let list = self.id.borrow().clone();
        let anchor = after.or_else(|| self.rows.borrow().iter().filter(|r| !r.checked).last().map(|r| r.id.clone()));
        match app().store.add_item(&list, anchor.as_deref(), "") {
            Ok(new_id) => self.reload_rows_and_activate(Some(new_id)),
            Err(e) => report_error(&format!("The item could not be added: {e}")),
        }
    }

    fn on_item_enter(&self) {
        let cur = self.active.borrow().clone();
        self.add_item_after(cur);
    }

    fn on_item_backspace_empty(&self) {
        let Some(cur) = self.active.borrow().clone() else { return };
        let prev = {
            let rows = self.rows.borrow();
            let pos = rows.iter().position(|r| r.id == cur);
            pos.and_then(|p| p.checked_sub(1)).and_then(|p| rows.get(p)).filter(|r| !r.checked).map(|r| r.id.clone())
        };
        *self.active.borrow_mut() = None;
        let _ = app().store.delete_item(&cur);
        self.reload_rows_and_activate(prev);
    }

    fn on_item_changed(&self) {
        let Some(id) = self.active.borrow().clone() else { return };
        let text = get_text(self.item_edit.get());
        if let Some(r) = self.rows.borrow_mut().iter_mut().find(|r| r.id == id) {
            r.text = text;
        }
        self.layout_rows();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
        }
        self.schedule_save();
    }

    fn click_list(&self, x: i32, y: i32) {
        let cy = y - self.content_top() + self.scroll.get();
        if y < self.content_top() {
            return;
        }
        let hit = self.rows.borrow().iter().find(|r| cy >= r.y && cy < r.y + r.h).cloned();
        match hit {
            Some(r) if x < self.text_x() => {
                self.commit_active_text();
                *self.active.borrow_mut() = None;
                let _ = app().store.set_item_checked(&r.id, !r.checked);
                self.reload_rows_and_activate(None);
            }
            Some(r) => self.activate(Some(r.id)),
            None => {
                let ay = self.add_row_y.get();
                if cy >= ay && cy < ay + self.t(26, 42) {
                    self.add_item_after(None);
                } else {
                    self.activate(None);
                }
            }
        }
    }

    // ----- commands -----

    fn toggle_roll(&self) {
        self.flush();
        let hwnd = self.hwnd.get();
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let rolling = !self.rolled.get();
            if rolling {
                self.expanded_h.set(r.bottom - r.top);
                *self.active.borrow_mut() = None;
            }
            self.rolled.set(rolling);
            let h = if rolling { self.title_h() } else { self.expanded_h.get() };
            let _ = SetWindowPos(hwnd, None, 0, 0, r.right - r.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            self.layout();
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        self.save_state(true);
    }

    fn toggle_list(&self) {
        self.flush();
        *self.active.borrow_mut() = None;
        let id = self.id.borrow().clone();
        let a = app();
        match a.store.convert(&id) {
            Ok(Some(new_id)) => {
                self.save_state(false);
                let mut st = a.store.window_state(&id).ok().flatten().unwrap_or_default();
                st.open = true;
                let _ = a.store.save_window_state(&new_id, &st);
                st.open = false;
                let _ = a.store.save_window_state(&id, &st);
                *self.id.borrow_mut() = new_id;
                self.load_doc();
                self.layout();
                self.save_state(true);
                a.request_sync(SyncCmd::Soon);
                hub::refresh();
            }
            Ok(None) => {}
            Err(e) => report_error(&format!("The note could not be converted: {e}")),
        }
    }

    fn set_opacity(&self, v: i32) {
        self.opacity.set(v.clamp(MIN_OPACITY, 100));
        self.apply_opacity();
        self.save_state(true);
    }

    fn show_menu(&self) {
        self.flush();
        let a = app();
        let id = self.id.borrow().clone();
        let Ok(Some(top)) = a.store.get(&id) else { return };
        let labels = a.store.labels().unwrap_or_default();
        unsafe {
            let colours = CreatePopupMenu().unwrap();
            for (i, c) in COLOURS.iter().enumerate() {
                let flags = if c.0 == top.colour { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(colours, MF_STRING | flags, CMD_COLOUR + i, &HSTRING::from(c.1));
            }
            let opacity = CreatePopupMenu().unwrap();
            for (i, v) in OPACITIES.iter().enumerate() {
                let flags = if *v == self.opacity.get() { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(opacity, MF_STRING | flags, CMD_OPACITY + i, &HSTRING::from(format!("{v}%")));
            }
            let label_menu = CreatePopupMenu().unwrap();
            if labels.is_empty() {
                let _ = AppendMenuW(label_menu, MF_STRING | MF_GRAYED, 0, w!("No labels yet"));
            }
            for (i, (lid, name)) in labels.iter().enumerate() {
                let flags = if top.label_ids.contains(lid) { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(label_menu, MF_STRING | flags, CMD_LABEL + i, &HSTRING::from(name.as_str()));
            }
            let appearance = CreatePopupMenu().unwrap();
            for (i, t) in THEMES.iter().enumerate() {
                let flags = if a.theme.get() == t.0 { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(appearance, MF_STRING | flags, CMD_THEME + i, &HSTRING::from(t.2));
            }
            let menu = CreatePopupMenu().unwrap();
            let _ = AppendMenuW(menu, MF_POPUP, colours.0 as usize, w!("Colour"));
            let _ = AppendMenuW(menu, MF_POPUP, appearance.0 as usize, w!("Appearance"));
            let _ = AppendMenuW(menu, MF_POPUP, opacity.0 as usize, w!("Opacity"));
            let _ = AppendMenuW(menu, MF_POPUP, label_menu.0 as usize, w!("Labels"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let pin_flag = if top.pinned { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | pin_flag, CMD_PIN, w!("Pinned in Google Keep"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_ARCHIVE, w!("Archive"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_DELETE, w!("Delete (move to Bin)"));

            let hwnd = self.hwnd.get();
            let b = self.button_rect(BTN_MENU);
            let mut pt = POINT { x: b.left, y: b.bottom };
            let _ = ClientToScreen(hwnd, &mut pt);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, hwnd, None).0 as usize;
            let _ = DestroyMenu(menu);

            let mut changed = true;
            match cmd {
                0 => changed = false,
                c if (CMD_COLOUR..CMD_COLOUR + COLOURS.len()).contains(&c) => {
                    let api = COLOURS[c - CMD_COLOUR].0;
                    let _ = a.store.set_colour(&id, api);
                    self.set_colour_value(api);
                }
                c if (CMD_THEME..CMD_THEME + THEMES.len()).contains(&c) => {
                    a.set_theme(THEMES[c - CMD_THEME].0);
                    changed = false;
                }
                c if (CMD_OPACITY..CMD_OPACITY + OPACITIES.len()).contains(&c) => {
                    self.set_opacity(OPACITIES[c - CMD_OPACITY]);
                    changed = false;
                }
                c if (CMD_LABEL..CMD_LABEL + labels.len()).contains(&c) => {
                    let (lid, _) = &labels[c - CMD_LABEL];
                    let _ = a.store.set_label(&id, lid, !top.label_ids.contains(lid));
                }
                CMD_PIN => {
                    let _ = a.store.set_pinned(&id, !top.pinned);
                }
                CMD_ARCHIVE => {
                    let _ = a.store.set_archived(&id, true);
                    self.destroy(false);
                }
                CMD_DELETE => {
                    let _ = a.store.trash(&id);
                    self.destroy(false);
                }
                _ => changed = false,
            }
            if changed {
                a.request_sync(SyncCmd::Soon);
                hub::refresh();
            }
        }
    }

    fn click_button(&self, i: usize) {
        match i {
            BTN_ADD => app().new_note(TYPE_NOTE),
            BTN_LIST => self.toggle_list(),
            BTN_TOP => {
                self.topmost.set(!self.topmost.get());
                self.apply_topmost();
                self.save_state(true);
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
                }
            }
            BTN_ROLL => self.toggle_roll(),
            BTN_MENU => self.show_menu(),
            BTN_CLOSE => self.destroy(false),
            _ => {}
        }
    }

    // ----- painting -----

    unsafe fn draw_glyph(&self, dc: HDC, glyph: u16, r: RECT) {
        let mut buf = [glyph];
        let mut rc = r;
        DrawTextW(dc, &mut buf, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }

    unsafe fn paint(&self, target: HDC) {
        let hwnd = self.hwnd.get();
        let mut c = RECT::default();
        let _ = GetClientRect(hwnd, &mut c);
        let dc = CreateCompatibleDC(Some(target));
        let bmp = CreateCompatibleBitmap(target, c.right.max(1), c.bottom.max(1));
        let old_bmp = SelectObject(dc, bmp.into());
        let f = app().fonts_for(hwnd);
        let pal = self.pal.get();
        let mac = is_mac();
        FillRect(dc, &c, self.brush.get());
        if mac {
            // A soft vertical gradient, like a macOS title bar.
            let (top, bottom) = (theme::lighten(pal.bg, 35), pal.bar);
            let th = self.title_h().max(1);
            let mix = |a: u32, b: u32, k: i32, sh: u32| -> u32 {
                let (x, y) = (((a >> sh) & 0xFF) as i32, ((b >> sh) & 0xFF) as i32);
                ((x + (y - x) * k / th) as u32) << sh
            };
            for line in 0..th {
                let colour = mix(top, bottom, line, 16) | mix(top, bottom, line, 8) | mix(top, bottom, line, 0);
                let br = CreateSolidBrush(rgb(colour));
                FillRect(dc, &RECT { left: 0, top: line, right: c.right, bottom: line + 1 }, br);
                let _ = DeleteObject(br.into());
            }
            let sep = CreateSolidBrush(rgb(theme::darken(pal.bg, 18)));
            FillRect(dc, &RECT { left: 0, top: th - 1, right: c.right, bottom: th }, sep);
            let _ = DeleteObject(sep.into());
        } else {
            let bar = RECT { left: 0, top: 0, right: c.right, bottom: self.title_h() };
            let bar_brush = CreateSolidBrush(rgb(pal.bar));
            FillRect(dc, &bar, bar_brush);
            let _ = DeleteObject(bar_brush.into());
        }
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(pal.accent));

        let old_font = SelectObject(dc, f.icon.into());
        let hover_brush = CreateSolidBrush(rgb(pal.hover));
        let glyphs: [u16; BTN_COUNT] = [
            0xE710,
            0xE73A,
            if self.topmost.get() { 0xE840 } else { 0xE718 },
            if self.rolled.get() { 0xE70D } else { 0xE70E },
            0xE712,
            0xE711,
        ];
        let lights_hovered = matches!(self.hover.get(), h if h == BTN_CLOSE as i32 || h == BTN_ROLL as i32 || h == BTN_TOP as i32);
        for (i, g) in glyphs.iter().enumerate() {
            let r = self.button_rect(i);
            if mac && matches!(i, BTN_CLOSE | BTN_ROLL | BTN_TOP) {
                self.draw_light(dc, i, r, lights_hovered, f.tiny);
                continue;
            }
            if self.hover.get() == i as i32 {
                FillRect(dc, &r, hover_brush);
            }
            SelectObject(dc, f.icon.into());
            if mac {
                SetTextColor(dc, rgb(pal.muted));
            }
            self.draw_glyph(dc, *g, r);
        }
        SetTextColor(dc, rgb(pal.accent));
        if app().theme.get() == theme::Theme::Black {
            let stripe = CreateSolidBrush(rgb(pal.accent));
            let r = RECT { left: 0, top: self.title_h() - self.s(2), right: c.right, bottom: self.title_h() };
            FillRect(dc, &r, stripe);
            let _ = DeleteObject(stripe.into());
        }
        if !self.rolled.get() && !self.is_list.get() {
            let labels = ["B", "I", "U", "S", "H", "\u{2022}"];
            let faces = [f.bold, f.italic, f.underline, f.strike, f.bold, f.bold];
            SetTextColor(dc, rgb(pal.text));
            for i in 0..FMT_COUNT {
                let r = self.fmt_rect(i);
                if self.hover.get() == FMT_HOVER_BASE + i as i32 {
                    FillRect(dc, &r, hover_brush);
                }
                SelectObject(dc, faces[i].into());
                let mut w: Vec<u16> = labels[i].encode_utf16().collect();
                let mut t = r;
                DrawTextW(dc, &mut w, &mut t, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
            SetTextColor(dc, rgb(pal.accent));
        }
        let _ = DeleteObject(hover_brush.into());

        if self.rolled.get() {
            SelectObject(dc, f.bold.into());
            let title = self.last_title.borrow().clone();
            let shown = if title.is_empty() { "Note".to_string() } else { title };
            let mut w: Vec<u16> = shown.encode_utf16().collect();
            let mid = c.right / 2;
            let rects: Vec<RECT> = (0..BTN_COUNT).map(|i| self.button_rect(i)).collect();
            let left = rects.iter().filter(|r| r.left < mid).map(|r| r.right).max().unwrap_or(0) + self.s(4);
            let right = rects.iter().filter(|r| r.left >= mid).map(|r| r.left).min().unwrap_or(c.right) - self.s(4);
            let mut r = RECT { left, top: 0, right, bottom: self.title_h() };
            let align = if mac { DT_CENTER } else { DT_LEFT };
            SetTextColor(dc, rgb(pal.text));
            DrawTextW(dc, &mut w, &mut r, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        } else if self.is_list.get() {
            self.paint_rows(dc, c, f);
        }

        SelectObject(dc, old_font);
        let _ = BitBlt(target, 0, 0, c.right, c.bottom, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old_bmp);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
    }

    /// One macOS-style traffic light. Green shows whether the note is kept on top.
    unsafe fn draw_light(&self, dc: HDC, i: usize, slot: RECT, hovered: bool, tiny: HFONT) {
        let colour = match i {
            BTN_CLOSE => 0xFF5F57,
            BTN_ROLL => 0xFEBC2E,
            _ if self.topmost.get() => 0x28C840,
            _ => 0xC8C8C8,
        };
        let d = self.t(12, 20);
        let cx = (slot.left + slot.right) / 2;
        let cy = (slot.top + slot.bottom) / 2;
        let brush = CreateSolidBrush(rgb(colour));
        let pen = CreatePen(PS_SOLID, 1, rgb(theme::darken(colour, 18)));
        let ob = SelectObject(dc, brush.into());
        let op = SelectObject(dc, pen.into());
        let _ = Ellipse(dc, cx - d / 2, cy - d / 2, cx - d / 2 + d, cy - d / 2 + d);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
        if hovered {
            let glyph = match i {
                BTN_CLOSE => 0xE711,
                BTN_ROLL => if self.rolled.get() { 0xE70D } else { 0xE738 },
                _ => 0xE718,
            };
            SelectObject(dc, tiny.into());
            SetTextColor(dc, rgb(0x4A1F1C));
            self.draw_glyph(dc, glyph, RECT { left: cx - d / 2, top: cy - d / 2, right: cx - d / 2 + d, bottom: cy - d / 2 + d });
        }
    }

    unsafe fn paint_rows(&self, dc: HDC, c: RECT, f: super::Fonts) {
        let top = self.content_top();
        let saved = SaveDC(dc);
        IntersectClipRect(dc, 0, top, c.right, c.bottom);
        let scroll = self.scroll.get();
        let box_x = self.s(12);
        let box_size = self.t(16, 24);
        let row_min = self.t(26, 42);
        let text_x = self.text_x();
        let active = self.active.borrow().clone();
        let pal = self.pal.get();
        let edge = CreateSolidBrush(rgb(pal.muted));
        for r in self.rows.borrow().iter() {
            let y = top + r.y - scroll;
            if y + r.h < top || y > c.bottom {
                continue;
            }
            let by = y + (row_min - box_size) / 2;
            let b = RECT { left: box_x, top: by, right: box_x + box_size, bottom: by + box_size };
            FrameRect(dc, &b, edge);
            if r.checked {
                SelectObject(dc, f.icon.into());
                SetTextColor(dc, rgb(pal.accent));
                self.draw_glyph(dc, 0xE73E, b);
            }
            // Windows reads an empty buffer through an invalid pointer, so empty items draw no text.
            if active.as_deref() != Some(r.id.as_str()) && !r.text.is_empty() {
                SelectObject(dc, if r.checked { f.strike } else { f.ui }.into());
                SetTextColor(dc, rgb(if r.checked { pal.muted } else { pal.text }));
                let mut w: Vec<u16> = r.text.encode_utf16().collect();
                let mut t = RECT { left: text_x + self.s(4), top: y + (row_min - self.s(20)) / 2, right: c.right - self.s(10), bottom: y + r.h };
                DrawTextW(dc, &mut w, &mut t, DT_WORDBREAK | DT_NOPREFIX | DT_EDITCONTROL);
            }
        }
        let _ = DeleteObject(edge.into());

        let y = top + self.add_row_y.get() - scroll;
        SelectObject(dc, f.icon.into());
        SetTextColor(dc, rgb(pal.muted));
        self.draw_glyph(dc, 0xE710, RECT { left: box_x, top: y, right: box_x + box_size, bottom: y + row_min });
        SelectObject(dc, f.ui.into());
        let mut w: Vec<u16> = "Add item".encode_utf16().collect();
        let mut t = RECT { left: text_x + self.s(4), top: y, right: c.right - self.s(10), bottom: y + row_min };
        DrawTextW(dc, &mut w, &mut t, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        let _ = RestoreDC(dc, saved);
    }

    unsafe fn hit_test(&self, lp: LPARAM) -> u32 {
        let (sx, sy) = lp_xy(lp);
        let mut pt = POINT { x: sx, y: sy };
        let _ = ScreenToClient(self.hwnd.get(), &mut pt);
        let mut c = RECT::default();
        let _ = GetClientRect(self.hwnd.get(), &mut c);
        let e = self.t(6, 12);
        let (l, r, t, b) = (pt.x < e, pt.x >= c.right - e, pt.y < e, pt.y >= c.bottom - e);
        if self.rolled.get() {
            if l {
                return HTLEFT;
            }
            if r {
                return HTRIGHT;
            }
        } else {
            match (l, r, t, b) {
                (true, _, true, _) => return HTTOPLEFT,
                (_, true, true, _) if self.button_at(pt.x, pt.y).is_none() => return HTTOPRIGHT,
                (true, _, _, true) => return HTBOTTOMLEFT,
                (_, true, _, true) => return HTBOTTOMRIGHT,
                (true, ..) => return HTLEFT,
                (_, true, ..) if self.button_at(pt.x, pt.y).is_none() => return HTRIGHT,
                (_, _, true, _) if self.button_at(pt.x, pt.y).is_none() => return HTTOP,
                (_, _, _, true) => return HTBOTTOM,
                _ => {}
            }
        }
        if pt.y < self.title_h() && self.button_at(pt.x, pt.y).is_none() {
            return HTCAPTION;
        }
        HTCLIENT
    }
}

extern "system" fn item_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        let win = find(HWND(data as *mut _));
        match (msg, wp.0 as u16, win) {
            (WM_CHAR, 13, Some(_)) => return LRESULT(0),
            (WM_KEYDOWN, k, Some(w)) if k == VK_RETURN.0 => {
                w.on_item_enter();
                return LRESULT(0);
            }
            (WM_KEYDOWN, k, Some(w)) if k == VK_BACK.0 && GetWindowTextLengthW(hwnd) == 0 => {
                w.on_item_backspace_empty();
                return LRESULT(0);
            }
            (WM_KEYDOWN, k, Some(w)) if k == VK_ESCAPE.0 => {
                w.activate(None);
                return LRESULT(0);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

extern "system" fn body_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        if let Some(w) = find(HWND(data as *mut _)) {
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            if msg == WM_KEYDOWN {
                let key = wp.0 as u16;
                let shortcut = match key {
                    0x42 => Some(0),
                    0x49 => Some(1),
                    0x55 => Some(2),
                    _ => None,
                };
                if let (true, false, Some(i)) = (ctrl, shift, shortcut) {
                    w.apply_format(i);
                    return LRESULT(0);
                }
                if key == VK_RETURN.0 && !ctrl && !shift {
                    let r = get_range(hwnd);
                    if r.min == r.max {
                        let text: Vec<u16> = get_text(hwnd).encode_utf16().collect();
                        if let Some(e) = format::bullet_enter(&text, r.min.max(0) as usize) {
                            w.swallow_return.set(true);
                            w.perform_edit(e);
                            return LRESULT(0);
                        }
                    }
                }
            }
            if msg == WM_CHAR {
                if wp.0 == 13 && w.swallow_return.replace(false) {
                    return LRESULT(0);
                }
                if ctrl && matches!(wp.0, 2 | 9 | 21) {
                    return LRESULT(0);
                }
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCALCSIZE && wp.0 != 0 {
            return LRESULT(0); // The whole window is client area, keeping DWM's shadow.
        }
        let Some(win) = find(hwnd) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_NCHITTEST => LRESULT(win.hit_test(lp) as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                win.paint(dc);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_SIZE => {
                win.layout();
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                win.save_state(true);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = win.t(220, 300);
                info.ptMinTrackSize.y = if win.rolled.get() { win.title_h() } else { win.t(120, 180) };
                if win.rolled.get() {
                    info.ptMaxTrackSize.y = win.title_h();
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                win.apply_fonts();
                win.layout();
                LRESULT(0)
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let dc = HDC(wp.0 as *mut _);
                let p = win.pal.get();
                SetBkColor(dc, rgb(p.bg));
                SetTextColor(dc, rgb(p.text));
                LRESULT(win.brush.get().0 as isize)
            }
            WM_COMMAND => {
                if ((wp.0 >> 16) & 0xFFFF) as u32 == EN_CHANGE {
                    match (wp.0 & 0xFFFF) as isize {
                        ID_ITEM => {
                            if !win.loading.get() {
                                win.on_item_changed();
                            }
                        }
                        ID_BODY => {
                            if !win.restyling.get() && !win.loading.get() {
                                win.restyle();
                                win.schedule_save();
                            }
                        }
                        _ => win.schedule_save(),
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wp.0 == TIMER_SAVE {
                    win.flush();
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = lp_xy(lp);
                let h = win.button_at(x, y).map(|i| i as i32).or_else(|| win.fmt_at(x, y).map(|i| FMT_HOVER_BASE + i as i32)).unwrap_or(-1);
                if h != win.hover.replace(h) {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if !win.tracking.replace(true) {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut t);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE | WM_NCMOUSEMOVE => {
                win.tracking.set(false);
                if win.hover.replace(-1) != -1 {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = lp_xy(lp);
                if win.button_at(x, y).is_none() && win.is_list.get() {
                    win.click_list(x, y);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = lp_xy(lp);
                if let Some(i) = win.button_at(x, y) {
                    win.click_button(i);
                } else if let Some(i) = win.fmt_at(x, y) {
                    win.apply_format(i);
                }
                LRESULT(0)
            }
            WM_GESTURE => {
                let mut last = win.pan_y.get();
                match super::form::pan_delta(lp, &mut last) {
                    Some(d) if win.is_list.get() => {
                        win.pan_y.set(last);
                        win.scroll.set(win.scroll.get() + d);
                        win.clamp_scroll();
                        win.position_item_edit();
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        LRESULT(0)
                    }
                    Some(_) => {
                        win.pan_y.set(last);
                        LRESULT(0)
                    }
                    None => DefWindowProcW(hwnd, msg, wp, lp),
                }
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                if wp.0 & 0x0008 != 0 /* MK_CONTROL */ {
                    win.set_opacity(win.opacity.get() + delta.signum() * 5);
                } else if win.is_list.get() {
                    win.scroll.set(win.scroll.get() - delta * win.s(40) / 120);
                    win.clamp_scroll();
                    win.position_item_edit();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), TIMER_SAVE);
                let brush = win.brush.get();
                if !brush.is_invalid() {
                    let _ = DeleteObject(brush.into());
                }
                app().notes.borrow_mut().remove(&(hwnd.0 as isize));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

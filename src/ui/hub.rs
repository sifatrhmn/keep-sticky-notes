//! Note Manager: search and manage every note, including the archive and the Bin.

use super::form;
use super::{app, hinstance, SyncCmd, APP_NAME};
use keepsticky::store::{Shelf, TYPE_LIST, TYPE_NOTE};
use std::cell::{Cell, RefCell};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("KeepStickyNotes_Hub");
const ID_SEARCH: i32 = 1;
const ID_SHELF: i32 = 2;
const ID_LIST: i32 = 3;
const ID_NEW: i32 = 4;
const ID_NEW_LIST: i32 = 5;
const ID_OPEN: i32 = 6;
const ID_ARCHIVE: i32 = 7;
const ID_DELETE: i32 = 8;
const ID_STATUS: i32 = 9;
const ID_ACCOUNT: i32 = 10;
const ID_THEME: i32 = 11;
const ID_ABOUT: i32 = 12;
const ID_APPEARANCE: i32 = 13;
const BUTTONS: [i32; 9] = [ID_NEW, ID_NEW_LIST, ID_OPEN, ID_ARCHIVE, ID_DELETE, ID_ACCOUNT, ID_THEME, ID_APPEARANCE, ID_ABOUT];
const LBN_DBLCLK_CODE: u32 = 2;
const LBN_SELCHANGE_CODE: u32 = 1;
const CBN_SELCHANGE_CODE: u32 = 1;
const LBS_OWNERDRAWFIXED_STYLE: u32 = 0x10;
const LBS_HASSTRINGS_STYLE: u32 = 0x40;

thread_local! {
    static IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LAYING_OUT: Cell<bool> = const { Cell::new(false) };
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

pub fn show() {
    let a = app();
    unsafe {
        if !a.hub.get().is_invalid() {
            let _ = ShowWindow(a.hub.get(), SW_RESTORE);
            let _ = SetForegroundWindow(a.hub.get());
            return;
        }
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(format!("{APP_NAME}: Note Manager")),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            800,
            500,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            return;
        };
        a.hub.set(h);
        let tab = WS_TABSTOP.0;
        let b = tab | 0xB;
        let search = form::control(h, w!("EDIT"), PCWSTR::null(), tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE.0, ID_SEARCH);
        SendMessageW(search, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(w!("Search notes").as_ptr() as isize)));
        let shelf = form::control(h, w!("COMBOBOX"), PCWSTR::null(), tab | CBS_DROPDOWNLIST as u32, 0, ID_SHELF);
        for s in ["Notes", "Archive", "Bin"] {
            SendMessageW(shelf, CB_ADDSTRING, None, Some(LPARAM(HSTRING::from(s).as_ptr() as isize)));
        }
        SendMessageW(shelf, CB_SETCURSEL, Some(WPARAM(0)), None);
        let list_style = tab | WS_VSCROLL.0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32 | LBS_OWNERDRAWFIXED_STYLE | LBS_HASSTRINGS_STYLE;
        form::control(h, w!("LISTBOX"), PCWSTR::null(), list_style, WS_EX_CLIENTEDGE.0, ID_LIST);
        form::control(h, w!("BUTTON"), w!("New Note"), b, 0, ID_NEW);
        form::control(h, w!("BUTTON"), w!("New Checklist"), b, 0, ID_NEW_LIST);
        form::control(h, w!("BUTTON"), w!("Open"), b, 0, ID_OPEN);
        form::control(h, w!("BUTTON"), w!("Archive"), b, 0, ID_ARCHIVE);
        form::control(h, w!("BUTTON"), w!("Delete"), b, 0, ID_DELETE);
        form::control(h, w!("BUTTON"), w!("Account"), b, 0, ID_ACCOUNT);
        form::control(h, w!("BUTTON"), a.toggle_label(), b, 0, ID_THEME);
        form::control(h, w!("BUTTON"), w!("Appearance"), b, 0, ID_APPEARANCE);
        form::control(h, w!("BUTTON"), w!("About"), b, 0, ID_ABOUT);
        form::control(h, w!("STATIC"), w!(""), 0, 0, ID_STATUS);
        apply_touch();
        super::theme::apply_chrome(h);
        refresh();
        update_status();
        form::fit_client(h, 900, form::scale(h, 560));
        layout(h);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
    }
}

unsafe fn item(h: HWND, id: i32) -> HWND {
    GetDlgItem(Some(h), id).unwrap_or_default()
}

/// Re-applies fonts and target sizes after a touch, DPI or theme change.
pub fn apply_touch() {
    let h = app().hub.get();
    if h.is_invalid() {
        return;
    }
    unsafe {
        form::apply_fonts(h, &[]);
        let row = form::sized(h, 28, 46);
        SendMessageW(item(h, ID_LIST), LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(row as isize)));
        layout(h);
        let _ = InvalidateRect(Some(h), None, true);
    }
}

unsafe fn layout(h: HWND) {
    if LAYING_OUT.with(|l| l.replace(true)) {
        return;
    }
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    let m = form::scale(h, 10);
    let row = form::sized(h, 30, 44);
    let combo_w = form::sized(h, 130, 160);
    let width = (c.right - 2 * m).max(form::scale(h, 100));
    let _ = MoveWindow(item(h, ID_SEARCH), m, m, (c.right - 3 * m - combo_w).max(form::scale(h, 60)), row, true);
    let _ = MoveWindow(item(h, ID_SHELF), c.right - m - combo_w, m, combo_w, form::scale(h, 200), true);
    SendMessageW(item(h, ID_SHELF), CB_SETITEMHEIGHT, Some(WPARAM(usize::MAX)), Some(LPARAM((row - form::scale(h, 6)) as isize)));

    let status = item(h, ID_STATUS);
    let status_h = form::text_height(status, width);
    let buttons_h = form::flow_buttons(h, &BUTTONS, m, 0, width, false);
    let status_y = c.bottom - m - status_h;
    let buttons_y = status_y - form::scale(h, 8) - buttons_h;
    let list_y = m * 2 + row;
    let _ = MoveWindow(item(h, ID_LIST), m, list_y, width, (buttons_y - m - list_y).max(form::scale(h, 40)), true);
    form::flow_buttons(h, &BUTTONS, m, buttons_y, width, true);
    let _ = MoveWindow(status, m, status_y, width, status_h, true);
    LAYING_OUT.with(|l| l.set(false));
}

unsafe fn current_shelf(h: HWND) -> Shelf {
    match SendMessageW(item(h, ID_SHELF), CB_GETCURSEL, None, None).0 {
        1 => Shelf::Archive,
        2 => Shelf::Trash,
        _ => Shelf::Notes,
    }
}

fn summary(doc: &keepsticky::store::NoteDoc) -> String {
    let body = if doc.is_list() {
        doc.items.iter().map(|i| format!("{} {}", if i.checked { "[x]" } else { "[ ]" }, i.text)).collect::<Vec<_>>().join("  ")
    } else {
        keepsticky::format::strip_markers(&doc.body()).replace('\n', "  ")
    };
    let title = if doc.top.title.is_empty() { "(untitled)" } else { doc.top.title.as_str() };
    let pin = if doc.top.pinned { "* " } else { "" };
    let short: String = body.chars().take(90).collect();
    format!("{pin}{title}  |  {short}")
}

pub fn refresh() {
    let a = app();
    let h = a.hub.get();
    if h.is_invalid() {
        return;
    }
    unsafe {
        let shelf = current_shelf(h);
        let search_h = item(h, ID_SEARCH);
        let mut buf = vec![0u16; GetWindowTextLengthW(search_h) as usize + 1];
        let n = GetWindowTextW(search_h, &mut buf) as usize;
        let query = String::from_utf16_lossy(&buf[..n]);
        let docs = a.store.list_top(shelf, &query).unwrap_or_default();
        let list = item(h, ID_LIST);
        let selected = selected_id(h);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        let mut ids = Vec::with_capacity(docs.len());
        for d in &docs {
            let text = HSTRING::from(summary(d));
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
            ids.push(d.top.id.clone());
        }
        if let Some(pos) = selected.and_then(|s| ids.iter().position(|i| *i == s)) {
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(pos)), None);
        }
        IDS.with(|i| *i.borrow_mut() = ids);
        let _ = SetWindowTextW(item(h, ID_THEME), a.toggle_label());
        let in_bin = shelf == Shelf::Trash;
        let archive_label = if shelf == Shelf::Archive { w!("Unarchive") } else { w!("Archive") };
        let _ = SetWindowTextW(item(h, ID_ARCHIVE), archive_label);
        let _ = SetWindowTextW(item(h, ID_DELETE), if in_bin { w!("Restore") } else { w!("Delete") });
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(item(h, ID_ARCHIVE), !in_bin);
        layout(h);
    }
}

pub fn update_status() {
    let a = app();
    let h = a.hub.get();
    if !h.is_invalid() {
        unsafe {
            let _ = SetWindowTextW(item(h, ID_STATUS), &HSTRING::from(a.status_text()));
            layout(h);
        }
    }
}

unsafe fn selected_id(h: HWND) -> Option<String> {
    let i = SendMessageW(item(h, ID_LIST), LB_GETCURSEL, None, None).0;
    if i < 0 {
        return None;
    }
    IDS.with(|v| v.borrow().get(i as usize).cloned())
}

unsafe fn command(h: HWND, id: i32, code: u32) {
    let a = app();
    match (id, code) {
        (ID_SEARCH, c) if c == EN_CHANGE => refresh(),
        (ID_SHELF, CBN_SELCHANGE_CODE) => refresh(),
        (ID_LIST, LBN_DBLCLK_CODE) | (ID_OPEN, 0) => {
            if let Some(n) = selected_id(h) {
                a.open_note(&n);
            }
        }
        (ID_LIST, LBN_SELCHANGE_CODE) => {}
        (ID_ACCOUNT, 0) => super::account::show(),
        (ID_THEME, 0) => a.toggle_light(),
        (ID_ABOUT, 0) => super::about::show(),
        (ID_APPEARANCE, 0) => {
            let mut r = RECT::default();
            let _ = GetWindowRect(item(h, ID_APPEARANCE), &mut r);
            super::show_appearance_menu(h, r.left, r.bottom);
        }
        (ID_NEW, 0) => a.new_note(TYPE_NOTE),
        (ID_NEW_LIST, 0) => a.new_note(TYPE_LIST),
        (ID_ARCHIVE, 0) => {
            if let Some(n) = selected_id(h) {
                let archive = current_shelf(h) != Shelf::Archive;
                close_window_for(&n);
                let _ = a.store.set_archived(&n, archive);
                a.request_sync(SyncCmd::Soon);
                refresh();
            }
        }
        (ID_DELETE, 0) => {
            if let Some(n) = selected_id(h) {
                if current_shelf(h) == Shelf::Trash {
                    let _ = a.store.restore(&n);
                } else {
                    close_window_for(&n);
                    let _ = a.store.trash(&n);
                }
                a.request_sync(SyncCmd::Soon);
                refresh();
            }
        }
        _ => {}
    }
}

fn close_window_for(id: &str) {
    let found = app().snapshot_notes().into_iter().find(|n| *n.id.borrow() == id);
    if let Some(n) = found {
        n.flush();
        unsafe {
            let mut st = app().store.window_state(id).ok().flatten().unwrap_or_default();
            st.open = false;
            let _ = app().store.save_window_state(id, &st);
            let _ = DestroyWindow(n.hwnd.get());
        }
    }
}

extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if !app().hub.get().is_invalid() {
            if let Some(r) = super::theme::chrome_message(h, msg, wp, lp) {
                return r;
            }
        }
        match msg {
            WM_SIZE => {
                if !app().hub.get().is_invalid() {
                    layout(h);
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = form::scale(h, 360);
                info.ptMinTrackSize.y = form::scale(h, 320);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                apply_touch();
                LRESULT(0)
            }
            WM_COMMAND => {
                command(h, (wp.0 & 0xFFFF) as i32, ((wp.0 >> 16) & 0xFFFF) as u32);
                LRESULT(0)
            }
            WM_DESTROY => {
                app().hub.set(HWND::default());
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

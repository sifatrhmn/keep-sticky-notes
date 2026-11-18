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
const ID_SELECT_ALL: i32 = 14;
const ID_PIN: i32 = 15;
const ID_PURGE: i32 = 16;
const BUTTONS: [i32; 12] = [
    ID_NEW, ID_NEW_LIST, ID_OPEN, ID_SELECT_ALL, ID_PIN, ID_ARCHIVE, ID_DELETE, ID_PURGE, ID_ACCOUNT, ID_THEME, ID_APPEARANCE, ID_ABOUT,
];
const LBS_EXTENDEDSEL_STYLE: u32 = 0x800;
const LBS_WANTKEYBOARDINPUT_STYLE: u32 = 0x400;
const LBN_DBLCLK_CODE: u32 = 2;
const LBN_SELCHANGE_CODE: u32 = 1;
const CBN_SELCHANGE_CODE: u32 = 1;
const LBS_OWNERDRAWFIXED_STYLE: u32 = 0x10;
const LBS_HASSTRINGS_STYLE: u32 = 0x40;

thread_local! {
    static IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static PINNED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    static LAYING_OUT: Cell<bool> = const { Cell::new(false) };
    /// macOS theme: rounded panels drawn behind the search box and the list.
    static PANELS: Cell<(RECT, RECT)> = const { Cell::new((RECT { left: 0, top: 0, right: 0, bottom: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 })) };
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
        let list_style = tab | WS_VSCROLL.0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32 | LBS_OWNERDRAWFIXED_STYLE | LBS_HASSTRINGS_STYLE | LBS_EXTENDEDSEL_STYLE | LBS_WANTKEYBOARDINPUT_STYLE;
        form::control(h, w!("LISTBOX"), PCWSTR::null(), list_style, WS_EX_CLIENTEDGE.0, ID_LIST);
        form::control(h, w!("BUTTON"), w!("New Note"), b, 0, ID_NEW);
        form::control(h, w!("BUTTON"), w!("New Checklist"), b, 0, ID_NEW_LIST);
        form::control(h, w!("BUTTON"), w!("Open"), b, 0, ID_OPEN);
        form::control(h, w!("BUTTON"), w!("Select All"), b, 0, ID_SELECT_ALL);
        form::control(h, w!("BUTTON"), w!("Pin"), b, 0, ID_PIN);
        form::control(h, w!("BUTTON"), w!("Archive"), b, 0, ID_ARCHIVE);
        form::control(h, w!("BUTTON"), w!("Delete"), b, 0, ID_DELETE);
        form::control(h, w!("BUTTON"), w!("Delete Forever"), b, 0, ID_PURGE);
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
        let mac = super::theme::is_mac();
        for id in [ID_SEARCH, ID_LIST] {
            let c = item(h, id);
            let ex = GetWindowLongW(c, GWL_EXSTYLE) as u32;
            let new = if mac { ex & !WS_EX_CLIENTEDGE.0 } else { ex | WS_EX_CLIENTEDGE.0 };
            if new != ex {
                SetWindowLongW(c, GWL_EXSTYLE, new as i32);
                let _ = SetWindowPos(c, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
            }
        }
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
    let mac = super::theme::is_mac();
    let top = super::theme::top_inset(h);
    let m = form::scale(h, if mac { 14 } else { 10 });
    let row = form::sized(h, if mac { 34 } else { 30 }, 44);
    let combo_w = form::sized(h, 130, 160);
    let width = (c.right - 2 * m).max(form::scale(h, 100));
    let search = RECT { left: m, top: top + m, right: m + (c.right - 3 * m - combo_w).max(form::scale(h, 60)), bottom: top + m + row };
    if mac {
        let edit_h = form::text_height(item(h, ID_SEARCH), 1000) + form::scale(h, 2);
        let pad = row / 2;
        let _ = MoveWindow(item(h, ID_SEARCH), search.left + pad, search.top + (row - edit_h) / 2, search.right - search.left - 2 * pad, edit_h, true);
    } else {
        let _ = MoveWindow(item(h, ID_SEARCH), search.left, search.top, search.right - search.left, row, true);
    }
    let _ = MoveWindow(item(h, ID_SHELF), c.right - m - combo_w, top + m, combo_w, form::scale(h, 200), true);
    SendMessageW(item(h, ID_SHELF), CB_SETITEMHEIGHT, Some(WPARAM(usize::MAX)), Some(LPARAM((row - form::scale(h, 6)) as isize)));

    let status = item(h, ID_STATUS);
    let status_h = form::text_height(status, width);
    let buttons_h = form::flow_buttons(h, &BUTTONS, m, 0, width, false);
    let status_y = c.bottom - m - status_h;
    let buttons_y = status_y - form::scale(h, 8) - buttons_h;
    let list_y = top + m * 2 + row;
    let panel = RECT { left: m, top: list_y, right: m + width, bottom: list_y + (buttons_y - m - list_y).max(form::scale(h, 40)) };
    let inset = if mac { form::scale(h, 8) } else { 0 };
    let _ = MoveWindow(item(h, ID_LIST), panel.left + inset, panel.top + inset, width - 2 * inset, panel.bottom - panel.top - 2 * inset, true);
    form::flow_buttons(h, &BUTTONS, m, buttons_y, width, true);
    let _ = MoveWindow(status, m, status_y, width, status_h, true);
    let old = PANELS.with(|p| p.replace((search, panel)));
    if mac && old != (search, panel) {
        let _ = InvalidateRect(Some(h), None, true);
    }
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
    let pin = if doc.top.pinned { "\u{1F4CC} " } else { "" };
    let short: String = body.chars().take(90).collect();
    if short.trim().is_empty() {
        format!("{pin}{title}")
    } else {
        format!("{pin}{title}  |  {short}")
    }
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
        let selected = selected_ids(h);
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        let mut ids = Vec::with_capacity(docs.len() + 2);
        let mut pinned = Vec::with_capacity(docs.len() + 2);
        // Keep shows pinned notes under "Pinned" and the rest under "Others", but only when something is pinned.
        let sections = shelf == Shelf::Notes && docs.iter().any(|d| d.top.pinned);
        let header = |label: &str, ids: &mut Vec<String>, pinned: &mut Vec<bool>| {
            let text = HSTRING::from(label);
            let pos = SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize))).0;
            SendMessageW(list, LB_SETITEMDATA, Some(WPARAM(pos as usize)), Some(LPARAM(super::theme::SECTION_HEADER as isize)));
            ids.push(String::new());
            pinned.push(false);
        };
        if sections {
            header("PINNED", &mut ids, &mut pinned);
        }
        let mut others_shown = false;
        for d in &docs {
            if sections && !d.top.pinned && !others_shown {
                header("OTHERS", &mut ids, &mut pinned);
                others_shown = true;
            }
            let text = HSTRING::from(summary(d));
            let pos = SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize))).0;
            let tint = super::theme::colour_index(&d.top.colour) + 1;
            SendMessageW(list, LB_SETITEMDATA, Some(WPARAM(pos as usize)), Some(LPARAM(tint as isize)));
            if selected.contains(&d.top.id) {
                SendMessageW(list, LB_SETSEL, Some(WPARAM(1)), Some(LPARAM(pos)));
            }
            ids.push(d.top.id.clone());
            pinned.push(d.top.pinned);
        }
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(list), None, true);
        IDS.with(|i| *i.borrow_mut() = ids);
        PINNED.with(|p| *p.borrow_mut() = pinned);
        let _ = SetWindowTextW(item(h, ID_THEME), a.toggle_label());
        let in_bin = shelf == Shelf::Trash;
        let archive_label = if shelf == Shelf::Archive { w!("Unarchive") } else { w!("Archive") };
        let _ = SetWindowTextW(item(h, ID_ARCHIVE), archive_label);
        let _ = SetWindowTextW(item(h, ID_DELETE), if in_bin { w!("Restore") } else { w!("Delete") });
        for (id, show) in [(ID_ARCHIVE, !in_bin), (ID_PIN, !in_bin), (ID_PURGE, in_bin)] {
            let _ = ShowWindow(item(h, id), if show { SW_SHOW } else { SW_HIDE });
        }
        update_selection_buttons(h);
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

unsafe fn selected_indices(h: HWND) -> Vec<usize> {
    let list = item(h, ID_LIST);
    let n = SendMessageW(list, LB_GETSELCOUNT, None, None).0;
    if n <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0i32; n as usize];
    let got = SendMessageW(list, LB_GETSELITEMS, Some(WPARAM(n as usize)), Some(LPARAM(buf.as_mut_ptr() as isize))).0.max(0);
    buf.truncate(got as usize);
    // Section headings can never be selected.
    let mut out = Vec::with_capacity(buf.len());
    for i in buf {
        if is_header(i as usize) {
            SendMessageW(list, LB_SETSEL, Some(WPARAM(0)), Some(LPARAM(i as isize)));
        } else {
            out.push(i as usize);
        }
    }
    out
}

fn is_header(i: usize) -> bool {
    IDS.with(|v| v.borrow().get(i).is_some_and(|s| s.is_empty()))
}

fn note_count() -> usize {
    IDS.with(|v| v.borrow().iter().filter(|s| !s.is_empty()).count())
}

/// Every selected note, in list order.
unsafe fn selected_ids(h: HWND) -> Vec<String> {
    let idx = selected_indices(h);
    IDS.with(|v| idx.iter().filter_map(|i| v.borrow().get(*i).cloned()).collect())
}

/// The note under the caret, else the first selected one.
unsafe fn selected_id(h: HWND) -> Option<String> {
    let caret = SendMessageW(item(h, ID_LIST), LB_GETCARETINDEX, None, None).0;
    let sel = selected_indices(h);
    let i = if caret >= 0 && sel.contains(&(caret as usize)) { caret as usize } else { *sel.first()? };
    IDS.with(|v| v.borrow().get(i).cloned())
}

unsafe fn select_all(h: HWND, on: bool) {
    SendMessageW(item(h, ID_LIST), LB_SETSEL, Some(WPARAM(on as usize)), Some(LPARAM(-1)));
    selected_indices(h); // clears the headings
    let _ = InvalidateRect(Some(item(h, ID_LIST)), None, true);
    update_selection_buttons(h);
}

fn all_pinned(sel: &[usize]) -> bool {
    !sel.is_empty() && PINNED.with(|p| sel.iter().all(|i| p.borrow().get(*i).copied().unwrap_or(false)))
}

/// Keeps the Select All and Pin labels in step with the current selection.
unsafe fn update_selection_buttons(h: HWND) {
    let total = note_count();
    let sel = selected_indices(h);
    let all = total > 0 && sel.len() == total;
    let _ = SetWindowTextW(item(h, ID_SELECT_ALL), if all { w!("Select None") } else { w!("Select All") });
    let _ = SetWindowTextW(item(h, ID_PIN), if all_pinned(&sel) { w!("Unpin") } else { w!("Pin") });
    layout(h);
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
        (ID_LIST, LBN_SELCHANGE_CODE) => update_selection_buttons(h),
        (ID_SELECT_ALL, 0) => {
            let total = note_count();
            select_all(h, selected_indices(h).len() != total);
        }
        (ID_PIN, 0) => {
            let sel = selected_indices(h);
            if !sel.is_empty() {
                let pin = !all_pinned(&sel);
                for n in selected_ids(h) {
                    let _ = a.store.set_pinned(&n, pin);
                }
                a.request_sync(SyncCmd::Soon);
                refresh();
            }
        }
        (ID_PURGE, 0) => {
            let ids = selected_ids(h);
            if !ids.is_empty() {
                let msg = HSTRING::from(format!(
                    "Delete {} note{} forever?\n\nThey will also be deleted from Google Keep. This cannot be undone.",
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" }
                ));
                if MessageBoxW(Some(h), &msg, w!("Delete Forever"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES {
                    for n in &ids {
                        let _ = a.store.delete_forever(n);
                    }
                    a.request_sync(SyncCmd::Soon);
                    refresh();
                }
            }
        }
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
            let ids = selected_ids(h);
            if !ids.is_empty() {
                let archive = current_shelf(h) != Shelf::Archive;
                for n in &ids {
                    close_window_for(n);
                    let _ = a.store.set_archived(n, archive);
                }
                a.request_sync(SyncCmd::Soon);
                refresh();
            }
        }
        (ID_DELETE, 0) => {
            let ids = selected_ids(h);
            if !ids.is_empty() {
                let in_bin = current_shelf(h) == Shelf::Trash;
                for n in &ids {
                    if in_bin {
                        let _ = a.store.restore(n);
                    } else {
                        close_window_for(n);
                        let _ = a.store.trash(n);
                    }
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

/// macOS theme: white rounded panels behind the search box (a pill) and the list.
unsafe fn draw_panels(h: HWND, dc: HDC) {
    let (search, panel) = PANELS.with(|p| p.get());
    let t = app().theme.get();
    let brush = CreateSolidBrush(super::rgb(super::theme::field_bg(t)));
    let pen = CreatePen(PS_SOLID, 1, super::rgb(if t.is_dark() { 0x3A3A3C } else { 0xD2D2D7 }));
    let ob = SelectObject(dc, brush.into());
    let op = SelectObject(dc, pen.into());
    let pill = search.bottom - search.top;
    let _ = RoundRect(dc, search.left, search.top, search.right, search.bottom, pill, pill);
    let rad = form::scale(h, 20);
    let _ = RoundRect(dc, panel.left, panel.top, panel.right, panel.bottom, rad, rad);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());
}

extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(r) = super::theme::frame_message(h, msg, wp, lp) {
            return r;
        }
        if msg == WM_ERASEBKGND && super::theme::is_mac() && !app().hub.get().is_invalid() {
            super::theme::chrome_message(h, msg, wp, lp);
            draw_panels(h, HDC(wp.0 as *mut _));
            return LRESULT(1);
        }
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
            WM_VKEYTOITEM => {
                use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
                if (wp.0 & 0xFFFF) as u8 == b'A' && GetKeyState(VK_CONTROL.0 as i32) < 0 {
                    select_all(h, true);
                    return LRESULT(-2);
                }
                LRESULT(-1)
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

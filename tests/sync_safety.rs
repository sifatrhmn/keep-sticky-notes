//! Data-safety matrix: every test represents a way that a note could be lost.

use keepsticky::keep::{node_from_json, node_to_json, KeepBackend, KeepError, MockKeep};
use keepsticky::store::{Shelf, Store, ROOT, TYPE_ITEM, TYPE_LIST, TYPE_NOTE};
use keepsticky::sync::{SyncEngine, SyncError, CONFLICT_SUFFIX, KEY_BULK_APPROVED};
use serde_json::{json, Value};
use std::cell::Cell;

fn sync(store: &Store, server: &MockKeep) {
    SyncEngine { store, backend: server, allow_push: true }.sync_once().expect("sync");
}

fn new_store() -> Store {
    Store::open_memory().unwrap()
}

fn body_child(store: &Store, id: &str) -> String {
    store.children(id).unwrap()[0].id.clone()
}

fn make_synced_note(store: &Store, server: &MockKeep, title: &str, body: &str) -> String {
    let id = store.create_note(TYPE_NOTE).unwrap();
    store.set_title(&id, title).unwrap();
    store.set_body(&id, body).unwrap();
    sync(store, server);
    assert_eq!(store.dirty_count().unwrap(), 0);
    id
}

fn set_text(n: &mut Value, t: &str) {
    n["text"] = json!(t);
}

fn top_titles(store: &Store) -> Vec<String> {
    store.list_top(Shelf::Notes, "").unwrap().into_iter().map(|d| d.top.title).collect()
}

#[test]
fn new_note_reaches_server_with_all_fields() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = store.create_note(TYPE_NOTE).unwrap();
    store.set_title(&id, "Shopping").unwrap();
    store.set_body(&id, "Milk").unwrap();
    store.set_colour(&id, "TEAL").unwrap();
    store.set_pinned(&id, true).unwrap();
    sync(&store, &server);
    let top = server.node(&id).unwrap();
    assert_eq!(top["title"], "Shopping");
    assert_eq!(top["color"], "TEAL");
    assert_eq!(top["isPinned"], true);
    assert_eq!(top["parentId"], ROOT);
    assert_eq!(server.node(&body_child(&store, &id)).unwrap()["text"], "Milk");
    assert_eq!(store.dirty_count().unwrap(), 0);
}

#[test]
fn remote_note_is_downloaded() {
    let (store, server) = (new_store(), MockKeep::default());
    server.remote_insert(json!({"id":"t1","type":"NOTE","parentId":"root","title":"From phone","color":"PINK","isPinned":true,
        "timestamps":{"created":"2026-01-01T00:00:00.000000Z","updated":"2026-01-01T00:00:00.000000Z"}}));
    server.remote_insert(json!({"id":"c1","type":"LIST_ITEM","parentId":"t1","text":"Body","checked":false,
        "timestamps":{"updated":"2026-01-01T00:00:00.000000Z"}}));
    sync(&store, &server);
    let doc = store.doc("t1").unwrap().unwrap();
    assert_eq!(doc.top.title, "From phone");
    assert_eq!(doc.top.colour, "PINK");
    assert!(doc.top.pinned);
    assert_eq!(doc.body(), "Body");
}

#[test]
fn clean_local_note_accepts_remote_edit() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "old");
    let child = body_child(&store, &id);
    std::thread::sleep(std::time::Duration::from_millis(2));
    server.remote_edit(&child, |n| set_text(n, "new"));
    sync(&store, &server);
    assert_eq!(store.doc(&id).unwrap().unwrap().body(), "new");
    assert_eq!(top_titles(&store).len(), 1);
}

#[test]
fn stale_local_edit_never_overwrites_newer_remote() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "base");
    let child = body_child(&store, &id);
    store.set_body(&id, "local edit").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    server.remote_edit(&child, |n| set_text(n, "remote edit"));
    sync(&store, &server);

    assert_eq!(server.node(&child).unwrap()["text"], "remote edit", "server copy must survive");
    assert_eq!(store.doc(&id).unwrap().unwrap().body(), "remote edit");
    let copy = store.list_top(Shelf::Notes, "").unwrap().into_iter().find(|d| d.top.title.ends_with(CONFLICT_SUFFIX)).expect("conflict copy");
    assert_eq!(copy.body(), "local edit", "local text must survive");
    assert_eq!(copy.top.conflict_of.as_deref(), Some(id.as_str()));

    sync(&store, &server);
    assert!(server.node(&copy.top.id).is_some(), "conflict copy is uploaded");
    assert_eq!(store.dirty_count().unwrap(), 0);
}

#[test]
fn checklist_conflict_keeps_both_items() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = store.create_note(TYPE_LIST).unwrap();
    let item = store.add_item(&id, None, "Eggs").unwrap();
    sync(&store, &server);
    store.set_item_text(&item, "Eggs local").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    server.remote_edit(&item, |n| set_text(n, "Eggs remote"));
    sync(&store, &server);
    let texts: Vec<String> = store.doc(&id).unwrap().unwrap().items.into_iter().map(|i| i.text).collect();
    assert!(texts.contains(&"Eggs local".to_string()));
    assert!(texts.contains(&"Eggs remote".to_string()));
}

#[test]
fn non_overlapping_edits_on_different_items_both_apply() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = store.create_note(TYPE_LIST).unwrap();
    let a = store.add_item(&id, None, "A").unwrap();
    let b = store.add_item(&id, Some(&a), "B").unwrap();
    sync(&store, &server);
    store.set_item_checked(&a, true).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    server.remote_edit(&b, |n| set_text(n, "B2"));
    sync(&store, &server);
    assert_eq!(server.node(&a).unwrap()["checked"], true);
    assert_eq!(store.get(&b).unwrap().unwrap().text, "B2");
    assert_eq!(top_titles(&store).len(), 1);
}

/// Runs a hook just before the first request carrying nodes, i.e. between pull and push.
struct Interleave<'a, F: Fn()> {
    server: &'a MockKeep,
    hook: F,
    fired: Cell<bool>,
}

impl<F: Fn()> KeepBackend for Interleave<'_, F> {
    fn changes(&self, request: &Value) -> Result<Value, KeepError> {
        if !self.fired.get() && !request["nodes"].as_array().unwrap().is_empty() {
            self.fired.set(true);
            (self.hook)();
        }
        self.server.changes(request)
    }
}

#[test]
fn remote_edit_between_pull_and_push_preserves_both() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "base");
    let child = body_child(&store, &id);
    store.set_body(&id, "local").unwrap();
    let backend = Interleave {
        server: &server,
        hook: || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            server.remote_edit(&child, |r| set_text(r, "racing remote"));
        },
        fired: Cell::new(false),
    };
    SyncEngine { store: &store, backend: &backend, allow_push: true }.sync_once().unwrap();
    assert_eq!(server.node(&child).unwrap()["text"], "racing remote", "server copy must survive");
    let all: Vec<String> = store.list_top(Shelf::Notes, "").unwrap().into_iter().map(|d| d.body()).collect();
    assert!(all.contains(&"local".to_string()), "local preserved: {all:?}");
    assert!(all.contains(&"racing remote".to_string()), "remote preserved: {all:?}");
}

#[test]
fn edit_during_flight_stays_dirty() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "one");
    let child = body_child(&store, &id);
    store.set_body(&id, "two").unwrap();
    let backend = Interleave { server: &server, hook: || store.set_body(&id, "three").unwrap(), fired: Cell::new(false) };
    SyncEngine { store: &store, backend: &backend, allow_push: true }.sync_once().unwrap();
    assert_eq!(server.node(&child).unwrap()["text"], "two");
    assert_eq!(store.doc(&id).unwrap().unwrap().body(), "three", "newer typing must not be replaced");
    assert_eq!(store.dirty_count().unwrap(), 1);
    sync(&store, &server);
    assert_eq!(server.node(&child).unwrap()["text"], "three");
    assert_eq!(store.dirty_count().unwrap(), 0);
}

#[test]
fn remote_purge_of_edited_note_keeps_the_edit() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "Keep me", "base");
    let child = body_child(&store, &id);
    store.set_body(&id, "precious").unwrap();
    server.remote_purge(&child);
    server.remote_purge(&id);
    sync(&store, &server);
    let docs = store.list_top(Shelf::Notes, "").unwrap();
    assert!(docs.iter().any(|d| d.body() == "precious"), "edit must survive purge");
}

#[test]
fn remote_purge_of_clean_note_removes_it() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "Gone", "x");
    let child = body_child(&store, &id);
    server.remote_purge(&child);
    server.remote_purge(&id);
    sync(&store, &server);
    assert!(store.get(&id).unwrap().is_none());
    assert!(top_titles(&store).is_empty());
}

#[test]
fn local_trash_syncs_and_is_recoverable() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "Bin me", "x");
    store.trash(&id).unwrap();
    sync(&store, &server);
    assert!(server.node(&id).unwrap()["timestamps"]["trashed"].as_str().unwrap().starts_with("20"));
    assert_eq!(store.list_top(Shelf::Trash, "").unwrap().len(), 1);
    store.restore(&id).unwrap();
    sync(&store, &server);
    assert_eq!(server.node(&id).unwrap()["timestamps"]["trashed"], "1970-01-01T00:00:00.000000Z");
    assert_eq!(top_titles(&store).len(), 1);
}

#[test]
fn local_trash_yields_to_newer_remote_edit() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "Title", "x");
    store.trash(&id).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    server.remote_edit(&id, |n| n["title"] = json!("Renamed elsewhere"));
    sync(&store, &server);
    assert_eq!(top_titles(&store), vec!["Renamed elsewhere".to_string()]);
}

#[test]
fn offline_edits_are_retained_and_sent_later() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "one");
    store.set_body(&id, "offline").unwrap();
    *server.fail_with.lock().unwrap() = Some(KeepError::Network("offline".into()));
    assert!(matches!(SyncEngine { store: &store, backend: &server, allow_push: true }.sync_once(), Err(SyncError::Keep(_))));
    assert_eq!(store.dirty_count().unwrap(), 1);
    *server.fail_with.lock().unwrap() = None;
    sync(&store, &server);
    assert_eq!(store.dirty_count().unwrap(), 0);
    assert_eq!(server.node(&body_child(&store, &id)).unwrap()["text"], "offline");
}

#[test]
fn forced_resync_does_not_clear_dirty_rows() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "A", "one");
    store.set_body(&id, "unsent").unwrap();
    *server.force_resync_once.lock().unwrap() = true;
    sync(&store, &server);
    assert_eq!(store.doc(&id).unwrap().unwrap().body(), "unsent");
    assert_eq!(server.node(&body_child(&store, &id)).unwrap()["text"], "unsent");
}

#[test]
fn truncated_responses_are_followed_to_the_end() {
    let (store, server) = (new_store(), MockKeep::default());
    for i in 0..7 {
        server.remote_insert(json!({"id":format!("t{i}"),"type":"NOTE","parentId":"root","title":format!("N{i}"),
            "timestamps":{"updated":"2026-01-01T00:00:00.000000Z"}}));
    }
    *server.page_size.lock().unwrap() = Some(2);
    sync(&store, &server);
    assert_eq!(top_titles(&store).len(), 7);
}

#[test]
fn unknown_server_fields_survive_round_trip() {
    let (store, server) = (new_store(), MockKeep::default());
    server.remote_insert(json!({"id":"t","type":"NOTE","parentId":"root","title":"T","futureField":{"a":1},
        "annotationsGroup":{"kind":"notes#annotationsGroup","annotations":[{"webLink":{"url":"x"}}]},
        "timestamps":{"updated":"2026-01-01T00:00:00.000000Z"}}));
    sync(&store, &server);
    store.set_title("t", "T2").unwrap();
    sync(&store, &server);
    let n = server.node("t").unwrap();
    assert_eq!(n["title"], "T2");
    assert_eq!(n["futureField"]["a"], 1);
    assert_eq!(n["annotationsGroup"]["annotations"][0]["webLink"]["url"], "x");
}

#[test]
fn labels_are_added_and_removed_without_losing_history() {
    let (store, server) = (new_store(), MockKeep::default());
    server.remote_insert(json!({"id":"t","type":"NOTE","parentId":"root","title":"T",
        "labelIds":[{"labelId":"L1","deleted":"1970-01-01T00:00:00.000000Z"}],
        "timestamps":{"updated":"2026-01-01T00:00:00.000000Z"}}));
    sync(&store, &server);
    assert_eq!(store.get("t").unwrap().unwrap().label_ids, vec!["L1"]);
    store.set_label("t", "L1", false).unwrap();
    store.set_label("t", "L2", true).unwrap();
    sync(&store, &server);
    let entries = server.node("t").unwrap()["labelIds"].as_array().unwrap().clone();
    let get = |id: &str| entries.iter().find(|e| e["labelId"] == id).unwrap()["deleted"].as_str().unwrap().to_string();
    assert!(get("L1").starts_with("20"), "removed label carries a deletion time");
    assert_eq!(get("L2"), "1970-01-01T00:00:00.000000Z");
}

#[test]
fn checklist_order_and_ticks_round_trip() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = store.create_note(TYPE_LIST).unwrap();
    let a = store.add_item(&id, None, "A").unwrap();
    let c = store.add_item(&id, Some(&a), "C").unwrap();
    let _b = store.add_item(&id, Some(&a), "B").unwrap();
    store.set_item_checked(&c, true).unwrap();
    sync(&store, &server);
    let texts: Vec<String> = store.doc(&id).unwrap().unwrap().items.iter().map(|i| i.text.clone()).collect();
    assert_eq!(texts, vec!["A", "B", "C"]);
    assert_eq!(server.node(&c).unwrap()["checked"], true);
    assert_eq!(server.node(&c).unwrap()["type"], TYPE_ITEM);
    store.set_item_text(&c, "C2").unwrap();
    sync(&store, &server);
    assert_eq!(server.node(&c).unwrap()["parentServerId"], format!("srv-{id}"));
}

#[test]
fn convert_keeps_original_in_bin() {
    let (store, server) = (new_store(), MockKeep::default());
    let id = make_synced_note(&store, &server, "Lines", "a\nb");
    let new_id = store.convert(&id).unwrap().unwrap();
    let doc = store.doc(&new_id).unwrap().unwrap();
    assert!(doc.is_list());
    assert_eq!(doc.items.len(), 2);
    assert_eq!(store.list_top(Shelf::Trash, "").unwrap()[0].body(), "a\nb");
}

#[test]
fn wire_format_is_stable_through_parse() {
    let store = new_store();
    let id = store.create_note(TYPE_NOTE).unwrap();
    store.set_title(&id, "Round").unwrap();
    let n = store.get(&id).unwrap().unwrap();
    let parsed = node_from_json(&node_to_json(&n, None)).unwrap();
    assert_eq!(parsed.title, "Round");
    assert_eq!(parsed.sort_value, n.sort_value);
    assert_eq!(parsed.created, n.created);
}

#[test]
fn second_sync_is_idle() {
    let (store, server) = (new_store(), MockKeep::default());
    make_synced_note(&store, &server, "A", "x");
    server.requests.lock().unwrap().clear();
    sync(&store, &server);
    let reqs = server.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1, "only a pull");
    assert!(reqs[0]["nodes"].as_array().unwrap().is_empty());
}

#[test]
fn read_only_mode_never_sends_anything() {
    let (store, server) = (new_store(), MockKeep::default());
    server.remote_insert(json!({"id":"t","type":"NOTE","parentId":"root","title":"Real",
        "timestamps":{"updated":"2026-01-01T00:00:00.000000Z"}}));
    let id = store.create_note(TYPE_NOTE).unwrap();
    store.set_title(&id, "Local only").unwrap();
    let engine = SyncEngine { store: &store, backend: &server, allow_push: false };
    engine.sync_once().unwrap();
    store.set_title("t", "Changed here").unwrap();
    engine.sync_once().unwrap();
    assert!(server.requests.lock().unwrap().iter().all(|r| r["nodes"].as_array().unwrap().is_empty()));
    assert_eq!(server.node("t").unwrap()["title"], "Real");
    assert!(server.node(&id).is_none());
    assert_eq!(store.get(&id).unwrap().unwrap().title, "Local only");
    assert!(store.dirty_count().unwrap() >= 2, "edits wait for two-way sync");
}

fn bin_many(store: &Store, server: &MockKeep, n: usize) -> Vec<String> {
    let ids: Vec<String> = (0..n).map(|i| make_synced_note(store, server, &format!("N{i}"), "x")).collect();
    for id in &ids {
        store.trash(id).unwrap();
    }
    ids
}

#[test]
fn mass_delete_is_held_until_approved() {
    let (store, server) = (new_store(), MockKeep::default());
    let ids = bin_many(&store, &server, 6);
    let held = SyncEngine { store: &store, backend: &server, allow_push: true }.sync_once();
    assert!(matches!(held, Err(SyncError::BulkHeld { outgoing: true, .. })));
    assert!(server.node(&ids[0]).unwrap()["timestamps"].get("trashed").is_none(), "nothing sent");
    store.kv_set(KEY_BULK_APPROVED, "1").unwrap();
    sync(&store, &server);
    assert!(server.node(&ids[0]).unwrap()["timestamps"]["trashed"].as_str().unwrap().starts_with("20"));
    assert!(store.kv_get(KEY_BULK_APPROVED).unwrap().is_none(), "approval covers one sync only");
}

#[test]
fn a_few_deletes_are_not_interrupted() {
    let (store, server) = (new_store(), MockKeep::default());
    bin_many(&store, &server, 5);
    sync(&store, &server);
    assert_eq!(store.dirty_count().unwrap(), 0);
}

#[test]
fn mass_remote_removal_is_held_and_leaves_local_copies() {
    let (store, server) = (new_store(), MockKeep::default());
    let ids: Vec<String> = (0..6).map(|i| make_synced_note(&store, &server, &format!("N{i}"), "x")).collect();
    for id in &ids {
        server.remote_purge(id);
    }
    let held = SyncEngine { store: &store, backend: &server, allow_push: true }.sync_once();
    assert!(matches!(held, Err(SyncError::BulkHeld { outgoing: false, .. })));
    assert_eq!(top_titles(&store).len(), 6);
}

#[test]
fn backups_are_restorable_and_rotated() {
    let store = new_store();
    let id = store.create_note(TYPE_NOTE).unwrap();
    store.set_title(&id, "Saved").unwrap();
    let dir = std::env::temp_dir().join(format!("ksn-backup-{}", fastrand::u64(..)));
    let mut last = None;
    for _ in 0..4 {
        last = Some(store.backup_to(&dir, 2).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    let restored = Store::open(&last.unwrap()).unwrap();
    assert_eq!(restored.get(&id).unwrap().unwrap().title, "Saved");
    drop(restored);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn account_reset_clears_cursor_and_notes() {
    let (store, server) = (new_store(), MockKeep::default());
    make_synced_note(&store, &server, "Other account", "x");
    store.kv_set("two_way_sync", "1").unwrap();
    store.reset_account_data().unwrap();
    assert!(top_titles(&store).is_empty());
    assert!(store.kv_get("keep_to_version").unwrap().is_none());
    assert!(store.kv_get("two_way_sync").unwrap().is_none());
}

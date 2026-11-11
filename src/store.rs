//! Offline-first SQLite cache. Each row mirrors one Google Keep node.
//! `raw` keeps the last JSON received from the server, so that fields this application
//! does not understand are returned to the server untouched.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};

pub const ROOT: &str = "root";
pub const ZERO_TS: &str = "1970-01-01T00:00:00.000000Z";
pub const TYPE_NOTE: &str = "NOTE";
pub const TYPE_LIST: &str = "LIST";
pub const TYPE_ITEM: &str = "LIST_ITEM";
pub const SORT_DELTA: i64 = 10_000;

pub type DbResult<T> = rusqlite::Result<T>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub id: String,
    pub server_id: Option<String>,
    pub parent_id: String,
    pub node_type: String,
    pub title: String,
    pub text: String,
    pub checked: bool,
    pub sort_value: i64,
    pub colour: String,
    pub pinned: bool,
    pub archived: bool,
    pub created: String,
    pub updated: String,
    pub user_edited: String,
    pub trashed: String,
    pub deleted: String,
    pub server_updated: String,
    pub raw: String,
    pub label_ids: Vec<String>,
    pub dirty: bool,
    pub edit_seq: i64,
    pub conflict_of: Option<String>,
}

impl Node {
    pub fn is_top(&self) -> bool {
        self.parent_id == ROOT
    }
    pub fn is_deleted(&self) -> bool {
        ts_micros(&self.deleted) > 0
    }
    pub fn is_trashed(&self) -> bool {
        ts_micros(&self.trashed) > 0
    }
}

#[derive(Clone, Debug)]
pub struct NoteDoc {
    pub top: Node,
    pub items: Vec<Node>,
}

impl NoteDoc {
    pub fn is_list(&self) -> bool {
        self.top.node_type == TYPE_LIST
    }
    pub fn body(&self) -> String {
        self.items.first().map(|n| n.text.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub opacity: i32,
    pub topmost: bool,
    pub rolled: bool,
    pub open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelf {
    Notes,
    Archive,
    Trash,
}

pub fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Microseconds since the epoch. Empty or malformed values count as zero.
pub fn ts_micros(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).map(|d| d.timestamp_micros()).unwrap_or(0)
}

pub fn new_node_id() -> String {
    format!("{:x}.{:016x}", Utc::now().timestamp_millis(), fastrand::u64(..))
}

fn random_sort() -> i64 {
    fastrand::i64(1_000_000_000..=9_999_999_999)
}

pub struct Store {
    pub conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS nodes(
  id TEXT PRIMARY KEY, server_id TEXT, parent_id TEXT NOT NULL, node_type TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '', text TEXT NOT NULL DEFAULT '', checked INTEGER NOT NULL DEFAULT 0,
  sort_value INTEGER NOT NULL DEFAULT 0, colour TEXT NOT NULL DEFAULT 'DEFAULT',
  pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
  created TEXT NOT NULL DEFAULT '', updated TEXT NOT NULL DEFAULT '', user_edited TEXT NOT NULL DEFAULT '',
  trashed TEXT NOT NULL DEFAULT '', deleted TEXT NOT NULL DEFAULT '',
  server_updated TEXT NOT NULL DEFAULT '', raw TEXT NOT NULL DEFAULT '', label_ids TEXT NOT NULL DEFAULT '',
  dirty INTEGER NOT NULL DEFAULT 0, edit_seq INTEGER NOT NULL DEFAULT 0, conflict_of TEXT
);
CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id);
CREATE INDEX IF NOT EXISTS idx_nodes_dirty ON nodes(dirty);
CREATE TABLE IF NOT EXISTS labels(main_id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS window_state(
  note_id TEXT PRIMARY KEY, x INTEGER, y INTEGER, w INTEGER, h INTEGER,
  opacity INTEGER, topmost INTEGER, rolled INTEGER, open INTEGER
);
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
PRAGMA user_version = 1;
";

const COLUMNS: &str = "id,server_id,parent_id,node_type,title,text,checked,sort_value,colour,pinned,archived,\
created,updated,user_edited,trashed,deleted,server_updated,raw,label_ids,dirty,edit_seq,conflict_of";

fn row_node(r: &Row) -> DbResult<Node> {
    let labels: String = r.get(18)?;
    Ok(Node {
        id: r.get(0)?,
        server_id: r.get(1)?,
        parent_id: r.get(2)?,
        node_type: r.get(3)?,
        title: r.get(4)?,
        text: r.get(5)?,
        checked: r.get(6)?,
        sort_value: r.get(7)?,
        colour: r.get(8)?,
        pinned: r.get(9)?,
        archived: r.get(10)?,
        created: r.get(11)?,
        updated: r.get(12)?,
        user_edited: r.get(13)?,
        trashed: r.get(14)?,
        deleted: r.get(15)?,
        server_updated: r.get(16)?,
        raw: r.get(17)?,
        label_ids: labels.split('\n').filter(|s| !s.is_empty()).map(String::from).collect(),
        dirty: r.get(19)?,
        edit_seq: r.get(20)?,
        conflict_of: r.get(21)?,
    })
}

impl Store {
    pub fn open(path: &Path) -> DbResult<Store> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_memory() -> DbResult<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> DbResult<Store> {
        conn.busy_timeout(std::time::Duration::from_secs(8))?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    // ----- generic rows -----

    pub fn get(&self, id: &str) -> DbResult<Option<Node>> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM nodes WHERE id=?1"), [id], row_node)
            .optional()
    }

    pub fn upsert(&self, n: &Node) -> DbResult<()> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO nodes({COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)"
            ),
            params![
                n.id, n.server_id, n.parent_id, n.node_type, n.title, n.text, n.checked, n.sort_value, n.colour,
                n.pinned, n.archived, n.created, n.updated, n.user_edited, n.trashed, n.deleted, n.server_updated,
                n.raw, n.label_ids.join("\n"), n.dirty, n.edit_seq, n.conflict_of
            ],
        )?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> DbResult<()> {
        self.conn.execute("DELETE FROM nodes WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn children(&self, parent: &str) -> DbResult<Vec<Node>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM nodes WHERE parent_id=?1 ORDER BY sort_value DESC, id"
        ))?;
        let rows = st.query_map([parent], row_node)?;
        rows.collect()
    }

    pub fn dirty_nodes(&self) -> DbResult<Vec<Node>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM nodes WHERE dirty=1 ORDER BY (parent_id='root') DESC, id"
        ))?;
        let rows = st.query_map([], row_node)?;
        rows.collect()
    }

    pub fn dirty_count(&self) -> DbResult<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE dirty=1", [], |r| r.get(0))
    }

    /// Records a user edit on a node: the single route by which local changes become pushable.
    fn touch(&self, n: &mut Node) -> DbResult<()> {
        let now = now_ts();
        n.updated = now.clone();
        n.user_edited = now;
        n.dirty = true;
        n.edit_seq += 1;
        self.upsert(n)
    }

    fn new_node(parent: &str, node_type: &str) -> Node {
        let now = now_ts();
        Node {
            id: new_node_id(),
            parent_id: parent.into(),
            node_type: node_type.into(),
            sort_value: random_sort(),
            colour: "DEFAULT".into(),
            created: now.clone(),
            updated: now.clone(),
            user_edited: now,
            dirty: true,
            edit_seq: 1,
            ..Default::default()
        }
    }

    // ----- notes -----

    pub fn create_note(&self, node_type: &str) -> DbResult<String> {
        let top = Self::new_node(ROOT, node_type);
        self.upsert(&top)?;
        if node_type == TYPE_NOTE {
            self.upsert(&Self::new_node(&top.id, TYPE_ITEM))?;
        }
        Ok(top.id)
    }

    pub fn doc(&self, id: &str) -> DbResult<Option<NoteDoc>> {
        let Some(top) = self.get(id)? else { return Ok(None) };
        let items = self
            .children(id)?
            .into_iter()
            .filter(|n| n.node_type == TYPE_ITEM && !n.is_deleted())
            .collect();
        Ok(Some(NoteDoc { top, items }))
    }

    pub fn list_top(&self, shelf: Shelf, query: &str) -> DbResult<Vec<NoteDoc>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM nodes WHERE parent_id='root' AND node_type IN ('NOTE','LIST') ORDER BY pinned DESC, updated DESC"
        ))?;
        let tops: Vec<Node> = st.query_map([], row_node)?.collect::<DbResult<_>>()?;
        let needle = query.to_lowercase();
        let mut out = Vec::new();
        for top in tops {
            if top.is_deleted() {
                continue;
            }
            let on_shelf = match shelf {
                Shelf::Trash => top.is_trashed(),
                Shelf::Archive => top.archived && !top.is_trashed(),
                Shelf::Notes => !top.archived && !top.is_trashed(),
            };
            if !on_shelf {
                continue;
            }
            let Some(doc) = self.doc(&top.id)? else { continue };
            if !needle.is_empty() {
                let hit = doc.top.title.to_lowercase().contains(&needle)
                    || doc.items.iter().any(|i| i.text.to_lowercase().contains(&needle));
                if !hit {
                    continue;
                }
            }
            out.push(doc);
        }
        Ok(out)
    }

    fn update_top(&self, id: &str, f: impl FnOnce(&mut Node)) -> DbResult<()> {
        if let Some(mut n) = self.get(id)? {
            f(&mut n);
            self.touch(&mut n)?;
        }
        Ok(())
    }

    pub fn set_title(&self, id: &str, title: &str) -> DbResult<()> {
        self.update_top(id, |n| n.title = title.into())
    }
    pub fn set_colour(&self, id: &str, colour: &str) -> DbResult<()> {
        self.update_top(id, |n| n.colour = colour.into())
    }
    pub fn set_pinned(&self, id: &str, v: bool) -> DbResult<()> {
        self.update_top(id, |n| n.pinned = v)
    }
    pub fn set_archived(&self, id: &str, v: bool) -> DbResult<()> {
        self.update_top(id, |n| n.archived = v)
    }
    /// Moves a note to Google Keep's bin, from which it remains recoverable.
    pub fn trash(&self, id: &str) -> DbResult<()> {
        self.update_top(id, |n| n.trashed = now_ts())
    }
    pub fn restore(&self, id: &str) -> DbResult<()> {
        self.update_top(id, |n| n.trashed = String::new())
    }
    pub fn set_label(&self, id: &str, label: &str, on: bool) -> DbResult<()> {
        self.update_top(id, |n| {
            n.label_ids.retain(|l| l != label);
            if on {
                n.label_ids.push(label.into());
            }
        })
    }

    pub fn set_body(&self, id: &str, text: &str) -> DbResult<()> {
        let existing = self.children(id)?.into_iter().find(|n| n.node_type == TYPE_ITEM && !n.is_deleted());
        match existing {
            Some(mut item) => {
                if item.text != text {
                    item.text = text.into();
                    self.touch(&mut item)?;
                }
            }
            None => {
                let mut item = Self::new_node(id, TYPE_ITEM);
                item.text = text.into();
                self.upsert(&item)?;
            }
        }
        Ok(())
    }

    /// Adds a checklist item directly after `after`, or at the end of the list.
    pub fn add_item(&self, list: &str, after: Option<&str>, text: &str) -> DbResult<String> {
        let items: Vec<Node> = self.children(list)?.into_iter().filter(|n| !n.is_deleted()).collect();
        let sort = match after.and_then(|a| items.iter().position(|n| n.id == a)) {
            Some(pos) => match items.get(pos + 1) {
                Some(next) if items[pos].sort_value - next.sort_value >= 2 => (items[pos].sort_value + next.sort_value) / 2,
                Some(_) => {
                    // No gap remains, so reopen one by moving the items above upwards.
                    for it in items.iter().take(pos + 1) {
                        let mut it = it.clone();
                        it.sort_value += SORT_DELTA;
                        self.touch(&mut it)?;
                    }
                    items[pos].sort_value + SORT_DELTA / 2
                }
                None => items[pos].sort_value - SORT_DELTA,
            },
            None => items.last().map(|n| n.sort_value - SORT_DELTA).unwrap_or_else(random_sort),
        };
        let mut item = Self::new_node(list, TYPE_ITEM);
        item.text = text.into();
        item.sort_value = sort;
        self.upsert(&item)?;
        Ok(item.id)
    }

    pub fn set_item_text(&self, item: &str, text: &str) -> DbResult<()> {
        if let Some(mut n) = self.get(item)? {
            if n.text != text {
                n.text = text.into();
                self.touch(&mut n)?;
            }
        }
        Ok(())
    }

    pub fn set_item_checked(&self, item: &str, checked: bool) -> DbResult<()> {
        if let Some(mut n) = self.get(item)? {
            n.checked = checked;
            self.touch(&mut n)?;
        }
        Ok(())
    }

    pub fn delete_item(&self, item: &str) -> DbResult<()> {
        if let Some(mut n) = self.get(item)? {
            if n.raw.is_empty() {
                self.remove(item)?; // Never reached the server, so nothing needs a tombstone.
            } else {
                n.deleted = now_ts();
                self.touch(&mut n)?;
            }
        }
        Ok(())
    }

    /// Creates a new local note with the given content and returns its id.
    pub fn insert_copy(&self, source: &Node, title: &str, as_type: &str, lines: &[(String, bool)]) -> DbResult<String> {
        let mut top = Self::new_node(ROOT, as_type);
        top.title = title.into();
        top.colour = source.colour.clone();
        top.pinned = source.pinned;
        top.archived = source.archived;
        top.label_ids = source.label_ids.clone();
        self.upsert(&top)?;
        let mut sort = random_sort();
        for (text, checked) in lines {
            let mut item = Self::new_node(&top.id, TYPE_ITEM);
            item.text = text.clone();
            item.checked = *checked;
            item.sort_value = sort;
            sort -= SORT_DELTA;
            self.upsert(&item)?;
        }
        Ok(top.id)
    }

    /// Converts between a text note and a checklist. The server's handling of an in-place
    /// type change is undocumented, so a new note is made and the original goes to the bin.
    pub fn convert(&self, id: &str) -> DbResult<Option<String>> {
        let Some(doc) = self.doc(id)? else { return Ok(None) };
        let (new_type, lines): (&str, Vec<(String, bool)>) = if doc.is_list() {
            (TYPE_NOTE, vec![(doc.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("\n"), false)])
        } else {
            let body = doc.body();
            let parts: Vec<(String, bool)> = body.split('\n').map(|l| (l.to_string(), false)).collect();
            (TYPE_LIST, parts)
        };
        let new_id = self.insert_copy(&doc.top, &doc.top.title, new_type, &lines)?;
        self.trash(id)?;
        Ok(Some(new_id))
    }

    // ----- labels -----

    pub fn replace_labels(&self, labels: &[(String, String)]) -> DbResult<()> {
        self.conn.execute("DELETE FROM labels", [])?;
        for (id, name) in labels {
            self.conn.execute("INSERT OR REPLACE INTO labels(main_id,name) VALUES(?1,?2)", params![id, name])?;
        }
        Ok(())
    }

    pub fn labels(&self) -> DbResult<Vec<(String, String)>> {
        let mut st = self.conn.prepare("SELECT main_id,name FROM labels ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    // ----- window state -----

    pub fn window_state(&self, id: &str) -> DbResult<Option<WindowState>> {
        self.conn
            .query_row(
                "SELECT x,y,w,h,opacity,topmost,rolled,open FROM window_state WHERE note_id=?1",
                [id],
                |r| {
                    Ok(WindowState {
                        x: r.get(0)?,
                        y: r.get(1)?,
                        w: r.get(2)?,
                        h: r.get(3)?,
                        opacity: r.get(4)?,
                        topmost: r.get(5)?,
                        rolled: r.get(6)?,
                        open: r.get(7)?,
                    })
                },
            )
            .optional()
    }

    pub fn save_window_state(&self, id: &str, s: &WindowState) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO window_state(note_id,x,y,w,h,opacity,topmost,rolled,open) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, s.x, s.y, s.w, s.h, s.opacity, s.topmost, s.rolled, s.open],
        )?;
        Ok(())
    }

    pub fn open_note_ids(&self) -> DbResult<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT w.note_id FROM window_state w JOIN nodes n ON n.id=w.note_id WHERE w.open=1 AND n.deleted='' AND n.trashed=''",
        )?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    // ----- safety -----

    /// Writes a consistent copy of the database and keeps only the newest `keep` copies.
    pub fn backup_to(&self, dir: &Path, keep: usize) -> DbResult<PathBuf> {
        let _ = std::fs::create_dir_all(dir);
        let stamp = Utc::now().format("%Y%m%d-%H%M%S-%6f");
        let path = dir.join(format!("notes-{stamp}.db"));
        self.conn.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
        let mut copies: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "db")).collect())
            .unwrap_or_default();
        copies.sort();
        let excess = copies.len().saturating_sub(keep);
        for old in copies.into_iter().take(excess) {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }

    /// Clears everything tied to one Google account. Callers take a backup first.
    pub fn reset_account_data(&self) -> DbResult<()> {
        self.conn.execute_batch(
            "BEGIN; DELETE FROM nodes; DELETE FROM labels; DELETE FROM window_state;
             DELETE FROM kv WHERE key IN ('keep_to_version','two_way_sync','bulk_approved','bound_email'); COMMIT;",
        )
    }

    pub fn server_known_count(&self) -> DbResult<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE raw<>''", [], |r| r.get(0))
    }

    // ----- key/value -----

    pub fn kv_get(&self, key: &str) -> DbResult<Option<String>> {
        self.conn.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0)).optional()
    }

    pub fn kv_set(&self, key: &str, value: &str) -> DbResult<()> {
        self.conn.execute("INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)", params![key, value])?;
        Ok(())
    }

    pub fn kv_del(&self, key: &str) -> DbResult<()> {
        self.conn.execute("DELETE FROM kv WHERE key=?1", [key])?;
        Ok(())
    }
}

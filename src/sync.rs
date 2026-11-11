//! Two-way synchronisation. The governing rule is that no version of a note is ever
//! discarded silently: where both sides changed, the remote version stays in place and
//! the local version is preserved as a conflict copy or a new checklist item.

use crate::keep::{changes_request, is_server_deleted, node_from_json, node_to_json, KeepBackend, KeepError};
use crate::store::{ts_micros, DbResult, Node, Store, ROOT, TYPE_ITEM, TYPE_LIST, TYPE_NOTE};
use serde_json::Value;
use std::collections::HashMap;

const BATCH: usize = 100;
const MAX_PAGES: usize = 500;
const VERSION_KEY: &str = "keep_to_version";
pub const CONFLICT_SUFFIX: &str = " (conflict copy)";
pub const KEY_TWO_WAY: &str = "two_way_sync";
pub const KEY_BULK_APPROVED: &str = "bulk_approved";
/// Above these limits a sync stops and asks the person first.
const MAX_REMOVALS: usize = 5;
const MIN_MODIFY_LIMIT: usize = 20;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
    pub changed_notes: bool,
}

#[derive(Debug)]
pub enum SyncError {
    Keep(KeepError),
    Db(rusqlite::Error),
    /// Google asked for a newer client; the protocol may have moved on.
    UpgradeRecommended,
    /// An unusually large change was stopped before anything was applied.
    BulkHeld { outgoing: bool, count: usize },
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SyncError::Keep(e) => write!(f, "{e}"),
            SyncError::Db(e) => write!(f, "Local database error: {e}"),
            SyncError::UpgradeRecommended => write!(f, "Google Keep has requested a newer client version"),
            SyncError::BulkHeld { outgoing: true, count } => write!(f, "Paused before sending {count} changes to Google Keep"),
            SyncError::BulkHeld { outgoing: false, count } => write!(f, "Paused before removing {count} notes from this computer"),
        }
    }
}

impl From<KeepError> for SyncError {
    fn from(e: KeepError) -> Self {
        SyncError::Keep(e)
    }
}
impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError::Db(e)
    }
}

pub struct SyncEngine<'a, B: KeepBackend> {
    pub store: &'a Store,
    pub backend: &'a B,
    /// False during the read-only stage: nothing at all is sent to Google Keep.
    pub allow_push: bool,
}

impl<'a, B: KeepBackend> SyncEngine<'a, B> {
    pub fn sync_once(&self) -> Result<SyncReport, SyncError> {
        let mut report = SyncReport::default();
        self.pull(&mut report)?; // Always learn of remote changes before sending local ones.
        if self.allow_push {
            self.push(&mut report)?;
        }
        self.store.kv_del(KEY_BULK_APPROVED)?;
        Ok(report)
    }

    fn pull(&self, report: &mut SyncReport) -> Result<(), SyncError> {
        let mut resynced = false;
        for _ in 0..MAX_PAGES {
            let version = self.store.kv_get(VERSION_KEY)?;
            let resp = self.backend.changes(&changes_request(Vec::new(), version.as_deref()))?;
            if resp.get("forceFullResync").and_then(Value::as_bool).unwrap_or(false) {
                if resynced {
                    return Err(KeepError::Protocol("repeated full resynchronisation request".into()).into());
                }
                // Dirty rows are deliberately retained; only the version cursor is reset.
                self.store.kv_del(VERSION_KEY)?;
                resynced = true;
                continue;
            }
            if resp.get("upgradeRecommended").and_then(Value::as_bool).unwrap_or(false) {
                return Err(SyncError::UpgradeRecommended);
            }
            self.check_incoming(&resp)?;
            self.apply_response(&resp, &HashMap::new(), report)?;
            if !resp.get("truncated").and_then(Value::as_bool).unwrap_or(false) {
                return Ok(());
            }
        }
        Err(KeepError::Protocol("too many pages".into()).into())
    }

    fn bulk_approved(&self) -> DbResult<bool> {
        Ok(self.store.kv_get(KEY_BULK_APPROVED)?.as_deref() == Some("1"))
    }

    fn check_incoming(&self, resp: &Value) -> Result<(), SyncError> {
        let mut removals = 0;
        for raw in resp.get("nodes").and_then(Value::as_array).into_iter().flatten() {
            if !is_server_deleted(raw) {
                continue;
            }
            if let Some(id) = raw.get("id").and_then(Value::as_str) {
                if self.store.get(id)?.is_some_and(|n| n.is_top()) {
                    removals += 1;
                }
            }
        }
        if removals > MAX_REMOVALS && !self.bulk_approved()? {
            return Err(SyncError::BulkHeld { outgoing: false, count: removals });
        }
        Ok(())
    }

    fn check_outgoing(&self, dirty: &[Node]) -> Result<(), SyncError> {
        let removals = dirty.iter().filter(|n| n.is_top() && (n.is_trashed() || n.is_deleted())).count();
        let modified = dirty.iter().filter(|n| !n.raw.is_empty()).count();
        let limit = MIN_MODIFY_LIMIT.max(self.store.server_known_count()? as usize / 4);
        if (removals > MAX_REMOVALS || modified > limit) && !self.bulk_approved()? {
            return Err(SyncError::BulkHeld { outgoing: true, count: modified.max(removals) });
        }
        Ok(())
    }

    fn push(&self, report: &mut SyncReport) -> Result<(), SyncError> {
        let dirty = self.store.dirty_nodes()?;
        if dirty.is_empty() {
            return Ok(());
        }
        self.check_outgoing(&dirty)?;
        for chunk in dirty.chunks(BATCH) {
            let mut payload = Vec::with_capacity(chunk.len());
            let mut sent = HashMap::new();
            for node in chunk {
                let parent_sid = if node.parent_id == ROOT {
                    None
                } else {
                    self.store.get(&node.parent_id)?.and_then(|p| p.server_id)
                };
                payload.push(node_to_json(node, parent_sid.as_deref()));
                sent.insert(node.id.clone(), node.clone());
            }
            let version = self.store.kv_get(VERSION_KEY)?;
            let resp = self.backend.changes(&changes_request(payload, version.as_deref()))?;
            if resp.get("forceFullResync").and_then(Value::as_bool).unwrap_or(false) {
                self.store.kv_del(VERSION_KEY)?;
                return Ok(()); // Rows remain dirty and are retried after the next pull.
            }
            self.apply_response(&resp, &sent, report)?;
            report.pushed += chunk.len();
        }
        Ok(())
    }

    fn apply_response(&self, resp: &Value, sent: &HashMap<String, Node>, report: &mut SyncReport) -> DbResult<()> {
        let tx = self.store.conn.unchecked_transaction()?;
        if let Some(labels) = resp.pointer("/userInfo/labels").and_then(Value::as_array) {
            let parsed: Vec<(String, String)> = labels
                .iter()
                .filter(|l| ts_micros(l.pointer("/timestamps/deleted").and_then(Value::as_str).unwrap_or("")) <= 0)
                .filter_map(|l| Some((l.get("mainId")?.as_str()?.to_string(), l.get("name")?.as_str()?.to_string())))
                .collect();
            self.store.replace_labels(&parsed)?;
        }
        for raw in resp.get("nodes").and_then(Value::as_array).into_iter().flatten() {
            self.apply_node(raw, sent, report)?;
        }
        if let Some(v) = resp.get("toVersion").and_then(Value::as_str) {
            self.store.kv_set(VERSION_KEY, v)?;
        }
        tx.commit()
    }

    fn apply_node(&self, raw: &Value, sent: &HashMap<String, Node>, report: &mut SyncReport) -> DbResult<()> {
        let Some(remote) = node_from_json(raw) else { return Ok(()) };
        let gone = is_server_deleted(raw);
        let merge_conflict = raw.get("mergeConflict").is_some();
        report.pulled += 1;
        report.changed_notes = true;

        let Some(local) = self.store.get(&remote.id)? else {
            if !gone {
                self.store.upsert(&remote)?;
            }
            return Ok(());
        };

        // Only a response matching what this request sent counts as its acknowledgement.
        // Anything else is another device's change and must go through conflict handling.
        if let Some(snapshot) = sent.get(&remote.id).filter(|_| !merge_conflict) {
            let acknowledged = if gone { snapshot.is_deleted() } else { same_content(snapshot, &remote) };
            if acknowledged {
                if local.edit_seq == snapshot.edit_seq {
                    if gone {
                        self.store.remove(&local.id)?;
                    } else {
                        self.store.upsert(&Node { edit_seq: local.edit_seq, conflict_of: local.conflict_of.clone(), ..remote })?;
                    }
                } else {
                    // Edited again while the request was in flight: keep the newer text and stay dirty.
                    self.store.upsert(&Node {
                        server_id: remote.server_id.clone(),
                        server_updated: remote.server_updated.clone(),
                        raw: remote.raw.clone(),
                        ..local
                    })?;
                }
                return Ok(());
            }
        }

        if gone {
            return self.apply_remote_deletion(&local, report);
        }
        if !local.dirty {
            return self.store.upsert(&Node { edit_seq: local.edit_seq, conflict_of: local.conflict_of.clone(), ..remote });
        }
        if same_content(&local, &remote) {
            return self.store.upsert(&Node { edit_seq: local.edit_seq, conflict_of: local.conflict_of.clone(), ..remote });
        }
        if !merge_conflict && ts_micros(&remote.updated) <= ts_micros(&local.server_updated) {
            // Version bump with no newer content, so the local edit remains valid.
            return self.store.upsert(&Node { server_id: remote.server_id.clone(), raw: remote.raw.clone(), ..local });
        }
        self.resolve_conflict(local, remote, report)
    }

    /// Both sides changed. The remote version takes the node and the local version is preserved.
    fn resolve_conflict(&self, local: Node, remote: Node, report: &mut SyncReport) -> DbResult<()> {
        let clean = Node { edit_seq: local.edit_seq, conflict_of: local.conflict_of.clone(), ..remote.clone() };
        if local.is_deleted() || local.is_trashed() && !remote.is_trashed() {
            return self.store.upsert(&clean); // A local removal yields to newer remote content.
        }
        if local.node_type == TYPE_ITEM {
            let parent = self.store.get(&local.parent_id)?;
            let in_list = parent.as_ref().map(|p| p.node_type == TYPE_LIST).unwrap_or(false);
            if local.text != remote.text {
                report.conflicts += 1;
                if in_list {
                    self.store.upsert(&clean)?;
                    self.store.add_item(&local.parent_id, Some(&local.id), &local.text)?;
                    return Ok(());
                }
                if let Some(parent) = parent {
                    let title = format!("{}{}", parent.title, CONFLICT_SUFFIX);
                    let copy = self.store.insert_copy(&parent, &title, TYPE_NOTE, &[(local.text.clone(), false)])?;
                    self.mark_conflict(&copy, &parent.id)?;
                }
                return self.store.upsert(&clean);
            }
            // Same text, differing tick state: the newer remote state applies.
            return self.store.upsert(&clean);
        }
        if local.parent_id == ROOT && local.title != remote.title && !local.title.is_empty() {
            report.conflicts += 1;
            let lines = self.current_lines(&local.id)?;
            let title = format!("{}{}", local.title, CONFLICT_SUFFIX);
            let copy = self.store.insert_copy(&local, &title, &local.node_type, &lines)?;
            self.mark_conflict(&copy, &local.id)?;
        }
        self.store.upsert(&clean)
    }

    fn current_lines(&self, top: &str) -> DbResult<Vec<(String, bool)>> {
        Ok(self
            .store
            .children(top)?
            .into_iter()
            .filter(|c| c.node_type == TYPE_ITEM && !c.is_deleted())
            .map(|c| (c.text, c.checked))
            .collect())
    }

    fn mark_conflict(&self, copy: &str, original: &str) -> DbResult<()> {
        if let Some(mut n) = self.store.get(copy)? {
            n.conflict_of = Some(original.to_string());
            self.store.upsert(&n)?;
        }
        Ok(())
    }

    fn apply_remote_deletion(&self, local: &Node, report: &mut SyncReport) -> DbResult<()> {
        if local.parent_id == ROOT {
            let children = self.store.children(&local.id)?;
            let unsaved = (local.dirty && !local.is_deleted()) || children.iter().any(|c| c.dirty && !c.is_deleted());
            if unsaved {
                // Purged elsewhere while edited here, so the edits live on as a new note.
                report.conflicts += 1;
                let lines = self.current_lines(&local.id)?;
                self.store.insert_copy(local, &local.title, &local.node_type, &lines)?;
            }
            for c in children {
                self.store.remove(&c.id)?;
            }
            return self.store.remove(&local.id);
        }
        if local.dirty && !local.is_deleted() && !local.text.is_empty() {
            report.conflicts += 1;
            self.store.add_item(&local.parent_id, None, &local.text)?;
        }
        self.store.remove(&local.id)
    }
}

fn sorted(v: &[String]) -> Vec<&String> {
    let mut out: Vec<&String> = v.iter().collect();
    out.sort();
    out
}

fn same_content(a: &Node, b: &Node) -> bool {
    a.title == b.title
        && a.text == b.text
        && a.checked == b.checked
        && a.colour == b.colour
        && a.pinned == b.pinned
        && a.archived == b.archived
        && sorted(&a.label_ids) == sorted(&b.label_ids)
        && a.is_trashed() == b.is_trashed()
        && a.is_deleted() == b.is_deleted()
}

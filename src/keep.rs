//! Google Keep wire format and backends.
//! The protocol is unofficial; everything specific to it is confined to this module and `auth`.

use crate::auth::{self, Account};
use crate::store::{now_ts, ts_micros, Node, ROOT, TYPE_ITEM, ZERO_TS};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API_URL: &str = "https://www.googleapis.com/notes/v1/changes";

#[derive(Debug, Clone, PartialEq)]
pub enum KeepError {
    /// The sign-in is no longer valid and the person must sign in again.
    Auth(String),
    Network(String),
    Http(u16),
    Protocol(String),
}

impl std::fmt::Display for KeepError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            KeepError::Auth(m) => write!(f, "Sign-in required: {m}"),
            KeepError::Network(m) => write!(f, "Network problem: {m}"),
            KeepError::Http(c) => write!(f, "Google Keep returned HTTP {c}"),
            KeepError::Protocol(m) => write!(f, "Unexpected response: {m}"),
        }
    }
}

pub trait KeepBackend {
    fn changes(&self, request: &Value) -> Result<Value, KeepError>;
}

pub fn changes_request(nodes: Vec<Value>, target_version: Option<&str>) -> Value {
    let session = format!("s--{}--{:010}", chrono::Utc::now().timestamp_millis(), fastrand::u64(0..10_000_000_000));
    let capabilities: Vec<Value> =
        ["NC", "PI", "LB", "AN", "SH", "DR", "TR", "IN", "SNB", "MI", "CO"].iter().map(|t| json!({"type": t})).collect();
    let mut req = json!({
        "nodes": nodes,
        "clientTimestamp": now_ts(),
        "requestHeader": {
            "clientSessionId": session,
            "clientPlatform": "ANDROID",
            "clientVersion": {"major": "9", "minor": "9", "build": "9", "revision": "9"},
            "capabilities": capabilities,
        },
    });
    if let Some(v) = target_version {
        req["targetVersion"] = json!(v);
    }
    req
}

fn active_label(entry: &Value) -> bool {
    entry.get("deleted").and_then(Value::as_str).map(|d| ts_micros(d) <= 0).unwrap_or(true)
}

/// Serialises a local node over its last-known server JSON, retaining unrecognised fields.
pub fn node_to_json(n: &Node, parent_server_id: Option<&str>) -> Value {
    let mut o: Map<String, Value> =
        if n.raw.is_empty() { Map::new() } else { serde_json::from_str(&n.raw).unwrap_or_default() };
    o.remove("mergeConflict");
    if o.remove("moved").is_some() {
        o.remove("baseVersion");
    }
    o.insert("id".into(), json!(n.id));
    o.insert("kind".into(), json!("notes#node"));
    o.insert("type".into(), json!(n.node_type));
    o.insert("parentId".into(), json!(n.parent_id));
    o.insert("sortValue".into(), json!(n.sort_value));
    if let Some(sid) = &n.server_id {
        o.insert("serverId".into(), json!(sid));
    }
    if !o.contains_key("text") || n.node_type == TYPE_ITEM {
        o.insert("text".into(), json!(n.text));
    }

    let ts = o.entry("timestamps").or_insert_with(|| json!({"kind": "notes#timestamps"}));
    ts["created"] = json!(n.created);
    ts["updated"] = json!(n.updated);
    ts["userEdited"] = json!(n.user_edited);
    if ts_micros(&n.trashed) > 0 {
        ts["trashed"] = json!(n.trashed);
    } else if ts.get("trashed").is_some() {
        ts["trashed"] = json!(ZERO_TS);
    }
    if ts_micros(&n.deleted) > 0 {
        ts["deleted"] = json!(n.deleted);
    }

    o.entry("nodeSettings").or_insert_with(|| {
        json!({"newListItemPlacement": "BOTTOM", "graveyardState": "COLLAPSED", "checkedListItemsPolicy": "GRAVEYARD"})
    });
    o.entry("annotationsGroup").or_insert_with(|| json!({"kind": "notes#annotationsGroup", "annotations": []}));

    if n.parent_id == ROOT {
        o.insert("title".into(), json!(n.title));
        o.insert("color".into(), json!(n.colour));
        o.insert("isPinned".into(), json!(n.pinned));
        o.insert("isArchived".into(), json!(n.archived));
        o.entry("collaborators").or_insert_with(|| json!([]));

        let mut entries: Vec<Value> = o.get("labelIds").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut seen = HashSet::new();
        for e in entries.iter_mut() {
            let id = e.get("labelId").and_then(Value::as_str).unwrap_or("").to_string();
            let wanted = n.label_ids.contains(&id);
            if wanted && !active_label(e) {
                e["deleted"] = json!(ZERO_TS);
            } else if !wanted && active_label(e) {
                e["deleted"] = json!(now_ts());
            }
            seen.insert(id);
        }
        for id in n.label_ids.iter().filter(|i| !seen.contains(*i)) {
            entries.push(json!({"labelId": id, "deleted": ZERO_TS}));
        }
        if !entries.is_empty() {
            o.insert("labelIds".into(), Value::Array(entries));
        }
    }
    if n.node_type == TYPE_ITEM {
        o.insert("checked".into(), json!(n.checked));
        o.insert("parentServerId".into(), parent_server_id.map(|s| json!(s)).unwrap_or(Value::Null));
        o.entry("superListItemId").or_insert(Value::Null);
    }
    Value::Object(o)
}

/// True when the server reports this node as permanently gone.
pub fn is_server_deleted(raw: &Value) -> bool {
    raw.get("parentId").is_none() || ts_micros(raw.pointer("/timestamps/deleted").and_then(Value::as_str).unwrap_or("")) > 0
}

pub fn node_from_json(raw: &Value) -> Option<Node> {
    let id = raw.get("id")?.as_str()?.to_string();
    let s = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ts = |k: &str| raw.pointer(&format!("/timestamps/{k}")).and_then(Value::as_str).unwrap_or("").to_string();
    let sort_value = match raw.get("sortValue") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(v)) => v.parse().unwrap_or(0),
        _ => 0,
    };
    let label_ids = raw
        .get("labelIds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|e| active_label(e))
                .filter_map(|e| e.get("labelId").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let colour = raw.get("color").and_then(Value::as_str).unwrap_or("DEFAULT").to_string();
    let trashed = ts("trashed");
    let deleted = ts("deleted");
    Some(Node {
        id,
        server_id: raw.get("serverId").and_then(Value::as_str).map(String::from),
        parent_id: s("parentId"),
        node_type: s("type"),
        title: s("title"),
        text: s("text"),
        checked: raw.get("checked").and_then(Value::as_bool).unwrap_or(false),
        sort_value,
        colour,
        pinned: raw.get("isPinned").and_then(Value::as_bool).unwrap_or(false),
        archived: raw.get("isArchived").and_then(Value::as_bool).unwrap_or(false),
        created: ts("created"),
        updated: ts("updated"),
        user_edited: ts("userEdited"),
        trashed: if ts_micros(&trashed) > 0 { trashed } else { String::new() },
        deleted: if ts_micros(&deleted) > 0 { deleted } else { String::new() },
        server_updated: ts("updated"),
        raw: raw.to_string(),
        label_ids,
        dirty: false,
        edit_seq: 0,
        conflict_of: None,
    })
}

// ----- live backend -----

pub struct UnofficialKeep {
    account: Account,
    token: Mutex<Option<(String, Instant)>>,
    agent: ureq::Agent,
}

impl UnofficialKeep {
    pub fn new(account: Account) -> Result<Self, KeepError> {
        Ok(Self { account, token: Mutex::new(None), agent: auth::http_agent()? })
    }

    fn access_token(&self, force: bool) -> Result<String, KeepError> {
        let mut guard = self.token.lock().unwrap();
        if !force {
            if let Some((t, expiry)) = guard.as_ref() {
                if Instant::now() < *expiry {
                    return Ok(t.clone());
                }
            }
        }
        let (token, lifetime) = auth::fetch_access_token(&self.agent, &self.account)?;
        let margin = Duration::from_secs(120);
        *guard = Some((token.clone(), Instant::now() + lifetime.saturating_sub(margin)));
        Ok(token)
    }
}

impl KeepBackend for UnofficialKeep {
    fn changes(&self, request: &Value) -> Result<Value, KeepError> {
        let mut force = false;
        for _ in 0..3 {
            let token = self.access_token(force)?;
            let result = self
                .agent
                .post(API_URL)
                .set("Authorization", &format!("OAuth {token}"))
                .set("User-Agent", concat!("KeepStickyNotes/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(60))
                .send_json(request);
            match result {
                Ok(resp) => return resp.into_json().map_err(|e| KeepError::Protocol(e.to_string())),
                Err(ureq::Error::Status(401, _)) => force = true,
                Err(ureq::Error::Status(code, _)) => return Err(KeepError::Http(code)),
                Err(e) => return Err(KeepError::Network(e.to_string())),
            }
        }
        Err(KeepError::Auth("Google rejected the access token".into()))
    }
}

// ----- in-memory server for tests -----

#[derive(Default)]
struct MockState {
    version: u64,
    nodes: HashMap<String, Value>,
    changed_at: HashMap<String, u64>,
}

/// Models the behaviours that matter for data safety: versioning, conflict
/// detection, truncated responses and forced resynchronisation.
#[derive(Default)]
pub struct MockKeep {
    state: Mutex<MockState>,
    pub page_size: Mutex<Option<usize>>,
    pub force_resync_once: Mutex<bool>,
    pub fail_with: Mutex<Option<KeepError>>,
    pub requests: Mutex<Vec<Value>>,
}

impl MockKeep {
    pub fn node(&self, id: &str) -> Option<Value> {
        self.state.lock().unwrap().nodes.get(id).cloned()
    }

    pub fn all_nodes(&self) -> Vec<Value> {
        self.state.lock().unwrap().nodes.values().cloned().collect()
    }

    /// Applies an edit as if another device had made it.
    pub fn remote_edit(&self, id: &str, f: impl FnOnce(&mut Value)) {
        let mut s = self.state.lock().unwrap();
        s.version += 1;
        let v = s.version;
        if let Some(n) = s.nodes.get_mut(id) {
            f(n);
            n["timestamps"]["updated"] = json!(now_ts());
            n["baseVersion"] = json!(v.to_string());
        }
        s.changed_at.insert(id.to_string(), v);
    }

    pub fn remote_insert(&self, node: Value) {
        let mut s = self.state.lock().unwrap();
        s.version += 1;
        let v = s.version;
        let id = node["id"].as_str().unwrap().to_string();
        let mut node = node;
        node["baseVersion"] = json!(v.to_string());
        s.nodes.insert(id.clone(), node);
        s.changed_at.insert(id, v);
    }

    /// Permanently removes a node, reporting it without `parentId` as the server does.
    pub fn remote_purge(&self, id: &str) {
        self.remote_edit(id, |n| {
            n.as_object_mut().unwrap().remove("parentId");
        });
    }
}

impl KeepBackend for MockKeep {
    fn changes(&self, request: &Value) -> Result<Value, KeepError> {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(e) = self.fail_with.lock().unwrap().clone() {
            return Err(e);
        }
        if std::mem::take(&mut *self.force_resync_once.lock().unwrap()) {
            return Ok(json!({"forceFullResync": true}));
        }
        let mut s = self.state.lock().unwrap();
        let target: u64 = request.get("targetVersion").and_then(Value::as_str).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut conflicts = HashSet::new();
        for incoming in request["nodes"].as_array().cloned().unwrap_or_default() {
            let id = incoming["id"].as_str().unwrap_or("").to_string();
            let current_version = s.nodes.get(&id).and_then(|n| n.get("baseVersion").cloned());
            if current_version.is_some() && incoming.get("baseVersion") != current_version.as_ref() {
                conflicts.insert(id);
                continue;
            }
            s.version += 1;
            let v = s.version;
            let mut stored = incoming;
            stored["baseVersion"] = json!(v.to_string());
            if stored.get("serverId").is_none() {
                stored["serverId"] = json!(format!("srv-{id}"));
            }
            s.nodes.insert(id.clone(), stored);
            s.changed_at.insert(id, v);
        }
        let mut changed: Vec<(u64, String)> =
            s.changed_at.iter().filter(|(_, v)| **v > target).map(|(k, v)| (*v, k.clone())).collect();
        changed.sort();
        let limit = self.page_size.lock().unwrap().unwrap_or(usize::MAX);
        let truncated = changed.len() > limit;
        changed.truncate(limit);
        let to_version = if truncated { changed.last().map(|c| c.0).unwrap_or(target) } else { s.version };
        let mut nodes: Vec<Value> = changed.iter().map(|(_, id)| s.nodes[id].clone()).collect();
        for id in &conflicts {
            let mut n = s.nodes[id].clone();
            n["mergeConflict"] = json!(true);
            nodes.push(n);
        }
        Ok(json!({"nodes": nodes, "toVersion": to_version.to_string(), "truncated": truncated}))
    }
}

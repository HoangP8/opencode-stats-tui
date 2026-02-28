//! Statistics collection from opencode storage.

use chrono::Timelike;
use rayon::prelude::*;
use rusqlite::{params, Connection, OpenFlags};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::env;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const MAX_CHARS_PER_TEXT_PART: usize = 2000;
const DB_MESSAGE_PREFIX: &str = "db://message/";

static HOME_DIR: OnceLock<String> = OnceLock::new();
static OPENCODE_ROOT_PATH: OnceLock<PathBuf> = OnceLock::new();
static OPENCODE_DB_PATH: OnceLock<PathBuf> = OnceLock::new();
static DB_MODE: OnceLock<bool> = OnceLock::new();

thread_local! {
    static DB_CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

pub type SessionTitlesMap = FxHashMap<Box<str>, String>;
pub type SessionParentsMap = FxHashMap<Box<str>, Box<str>>;
pub type MessageBatch = Vec<(Message, Vec<MessageContent>, bool, Option<Box<str>>)>;

// ============================================================================
// Path & Database Helpers
// ============================================================================

#[inline]
fn get_home() -> &'static str {
    HOME_DIR.get_or_init(|| env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

#[inline]
pub fn get_storage_path(subdir: &str) -> String {
    format!("{}/.local/share/opencode/storage/{}", get_home(), subdir)
}

#[inline]
pub(crate) fn get_opencode_root_path() -> PathBuf {
    OPENCODE_ROOT_PATH
        .get_or_init(|| {
            if let Ok(xdg_data_home) = env::var("XDG_DATA_HOME") {
                PathBuf::from(xdg_data_home).join("opencode")
            } else {
                PathBuf::from(format!("{}/.local/share/opencode", get_home()))
            }
        })
        .clone()
}

#[inline]
pub(crate) fn get_opencode_db_path() -> PathBuf {
    OPENCODE_DB_PATH
        .get_or_init(|| get_opencode_root_path().join("opencode.db"))
        .clone()
}

#[inline]
pub(crate) fn is_db_mode() -> bool {
    *DB_MODE.get_or_init(|| get_opencode_db_path().exists())
}

fn open_opencode_db() -> Option<Connection> {
    let db_path = get_opencode_db_path();
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(300));
    Some(conn)
}

#[inline]
fn with_opencode_db<T>(f: impl FnOnce(&Connection) -> Option<T>) -> Option<T> {
    if !is_db_mode() {
        return None;
    }
    DB_CONN.with(|slot| {
        if slot.borrow().is_none() {
            *slot.borrow_mut() = open_opencode_db();
        }
        let guard = slot.borrow();
        let conn = guard.as_ref()?;
        f(conn)
    })
}

#[inline]
fn db_message_id_from_path(path: &Path) -> Option<String> {
    let p = path.to_str()?;
    p.strip_prefix(DB_MESSAGE_PREFIX).map(|s| s.to_string())
}

pub(crate) fn load_message_from_path(path: &Path) -> Option<Message> {
    if let Some(message_id) = db_message_id_from_path(path) {
        let (row_id, row_session_id, row_time_created, data): (String, String, i64, String) =
            with_opencode_db(|conn| {
                let Ok(mut stmt) = conn.prepare_cached(
                    "SELECT id, session_id, time_created, data FROM message WHERE id = ?1",
                ) else {
                    return None;
                };
                stmt.query_row(params![message_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .ok()
            })?;

        let mut msg: Message = serde_json::from_str(&data).ok()?;

        // Populate missing fields from DB row
        if msg.id.is_none() || msg.id.as_ref().is_some_and(|id| id.0.is_empty()) {
            msg.id = Some(LenientString(row_id));
        }
        if msg.session_id.is_none() || msg.session_id.as_ref().is_some_and(|s| s.0.is_empty()) {
            msg.session_id = Some(LenientString(row_session_id));
        }
        if msg.time.is_none() {
            msg.time = Some(TimeData {
                created: Some(LenientI64(row_time_created)),
                completed: None,
            });
        } else if msg.time.as_ref().is_some_and(|t| t.created.is_none()) {
            if let Some(ref mut time) = msg.time {
                time.created = Some(LenientI64(row_time_created));
            }
        }

        return Some(msg);
    }

    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

// ============================================================================
// Batch Part Loading
// ============================================================================

/// Batch-load parts for multiple messages
fn batch_load_parts_db(message_ids: &[&str]) -> FxHashMap<Box<str>, Vec<PartData>> {
    if message_ids.is_empty() {
        return FxHashMap::default();
    }
    with_opencode_db(|conn| {
        let mut result: FxHashMap<Box<str>, Vec<PartData>> =
            FxHashMap::with_capacity_and_hasher(message_ids.len(), Default::default());
        for chunk in message_ids.chunks(500) {
            let placeholders: String = (0..chunk.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT message_id, data FROM part WHERE message_id IN ({}) \
                 ORDER BY message_id, time_created ASC, id ASC",
                placeholders
            );
            let Ok(mut stmt) = conn.prepare(&sql) else {
                continue;
            };
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            let Ok(rows) = stmt.query_map(params.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) else {
                continue;
            };
            // Track current message_id to avoid re-allocating Box<str> per row
            let mut cur_id: Box<str> = "".into();
            for row in rows.flatten() {
                let part = match serde_json::from_str::<PartData>(&row.1) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                // Rows ordered by message_id: only allocate key when id changes
                if *cur_id != row.0 {
                    cur_id = row.0.into_boxed_str();
                    result.entry(cur_id.clone()).or_default();
                }
                if let Some(vec) = result.get_mut(&cur_id) {
                    vec.push(part);
                }
            }
        }
        Some(result)
    })
    .unwrap_or_default()
}

/// Convert raw parts to MessageContent, skipping reasoning parts
fn parts_to_content(parts: Vec<PartData>) -> Vec<MessageContent> {
    let mut result = Vec::with_capacity(parts.len());
    for part in parts {
        if part.part_type.as_deref() == Some("reasoning") {
            continue;
        }
        if part.thought.is_some() {
            result.push(MessageContent::Thinking(()));
        }
        let mut current_text: Option<Box<str>> = None;
        if let Some(t) = part.text {
            let truncated = truncate_string(&t, MAX_CHARS_PER_TEXT_PART);
            current_text = Some(truncated.clone());
            result.push(MessageContent::Text(truncated));
        }
        if let Some(tool) = part.tool {
            let state_input = part.state.as_ref().and_then(|s| s.input.as_ref());
            let fp: Option<Box<str>> = state_input
                .and_then(|i| infer_tool_file_path(&tool, i).map(|s| s.into_boxed_str()));
            let tool_detail = state_input
                .map(|i| build_tool_detail(&tool, i).into_boxed_str())
                .or(current_text);
            result.push(MessageContent::ToolCall(ToolCallInfo {
                name: tool.into(),
                file_path: fp,
                input: tool_detail,
                additions: None,
                deletions: None,
            }));
        }
    }
    result
}

/// Batch-load parts for multiple messages from filesystem in parallel.
fn batch_load_parts_fs(
    message_ids: &[&str],
    part_root: &Path,
) -> FxHashMap<Box<str>, Vec<PartData>> {
    if message_ids.is_empty() {
        return FxHashMap::default();
    }
    message_ids
        .par_iter()
        .filter_map(|msg_id| {
            let mut parts = Vec::with_capacity(8);
            if let Ok(entries) = fs::read_dir(part_root.join(msg_id)) {
                let mut p_files: Vec<_> = entries.flatten().collect();
                p_files.sort_by_key(|e| e.path());
                for e in p_files {
                    if let Ok(bytes) = fs::read(e.path()) {
                        if let Ok(part) = serde_json::from_slice::<PartData>(&bytes) {
                            parts.push(part);
                        }
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some((msg_id.to_string().into_boxed_str(), parts))
            }
        })
        .collect()
}

// ============================================================================
// Data Structures
// ============================================================================

impl Totals {
    #[inline]
    pub fn display_cost(&self) -> f64 {
        self.cost
    }
}

impl DayStat {
    #[inline]
    pub fn display_cost(&self) -> f64 {
        self.cost
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    #[inline]
    pub fn total(&self) -> u64 {
        self.input + self.output + self.reasoning + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Diffs {
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStat {
    pub id: Box<str>,
    pub messages: u64,
    pub prompts: u64,
    pub cost: f64,
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub models: FxHashSet<Box<str>>,
    pub tools: FxHashMap<Box<str>, u64>,
    pub first_activity: i64,
    pub last_activity: i64,
    pub path_cwd: Box<str>,
    pub path_root: Box<str>,
    pub file_diffs: Vec<FileDiff>,
    pub original_session_id: Option<Box<str>>,
    pub first_created_date: Option<Box<str>>,
    pub is_continuation: bool,
    pub agents: Vec<AgentInfo>,
    pub active_duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: Box<str>,
    pub is_main: bool,
    pub models: FxHashSet<Box<str>>,
    pub messages: u64,
    pub tokens: Tokens,
    pub first_activity: i64,
    pub last_activity: i64,
    pub active_duration_ms: i64,
}

impl SessionStat {
    pub fn new(id: impl Into<Box<str>>) -> Self {
        Self {
            id: id.into(),
            messages: 0,
            prompts: 0,
            cost: 0.0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            models: FxHashSet::default(),
            tools: FxHashMap::default(),
            first_activity: i64::MAX,
            last_activity: 0,
            path_cwd: String::new().into_boxed_str(),
            path_root: String::new().into_boxed_str(),
            file_diffs: Vec::with_capacity(4),
            original_session_id: None,
            first_created_date: None,
            is_continuation: false,
            agents: Vec::with_capacity(2),
            active_duration_ms: 0,
        }
    }

    #[inline]
    pub fn display_cost(&self) -> f64 {
        self.cost
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStat {
    pub messages: u64,
    pub prompts: u64,
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub sessions: FxHashMap<String, Arc<SessionStat>>,
    pub cost: f64,
}

impl Default for DayStat {
    fn default() -> Self {
        Self {
            messages: 0,
            prompts: 0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            sessions: FxHashMap::default(),
            cost: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Totals {
    pub sessions: FxHashSet<Box<str>>,
    pub messages: u64,
    pub prompts: u64,
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub tools: FxHashMap<Box<str>, u64>,
    pub cost: f64,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            sessions: FxHashSet::default(),
            messages: 0,
            prompts: 0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            tools: FxHashMap::default(),
            cost: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stats {
    pub totals: Totals,
    pub per_day: FxHashMap<String, DayStat>,
    pub session_titles: FxHashMap<Box<str>, String>,
    pub model_usage: Vec<ModelUsage>,
    pub session_message_files: FxHashMap<String, FxHashSet<std::path::PathBuf>>,
    pub processed_message_ids: FxHashSet<Box<str>>,
    pub parent_map: FxHashMap<Box<str>, Box<str>>,
    pub children_map: FxHashMap<Box<str>, Vec<Box<str>>>,
}

/// Key for session-day lookups.
pub type SessDayKey = String;

fn make_sess_day_key(session: &str, day: &str) -> SessDayKey {
    format!("{}|{}", session, day)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub name: Box<str>,
    pub short_name: Box<str>,
    pub provider: Box<str>,
    pub display_name: Box<str>,
    pub messages: u64,
    pub sessions: FxHashSet<Box<str>>,
    pub tokens: Tokens,
    pub tools: FxHashMap<Box<str>, u64>,
    pub agents: FxHashMap<Box<str>, u64>,
    #[serde(default)]
    pub daily_tokens: FxHashMap<String, u64>,
    #[serde(default)]
    pub daily_last_hour: FxHashMap<String, u8>,
    pub cost: f64,
}

#[derive(Clone, Default)]
pub struct ToolUsage {
    pub name: Box<str>,
    pub count: u64,
}

#[derive(Clone)]
pub struct ToolCallInfo {
    pub name: Box<str>,
    pub file_path: Option<Box<str>>,
    pub input: Option<Box<str>>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Clone)]
pub enum MessageContent {
    Text(Box<str>),
    ToolCall(ToolCallInfo),
    Thinking(()),
}

#[derive(Clone)]
pub struct ChatMessage {
    pub role: Box<str>,
    pub model: Option<Box<str>>,
    pub parts: Vec<MessageContent>,
    pub is_subagent: bool,
    pub agent_label: Option<Box<str>>,
}

// ============================================================================
// Lenient Type Wrappers
// ============================================================================

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LenientU64(pub u64);

impl<'de> serde::Deserialize<'de> for LenientU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Visitor;
        struct LenientVisitor;
        impl<'de> Visitor<'de> for LenientVisitor {
            type Value = u64;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an integer or a float")
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v)
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v as u64)
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(v as u64)
            }
        }
        deserializer.deserialize_any(LenientVisitor).map(LenientU64)
    }
}

impl Deref for LenientU64 {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LenientI64(pub i64);

impl<'de> serde::Deserialize<'de> for LenientI64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Visitor;
        struct LenientVisitor;
        impl<'de> Visitor<'de> for LenientVisitor {
            type Value = i64;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an integer or a float")
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v)
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v as i64)
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(v as i64)
            }
        }
        deserializer.deserialize_any(LenientVisitor).map(LenientI64)
    }
}

impl Deref for LenientI64 {
    type Target = i64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct CacheData {
    pub(crate) read: Option<LenientU64>,
    pub(crate) write: Option<LenientU64>,
}

#[derive(Deserialize, Default)]
pub(crate) struct TokensData {
    pub(crate) input: Option<LenientU64>,
    pub(crate) output: Option<LenientU64>,
    pub(crate) reasoning: Option<LenientU64>,
    pub(crate) cache: Option<CacheData>,
}

// DiffItem and Summary are used to extract cumulative diff state from messages
#[derive(Deserialize, Default, Clone)]
pub(crate) struct DiffItem {
    pub(crate) file: Option<LenientString>,
    pub(crate) additions: Option<LenientU64>,
    pub(crate) deletions: Option<LenientU64>,
    pub(crate) status: Option<LenientString>,
}

#[derive(Deserialize, Default)]
pub(crate) struct Summary {
    pub(crate) diffs: Option<Vec<DiffItem>>,
}

#[derive(Deserialize, Default)]
pub(crate) struct TimeData {
    pub(crate) created: Option<LenientI64>,
    pub(crate) completed: Option<LenientI64>,
}

#[derive(Deserialize, Default)]
pub(crate) struct PathData {
    pub(crate) cwd: Option<String>,
    pub(crate) root: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct ModelData {
    #[serde(rename = "providerID")]
    pub(crate) provider_id: Option<LenientString>,
    #[serde(rename = "modelID")]
    pub(crate) model_id: Option<LenientString>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LenientString(pub String);

impl<'de> serde::Deserialize<'de> for LenientString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Visitor;
        struct LenientVisitor;
        impl<'de> Visitor<'de> for LenientVisitor {
            type Value = String;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or a number")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(v.to_string())
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(v)
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v.to_string())
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v.to_string())
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(v.to_string())
            }
        }
        deserializer
            .deserialize_any(LenientVisitor)
            .map(LenientString)
    }
}

impl Deref for LenientString {
    type Target = String;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for LenientString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LenientF64(pub f64);

impl<'de> serde::Deserialize<'de> for LenientF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Visitor;
        struct LenientVisitor;
        impl<'de> Visitor<'de> for LenientVisitor {
            type Value = f64;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a float, an integer or a string")
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(v)
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v as f64)
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v as f64)
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse::<f64>().map_err(serde::de::Error::custom)
            }
        }
        deserializer.deserialize_any(LenientVisitor).map(LenientF64)
    }
}

impl Deref for LenientF64 {
    type Target = f64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct Message {
    pub(crate) id: Option<LenientString>,
    #[serde(rename = "sessionID")]
    pub(crate) session_id: Option<LenientString>,
    pub(crate) role: Option<LenientString>,
    pub(crate) agent: Option<LenientString>,
    #[serde(rename = "providerID")]
    pub(crate) provider_id: Option<LenientString>,
    #[serde(rename = "modelID")]
    pub(crate) model_id: Option<LenientString>,
    pub(crate) model: Option<ModelData>,
    pub(crate) time: Option<TimeData>,
    pub(crate) tokens: Option<TokensData>,
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "deserialize_lenient_summary")]
    pub(crate) summary: Option<Summary>,
    pub(crate) path: Option<PathData>,
    pub(crate) cost: Option<LenientF64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: Box<str>,
    pub additions: u64,
    pub deletions: u64,
    pub status: Box<str>,
}

#[derive(Deserialize, Default, Clone)]
struct SessionDiffEntry {
    file: Option<LenientString>,
    additions: Option<LenientU64>,
    deletions: Option<LenientU64>,
    status: Option<LenientString>,
}

#[derive(Deserialize)]
struct SessionData {
    id: Option<LenientString>,
    title: Option<LenientString>,
    #[serde(rename = "parentID")]
    parent_id: Option<LenientString>,
}

#[derive(Deserialize, Default, Clone)]
pub(crate) struct ToolStateInput {
    #[serde(rename = "filePath")]
    pub(crate) file_path: Option<String>,
    #[serde(alias = "old_str", alias = "oldStr")]
    pub(crate) old_str: Option<String>,
    #[serde(alias = "new_str", alias = "newStr")]
    pub(crate) new_str: Option<String>,
    #[serde(alias = "content")]
    pub(crate) content: Option<String>,
    #[serde(alias = "patchText")]
    pub(crate) patch_text: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) pattern: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) limit: Option<serde_json::Value>,
    pub(crate) offset: Option<serde_json::Value>,
    pub(crate) todos: Option<serde_json::Value>,
    pub(crate) ids: Option<Vec<String>>,
}

#[derive(Deserialize, Default, Clone)]
pub(crate) struct ToolState {
    pub(crate) input: Option<ToolStateInput>,
}

#[derive(Deserialize, Default, Clone)]
pub(crate) struct PartData {
    #[serde(rename = "type")]
    pub(crate) part_type: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) tool: Option<String>,
    pub(crate) thought: Option<String>,
    pub(crate) state: Option<ToolState>,
}

// ============================================================================
// Formatting Utilities
// ============================================================================

#[inline]
pub fn format_active_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0s".into();
    }
    let total_secs = (ms / 1000) as u64;
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

#[inline]
pub fn format_number(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        let k = value / 1_000;
        let remainder = value % 1_000;
        format!("{}.{}K", k, remainder / 100)
    } else {
        value.to_string()
    }
}

#[inline]
pub fn format_number_full(value: u64) -> String {
    let s = value.to_string();
    let len = s.len();
    if len <= 3 {
        return s;
    }
    let mut result = String::with_capacity(len + (len - 1) / 3);
    let bytes = s.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(byte as char);
    }
    result
}

#[inline]
fn add_tokens(dst: &mut Tokens, src: &Option<TokensData>) {
    if let Some(t) = src {
        dst.input += t.input.map(|v| *v).unwrap_or(0);
        dst.output += t.output.map(|v| *v).unwrap_or(0);
        dst.reasoning += t.reasoning.map(|v| *v).unwrap_or(0);
        if let Some(cache) = &t.cache {
            dst.cache_read += cache.read.map(|v| *v).unwrap_or(0);
            dst.cache_write += cache.write.map(|v| *v).unwrap_or(0);
        }
    }
}

#[inline]
pub fn get_day(ts: Option<i64>) -> String {
    match ts {
        Some(ms) => {
            let secs = ms / 1000;
            chrono::DateTime::from_timestamp(secs, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_else(|| "Unknown".into())
        }
        None => "Unknown".into(),
    }
}

/// Detect if a session is a continuation from a previous day.
#[inline]
fn detect_session_continuation(
    session_id: &str,
    current_day: &str,
    all_session_first_days: &FxHashMap<String, String>,
) -> (Option<Box<str>>, Option<Box<str>>) {
    if let Some(first_day) = all_session_first_days.get(session_id) {
        if first_day != current_day {
            return (
                Some(session_id.to_string().into_boxed_str()),
                Some(first_day.clone().into_boxed_str()),
            );
        }
    }
    (None, None)
}

#[inline]
pub(crate) fn get_model_id(msg: &Message) -> Box<str> {
    let (provider, model) = if let Some(m) = &msg.model {
        (m.provider_id.as_deref(), m.model_id.as_deref())
    } else {
        (msg.provider_id.as_deref(), msg.model_id.as_deref())
    };

    match (provider, model) {
        (Some(p), Some(m)) => format!("{}/{}", p, m).into_boxed_str(),
        (None, Some(m)) => m.to_string().into_boxed_str(),
        _ => "unknown".into(),
    }
}

pub(crate) fn list_message_files(root: &Path) -> Vec<std::path::PathBuf> {
    if is_db_mode() {
        return with_opencode_db(|conn| {
            let Ok(mut stmt) = conn.prepare("SELECT id FROM message") else {
                return Some(Vec::new());
            };
            let rows = stmt.query_map([], |r| r.get::<_, String>(0));
            let Ok(rows) = rows else {
                return Some(Vec::new());
            };
            Some(
                rows.filter_map(|row| row.ok())
                    .map(|id| PathBuf::from(format!("{}{}", DB_MESSAGE_PREFIX, id)))
                    .collect(),
            )
        })
        .unwrap_or_default();
    }

    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    // Collect top-level entries first for better parallelization
    let top_entries: Vec<_> = entries.flatten().collect();

    top_entries
        .par_iter()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                fs::read_dir(&path)
                    .map(|sub_entries| {
                        sub_entries
                            .flatten()
                            .filter_map(|sub_entry| {
                                let sub_path = sub_entry.path();
                                if sub_path.extension().is_some_and(|e| e == "json") {
                                    Some(sub_path)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else if path.extension().is_some_and(|e| e == "json") {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn load_session_titles() -> (SessionTitlesMap, SessionParentsMap) {
    if is_db_mode() {
        return with_opencode_db(|conn| {
            let Ok(mut stmt) = conn.prepare("SELECT id, title, parent_id FROM session") else {
                return Some((FxHashMap::default(), FxHashMap::default()));
            };
            let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1).unwrap_or_default(),
                    r.get::<_, Option<String>>(2).unwrap_or(None),
                ))
            }) else {
                return Some((FxHashMap::default(), FxHashMap::default()));
            };

            let mut titles = FxHashMap::default();
            let mut parent_map = FxHashMap::default();
            for row in rows.flatten() {
                let id: Box<str> = row.0.into_boxed_str();
                titles.insert(id.clone(), row.1);
                if let Some(pid) = row.2 {
                    if !pid.is_empty() {
                        parent_map.insert(id, pid.into_boxed_str());
                    }
                }
            }
            Some((titles, parent_map))
        })
        .unwrap_or_else(|| (FxHashMap::default(), FxHashMap::default()));
    }

    let session_path = get_storage_path("session");
    let root = Path::new(&session_path);
    let Ok(entries) = fs::read_dir(root) else {
        return (FxHashMap::default(), FxHashMap::default());
    };

    let top_entries: Vec<_> = entries.flatten().collect();

    let all_sessions: Vec<(Box<str>, String, Option<Box<str>>)> = top_entries
        .par_iter()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                fs::read_dir(&path)
                    .map(|sub| {
                        sub.flatten()
                            .filter_map(|se| {
                                let bytes = fs::read(se.path()).ok()?;
                                let session = serde_json::from_slice::<SessionData>(&bytes).ok()?;
                                let id =
                                    session.id.map(|s| s.0).unwrap_or_default().into_boxed_str();
                                let title = session.title.map(|s| s.0).unwrap_or_default();
                                let parent = session.parent_id.map(|s| s.0.into_boxed_str());
                                Some((id, title, parent))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else if path.extension().is_some_and(|e| e == "json") {
                fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<SessionData>(&bytes).ok())
                    .map(|session| {
                        let id = session.id.map(|s| s.0).unwrap_or_default().into_boxed_str();
                        let title = session.title.map(|s| s.0).unwrap_or_default();
                        let parent = session.parent_id.map(|s| s.0.into_boxed_str());
                        vec![(id, title, parent)]
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        })
        .collect();

    let mut titles = FxHashMap::with_capacity_and_hasher(all_sessions.len(), Default::default());
    let mut parent_map = FxHashMap::default();
    for (id, title, parent) in all_sessions {
        if let Some(pid) = parent {
            if !pid.is_empty() {
                parent_map.insert(id.clone(), pid);
            }
        }
        titles.insert(id, title);
    }
    (titles, parent_map)
}

pub fn extract_agent_name(title: &str) -> Box<str> {
    if let Some(start) = title.find("(@") {
        if let Some(end) = title[start..].find(" subagent)") {
            return title[start + 1..start + end].to_string().into_boxed_str();
        }
    }
    if title.len() > 20 {
        title[..20].to_string().into_boxed_str()
    } else {
        title.to_string().into_boxed_str()
    }
}

#[inline]
fn sort_file_diffs(file_diffs: &mut [FileDiff]) {
    file_diffs.sort_by(|a, b| {
        let order = |s: &str| match s {
            "modified" => 0,
            "added" => 1,
            "deleted" => 2,
            _ => 3,
        };
        order(&a.status)
            .cmp(&order(&b.status))
            .then_with(|| a.path.cmp(&b.path))
    });
}

pub(crate) fn load_session_diff_map() -> FxHashMap<String, Vec<FileDiff>> {
    let diff_path = get_storage_path("session_diff");
    let root = Path::new(&diff_path);
    let mut out: FxHashMap<String, Vec<FileDiff>> = if let Ok(entries) = fs::read_dir(root) {
        // Collect entries for parallel processing
        let all_entries: Vec<_> = entries.flatten().collect();
        all_entries
            .par_iter()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?;
                let bytes = fs::read(&path).ok()?;
                let entries = serde_json::from_slice::<Vec<SessionDiffEntry>>(&bytes).ok()?;

                let mut diffs: Vec<FileDiff> = entries
                    .into_iter()
                    .map(|item| FileDiff {
                        path: item
                            .file
                            .map(|s| s.0)
                            .unwrap_or_else(|| "unknown".into())
                            .into_boxed_str(),
                        additions: item.additions.map(|v| *v).unwrap_or(0),
                        deletions: item.deletions.map(|v| *v).unwrap_or(0),
                        status: item
                            .status
                            .map(|s| s.0)
                            .unwrap_or_else(|| "modified".into())
                            .into_boxed_str(),
                    })
                    .collect();
                sort_file_diffs(&mut diffs);
                Some((stem.to_string(), diffs))
            })
            .collect()
    } else {
        FxHashMap::default()
    };

    // DB fallback: if session_diff files are unavailable for a session,
    // try reading summary_diffs from SQLite.
    if is_db_mode() {
        let db_rows: Vec<(String, String)> = with_opencode_db(|conn| {
            let Ok(mut stmt) = conn.prepare(
                "SELECT id, summary_diffs FROM session WHERE summary_diffs IS NOT NULL AND summary_diffs <> ''",
            ) else {
                return Some(Vec::new());
            };
            let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1).unwrap_or_default(),
                ))
            }) else {
                return Some(Vec::new());
            };
            Some(rows.flatten().collect())
        })
        .unwrap_or_default();

        for (session_id, json) in db_rows {
            if out.contains_key(&session_id) {
                continue;
            }
            let Ok(entries) = serde_json::from_str::<Vec<SessionDiffEntry>>(&json) else {
                continue;
            };
            let mut diffs: Vec<FileDiff> = entries
                .into_iter()
                .map(|item| FileDiff {
                    path: item
                        .file
                        .map(|s| s.0)
                        .unwrap_or_else(|| "unknown".into())
                        .into_boxed_str(),
                    additions: item.additions.map(|v| *v).unwrap_or(0),
                    deletions: item.deletions.map(|v| *v).unwrap_or(0),
                    status: item
                        .status
                        .map(|s| s.0)
                        .unwrap_or_else(|| "modified".into())
                        .into_boxed_str(),
                })
                .collect();
            sort_file_diffs(&mut diffs);
            out.insert(session_id, diffs);
        }
    }

    out
}

// ============================================================================
// Session Diff Loading
// ============================================================================

pub(crate) fn load_session_diff_totals(
    session_diff_map: &FxHashMap<String, Vec<FileDiff>>,
) -> FxHashMap<String, (u64, u64)> {
    let mut totals: FxHashMap<String, (u64, u64)> = session_diff_map
        .iter()
        .map(|(id, diffs)| {
            let adds: u64 = diffs.iter().map(|d| d.additions).sum();
            let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
            (id.clone(), (adds, dels))
        })
        .collect();

    if is_db_mode() {
        let db_rows: Vec<(String, i64, i64)> = with_opencode_db(|conn| {
            let Ok(mut stmt) = conn.prepare(
                "SELECT id, COALESCE(summary_additions, 0), COALESCE(summary_deletions, 0) FROM session",
            ) else {
                return Some(Vec::new());
            };
            let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1).unwrap_or(0),
                    r.get::<_, i64>(2).unwrap_or(0),
                ))
            }) else {
                return Some(Vec::new());
            };
            Some(rows.flatten().collect())
        })
        .unwrap_or_default();

        for (id, adds, dels) in db_rows {
            totals
                .entry(id)
                .or_insert((adds.max(0) as u64, dels.max(0) as u64));
        }
    }

    totals
}

/// Compute incremental diffs: current minus previous cumulative state.
#[inline]
fn compute_incremental_diffs(current: &[FileDiff], previous: &[FileDiff]) -> Vec<FileDiff> {
    if previous.is_empty() {
        return current.to_vec();
    }

    // Optimization: Assume both are already sorted by path (enforced at creation).
    let mut result = Vec::with_capacity(current.len());
    let mut i = 0;
    let mut j = 0;

    while i < current.len() && j < previous.len() {
        let c = &current[i];
        let p = &previous[j];

        match c.path.cmp(&p.path) {
            std::cmp::Ordering::Equal => {
                let a = c.additions.saturating_sub(p.additions);
                let d = c.deletions.saturating_sub(p.deletions);
                if a > 0 || d > 0 {
                    result.push(FileDiff {
                        path: c.path.clone(),
                        additions: a,
                        deletions: d,
                        status: c.status.clone(),
                    });
                }
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                result.push(c.clone());
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                j += 1;
            }
        }
    }

    while i < current.len() {
        result.push(current[i].clone());
        i += 1;
    }

    result
}

// ============================================================================
// Main Statistics Collection
// ============================================================================

pub fn collect_stats() -> Stats {
    let mut totals = Totals::default();
    let (session_titles, parent_map) = load_session_titles();

    let mut resolved_parent_map: FxHashMap<Box<str>, Box<str>> =
        FxHashMap::with_capacity_and_hasher(parent_map.len(), Default::default());
    for child in parent_map.keys() {
        let mut cur = child.clone();
        let mut depth = 0;
        while let Some(p) = parent_map.get(&cur) {
            cur = p.clone();
            depth += 1;
            if depth > 20 {
                break;
            }
        }
        resolved_parent_map.insert(child.clone(), cur);
    }
    let parent_map = resolved_parent_map;

    let mut children_map: FxHashMap<Box<str>, Vec<Box<str>>> = FxHashMap::default();
    for (child, parent) in &parent_map {
        children_map
            .entry(parent.clone())
            .or_default()
            .push(child.clone());
    }

    let session_diff_map = load_session_diff_map();
    let message_path = get_storage_path("message");
    let part_path_str = get_storage_path("part");
    let part_root = Path::new(&part_path_str);
    let msg_files = list_message_files(Path::new(&message_path));

    let mut per_day: FxHashMap<String, DayStat> =
        FxHashMap::with_capacity_and_hasher(msg_files.len() / 20, Default::default());
    let mut model_stats: FxHashMap<Box<str>, ModelUsage> =
        FxHashMap::with_capacity_and_hasher(8, Default::default());
    let mut session_message_files: FxHashMap<String, FxHashSet<std::path::PathBuf>> =
        FxHashMap::with_capacity_and_hasher(128, Default::default());
    let mut processed_message_ids: FxHashSet<Box<str>> =
        FxHashSet::with_capacity_and_hasher(msg_files.len(), Default::default());
    let mut session_first_days: FxHashMap<String, String> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());

    struct FullMessageData {
        msg: Message,
        tools: Vec<Box<str>>,
        parts: Vec<PartData>,
        path: std::path::PathBuf,
        message_id: Box<str>,
        cumulative_diffs: Vec<FileDiff>,
    }

    // Step 1: Load all messages in parallel
    let raw_messages: Vec<(Message, std::path::PathBuf, Box<str>)> = msg_files
        .par_iter()
        .filter_map(|p| {
            let msg: Message = load_message_from_path(p)?;
            let message_id = match &msg.id {
                Some(id) if !id.0.is_empty() => id.0.clone().into_boxed_str(),
                _ => p.to_string_lossy().to_string().into_boxed_str(),
            };
            Some((msg, p.clone(), message_id))
        })
        .collect();

    // Step 2: Batch load ALL parts
    let all_msg_ids: Vec<&str> = raw_messages
        .iter()
        .filter_map(|(msg, _, _)| msg.id.as_ref().map(|id| id.0.as_str()))
        .filter(|id| !id.is_empty())
        .collect();
    let all_parts_map: FxHashMap<Box<str>, Vec<PartData>> = if is_db_mode() {
        batch_load_parts_db(&all_msg_ids)
    } else {
        batch_load_parts_fs(&all_msg_ids, part_root)
    };

    // Step 3: Build FullMessageData with cached parts
    let mut processed_data: Vec<FullMessageData> = raw_messages
        .into_iter()
        .map(|(msg, path, message_id)| {
            let parts: Vec<PartData> = msg
                .id
                .as_ref()
                .and_then(|id| all_parts_map.get(id.0.as_str()).cloned())
                .unwrap_or_default();

            let tools: Vec<Box<str>> = parts
                .iter()
                .filter(|p| p.part_type.as_deref() == Some("tool"))
                .filter_map(|p| p.tool.as_ref().map(|t| t.as_str().into()))
                .collect();

            let cumulative_diffs: Vec<FileDiff> = msg
                .summary
                .as_ref()
                .and_then(|s| s.diffs.as_ref())
                .map(|diffs| {
                    diffs
                        .iter()
                        .map(|d| FileDiff {
                            path: d
                                .file
                                .as_ref()
                                .map(|s| s.0.clone())
                                .unwrap_or_default()
                                .into_boxed_str(),
                            additions: d.additions.map(|v| *v).unwrap_or(0),
                            deletions: d.deletions.map(|v| *v).unwrap_or(0),
                            status: d
                                .status
                                .as_ref()
                                .map(|s| s.0.clone())
                                .unwrap_or_else(|| "modified".into())
                                .into_boxed_str(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            FullMessageData {
                msg,
                tools,
                parts,
                path,
                message_id,
                cumulative_diffs,
            }
        })
        .collect();

    processed_data.sort_unstable_by_key(|d| {
        d.msg
            .time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0)
    });

    // Track per-file cumulative diff state per session per day
    let mut session_day_union_diffs: FxHashMap<SessDayKey, FxHashMap<Box<str>, FileDiff>> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());

    let mut last_ts = None;
    let mut last_day_str = String::new();
    let mut session_overall_start: FxHashMap<Box<str>, i64> = FxHashMap::default();
    let mut session_day_intervals: FxHashMap<SessDayKey, Vec<(i64, i64)>> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());
    let mut agent_intervals: FxHashMap<String, Vec<(i64, i64)>> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());

    // Process all messages
    for data in processed_data {
        if !processed_message_ids.insert(data.message_id) {
            continue;
        }

        let msg = &data.msg;
        let session_id_boxed: Box<str> = msg
            .session_id
            .as_ref()
            .map(|s| s.0.as_str())
            .unwrap_or_default()
            .into();

        let effective_session_id: Box<str> = parent_map
            .get(&session_id_boxed)
            .cloned()
            .unwrap_or_else(|| session_id_boxed.clone());
        let is_subagent_msg = parent_map.contains_key(&session_id_boxed);

        let agent_name: Box<str> = msg
            .agent
            .as_ref()
            .filter(|a| !a.0.is_empty())
            .map(|a| a.0.clone().into_boxed_str())
            .unwrap_or_else(|| "unknown".into());

        if !session_id_boxed.is_empty() {
            session_message_files
                .entry(session_id_boxed.to_string())
                .or_insert_with(|| FxHashSet::with_capacity_and_hasher(16, Default::default()))
                .insert(data.path);
        }

        let ts_val = msg.time.as_ref().and_then(|t| t.created.map(|v| *v));
        let day = if ts_val == last_ts && !last_day_str.is_empty() {
            last_day_str.clone()
        } else {
            let d = get_day(ts_val);
            last_ts = ts_val;
            last_day_str = d.clone();
            d
        };

        let role = msg.role.as_ref().map(|s| s.0.as_str()).unwrap_or("");
        let is_user = role == "user";
        let is_assistant = role == "assistant";
        let model_id = get_model_id(msg);
        let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);

        let mut tokens_from_msg = if let Some(t) = &msg.tokens {
            Tokens {
                input: t.input.map(|v| *v).unwrap_or(0),
                output: t.output.map(|v| *v).unwrap_or(0),
                reasoning: t.reasoning.map(|v| *v).unwrap_or(0),
                cache_read: t
                    .cache
                    .as_ref()
                    .and_then(|c| c.read.map(|v| *v))
                    .unwrap_or(0),
                cache_write: t
                    .cache
                    .as_ref()
                    .and_then(|c| c.write.map(|v| *v))
                    .unwrap_or(0),
            }
        } else {
            Tokens::default()
        };

        // Estimate reasoning tokens from parts if not provided
        if tokens_from_msg.reasoning == 0 && is_assistant {
            let reasoning_parts: Vec<_> = data
                .parts
                .iter()
                .filter(|p| p.part_type.as_deref() == Some("reasoning"))
                .collect();
            if !reasoning_parts.is_empty() {
                let reasoning_chars: usize = reasoning_parts
                    .iter()
                    .filter_map(|p| p.text.as_ref().map(|t| t.len()))
                    .sum();
                if reasoning_chars > 0 {
                    tokens_from_msg.reasoning = (reasoning_chars / 4) as u64;
                }
            }
        }

        // Track first day session was seen for continuation detection (use effective)
        if !effective_session_id.is_empty()
            && !session_first_days.contains_key(effective_session_id.as_ref())
        {
            session_first_days.insert(effective_session_id.to_string(), day.clone());
        }

        // Only count main sessions in totals
        if !effective_session_id.is_empty()
            && !totals.sessions.contains(effective_session_id.as_ref())
        {
            totals.sessions.insert(effective_session_id.clone());
        }
        totals.messages += 1;
        if is_user && !is_subagent_msg {
            totals.prompts += 1;
        }
        totals.cost += cost;
        // Use tokens_from_msg which includes estimated reasoning tokens
        totals.tokens.input += tokens_from_msg.input;
        totals.tokens.output += tokens_from_msg.output;
        totals.tokens.reasoning += tokens_from_msg.reasoning;
        totals.tokens.cache_read += tokens_from_msg.cache_read;
        totals.tokens.cache_write += tokens_from_msg.cache_write;

        if is_assistant {
            let model_entry = model_stats.entry(model_id.clone()).or_insert_with(|| {
                let name_str: &str = &model_id;
                let short: Box<str> = name_str.rsplit('/').next().unwrap_or(name_str).into();
                let provider: Box<str> = name_str.split('/').next().unwrap_or(name_str).into();
                ModelUsage {
                    name: model_id.clone(),
                    short_name: short.clone(),
                    provider: provider.clone(),
                    display_name: format!("{}/{}", provider, short).into_boxed_str(),
                    messages: 0,
                    sessions: FxHashSet::default(),
                    tokens: Tokens::default(),
                    tools: FxHashMap::default(),
                    agents: FxHashMap::default(),
                    daily_tokens: FxHashMap::default(),
                    daily_last_hour: FxHashMap::default(),
                    cost: 0.0,
                }
            });
            model_entry.messages += 1;
            if !effective_session_id.is_empty()
                && !model_entry.sessions.contains(effective_session_id.as_ref())
            {
                model_entry.sessions.insert(effective_session_id.clone());
            }
            model_entry.cost += cost;
            // Use tokens_from_msg which includes estimated reasoning tokens
            model_entry.tokens.input += tokens_from_msg.input;
            model_entry.tokens.output += tokens_from_msg.output;
            model_entry.tokens.reasoning += tokens_from_msg.reasoning;
            model_entry.tokens.cache_read += tokens_from_msg.cache_read;
            model_entry.tokens.cache_write += tokens_from_msg.cache_write;
            *model_entry.daily_tokens.entry(day.clone()).or_insert(0) += tokens_from_msg.total();
            if let Some(secs) = ts_val {
                if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                    model_entry
                        .daily_last_hour
                        .insert(day.clone(), dt.hour() as u8);
                }
            }
            if let Some(agent) = msg
                .agent
                .as_ref()
                .map(|s| s.0.as_str())
                .filter(|s| !s.is_empty())
            {
                *model_entry
                    .agents
                    .entry(agent.to_string().into_boxed_str())
                    .or_insert(0) += 1;
            }
        }

        // Optimization: avoid allocation if day already exists
        let day_stat = if let Some(existing) = per_day.get_mut(&day) {
            existing
        } else {
            per_day.insert(day.clone(), DayStat::default());
            per_day.get_mut(&day).unwrap()
        };
        day_stat.messages += 1;
        if is_user && !is_subagent_msg {
            day_stat.prompts += 1;
        }
        day_stat.cost += cost;
        // Use tokens_from_msg which includes estimated reasoning tokens
        day_stat.tokens.input += tokens_from_msg.input;
        day_stat.tokens.output += tokens_from_msg.output;
        day_stat.tokens.reasoning += tokens_from_msg.reasoning;
        day_stat.tokens.cache_read += tokens_from_msg.cache_read;
        day_stat.tokens.cache_write += tokens_from_msg.cache_write;

        // Get or create the session for THIS specific day using effective_session_id
        let session_stat_arc =
            if let Some(existing) = day_stat.sessions.get_mut(effective_session_id.as_ref()) {
                existing
            } else {
                // Detect if this is a continuation from a previous day
                let (original_id, first_created) = if !effective_session_id.is_empty() {
                    detect_session_continuation(
                        effective_session_id.as_ref(),
                        &day,
                        &session_first_days,
                    )
                } else {
                    (None, None)
                };

                let is_continued = original_id.is_some();
                let mut stat = SessionStat::new(effective_session_id.clone());
                stat.original_session_id = original_id;
                stat.first_created_date = first_created;
                stat.is_continuation = is_continued;
                day_stat
                    .sessions
                    .insert(effective_session_id.to_string(), Arc::new(stat));
                day_stat
                    .sessions
                    .get_mut(effective_session_id.as_ref())
                    .unwrap()
            };

        // Accumulate data for this day's session (separate from other days)
        let session_stat = Arc::make_mut(session_stat_arc);
        session_stat.messages += 1;
        if is_user && !is_subagent_msg {
            session_stat.prompts += 1;
        }
        session_stat.cost += cost;
        if is_assistant {
            session_stat.models.insert(model_id.clone());
        }
        // Use tokens_from_msg which includes estimated reasoning tokens
        session_stat.tokens.input += tokens_from_msg.input;
        session_stat.tokens.output += tokens_from_msg.output;
        session_stat.tokens.reasoning += tokens_from_msg.reasoning;
        session_stat.tokens.cache_read += tokens_from_msg.cache_read;
        session_stat.tokens.cache_write += tokens_from_msg.cache_write;
        if let Some(t) = ts_val {
            if t < session_stat.first_activity {
                session_stat.first_activity = t;
            }
            // Track overall session start across all days
            let start_entry = session_overall_start
                .entry(effective_session_id.clone())
                .or_insert(t);
            if t < *start_entry {
                *start_entry = t;
            }
        }
        let end_ts = msg
            .time
            .as_ref()
            .and_then(|t| t.completed.map(|v| *v))
            .or(ts_val);
        if let Some(t) = end_ts {
            if t > session_stat.last_activity {
                session_stat.last_activity = t;
            }
        }

        if is_assistant {
            // Collect interval for merged duration calculation (done after the loop)
            if let (Some(created), Some(completed)) = (ts_val, end_ts) {
                if completed > created {
                    let key = make_sess_day_key(effective_session_id.as_ref(), day.as_str());
                    session_day_intervals
                        .entry(key)
                        .or_default()
                        .push((created, completed));
                    // Also collect per-agent interval
                    let agent_key = format!("{}|{}|{}", effective_session_id, day, agent_name);
                    agent_intervals
                        .entry(agent_key)
                        .or_default()
                        .push((created, completed));
                }
            }

            // Update agent info (duration will be set after the loop)
            let agent_entry = session_stat
                .agents
                .iter_mut()
                .find(|a| *a.name == *agent_name);
            if let Some(agent) = agent_entry {
                agent.messages += 1;
                agent.tokens.input += tokens_from_msg.input;
                agent.tokens.output += tokens_from_msg.output;
                agent.tokens.reasoning += tokens_from_msg.reasoning;
                agent.tokens.cache_read += tokens_from_msg.cache_read;
                agent.tokens.cache_write += tokens_from_msg.cache_write;
                agent.models.insert(model_id.clone());
                if let Some(t) = ts_val {
                    if t < agent.first_activity {
                        agent.first_activity = t;
                    }
                }
                if let Some(t) = end_ts {
                    if t > agent.last_activity {
                        agent.last_activity = t;
                    }
                }
            } else {
                let mut models = FxHashSet::default();
                models.insert(model_id.clone());
                session_stat.agents.push(AgentInfo {
                    name: agent_name.clone(),
                    is_main: !is_subagent_msg,
                    models,
                    messages: 1,
                    tokens: tokens_from_msg,
                    first_activity: ts_val.unwrap_or(i64::MAX),
                    last_activity: end_ts.unwrap_or(0),
                    active_duration_ms: 0,
                });
            }
        } else {
            // Update non-assistant agent info (e.g. "unknown" for user)
            let agent_entry = session_stat
                .agents
                .iter_mut()
                .find(|a| *a.name == *agent_name);
            if let Some(agent) = agent_entry {
                agent.messages += 1;
                agent.tokens.input += tokens_from_msg.input;
                agent.tokens.output += tokens_from_msg.output;
                agent.tokens.reasoning += tokens_from_msg.reasoning;
                agent.tokens.cache_read += tokens_from_msg.cache_read;
                agent.tokens.cache_write += tokens_from_msg.cache_write;
                if let Some(t) = ts_val {
                    if t < agent.first_activity {
                        agent.first_activity = t;
                    }
                }
                if let Some(t) = end_ts {
                    if t > agent.last_activity {
                        agent.last_activity = t;
                    }
                }
            } else {
                session_stat.agents.push(AgentInfo {
                    name: agent_name.clone(),
                    is_main: !is_subagent_msg,
                    models: FxHashSet::default(),
                    messages: 1,
                    tokens: tokens_from_msg,
                    first_activity: ts_val.unwrap_or(i64::MAX),
                    last_activity: end_ts.unwrap_or(0),
                    active_duration_ms: 0,
                });
            }
        }

        for t in data.tools {
            *totals.tools.entry(t.clone()).or_insert(0) += 1;
            *session_stat.tools.entry(t.clone()).or_insert(0) += 1;
            if is_assistant {
                if let Some(model_entry) = model_stats.get_mut(&model_id) {
                    *model_entry.tools.entry(t).or_insert(0) += 1;
                }
            }
        }

        if let Some(p) = &msg.path {
            if let Some(cwd) = &p.cwd {
                session_stat.path_cwd = cwd.clone().into();
            }
            if let Some(root) = &p.root {
                session_stat.path_root = root.clone().into();
            }
        }

        // Accumulate per-file diffs using effective_session_id
        if !effective_session_id.is_empty() {
            let key = make_sess_day_key(effective_session_id.as_ref(), day.as_str());
            let file_map = session_day_union_diffs.entry(key).or_insert_with(|| {
                FxHashMap::with_capacity_and_hasher(
                    data.cumulative_diffs.len().max(1),
                    Default::default(),
                )
            });
            for d in data.cumulative_diffs {
                if !d.path.is_empty() {
                    file_map.insert(d.path.clone(), d);
                }
            }
        }
    }

    // Compute merged active durations from collected intervals
    fn merge_intervals_duration(intervals: &mut [(i64, i64)]) -> i64 {
        if intervals.is_empty() {
            return 0;
        }
        intervals.sort_unstable_by_key(|&(start, _)| start);
        let mut total: i64 = 0;
        let mut cur_start = intervals[0].0;
        let mut cur_end = intervals[0].1;
        for &(start, end) in &intervals[1..] {
            if start <= cur_end {
                // Overlapping or adjacent - extend
                if end > cur_end {
                    cur_end = end;
                }
            } else {
                // Gap - finalize previous interval
                total += cur_end - cur_start;
                cur_start = start;
                cur_end = end;
            }
        }
        total += cur_end - cur_start;
        total
    }

    // Apply merged session durations to session stats
    for (key, mut intervals) in session_day_intervals {
        let merged_dur = merge_intervals_duration(&mut intervals);
        if let Some((session_id, day_str)) = key.split_once('|') {
            if let Some(day_stat) = per_day.get_mut(day_str) {
                if let Some(sess_arc) = day_stat.sessions.get_mut(session_id) {
                    let sess = Arc::make_mut(sess_arc);
                    sess.active_duration_ms = merged_dur;
                }
            }
        }
    }

    // Apply merged agent durations
    for (key, mut intervals) in agent_intervals {
        let merged_dur = merge_intervals_duration(&mut intervals);
        // key format: "session_id|day|agent_name"
        let mut parts = key.splitn(3, '|');
        let session_id = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let day_str = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let agent_name_str = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        if let Some(day_stat) = per_day.get_mut(day_str) {
            if let Some(sess_arc) = day_stat.sessions.get_mut(session_id) {
                let sess = Arc::make_mut(sess_arc);
                if let Some(agent) = sess.agents.iter_mut().find(|a| *a.name == *agent_name_str) {
                    agent.active_duration_ms = merged_dur;
                }
            }
        }
    }

    // Precompute diff totals from session_diff_map for global totals
    let precomputed_diff_totals: FxHashMap<String, (u64, u64)> =
        load_session_diff_totals(&session_diff_map);

    // Build sorted list of days per session to compute previous day's cumulative state
    let mut session_sorted_days: FxHashMap<String, Vec<String>> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());
    for key in session_day_union_diffs.keys() {
        if let Some((session_id, day)) = key.split_once('|') {
            session_sorted_days
                .entry(session_id.to_string())
                .or_default()
                .push(day.to_string());
        }
    }
    for days in session_sorted_days.values_mut() {
        days.sort_unstable();
    }

    // Track which session IDs have been counted in global totals
    let mut counted_session_diffs: FxHashSet<String> =
        FxHashSet::with_capacity_and_hasher(64, Default::default());

    for (day_str, day_stat) in per_day.iter_mut() {
        for sess_arc in day_stat.sessions.values_mut() {
            let sess_id: String = sess_arc.id.to_string();
            let sess = Arc::make_mut(sess_arc);

            // Update session-wide first_activity if it's a continuation
            if let Some(&overall_start) = session_overall_start.get(sess.id.as_ref()) {
                if overall_start < sess.first_activity {
                    sess.first_activity = overall_start;
                }
            }

            // Build lookup key once
            let lookup_key = make_sess_day_key(&sess_id, day_str.as_str());

            // Get union of per-file diff states from ALL messages of THIS day
            let current_day_diffs: Option<Vec<FileDiff>> = session_day_union_diffs
                .get(&lookup_key)
                .map(|m| m.values().cloned().collect());

            if !sess.is_continuation {
                // Non-continuation: prefer session_diff (authoritative final state),
                // fall back to message union diffs, then nothing
                if let Some(session_diffs) = session_diff_map.get(sess_id.as_str()) {
                    sess.file_diffs = session_diffs.clone();
                    sort_file_diffs(&mut sess.file_diffs);
                    if let Some(&(adds, dels)) = precomputed_diff_totals.get(sess_id.as_str()) {
                        sess.diffs.additions = adds;
                        sess.diffs.deletions = dels;
                    }
                } else if let Some(mut diffs) = current_day_diffs {
                    sort_file_diffs(&mut diffs);
                    let adds: u64 = diffs.iter().map(|d| d.additions).sum();
                    let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
                    sess.file_diffs = diffs;
                    sess.diffs.additions = adds;
                    sess.diffs.deletions = dels;
                } else if let Some(&(adds, dels)) = precomputed_diff_totals.get(sess_id.as_str()) {
                    // DB fallback when detailed per-file diffs are unavailable.
                    sess.diffs.additions = adds;
                    sess.diffs.deletions = dels;
                }
            } else if let Some(mut diffs) = current_day_diffs {
                // Continuation sessions: compute incremental diffs per day
                // by subtracting previous day's cumulative state
                if let Some(sorted_days) = session_sorted_days.get(sess_id.as_str()) {
                    if let Some(pos) = sorted_days.iter().position(|d| d.as_str() == day_str) {
                        if pos > 0 {
                            let prev_day = &sorted_days[pos - 1];
                            let prev_key = make_sess_day_key(&sess_id, prev_day.as_str());
                            if let Some(prev_map) = session_day_union_diffs.get(&prev_key) {
                                let prev_vec: Vec<FileDiff> = prev_map.values().cloned().collect();
                                diffs = compute_incremental_diffs(&diffs, &prev_vec);
                            }
                        }
                    }
                }

                sort_file_diffs(&mut diffs);
                let adds: u64 = diffs.iter().map(|d| d.additions).sum();
                let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
                sess.file_diffs = diffs;
                sess.diffs.additions = adds;
                sess.diffs.deletions = dels;
            }

            // Add to day totals
            day_stat.diffs.additions += sess.diffs.additions;
            day_stat.diffs.deletions += sess.diffs.deletions;

            // Global totals: use session_diff (final state) once per session
            if !counted_session_diffs.contains(sess_id.as_str()) {
                if let Some(&(adds, dels)) = precomputed_diff_totals.get(sess_id.as_str()) {
                    totals.diffs.additions += adds;
                    totals.diffs.deletions += dels;
                }
                counted_session_diffs.insert(sess_id);
            }
        }
    }

    let mut model_usage: Vec<ModelUsage> = model_stats.into_values().collect();
    model_usage.sort_unstable_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    // Sort agents in each session: main agent first, then alphabetically
    for day_stat in per_day.values_mut() {
        for sess_arc in day_stat.sessions.values_mut() {
            let sess = Arc::make_mut(sess_arc);
            sess.agents.sort_by(|a, b| {
                if a.is_main && !b.is_main {
                    std::cmp::Ordering::Less
                } else if !a.is_main && b.is_main {
                    std::cmp::Ordering::Greater
                } else {
                    a.name.cmp(&b.name)
                }
            });
        }
    }

    Stats {
        totals,
        per_day,
        session_titles,
        model_usage,
        session_message_files,
        processed_message_ids,
        parent_map,
        children_map,
    }
}

fn load_session_chat_internal(
    session_id: Option<&str>,
    files: Option<&[std::path::PathBuf]>,
    day_filter: Option<&str>,
    since_ts: Option<i64>,
) -> (Vec<ChatMessage>, i64) {
    let part_path_str = get_storage_path("part");
    let part_root = Path::new(&part_path_str);

    let mut session_msgs: Vec<Message> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let msg: Message = load_message_from_path(p)?;
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }
                if let Some(since) = since_ts {
                    let created = msg
                        .time
                        .as_ref()
                        .and_then(|t| t.created.map(|v| *v))
                        .unwrap_or(0);
                    if created <= since {
                        return None;
                    }
                }
                Some(msg)
            })
            .collect()
    } else {
        let message_path = get_storage_path("message");
        let msg_files = list_message_files(Path::new(&message_path));
        msg_files
            .par_iter()
            .filter_map(|p| {
                let msg: Message = load_message_from_path(p)?;
                if let Some(session_id) = session_id {
                    if msg.session_id.as_ref().map(|s| s.as_ref()) != Some(session_id) {
                        return None;
                    }
                }
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }
                if let Some(since) = since_ts {
                    let created = msg
                        .time
                        .as_ref()
                        .and_then(|t| t.created.map(|v| *v))
                        .unwrap_or(0);
                    if created <= since {
                        return None;
                    }
                }
                Some(msg)
            })
            .collect()
    };

    session_msgs.sort_unstable_by_key(|m| {
        m.time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0)
    });

    // Batch-load parts: single DB query instead of N individual queries
    let session_msgs_with_parts: Vec<(Message, Vec<MessageContent>)> = if is_db_mode() {
        let msg_ids: Vec<&str> = session_msgs
            .iter()
            .filter_map(|m| m.id.as_ref().map(|id| id.0.as_str()))
            .filter(|id| !id.is_empty())
            .collect();
        let mut parts_map = batch_load_parts_db(&msg_ids);
        session_msgs
            .into_iter()
            .map(|msg| {
                let parts_vec = msg
                    .id
                    .as_ref()
                    .and_then(|id| parts_map.remove(id.0.as_str()))
                    .map(parts_to_content)
                    .unwrap_or_default();
                (msg, parts_vec)
            })
            .collect()
    } else {
        // Batch-load parts for file mode
        let msg_ids: Vec<&str> = session_msgs
            .iter()
            .filter_map(|m| m.id.as_ref().map(|id| id.0.as_str()))
            .filter(|id| !id.is_empty())
            .collect();
        let mut parts_map = batch_load_parts_fs(&msg_ids, part_root);
        session_msgs
            .into_iter()
            .map(|msg| {
                let parts_vec = msg
                    .id
                    .as_ref()
                    .and_then(|id| parts_map.remove(id.0.as_str()))
                    .map(parts_to_content)
                    .unwrap_or_default();
                (msg, parts_vec)
            })
            .collect()
    };

    let mut max_ts = since_ts.unwrap_or(0);
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(session_msgs_with_parts.len());
    let mut last_cumulative_diffs: Vec<FileDiff> = Vec::new();

    for (msg, mut parts_vec) in session_msgs_with_parts {
        let created = msg
            .time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0);
        if created > max_ts {
            max_ts = created;
        }

        let current_cumulative: Vec<FileDiff> = msg
            .summary
            .as_ref()
            .and_then(|s| s.diffs.as_ref())
            .map(|diffs| {
                let mut v: Vec<FileDiff> = diffs
                    .iter()
                    .map(|d| FileDiff {
                        path: d
                            .file
                            .as_ref()
                            .map(|s| s.0.clone())
                            .unwrap_or_default()
                            .into_boxed_str(),
                        additions: d.additions.map(|v| *v).unwrap_or(0),
                        deletions: d.deletions.map(|v| *v).unwrap_or(0),
                        status: d
                            .status
                            .as_ref()
                            .map(|s| s.0.clone())
                            .unwrap_or_else(|| "modified".into())
                            .into_boxed_str(),
                    })
                    .collect();
                sort_file_diffs(&mut v);
                v
            })
            .unwrap_or_else(|| last_cumulative_diffs.clone());

        let incremental = compute_incremental_diffs(&current_cumulative, &last_cumulative_diffs);
        last_cumulative_diffs = current_cumulative;

        match_tool_calls_with_diffs(&mut parts_vec, &incremental);

        let role: Box<str> = msg
            .role
            .as_ref()
            .map(|s| s.as_ref())
            .unwrap_or("unknown")
            .into();

        if let Some(last) = merged.last_mut() {
            if *last.role == *role {
                last.parts.extend(parts_vec);
                continue;
            }
        }
        let full_model = match (
            msg.provider_id.as_ref().map(|s| s.0.clone()).or_else(|| {
                msg.model
                    .as_ref()
                    .and_then(|m| m.provider_id.as_ref().map(|s| s.0.clone()))
            }),
            msg.model_id.as_ref().map(|s| s.0.clone()).or_else(|| {
                msg.model
                    .as_ref()
                    .and_then(|m| m.model_id.as_ref().map(|s| s.0.clone()))
            }),
        ) {
            (Some(p), Some(m)) => Some(format!("{}/{}", p, m).into()),
            (None, Some(m)) => Some(m.into()),
            _ => None,
        };
        merged.push(ChatMessage {
            role,
            model: full_model,
            parts: parts_vec,
            is_subagent: false,
            agent_label: None,
        });
    }
    (merged, max_ts)
}

pub fn load_session_chat_with_max_ts(
    session_id: &str,
    files: Option<&[std::path::PathBuf]>,
    day_filter: Option<&str>,
) -> (Vec<ChatMessage>, i64) {
    load_session_chat_internal(Some(session_id), files, day_filter, None)
}

#[derive(Clone)]
pub struct ModelTokenStats {
    pub name: Box<str>,
    pub messages: u64,
    pub prompts: u64,
    pub tokens: Tokens,
    pub cost: f64,
}

#[derive(Clone, Default)]
pub struct SessionDetails {
    pub model_stats: Vec<ModelTokenStats>,
}

pub fn load_session_details(
    session_id: &str,
    files: Option<&[std::path::PathBuf]>,
    day_filter: Option<&str>,
    parent_map: &FxHashMap<Box<str>, Box<str>>,
) -> SessionDetails {
    struct MsgStats {
        model: Box<str>,
        is_user: bool,
        is_subagent: bool,
        tokens: Tokens,
        cost: f64,
    }

    #[inline]
    fn fold_msg(
        mut acc: FxHashMap<Box<str>, ModelTokenStats>,
        ms: MsgStats,
    ) -> FxHashMap<Box<str>, ModelTokenStats> {
        let entry = acc
            .entry(ms.model.clone())
            .or_insert_with(|| ModelTokenStats {
                name: ms.model,
                messages: 0,
                prompts: 0,
                tokens: Tokens::default(),
                cost: 0.0,
            });
        entry.messages += 1;
        if ms.is_user && !ms.is_subagent {
            entry.prompts += 1;
        }
        entry.cost += ms.cost;
        entry.tokens.input += ms.tokens.input;
        entry.tokens.output += ms.tokens.output;
        entry.tokens.reasoning += ms.tokens.reasoning;
        entry.tokens.cache_read += ms.tokens.cache_read;
        entry.tokens.cache_write += ms.tokens.cache_write;
        acc
    }

    fn reduce_maps(
        mut a: FxHashMap<Box<str>, ModelTokenStats>,
        b: FxHashMap<Box<str>, ModelTokenStats>,
    ) -> FxHashMap<Box<str>, ModelTokenStats> {
        for (k, v) in b {
            let entry = a.entry(k).or_insert_with(|| ModelTokenStats {
                name: v.name,
                messages: 0,
                prompts: 0,
                tokens: Tokens::default(),
                cost: 0.0,
            });
            entry.messages += v.messages;
            entry.prompts += v.prompts;
            entry.cost += v.cost;
            entry.tokens.input += v.tokens.input;
            entry.tokens.output += v.tokens.output;
            entry.tokens.reasoning += v.tokens.reasoning;
            entry.tokens.cache_read += v.tokens.cache_read;
            entry.tokens.cache_write += v.tokens.cache_write;
        }
        a
    }

    // Step 1: Load all messages in parallel
    let messages: Vec<Message> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let msg: Message = load_message_from_path(p)?;
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }
                Some(msg)
            })
            .collect()
    } else {
        let message_path = get_storage_path("message");
        let msg_files = list_message_files(Path::new(&message_path));
        msg_files
            .par_iter()
            .filter_map(|p| {
                let msg: Message = load_message_from_path(p)?;
                if msg.session_id.as_ref().map(|s| s.as_ref()) != Some(session_id) {
                    return None;
                }
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }
                Some(msg)
            })
            .collect()
    };

    // Step 2: Batch load ALL parts
    let msg_ids: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.id.as_ref().map(|id| id.0.as_str()))
        .filter(|id| !id.is_empty())
        .collect();
    let parts_map: FxHashMap<Box<str>, Vec<PartData>> = if is_db_mode() {
        batch_load_parts_db(&msg_ids)
    } else {
        let part_path_str = get_storage_path("part");
        let part_root = Path::new(&part_path_str);
        batch_load_parts_fs(&msg_ids, part_root)
    };

    // Step 3: Process messages with cached parts
    let model_map: FxHashMap<Box<str>, ModelTokenStats> = messages
        .into_par_iter()
        .map(|msg| {
            let role = msg.role.as_ref().map(|s| s.0.as_str()).unwrap_or("");
            let is_user = role == "user";
            let model_id = get_model_id(&msg);
            let mut tokens = Tokens::default();
            add_tokens(&mut tokens, &msg.tokens);
            let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);
            let is_subagent = msg
                .session_id
                .as_ref()
                .is_some_and(|sid| parent_map.contains_key(sid.as_str()));

            // Estimate reasoning tokens from cached parts if tokens.reasoning is 0
            if tokens.reasoning == 0 && !is_user {
                if let Some(msg_id) = msg.id.as_ref() {
                    if let Some(parts) = parts_map.get(msg_id.0.as_str()) {
                        let reasoning_chars: usize = parts
                            .iter()
                            .filter(|p| p.part_type.as_deref() == Some("reasoning"))
                            .filter_map(|p| p.text.as_ref().map(|t| t.len()))
                            .sum();
                        if reasoning_chars > 0 {
                            tokens.reasoning = (reasoning_chars / 4) as u64;
                        }
                    }
                }
            }

            MsgStats {
                model: model_id,
                is_user,
                is_subagent,
                tokens,
                cost,
            }
        })
        .fold(FxHashMap::default, fold_msg)
        .reduce(FxHashMap::default, reduce_maps);

    let mut model_stats: Vec<ModelTokenStats> = model_map.into_values().collect();
    model_stats.sort_unstable_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    SessionDetails { model_stats }
}

pub fn load_combined_session_chat(
    parent_session_id: &str,
    children: &[(Box<str>, Box<str>)],
    session_message_files: &FxHashMap<String, FxHashSet<std::path::PathBuf>>,
    day_filter: Option<&str>,
) -> (Vec<ChatMessage>, i64) {
    let mut all_files: Vec<std::path::PathBuf> = session_message_files
        .get(parent_session_id)
        .map(|f| f.iter().cloned().collect())
        .unwrap_or_default();
    let child_agent_map: FxHashMap<&str, &str> = children
        .iter()
        .map(|(id, name)| (id.as_ref(), name.as_ref()))
        .collect();
    for (child_id, _) in children {
        if let Some(files) = session_message_files.get(child_id.as_ref()) {
            all_files.extend(files.iter().cloned());
        }
    }
    if all_files.is_empty() {
        return (Vec::new(), 0);
    }

    // Load messages, filtering by day
    let mut filtered_msgs: Vec<(Message, bool, Option<Box<str>>)> = all_files
        .par_iter()
        .filter_map(|p| {
            let msg: Message = load_message_from_path(p)?;
            if let Some(target_day) = day_filter {
                let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                if msg_day != target_day {
                    return None;
                }
            }
            let msg_session = msg.session_id.as_ref().map(|s| s.0.as_str()).unwrap_or("");
            let (is_sub, agent_lbl) = if let Some(agent_name) = child_agent_map.get(msg_session) {
                (true, Some((*agent_name).to_string().into_boxed_str()))
            } else {
                (false, None)
            };
            Some((msg, is_sub, agent_lbl))
        })
        .collect();

    filtered_msgs.sort_unstable_by_key(|(m, _, _)| {
        m.time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0)
    });

    // Batch-load parts
    let all_messages: MessageBatch = if is_db_mode() {
        let msg_ids: Vec<&str> = filtered_msgs
            .iter()
            .filter_map(|(m, _, _)| m.id.as_ref().map(|id| id.0.as_str()))
            .filter(|id| !id.is_empty())
            .collect();
        let mut parts_map = batch_load_parts_db(&msg_ids);
        filtered_msgs
            .into_iter()
            .map(|(msg, is_sub, agent_lbl)| {
                let parts_vec = msg
                    .id
                    .as_ref()
                    .and_then(|id| parts_map.remove(id.0.as_str()))
                    .map(parts_to_content)
                    .unwrap_or_default();
                (msg, parts_vec, is_sub, agent_lbl)
            })
            .collect()
    } else {
        let part_path_str = get_storage_path("part");
        let part_root = std::path::Path::new(&part_path_str);
        let msg_ids: Vec<&str> = filtered_msgs
            .iter()
            .filter_map(|(m, _, _)| m.id.as_ref().map(|id| id.0.as_str()))
            .filter(|id| !id.is_empty())
            .collect();
        let mut parts_map = batch_load_parts_fs(&msg_ids, part_root);
        filtered_msgs
            .into_iter()
            .map(|(msg, is_sub, agent_lbl)| {
                let parts_vec = msg
                    .id
                    .as_ref()
                    .and_then(|id| parts_map.remove(id.0.as_str()))
                    .map(parts_to_content)
                    .unwrap_or_default();
                (msg, parts_vec, is_sub, agent_lbl)
            })
            .collect()
    };

    let mut max_ts: i64 = 0;
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(all_messages.len());
    let mut last_cumulative_diffs: Vec<FileDiff> = Vec::new();

    for (msg, mut parts_vec, is_sub, agent_lbl) in all_messages {
        let created = msg
            .time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0);
        if created > max_ts {
            max_ts = created;
        }

        let current_cumulative: Vec<FileDiff> = msg
            .summary
            .as_ref()
            .and_then(|s| s.diffs.as_ref())
            .map(|diffs| {
                let mut v: Vec<FileDiff> = diffs
                    .iter()
                    .map(|d| FileDiff {
                        path: d
                            .file
                            .as_ref()
                            .map(|s| s.0.clone())
                            .unwrap_or_default()
                            .into_boxed_str(),
                        additions: d.additions.map(|v| *v).unwrap_or(0),
                        deletions: d.deletions.map(|v| *v).unwrap_or(0),
                        status: d
                            .status
                            .as_ref()
                            .map(|s| s.0.clone())
                            .unwrap_or_else(|| "modified".into())
                            .into_boxed_str(),
                    })
                    .collect();
                sort_file_diffs(&mut v);
                v
            })
            .unwrap_or_else(|| last_cumulative_diffs.clone());

        let incremental = compute_incremental_diffs(&current_cumulative, &last_cumulative_diffs);
        last_cumulative_diffs = current_cumulative;

        match_tool_calls_with_diffs(&mut parts_vec, &incremental);

        let role: Box<str> = msg
            .role
            .as_ref()
            .map(|s| s.as_ref())
            .unwrap_or("unknown")
            .into();
        if let Some(last) = merged.last_mut() {
            if *last.role == *role && last.is_subagent == is_sub && last.agent_label == agent_lbl {
                last.parts.extend(parts_vec);
                continue;
            }
        }
        let full_model = match (
            msg.provider_id.as_ref().map(|s| s.0.clone()).or_else(|| {
                msg.model
                    .as_ref()
                    .and_then(|m| m.provider_id.as_ref().map(|s| s.0.clone()))
            }),
            msg.model_id.as_ref().map(|s| s.0.clone()).or_else(|| {
                msg.model
                    .as_ref()
                    .and_then(|m| m.model_id.as_ref().map(|s| s.0.clone()))
            }),
        ) {
            (Some(p), Some(m)) => Some(format!("{}/{}", p, m).into()),
            (None, Some(m)) => Some(m.into()),
            _ => None,
        };
        merged.push(ChatMessage {
            role,
            model: full_model,
            parts: parts_vec,
            is_subagent: is_sub,
            agent_label: agent_lbl,
        });
    }
    (merged, max_ts)
}

/// Build a compact one-line detail string from tool state input fields
fn build_tool_detail(tool_name: &str, input: &ToolStateInput) -> String {
    let lower = tool_name.to_ascii_lowercase();
    match lower.as_str() {
        "read" => {
            let fp = input
                .file_path
                .as_deref()
                .or(input.path.as_deref())
                .unwrap_or("");
            let range_str = match (&input.offset, &input.limit) {
                (Some(off), Some(lim)) => {
                    format!(" (offset {}, limit {})", json_num(off), json_num(lim))
                }
                (Some(off), None) => format!(" (offset {})", json_num(off)),
                (None, Some(lim)) => format!(" (limit {})", json_num(lim)),
                _ => " (full file)".to_string(),
            };
            format!("{}{}", short_path(fp), range_str)
        }
        "bash" | "shell" | "exec" | "terminal" => {
            let cmd = input
                .command
                .as_deref()
                .or(input.description.as_deref())
                .unwrap_or("");
            let first = cmd.lines().next().unwrap_or(cmd);
            first.to_string()
        }
        "grep" | "find" | "finder" => {
            let pat = input
                .pattern
                .as_deref()
                .or(input.query.as_deref())
                .unwrap_or("");
            let path = input.path.as_deref().or(input.file_path.as_deref());
            match path {
                Some(p) => format!("`{}` in {}", pat, short_path(p)),
                None => format!("`{}`", pat),
            }
        }
        "edit" | "edit_file" => {
            let fp = input.file_path.as_deref().unwrap_or("");
            let old_hint = input.old_str.as_deref().and_then(first_nonempty_line);
            let new_hint = input.new_str.as_deref().and_then(first_nonempty_line);
            match (old_hint, new_hint) {
                (Some(o), Some(n)) => format!(
                    "{} ↺ \"{}\" → \"{}\"",
                    short_path(fp),
                    truncate_inline(o, 24),
                    truncate_inline(n, 24)
                ),
                (Some(h), None) => format!("{} ↺ \"{}\"", short_path(fp), truncate_inline(h, 36)),
                (None, Some(h)) => format!("{} → \"{}\"", short_path(fp), truncate_inline(h, 36)),
                (None, None) => short_path(fp),
            }
        }
        "write" | "create" | "create_file" => {
            let fp = input.file_path.as_deref().unwrap_or("");
            if let Some(content) = input.content.as_deref().filter(|s| !s.is_empty()) {
                let lines = content.lines().count().max(1);
                format!("{} ({} lines)", short_path(fp), lines)
            } else {
                short_path(fp)
            }
        }
        "apply_patch" | "patch" | "apply" | "apply_diff" => {
            if let Some(patch) = input.patch_text.as_deref().filter(|s| !s.is_empty()) {
                let files = extract_patch_files(patch);
                if files.is_empty() {
                    "patch".to_string()
                } else {
                    let shown: Vec<String> = files.iter().take(2).map(|f| short_path(f)).collect();
                    let more = files.len().saturating_sub(shown.len());
                    if more > 0 {
                        format!("patch {} (+{} more)", shown.join(", "), more)
                    } else {
                        format!("patch {}", shown.join(", "))
                    }
                }
            } else {
                "patch".to_string()
            }
        }
        "todowrite" => summarize_todos(input.todos.as_ref()),
        "task" => {
            let desc = input.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                "task".to_string()
            } else {
                desc.to_string()
            }
        }
        _ => {
            // Fallback: show whatever fields we have (works for any MCP/plugin tool)
            let mut parts = Vec::new();
            if let Some(fp) = &input.file_path {
                parts.push(short_path(fp));
            }
            if let Some(p) = &input.pattern {
                parts.push(format!("`{}`", p));
            }
            if let Some(q) = &input.query {
                parts.push(truncate_inline(q, 60));
            }
            if let Some(c) = &input.command {
                parts.push(c.lines().next().unwrap_or("").to_string());
            }
            if let Some(u) = &input.url {
                parts.push(truncate_inline(u, 50));
            }
            if let Some(ids) = &input.ids {
                parts.push(format!("{} items", ids.len()));
            }
            if parts.is_empty() {
                String::new()
            } else {
                parts.join(" ")
            }
        }
    }
}

fn json_num(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) => {
            let nums: Vec<String> = arr.iter().map(json_num).collect();
            nums.join(", ")
        }
        _ => v.to_string(),
    }
}

/// Short path - show last 2 components
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.rsplit('/').take(2).collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[1], parts[0])
    } else {
        p.to_string()
    }
}

/// Truncate a string to max chars with ellipsis
fn truncate_inline(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    let target = max_chars.saturating_sub(1);
    let byte_pos = s
        .char_indices()
        .nth(target)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}…", &s[..byte_pos])
}

fn first_nonempty_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).find(|line| !line.is_empty())
}

fn summarize_todos(todos: Option<&serde_json::Value>) -> String {
    let Some(serde_json::Value::Array(items)) = todos else {
        return "todo update".to_string();
    };
    if items.is_empty() {
        return "todo update (0 items)".to_string();
    }

    let mut pending = 0usize;
    let mut in_progress = 0usize;
    let mut completed = 0usize;
    let mut cancelled = 0usize;
    let mut examples: Vec<String> = Vec::new();

    for item in items {
        let status = item
            .as_object()
            .and_then(|o| o.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match status {
            "pending" => pending += 1,
            "in_progress" => in_progress += 1,
            "completed" => completed += 1,
            "cancelled" => cancelled += 1,
            _ => {}
        }

        if examples.len() < 2 {
            if let Some(content) = item
                .as_object()
                .and_then(|o| o.get("content"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let prefix = match status {
                    "in_progress" => "▶",
                    "completed" => "✓",
                    "cancelled" => "✕",
                    _ => "•",
                };
                examples.push(format!("{} {}", prefix, truncate_inline(content, 32)));
            }
        }
    }

    let mut parts = Vec::new();
    if in_progress > 0 {
        parts.push(format!("{} in-progress", in_progress));
    }
    if pending > 0 {
        parts.push(format!("{} pending", pending));
    }
    if completed > 0 {
        parts.push(format!("{} completed", completed));
    }
    if cancelled > 0 {
        parts.push(format!("{} cancelled", cancelled));
    }

    if !examples.is_empty() {
        let extra = items.len().saturating_sub(examples.len());
        let tail = if extra > 0 {
            format!("; +{} more", extra)
        } else {
            String::new()
        };
        format!("{} todos: {}{}", items.len(), examples.join("; "), tail)
    } else if parts.is_empty() {
        format!("todo update ({} items)", items.len())
    } else {
        format!("{} todos ({})", items.len(), parts.join(", "))
    }
}

fn infer_tool_file_path(tool_name: &str, input: &ToolStateInput) -> Option<String> {
    if let Some(fp) = input.file_path.as_ref().or(input.path.as_ref()) {
        if !fp.trim().is_empty() {
            return Some(fp.clone());
        }
    }

    let lower = tool_name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "apply_patch" | "patch" | "apply" | "apply_diff"
    ) {
        if let Some(patch) = input.patch_text.as_deref() {
            return extract_patch_files(patch).into_iter().next();
        }
    }
    None
}

fn extract_patch_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        let trimmed = line.trim_start();
        for marker in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
            if let Some(rest) = trimmed.strip_prefix(marker) {
                let p = rest.trim();
                if !p.is_empty() {
                    files.push(p.to_string());
                }
                break;
            }
        }
    }
    files
}

fn truncate_string(s: &str, max: usize) -> Box<str> {
    let char_count = s.chars().count();
    if char_count <= max {
        return s.into();
    }
    let target = max.saturating_sub(3); // Reserve space for "..."
    let byte_pos = s
        .char_indices()
        .nth(target)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..byte_pos]).into_boxed_str()
}

/// Match tool calls with incremental file diffs - assigns additions/deletions to tool calls
fn match_tool_calls_with_diffs(parts: &mut [MessageContent], incremental: &[FileDiff]) {
    for part in parts.iter_mut() {
        if let MessageContent::ToolCall(ref mut tc) = part {
            if let Some(ref fp_str) = tc.file_path {
                let fp_name = fp_str.rsplit('/').next().unwrap_or(fp_str);
                // Get last 2 path components without Vec allocation
                let mut fp_parts: [&str; 2] = ["", ""];
                for (fp_idx, seg) in fp_str.rsplit('/').take(2).enumerate() {
                    fp_parts[1 - fp_idx] = seg;
                }
                for d in incremental {
                    let d_path_str = &d.path;
                    let mut d_parts: [&str; 2] = ["", ""];
                    for (d_idx, seg) in d_path_str.rsplit('/').take(2).enumerate() {
                        d_parts[1 - d_idx] = seg;
                    }
                    if fp_parts == d_parts {
                        tc.additions = Some(d.additions);
                        tc.deletions = Some(d.deletions);
                        break;
                    }
                    let d_name = d_path_str.rsplit('/').next().unwrap_or(d_path_str);
                    if d_name == fp_name {
                        tc.additions = Some(d.additions);
                        tc.deletions = Some(d.deletions);
                    }
                }
            } else {
                // Fallback for patch-like tools that often do not carry file_path
                let tool_name = tc.name.to_ascii_lowercase();
                if matches!(
                    tool_name.as_str(),
                    "apply_patch" | "patch" | "apply" | "apply_diff"
                ) {
                    let adds: u64 = incremental.iter().map(|d| d.additions).sum();
                    let dels: u64 = incremental.iter().map(|d| d.deletions).sum();
                    if adds > 0 || dels > 0 {
                        tc.additions = Some(adds);
                        tc.deletions = Some(dels);
                    }
                }
            }
        }
    }
}

fn deserialize_lenient_summary<'de, D>(deserializer: D) -> Result<Option<Summary>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Visitor;

    struct SummaryVisitor;

    impl<'de> Visitor<'de> for SummaryVisitor {
        type Value = Option<Summary>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a summary object, boolean, or null")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Some(Summary::default()))
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            use serde::Deserialize;
            let s = Summary::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(s))
        }
    }

    deserializer.deserialize_any(SummaryVisitor)
}

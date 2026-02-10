use fxhash::{FxHashMap, FxHashSet};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, OnceLock};

// Fast path constants for performance
pub const MAX_MESSAGES_TO_LOAD: usize = 100;
const MAX_CHARS_PER_TEXT_PART: usize = 500;
const MAX_TOOL_TITLE_CHARS: usize = 100;

static HOME_DIR: OnceLock<String> = OnceLock::new();

#[inline]
fn get_home() -> &'static str {
    HOME_DIR.get_or_init(|| env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

#[inline]
pub fn get_storage_path(subdir: &str) -> String {
    format!("{}/.local/share/opencode/storage/{}", get_home(), subdir)
}

impl Totals {
    pub fn display_cost(&self) -> f64 {
        self.cost
    }
}

impl DayStat {
    pub fn display_cost(&self) -> f64 {
        self.cost
    }
}

impl ModelUsage {}

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
    // Session continuation tracking
    pub original_session_id: Option<Box<str>>,
    pub first_created_date: Option<Box<str>>,
    pub is_continuation: bool,
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
            file_diffs: Vec::new(),
            original_session_id: None,
            first_created_date: None,
            is_continuation: false,
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
    pub sessions: HashMap<String, Arc<SessionStat>>,
    pub cost: f64,
}

impl Default for DayStat {
    fn default() -> Self {
        Self {
            messages: 0,
            prompts: 0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            sessions: HashMap::with_capacity(4),
            cost: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Totals {
    pub sessions: HashSet<Box<str>>,
    pub messages: u64,
    pub prompts: u64,
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub tools: HashMap<Box<str>, u64>,
    pub cost: f64,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            sessions: HashSet::with_capacity(16),
            messages: 0,
            prompts: 0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            tools: HashMap::with_capacity(16),
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
}

/// Key for session-day lookups.
/// Uses String for now to avoid complex Borrow implementation for (str, str).
pub type SessDayKey = String;

fn make_sess_day_key(session: &str, day: &str) -> SessDayKey {
    format!("{}|{}", session, day)
}

impl Stats {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub name: Box<str>,
    pub short_name: Box<str>,
    pub provider: Box<str>,
    pub display_name: Box<str>,
    pub messages: u64,
    pub sessions: HashSet<Box<str>>,
    pub tokens: Tokens,
    pub tools: HashMap<Box<str>, u64>,
    pub agents: HashMap<Box<str>, u64>,
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
    pub title: Option<Box<str>>,
    pub file_path: Option<Box<str>>,
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
}

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
// for per-day breakdown (since session_diff only has final state)
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
}

#[derive(Deserialize, Default)]
pub(crate) struct ToolStateInput {
    #[serde(rename = "filePath")]
    pub(crate) file_path: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct ToolState {
    pub(crate) input: Option<ToolStateInput>,
    pub(crate) title: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct PartData {
    #[serde(rename = "type")]
    pub(crate) part_type: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) tool: Option<String>,
    pub(crate) thought: Option<String>,
    pub(crate) state: Option<ToolState>,
}

#[inline]
pub fn format_duration_ms(first_ms: i64, last_ms: i64) -> Option<String> {
    if first_ms >= last_ms || first_ms == i64::MAX || last_ms == 0 {
        return None;
    }
    let total_secs = ((last_ms - first_ms) / 1000) as u64;
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    Some(if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    })
}

#[inline]
pub fn format_number(value: u64) -> String {
    // Optimized: use integer division for K case to avoid float conversion
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

    // Fast path: numbers with 3 or fewer digits don't need commas
    if len <= 3 {
        return s;
    }

    // Optimized: use byte operations since to_string() always produces ASCII
    // and pre-allocate correct capacity to avoid reallocations
    let mut result = String::with_capacity(len + (len - 1) / 3);
    let bytes = s.as_bytes();

    for (i, &byte) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
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

/// Detect if a session is a continuation from a previous day
/// Returns (original_session_id, first_created_date) if continuation detected
#[inline]
fn detect_session_continuation(
    session_id: &str,
    current_day: &str,
    all_session_first_days: &FxHashMap<String, String>,
) -> (Option<Box<str>>, Option<Box<str>>) {
    // Check if this session was first seen on a different day
    if let Some(first_day) = all_session_first_days.get(session_id) {
        if first_day != current_day {
            // This is a continuation - session started on a different day
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

fn list_message_files(root: &Path) -> Vec<std::path::PathBuf> {
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

fn load_session_titles() -> FxHashMap<Box<str>, String> {
    let session_path = get_storage_path("session");
    let root = Path::new(&session_path);
    let Ok(entries) = fs::read_dir(root) else {
        return FxHashMap::default();
    };

    // Collect entries first for better parallel distribution
    let top_entries: Vec<_> = entries.flatten().collect();

    top_entries
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
                                Some((
                                    session.id.map(|s| s.0).unwrap_or_default().into_boxed_str(),
                                    session.title.map(|s| s.0).unwrap_or_default(),
                                ))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else if path.extension().is_some_and(|e| e == "json") {
                fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<SessionData>(&bytes).ok())
                    .map(|session| {
                        vec![(
                            session.id.map(|s| s.0).unwrap_or_default().into_boxed_str(),
                            session.title.map(|s| s.0).unwrap_or_default(),
                        )]
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        })
        .collect()
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
    let Ok(entries) = fs::read_dir(root) else {
        return FxHashMap::default();
    };

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
}

/// Compute incremental diffs: current cumulative state minus previous cumulative state
/// For each file in current, subtract the values from previous (if present)
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

pub fn collect_stats() -> Stats {
    let mut totals = Totals::default();
    let session_titles = load_session_titles();
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
    // Track first day each session was seen for continuation detection
    let mut session_first_days: FxHashMap<String, String> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());

    struct FullMessageData {
        msg: Message,
        tools: Vec<Box<str>>,
        path: std::path::PathBuf,
        message_id: Box<str>,
        cumulative_diffs: Vec<FileDiff>, // Cumulative diff state from summary.diffs
    }

    let mut processed_data: Vec<FullMessageData> = msg_files
        .par_iter()
        .filter_map(|p| {
            let bytes = fs::read(p).ok()?;
            let msg: Message = serde_json::from_slice(&bytes).ok()?;

            let message_id = match &msg.id {
                Some(id) if !id.0.is_empty() => id.0.clone().into_boxed_str(),
                _ => p.to_string_lossy().to_string().into_boxed_str(),
            };

            let mut tools = Vec::new();
            if let Some(id) = &msg.id {
                let id_str = &id.0;
                if !id_str.is_empty() {
                    let part_dir = part_root.join(id_str);
                    if let Ok(entries) = fs::read_dir(part_dir) {
                        for entry in entries.flatten() {
                            if let Ok(bytes) = fs::read(entry.path()) {
                                if let Ok(part) = serde_json::from_slice::<PartData>(&bytes) {
                                    if part.part_type.as_deref() == Some("tool") {
                                        if let Some(tool) = part.tool {
                                            tools.push(tool.into());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Extract cumulative diff state from message summary
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

            Some(FullMessageData {
                msg,
                tools,
                path: p.clone(),
                message_id,
                cumulative_diffs,
            })
        })
        .collect();

    processed_data.sort_unstable_by_key(|d| {
        d.msg
            .time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0)
    });

    // Track union of latest per-file cumulative diff state per session per day
    // Key: SessDayKey { session, day } -> { file_path -> latest FileDiff seen that day }
    let mut session_day_union_diffs: FxHashMap<SessDayKey, FxHashMap<Box<str>, FileDiff>> =
        FxHashMap::with_capacity_and_hasher(64, Default::default());

    let mut last_ts = None;
    let mut last_day_str = String::new();

    for data in processed_data {
        // Skip duplicate messages (same message_id seen before)
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

        // Track first day session was seen for continuation detection
        if !session_id_boxed.is_empty()
            && !session_first_days.contains_key(session_id_boxed.as_ref())
        {
            session_first_days.insert(session_id_boxed.to_string(), day.clone());
        }

        // Optimization: only insert if not already present (avoid allocation on common path)
        if !session_id_boxed.is_empty() && !totals.sessions.contains(session_id_boxed.as_ref()) {
            totals.sessions.insert(session_id_boxed.clone());
        }
        totals.messages += 1;
        if is_user {
            totals.prompts += 1;
        }
        totals.cost += cost;
        add_tokens(&mut totals.tokens, &msg.tokens);

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
                    sessions: HashSet::new(),
                    tokens: Tokens::default(),
                    tools: HashMap::new(),
                    agents: HashMap::new(),
                    cost: 0.0,
                }
            });
            model_entry.messages += 1;
            if !session_id_boxed.is_empty()
                && !model_entry.sessions.contains(session_id_boxed.as_ref())
            {
                model_entry.sessions.insert(session_id_boxed.clone());
            }
            model_entry.cost += cost;
            add_tokens(&mut model_entry.tokens, &msg.tokens);
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
        if is_user {
            day_stat.prompts += 1;
        }
        day_stat.cost += cost;
        add_tokens(&mut day_stat.tokens, &msg.tokens);

        // Get or create the session for THIS specific day (each day has its own session entry)
        // Optimization: avoid allocation if session already exists for this day
        let session_stat_arc = if let Some(existing) =
            day_stat.sessions.get_mut(session_id_boxed.as_ref())
        {
            existing
        } else {
            // Detect if this is a continuation from a previous day
            let (original_id, first_created) = if !session_id_boxed.is_empty() {
                detect_session_continuation(session_id_boxed.as_ref(), &day, &session_first_days)
            } else {
                (None, None)
            };

            let is_continued = original_id.is_some();
            let mut stat = SessionStat::new(session_id_boxed.clone());
            stat.original_session_id = original_id;
            stat.first_created_date = first_created;
            stat.is_continuation = is_continued;
            day_stat
                .sessions
                .insert(session_id_boxed.to_string(), Arc::new(stat));
            day_stat
                .sessions
                .get_mut(session_id_boxed.as_ref())
                .unwrap()
        };

        // Accumulate data for this day's session (separate from other days)
        let session_stat = Arc::make_mut(session_stat_arc);
        session_stat.messages += 1;
        if is_user {
            session_stat.prompts += 1;
        }
        session_stat.cost += cost;
        if is_assistant {
            session_stat.models.insert(model_id.clone());
        }
        add_tokens(&mut session_stat.tokens, &msg.tokens);
        if let Some(t) = ts_val {
            if t < session_stat.first_activity {
                session_stat.first_activity = t;
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

        // Accumulate per-file diffs from ALL messages of each session/day (union-of-latest)
        // Each message's summary.diffs only lists files from that edit, so we merge
        // across all messages to get the complete picture
        if !session_id_boxed.is_empty() {
            let key = make_sess_day_key(session_id_boxed.as_ref(), day.as_str());
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

    // Precompute diff totals from session_diff_map for global totals
    let precomputed_diff_totals: FxHashMap<String, (u64, u64)> = session_diff_map
        .iter()
        .map(|(id, diffs)| {
            let adds: u64 = diffs.iter().map(|d| d.additions).sum();
            let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
            (id.clone(), (adds, dels))
        })
        .collect();

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

    Stats {
        totals,
        per_day,
        session_titles,
        model_usage,
        session_message_files,
        processed_message_ids,
    }
}

fn load_session_chat_internal(
    session_id: Option<&str>,
    files: Option<&[std::path::PathBuf]>,
    day_filter: Option<&str>,
    since_ts: Option<i64>,
    apply_limit: bool,
) -> (Vec<ChatMessage>, i64) {
    let part_path_str = get_storage_path("part");
    let part_root = Path::new(&part_path_str);

    let mut session_msgs: Vec<Message> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;

                // Filter by day if specified
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }

                // Filter by timestamp if specified
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
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;

                if let Some(session_id) = session_id {
                    if msg.session_id.as_ref().map(|s| s.as_ref()) != Some(session_id) {
                        return None;
                    }
                }

                // Filter by day if specified
                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }

                // Filter by timestamp if specified
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

    if apply_limit && session_msgs.len() > MAX_MESSAGES_TO_LOAD {
        let start = session_msgs.len() - MAX_MESSAGES_TO_LOAD;
        session_msgs.drain(..start);
    }

    // Now load parts in parallel for only the selected messages
    let session_msgs_with_parts: Vec<(Message, Vec<MessageContent>)> = session_msgs
        .into_par_iter()
        .map(|msg| {
            let mut parts_vec = Vec::new();
            if let Some(id) = &msg.id {
                let id_str = &id.0;
                if !id_str.is_empty() {
                    if let Ok(entries) = fs::read_dir(part_root.join(id_str)) {
                        let mut p_files: Vec<_> = entries.flatten().collect();
                        p_files.sort_by_key(|e| e.path());
                        for e in p_files {
                            if let Ok(bytes) = fs::read(e.path()) {
                                if let Ok(part) = serde_json::from_slice::<PartData>(&bytes) {
                                    if part.thought.is_some() {
                                        parts_vec.push(MessageContent::Thinking(()));
                                    }
                                    if let Some(t) = part.text {
                                        parts_vec.push(MessageContent::Text(truncate_string(
                                            &t,
                                            MAX_CHARS_PER_TEXT_PART,
                                        )));
                                    }
                                    if let Some(tool) = part.tool {
                                        let fp = part
                                            .state
                                            .as_ref()
                                            .and_then(|s| {
                                                s.input.as_ref().and_then(|i| i.file_path.as_ref())
                                            })
                                            .map(|s| s.clone().into());
                                        let title = part
                                            .state
                                            .as_ref()
                                            .and_then(|s| s.title.as_ref())
                                            .map(|t| truncate_string(t, MAX_TOOL_TITLE_CHARS));
                                        parts_vec.push(MessageContent::ToolCall(ToolCallInfo {
                                            name: tool.into(),
                                            title,
                                            file_path: fp,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (msg, parts_vec)
        })
        .collect();

    let mut max_ts = since_ts.unwrap_or(0);
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(session_msgs_with_parts.len());
    for (msg, parts_vec) in session_msgs_with_parts {
        let created = msg
            .time
            .as_ref()
            .and_then(|t| t.created.map(|v| *v))
            .unwrap_or(0);
        if created > max_ts {
            max_ts = created;
        }

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
        });
    }
    (merged, max_ts)
}

pub fn load_session_chat_with_max_ts(
    session_id: &str,
    files: Option<&[std::path::PathBuf]>,
    day_filter: Option<&str>,
) -> (Vec<ChatMessage>, i64) {
    load_session_chat_internal(Some(session_id), files, day_filter, None, true)
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
    day_filter: Option<&str>, // Only load messages from this specific day
) -> SessionDetails {
    struct MsgStats {
        model: Box<str>,
        is_user: bool,
        tokens: Tokens,
        cost: f64,
    }

    #[inline]
    fn fold_msg(
        mut acc: HashMap<Box<str>, ModelTokenStats>,
        ms: MsgStats,
    ) -> HashMap<Box<str>, ModelTokenStats> {
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
        if ms.is_user {
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
        mut a: HashMap<Box<str>, ModelTokenStats>,
        b: HashMap<Box<str>, ModelTokenStats>,
    ) -> HashMap<Box<str>, ModelTokenStats> {
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

    fn parse_msg(msg: &Message) -> (Box<str>, bool, Tokens, f64) {
        let role = msg.role.as_ref().map(|s| s.0.as_str()).unwrap_or("");
        let is_user = role == "user";
        let model_id = get_model_id(msg);
        let mut tokens = Tokens::default();
        add_tokens(&mut tokens, &msg.tokens);
        let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);
        (model_id, is_user, tokens, cost)
    }

    let model_map: HashMap<Box<str>, ModelTokenStats> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;

                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }

                let (model, is_user, tokens, cost) = parse_msg(&msg);
                Some(MsgStats {
                    model,
                    is_user,
                    tokens,
                    cost,
                })
            })
            .fold(HashMap::new, fold_msg)
            .reduce(HashMap::new, reduce_maps)
    } else {
        let message_path = get_storage_path("message");
        let msg_files = list_message_files(Path::new(&message_path));
        msg_files
            .par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;
                if msg.session_id.as_ref().map(|s| s.as_ref()) != Some(session_id) {
                    return None;
                }

                if let Some(target_day) = day_filter {
                    let msg_day = get_day(msg.time.as_ref().and_then(|t| t.created.map(|v| *v)));
                    if msg_day != target_day {
                        return None;
                    }
                }

                let (model, is_user, tokens, cost) = parse_msg(&msg);
                Some(MsgStats {
                    model,
                    is_user,
                    tokens,
                    cost,
                })
            })
            .fold(HashMap::new, fold_msg)
            .reduce(HashMap::new, reduce_maps)
    };

    let mut model_stats: Vec<ModelTokenStats> = model_map.into_values().collect();
    model_stats.sort_unstable_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    SessionDetails { model_stats }
}

#[inline]
fn truncate_string(s: &str, max: usize) -> Box<str> {
    if s.len() <= max {
        return s.into();
    }

    let mut char_count = 0;
    for (idx, _) in s.char_indices() {
        if char_count == max {
            let mut result = String::with_capacity(idx + 3);
            result.push_str(&s[..idx]);
            result.push_str("...");
            return result.into_boxed_str();
        }
        char_count += 1;
    }
    s.into()
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

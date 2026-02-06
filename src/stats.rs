use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, OnceLock};

// Fast path constants for performance
const MAX_MESSAGES_TO_LOAD: usize = 100;
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

impl ModelUsage {
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
    pub cost: f64,
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub models: HashSet<Box<str>>,
    pub tools: HashMap<Box<str>, u64>,
    pub last_activity: i64,
    pub path_cwd: Box<str>,
    pub path_root: Box<str>,
    pub file_diffs: Vec<FileDiff>,
}

impl SessionStat {
    pub fn new(id: impl Into<Box<str>>) -> Self {
        Self {
            id: id.into(),
            messages: 0,
            cost: 0.0,
            tokens: Tokens::default(),
            diffs: Diffs::default(),
            models: HashSet::new(),
            tools: HashMap::new(),
            last_activity: 0,
            path_cwd: String::new().into_boxed_str(),
            path_root: String::new().into_boxed_str(),
            file_diffs: Vec::new(),
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
    pub tokens: Tokens,
    pub diffs: Diffs,
    pub sessions: HashMap<String, Arc<SessionStat>>,
    pub cost: f64,
}

impl Default for DayStat {
    fn default() -> Self {
        Self {
            messages: 0,
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
    pub per_day: HashMap<String, DayStat>,
    pub session_titles: HashMap<String, String>,
    pub model_usage: Vec<ModelUsage>,
    #[serde(default)]
    pub session_message_files: HashMap<String, Vec<std::path::PathBuf>>,
    #[serde(default)]
    pub processed_message_ids: HashSet<Box<str>>,
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
    let mut result = String::with_capacity(len + (len - 1) / 3);
    let bytes = s.as_bytes();

    for (i, &byte) in bytes.iter().enumerate() {
        // Add comma before every 3rd digit from the right
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        // Safe to use as char since we know it's ASCII
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
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "Unknown".into())
        }
        None => "Unknown".into(),
    }
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

fn load_session_titles() -> HashMap<String, String> {
    let session_path = get_storage_path("session");
    let root = Path::new(&session_path);
    let Ok(entries) = fs::read_dir(root) else {
        return HashMap::new();
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
                                    session.id.map(|s| s.0).unwrap_or_default(),
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
                            session.id.map(|s| s.0).unwrap_or_default(),
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

fn load_session_diff_map() -> HashMap<String, Vec<FileDiff>> {
    let diff_path = get_storage_path("session_diff");
    let root = Path::new(&diff_path);
    let Ok(entries) = fs::read_dir(root) else {
        return HashMap::new();
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

pub fn collect_stats() -> Stats {
    let mut totals = Totals::default();
    let session_titles = load_session_titles();
    let session_diff_map = load_session_diff_map();
    let message_path = get_storage_path("message");
    let part_path_str = get_storage_path("part");
    let part_root = Path::new(&part_path_str);
    let msg_files = list_message_files(Path::new(&message_path));

    let mut per_day: HashMap<String, DayStat> = HashMap::with_capacity(msg_files.len() / 20);
    let mut model_stats: HashMap<Box<str>, ModelUsage> = HashMap::with_capacity(8);
    let mut session_message_files: HashMap<String, Vec<std::path::PathBuf>> =
        HashMap::with_capacity(128);
    let mut processed_message_ids: HashSet<Box<str>> = HashSet::with_capacity(msg_files.len());

    struct FullMessageData {
        msg: Message,
        tools: Vec<Box<str>>,
        path: std::path::PathBuf,
        message_id: Box<str>,
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

            Some(FullMessageData {
                msg,
                tools,
                path: p.clone(),
                message_id,
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

    for data in processed_data {
        processed_message_ids.insert(data.message_id);
        let msg = &data.msg;
        let session_id: String = msg
            .session_id
            .as_ref()
            .map(|s| s.0.clone())
            .unwrap_or_default();

        if !session_id.is_empty() {
            session_message_files
                .entry(session_id.clone())
                .or_default()
                .push(data.path);
        }

        let ts = msg.time.as_ref().and_then(|t| t.created.map(|v| *v));
        let day = get_day(ts);
        let model_id = get_model_id(msg);
        let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);

        totals.sessions.insert(session_id.clone().into_boxed_str());
        totals.messages += 1;
        totals.cost += cost;
        add_tokens(&mut totals.tokens, &msg.tokens);

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
        model_entry
            .sessions
            .insert(session_id.clone().into_boxed_str());
        model_entry.cost += cost;
        add_tokens(&mut model_entry.tokens, &msg.tokens);
        if let Some(agent) = msg.agent.as_ref().map(|s| s.0.as_str()).filter(|s| !s.is_empty())
        {
            *model_entry
                .agents
                .entry(agent.to_string().into_boxed_str())
                .or_insert(0) += 1;
        }

        let day_stat = per_day.entry(day).or_default();
        day_stat.messages += 1;
        day_stat.cost += cost;
        add_tokens(&mut day_stat.tokens, &msg.tokens);

        let session_stat_arc = day_stat
            .sessions
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(SessionStat::new(session_id.clone())));
        let session_stat = Arc::make_mut(session_stat_arc);
        session_stat.messages += 1;
        session_stat.cost += cost;
        session_stat.models.insert(model_id);
        add_tokens(&mut session_stat.tokens, &msg.tokens);
        if let Some(t) = ts {
            if t > session_stat.last_activity {
                session_stat.last_activity = t;
            }
        }

        for t_box in data.tools {
            *totals.tools.entry(t_box.clone()).or_insert(0) += 1;
            *session_stat.tools.entry(t_box.clone()).or_insert(0) += 1;
            *model_entry.tools.entry(t_box).or_insert(0) += 1;
        }

        if let Some(p) = &msg.path {
            if let Some(cwd) = &p.cwd {
                session_stat.path_cwd = cwd.clone().into();
            }
            if let Some(root) = &p.root {
                session_stat.path_root = root.clone().into();
            }
        }

        if let Some(summary) = &msg.summary {
            if let Some(diffs) = &summary.diffs {
                for item in diffs {
                    let path = item.file.as_ref().map(|s| s.as_ref()).unwrap_or("unknown");
                    let adds = item.additions.map(|v| *v).unwrap_or(0);
                    let dels = item.deletions.map(|v| *v).unwrap_or(0);
                    let status = item
                        .status
                        .as_ref()
                        .map(|s| s.as_ref())
                        .unwrap_or("modified");
                    if let Some(e) = session_stat
                        .file_diffs
                        .iter_mut()
                        .find(|d| &*d.path == path)
                    {
                        e.additions += adds;
                        e.deletions += dels;
                        if status != "modified" {
                            e.status = status.into();
                        }
                    } else {
                        session_stat.file_diffs.push(FileDiff {
                            path: path.into(),
                            additions: adds,
                            deletions: dels,
                            status: status.into(),
                        });
                    }
                }
            }
        }
    }

    for day in per_day.values_mut() {
        for sess_arc in day.sessions.values_mut() {
            let sess = Arc::make_mut(sess_arc);
            if let Some(diffs) = session_diff_map.get(sess.id.as_ref()) {
                sess.file_diffs = diffs.clone();
            }
            sort_file_diffs(&mut sess.file_diffs);
            sess.diffs.additions = sess.file_diffs.iter().map(|d| d.additions).sum();
            sess.diffs.deletions = sess.file_diffs.iter().map(|d| d.deletions).sum();
            day.diffs.additions += sess.diffs.additions;
            day.diffs.deletions += sess.diffs.deletions;
            totals.diffs.additions += sess.diffs.additions;
            totals.diffs.deletions += sess.diffs.deletions;
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

pub fn load_session_chat(
    session_id: &str,
    files: Option<&[std::path::PathBuf]>,
) -> Vec<ChatMessage> {
    let part_path_str = get_storage_path("part");
    let part_root = Path::new(&part_path_str);

    let mut session_msgs: Vec<Message> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;
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

                if msg.session_id.as_ref().map(|s| s.as_ref()) != Some(session_id) {
                    return None;
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
    if session_msgs.len() > MAX_MESSAGES_TO_LOAD {
        let start = session_msgs.len() - MAX_MESSAGES_TO_LOAD;
        session_msgs.drain(..start);
    }

    let mut merged: Vec<ChatMessage> = Vec::with_capacity(session_msgs.len());
    for msg in session_msgs {
        let role: Box<str> = msg
            .role
            .as_ref()
            .map(|s| s.as_ref())
            .unwrap_or("unknown")
            .into();
        let mut parts_vec = Vec::new();
        if let Some(id) = msg.id {
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
    merged
}

#[derive(Clone)]
pub struct ModelTokenStats {
    pub name: Box<str>,
    pub messages: u64,
    pub tokens: Tokens,
}

#[derive(Clone, Default)]
pub struct SessionDetails {
    pub model_stats: Vec<ModelTokenStats>,
}

pub fn load_session_details(
    session_id: &str,
    files: Option<&[std::path::PathBuf]>,
) -> SessionDetails {
    let model_map: HashMap<Box<str>, ModelTokenStats> = if let Some(f) = files {
        f.par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: Message = serde_json::from_slice(&bytes).ok()?;
                let model_id = get_model_id(&msg);
                let mut tokens = Tokens::default();
                add_tokens(&mut tokens, &msg.tokens);
                Some((model_id, tokens))
            })
            .fold(
                HashMap::new,
                |mut acc: HashMap<Box<str>, ModelTokenStats>, (model, tokens)| {
                    let entry = acc.entry(model.clone()).or_insert_with(|| ModelTokenStats {
                        name: model,
                        messages: 0,
                        tokens: Tokens::default(),
                    });
                    entry.messages += 1;
                    entry.tokens.input += tokens.input;
                    entry.tokens.output += tokens.output;
                    entry.tokens.reasoning += tokens.reasoning;
                    entry.tokens.cache_read += tokens.cache_read;
                    entry.tokens.cache_write += tokens.cache_write;
                    acc
                },
            )
            .reduce(
                HashMap::new,
                |mut a: HashMap<Box<str>, ModelTokenStats>, b| {
                    for (k, v) in b {
                        let entry = a.entry(k).or_insert_with(|| ModelTokenStats {
                            name: v.name,
                            messages: 0,
                            tokens: Tokens::default(),
                        });
                        entry.messages += v.messages;
                        entry.tokens.input += v.tokens.input;
                        entry.tokens.output += v.tokens.output;
                        entry.tokens.reasoning += v.tokens.reasoning;
                        entry.tokens.cache_read += v.tokens.cache_read;
                        entry.tokens.cache_write += v.tokens.cache_write;
                    }
                    a
                },
            )
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

                let model_id = get_model_id(&msg);
                let mut tokens = Tokens::default();
                add_tokens(&mut tokens, &msg.tokens);

                Some((model_id, tokens))
            })
            .fold(
                HashMap::new,
                |mut acc: HashMap<Box<str>, ModelTokenStats>, (model, tokens)| {
                    let entry = acc.entry(model.clone()).or_insert_with(|| ModelTokenStats {
                        name: model,
                        messages: 0,
                        tokens: Tokens::default(),
                    });
                    entry.messages += 1;
                    entry.tokens.input += tokens.input;
                    entry.tokens.output += tokens.output;
                    entry.tokens.reasoning += tokens.reasoning;
                    entry.tokens.cache_read += tokens.cache_read;
                    entry.tokens.cache_write += tokens.cache_write;
                    acc
                },
            )
            .reduce(
                HashMap::new,
                |mut a: HashMap<Box<str>, ModelTokenStats>, b| {
                    for (k, v) in b {
                        let entry = a.entry(k).or_insert_with(|| ModelTokenStats {
                            name: v.name,
                            messages: 0,
                            tokens: Tokens::default(),
                        });
                        entry.messages += v.messages;
                        entry.tokens.input += v.tokens.input;
                        entry.tokens.output += v.tokens.output;
                        entry.tokens.reasoning += v.tokens.reasoning;
                        entry.tokens.cache_read += v.tokens.cache_read;
                        entry.tokens.cache_write += v.tokens.cache_write;
                    }
                    a
                },
            )
    };

    let mut model_stats: Vec<ModelTokenStats> = model_map.into_values().collect();
    model_stats.sort_unstable_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    SessionDetails { model_stats }
}

#[inline]
fn truncate_string(s: &str, max: usize) -> Box<str> {
    // Optimized: early return if no truncation needed
    if s.len() <= max {
        return s.into();
    }

    // Fast path for ASCII strings (most common case)
    if s.is_ascii() {
        // For ASCII, byte length equals char count
        if s.len() <= max {
            return s.into();
        }
        // Truncate at byte position (safe for ASCII)
        let end = max.min(s.len());
        let mut result = String::with_capacity(end + 3);
        result.push_str(&s[..end]);
        result.push_str("...");
        return result.into_boxed_str();
    }

    // Slow path for non-ASCII: count chars properly
    if s.chars().count() <= max {
        return s.into();
    } else {
        // Find the byte position where we need to truncate
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());

        let mut result = String::with_capacity(end + 3);
        result.push_str(&s[..end]);
        result.push_str("...");
        result.into_boxed_str()
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

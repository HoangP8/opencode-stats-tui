use bincode::{deserialize, serialize};
use fxhash::{FxHashMap, FxHashSet};
use parking_lot::RwLock;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration};

// Type alias for complex return type to reduce complexity
type SessionDiffs = FxHashMap<String, FxHashMap<String, crate::stats::FileDiff>>;
type SessionSortedDays = FxHashMap<String, Vec<String>>;

const CACHE_FORMAT_VERSION: u64 = 8;

/// Metadata for file validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub mtime: u64,
    pub size: u64,
}

/// Cached statistics with version tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedStats {
    pub stats: crate::stats::Stats,
    pub version: u64,
    pub file_meta: FxHashMap<String, FileMeta>,
    #[serde(default)]
    pub format_version: u64,
    #[serde(default)]
    pub session_day_union_diffs: FxHashMap<String, FxHashMap<String, crate::stats::FileDiff>>,
    #[serde(default)]
    pub session_sorted_days: FxHashMap<String, Vec<String>>,
    #[serde(default)]
    pub session_diff_map: FxHashMap<String, Vec<crate::stats::FileDiff>>,
    #[serde(default)]
    pub session_diff_totals: FxHashMap<String, (u64, u64)>,
    #[serde(default)]
    pub message_contributions: FxHashMap<String, (f64, crate::stats::Tokens, i64)>,
    #[serde(default)]
    pub parent_map: FxHashMap<Box<str>, Box<str>>,
    #[serde(default)]
    pub children_map: FxHashMap<Box<str>, Vec<Box<str>>>,
}

/// Lightweight snapshot returned from update_files to avoid a separate full clone.
pub struct StatsUpdate {
    pub affected_sessions: FxHashSet<String>,
    pub totals: crate::stats::Totals,
    pub per_day: FxHashMap<String, crate::stats::DayStat>,
    pub session_titles: FxHashMap<Box<str>, String>,
    pub model_usage: Vec<crate::stats::ModelUsage>,
    pub session_message_files: FxHashMap<String, FxHashSet<PathBuf>>,
    pub parent_map: FxHashMap<Box<str>, Box<str>>,
    pub children_map: FxHashMap<Box<str>, Vec<Box<str>>>,
}

/// Incremental updater for stats
pub struct StatsCache {
    cache_path: PathBuf,
    _storage_path: PathBuf,
    stats: Arc<RwLock<CachedStats>>,
}

impl StatsCache {
    pub fn new(storage_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let cache_dir = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/.cache", home)
        });
        let cache_dir = PathBuf::from(cache_dir);
        let cache_path = cache_dir.join("opencode-stats-tui").join("cache.bincode");

        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }

        Ok(Self {
            cache_path,
            _storage_path: storage_path,
            stats: Arc::new(RwLock::new(CachedStats {
                stats: crate::stats::Stats::default(),
                version: 0,
                file_meta: FxHashMap::default(),
                format_version: CACHE_FORMAT_VERSION,
                session_day_union_diffs: FxHashMap::default(),
                session_sorted_days: FxHashMap::default(),
                session_diff_map: FxHashMap::default(),
                session_diff_totals: FxHashMap::default(),
                message_contributions: FxHashMap::default(),
                parent_map: FxHashMap::default(),
                children_map: FxHashMap::default(),
            })),
        })
    }

    pub fn load_or_compute(&self) -> crate::stats::Stats {
        // OPTIMIZATION: Check cache metadata BEFORE deserializing
        if let Ok(cache_meta) = fs::metadata(&self.cache_path) {
            if let Ok(modified) = cache_meta.modified() {
                if let Ok(age) = modified.elapsed() {
                    if age <= Duration::from_secs(120) {
                        if let Ok(cached) = self.load_cache() {
                            if self.validate_cache_fast(&cached) {
                                let mut stats_lock = self.stats.write();
                                stats_lock.stats.clone_from(&cached.stats);
                                stats_lock.version = cached.version;
                                stats_lock.file_meta.clone_from(&cached.file_meta);
                                stats_lock
                                    .session_day_union_diffs
                                    .clone_from(&cached.session_day_union_diffs);
                                stats_lock
                                    .session_sorted_days
                                    .clone_from(&cached.session_sorted_days);
                                stats_lock
                                    .session_diff_map
                                    .clone_from(&cached.session_diff_map);
                                stats_lock
                                    .session_diff_totals
                                    .clone_from(&cached.session_diff_totals);
                                stats_lock
                                    .message_contributions
                                    .clone_from(&cached.message_contributions);
                                stats_lock.parent_map.clone_from(&cached.parent_map);
                                stats_lock.children_map.clone_from(&cached.children_map);
                                return cached.stats.clone();
                            }
                        }
                    }
                }
            }
        }

        let stats = crate::stats::collect_stats();
        self.update_cache(&stats);
        stats
    }

    fn load_cache(&self) -> Result<CachedStats, Box<dyn std::error::Error>> {
        let data = fs::read(&self.cache_path)?;
        Ok(deserialize(&data)?)
    }

    fn validate_cache_fast(&self, cached: &CachedStats) -> bool {
        if cached.format_version != CACHE_FORMAT_VERSION {
            return false;
        }

        // Optimized: Check a subset of files for changes, but use mtime+size which is very fast
        // We still don't want to check thousands of files every time, so we sample
        // but the sample is now more robust.
        // Also check if the number of files matches.
        let dirs = ["message", "part", "session", "session_diff"];
        for dir in dirs {
            let dp = self._storage_path.join(dir);
            if !dp.exists() {
                continue;
            }
        }

        // More thorough check: sample more files but with cheaper check
        let sample_size = 50.min(cached.file_meta.len());
        let mut checked = 0;
        for (path, meta) in &cached.file_meta {
            if let Ok(current_meta) = fs::metadata(path) {
                let current_mtime = current_meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if current_mtime != meta.mtime || current_meta.len() != meta.size {
                    return false;
                }
            } else {
                return false;
            }
            checked += 1;
            if checked >= sample_size {
                break;
            }
        }

        true
    }

    pub fn update_files(&self, paths: Vec<String>) -> StatsUpdate {
        let mut stats_lock = self.stats.write();
        let affected_sessions = self.update_files_internal(&mut stats_lock, paths);
        StatsUpdate {
            affected_sessions,
            totals: stats_lock.stats.totals.clone(),
            per_day: stats_lock.stats.per_day.clone(),
            session_titles: stats_lock.stats.session_titles.clone(),
            model_usage: stats_lock.stats.model_usage.clone(),
            session_message_files: stats_lock.stats.session_message_files.clone(),
            parent_map: stats_lock.stats.parent_map.clone(),
            children_map: stats_lock.stats.children_map.clone(),
        }
    }

    fn update_files_internal(
        &self,
        cached: &mut CachedStats,
        paths: Vec<String>,
    ) -> FxHashSet<String> {
        let mut affected_sessions = FxHashSet::default();

        let has_session_json_root = paths.iter().any(|p| p.ends_with("session.json"));
        let has_deletion = paths.iter().any(|p| !std::path::Path::new(p).exists());

        // Only do full recompute if there are deletions or if it's the root session.json
        // Individual session files should be handled incrementally
        if has_session_json_root || has_deletion {
            cached.stats = crate::stats::collect_stats();
            cached.parent_map = cached.stats.parent_map.clone();
            cached.children_map = cached.stats.children_map.clone();
            // Invalidate file meta for deleted files
            for p in &paths {
                if !std::path::Path::new(p).exists() {
                    cached.file_meta.remove(p);
                }
            }
            // All sessions might be affected on deletion since we don't know which ones
            for day_stat in cached.stats.per_day.values() {
                for id in day_stat.sessions.keys() {
                    affected_sessions.insert(id.clone());
                }
            }
        } else {
            for p in &paths {
                if p.contains("session_diff/") {
                    if let Some(session_id) = self.incrementally_update_session_diff(cached, p) {
                        affected_sessions.insert(session_id);
                    }
                } else if p.contains("message/") {
                    if let Some(session_id) = self.incrementally_update_messages(cached, p) {
                        affected_sessions.insert(session_id);
                    }
                } else if p.contains("part/") {
                    self.incrementally_update_parts(&mut cached.stats, p);
                } else if p.contains("session/")
                    && p.ends_with(".json")
                    && !p.ends_with("session.json")
                {
                    // Handle individual session files incrementally
                    if let Some(session_id) = self.incrementally_update_session_title(cached, p) {
                        affected_sessions.insert(session_id);
                    }
                }
            }

            cached
                .stats
                .model_usage
                .sort_unstable_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));
        }

        cached.version += 1;
        cached.format_version = CACHE_FORMAT_VERSION;

        for p in &paths {
            if let Ok(m) = fs::metadata(p) {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                cached.file_meta.insert(
                    p.clone(),
                    FileMeta {
                        mtime,
                        size: m.len(),
                    },
                );
            }
        }

        // Write cache to disk on any significant change
        if let Ok(data) = serialize(&*cached) {
            let _ = fs::write(&self.cache_path, data);
        }

        affected_sessions
    }

    fn update_cache(&self, stats: &crate::stats::Stats) {
        let mut cached = self.stats.write();
        cached.stats.clone_from(stats);
        cached.parent_map = stats.parent_map.clone();
        cached.children_map = stats.children_map.clone();
        cached.session_diff_map = crate::stats::load_session_diff_map();
        cached.session_diff_totals = cached
            .session_diff_map
            .iter()
            .map(|(id, diffs)| {
                let adds: u64 = diffs.iter().map(|d| d.additions).sum();
                let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
                (id.clone(), (adds, dels))
            })
            .collect();
        let message_files = self.list_message_files();
        let (union_diffs, sorted_days, message_contributions) =
            self.build_session_day_union_diffs(&message_files);
        cached.session_day_union_diffs = union_diffs;
        cached.session_sorted_days = sorted_days;
        cached.message_contributions = message_contributions;
        cached.version += 1;
        cached.format_version = CACHE_FORMAT_VERSION;
        cached.file_meta.clear();

        if let Ok(files) = self.list_all_files() {
            let meta: FxHashMap<String, FileMeta> = files
                .par_iter()
                .filter_map(|f| {
                    let m = fs::metadata(f).ok()?;
                    let mtime = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    Some((
                        f.clone(),
                        FileMeta {
                            mtime,
                            size: m.len(),
                        },
                    ))
                })
                .collect();
            cached.file_meta = meta;
        }

        if let Ok(data) = serialize(&*cached) {
            let _ = fs::write(&self.cache_path, data);
        }
    }

    fn list_message_files(&self) -> Vec<PathBuf> {
        let message_path = self._storage_path.join("message");
        if !message_path.exists() {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(&message_path) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(sub_entries) = fs::read_dir(&path) {
                    for sub in sub_entries.flatten() {
                        let sp = sub.path();
                        if sp.extension().is_some_and(|e| e == "json") {
                            files.push(sp);
                        }
                    }
                }
            } else if path.extension().is_some_and(|e| e == "json") {
                files.push(path);
            }
        }
        files
    }

    fn build_session_day_union_diffs(
        &self,
        files: &[PathBuf],
    ) -> (
        SessionDiffs,
        SessionSortedDays,
        FxHashMap<String, (f64, crate::stats::Tokens, i64)>,
    ) {
        let mut union: SessionDiffs = FxHashMap::default();
        let mut session_sorted_days: SessionSortedDays = FxHashMap::default();
        let mut message_contributions: FxHashMap<String, (f64, crate::stats::Tokens, i64)> =
            FxHashMap::default();
        let mut processed_ids: FxHashSet<Box<str>> =
            FxHashSet::with_capacity_and_hasher(files.len(), Default::default());

        let mut messages: Vec<(crate::stats::Message, PathBuf)> = files
            .par_iter()
            .filter_map(|p| {
                let bytes = fs::read(p).ok()?;
                let msg: crate::stats::Message = serde_json::from_slice(&bytes).ok()?;
                Some((msg, p.clone()))
            })
            .collect();

        messages.sort_unstable_by_key(|(m, _)| {
            m.time
                .as_ref()
                .and_then(|t| t.created.map(|v| *v))
                .unwrap_or(0)
        });

        for (msg, path) in messages {
            let message_id = match msg.id.clone() {
                Some(id) if !id.0.is_empty() => id.0.into_boxed_str(),
                _ => path.to_string_lossy().to_string().into_boxed_str(),
            };
            if !processed_ids.insert(message_id.clone()) {
                continue;
            }

            let session_id = msg
                .session_id
                .as_ref()
                .map(|s| s.0.clone())
                .unwrap_or_default();
            if session_id.is_empty() {
                continue;
            }
            let ts = msg.time.as_ref().and_then(|t| t.created.map(|v| *v));
            let day = crate::stats::get_day(ts);

            // Track all days session was seen, regardless of diffs, for continuation detection
            let days = session_sorted_days.entry(session_id.clone()).or_default();
            if days.binary_search(&day).is_err() {
                days.push(day.clone());
                days.sort_unstable();
            }

            // Track message contributions for cost and tokens
            let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);
            let tokens = if let Some(t) = &msg.tokens {
                crate::stats::Tokens {
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
                crate::stats::Tokens::default()
            };

            let mut duration = 0;
            if msg.role.as_ref().map(|r| r.0.as_str()) == Some("assistant") {
                if let Some(t) = &msg.time {
                    if let (Some(created), Some(completed)) = (t.created, t.completed) {
                        if *completed > *created {
                            duration = *completed - *created;
                        }
                    }
                }
            }

            message_contributions.insert(message_id.to_string(), (cost, tokens, duration));

            let diffs = Self::extract_cumulative_diffs(&msg);
            if diffs.is_empty() {
                continue;
            }
            let key = format!("{}|{}", session_id, day);
            let file_map = union.entry(key).or_default();
            for d in diffs {
                if d.path.is_empty() {
                    continue;
                }
                file_map.insert(d.path.to_string(), d);
            }
        }

        (union, session_sorted_days, message_contributions)
    }

    fn extract_cumulative_diffs(msg: &crate::stats::Message) -> Vec<crate::stats::FileDiff> {
        msg.summary
            .as_ref()
            .and_then(|s| s.diffs.as_ref())
            .map(|diffs| {
                diffs
                    .iter()
                    .map(|d| crate::stats::FileDiff {
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
            .unwrap_or_default()
    }

    fn sort_file_diffs(file_diffs: &mut [crate::stats::FileDiff]) {
        file_diffs.sort_by(|a, b| {
            let order = |st: &str| match st {
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

    fn compute_incremental_diffs(
        current: &[crate::stats::FileDiff],
        previous: &[crate::stats::FileDiff],
    ) -> Vec<crate::stats::FileDiff> {
        let prev_map: FxHashMap<&str, &crate::stats::FileDiff> =
            previous.iter().map(|d| (d.path.as_ref(), d)).collect();

        current
            .iter()
            .filter_map(|curr| {
                let (adds, dels, status) = if let Some(prev) = prev_map.get(curr.path.as_ref()) {
                    let a = curr.additions.saturating_sub(prev.additions);
                    let d = curr.deletions.saturating_sub(prev.deletions);
                    if a == 0 && d == 0 {
                        return None;
                    }
                    (a, d, curr.status.clone())
                } else {
                    (curr.additions, curr.deletions, curr.status.clone())
                };

                Some(crate::stats::FileDiff {
                    path: curr.path.clone(),
                    additions: adds,
                    deletions: dels,
                    status,
                })
            })
            .collect()
    }

    fn list_all_files(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let dirs = ["message", "part", "session", "session_diff"];
        let files: Vec<String> = dirs
            .par_iter()
            .flat_map(|dir| {
                let dp = self._storage_path.join(dir);
                if !dp.exists() {
                    return Vec::new();
                }

                if let Ok(entries) = fs::read_dir(&dp) {
                    return entries
                        .flatten()
                        .par_bridge()
                        .flat_map(|entry| {
                            let p = entry.path();
                            if p.is_dir() {
                                if let Ok(sub_entries) = fs::read_dir(&p) {
                                    let mut local_files = Vec::new();
                                    for sub_entry in sub_entries.flatten() {
                                        let sp = sub_entry.path();
                                        if sp.extension().is_some_and(|e| e == "json") {
                                            if let Ok(s) = sp.into_os_string().into_string() {
                                                local_files.push(s);
                                            }
                                        }
                                    }
                                    return local_files;
                                }
                            } else if p.extension().is_some_and(|e| e == "json") {
                                if let Ok(s) = p.into_os_string().into_string() {
                                    return vec![s];
                                }
                            }
                            Vec::new()
                        })
                        .collect();
                }
                Vec::new()
            })
            .collect();

        Ok(files)
    }

    fn incrementally_update_messages(
        &self,
        cached: &mut CachedStats,
        path: &str,
    ) -> Option<String> {
        let stats = &mut cached.stats;
        let Ok(bytes) = fs::read(path) else {
            return None;
        };
        let Ok(msg) = serde_json::from_slice::<crate::stats::Message>(&bytes) else {
            return None;
        };

        let message_id = match msg.id.clone() {
            Some(id) if !id.0.is_empty() => id.0.into_boxed_str(),
            _ => path.to_string().into_boxed_str(),
        };
        let message_id_str = message_id.to_string();

        let ts = msg.time.as_ref().and_then(|t| t.created.map(|v| *v));
        let day = crate::stats::get_day(ts);
        let role = msg.role.as_ref().map(|s| s.0.as_str()).unwrap_or("");
        let is_user = role == "user";
        let is_assistant = role == "assistant";
        let model_id = crate::stats::get_model_id(&msg);
        let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);

        let agent_name: Box<str> = msg
            .agent
            .as_ref()
            .filter(|a| !a.0.is_empty())
            .map(|a| a.0.clone().into_boxed_str())
            .unwrap_or_else(|| "unknown".into());

        let session_id_lenient = msg.session_id.clone().unwrap_or_default();
        let original_session_id = session_id_lenient.0.clone();
        let original_boxed: Box<str> = original_session_id.clone().into_boxed_str();
        let session_id = cached
            .parent_map
            .get(&original_boxed)
            .map(|p| p.to_string())
            .unwrap_or_else(|| original_session_id.clone());
        let is_subagent_msg = cached.parent_map.contains_key(&original_boxed);

        let tokens_add = if let Some(t) = &msg.tokens {
            crate::stats::Tokens {
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
            crate::stats::Tokens::default()
        };

        let is_new_message = !cached.message_contributions.contains_key(&message_id_str);

        let mut duration_add = 0;
        if is_assistant {
            if let Some(t) = &msg.time {
                if let (Some(created), Some(completed)) = (t.created, t.completed) {
                    if *completed > *created {
                        duration_add = *completed - *created;
                    }
                }
            }
        }

        // Handle updates: if we already processed this message, subtract its old contribution
        if !is_new_message {
            let (old_cost, old_tokens, old_duration) =
                cached.message_contributions.get(&message_id_str).unwrap();
            let old_cost = *old_cost;
            let old_tokens = *old_tokens;
            let old_duration = *old_duration;
            stats.totals.tokens.input = stats.totals.tokens.input.saturating_sub(old_tokens.input);
            stats.totals.tokens.output =
                stats.totals.tokens.output.saturating_sub(old_tokens.output);
            stats.totals.tokens.reasoning = stats
                .totals
                .tokens
                .reasoning
                .saturating_sub(old_tokens.reasoning);
            stats.totals.tokens.cache_read = stats
                .totals
                .tokens
                .cache_read
                .saturating_sub(old_tokens.cache_read);
            stats.totals.tokens.cache_write = stats
                .totals
                .tokens
                .cache_write
                .saturating_sub(old_tokens.cache_write);
            stats.totals.cost -= old_cost;

            if is_assistant {
                if let Some(m) = stats.model_usage.iter_mut().find(|m| *m.name == *model_id) {
                    m.cost -= old_cost;
                    m.tokens.input = m.tokens.input.saturating_sub(old_tokens.input);
                    m.tokens.output = m.tokens.output.saturating_sub(old_tokens.output);
                    m.tokens.reasoning = m.tokens.reasoning.saturating_sub(old_tokens.reasoning);
                    m.tokens.cache_read = m.tokens.cache_read.saturating_sub(old_tokens.cache_read);
                    m.tokens.cache_write =
                        m.tokens.cache_write.saturating_sub(old_tokens.cache_write);
                }
            }

            if let Some(d) = stats.per_day.get_mut(&day) {
                d.cost -= old_cost;
                d.tokens.input = d.tokens.input.saturating_sub(old_tokens.input);
                d.tokens.output = d.tokens.output.saturating_sub(old_tokens.output);
                d.tokens.reasoning = d.tokens.reasoning.saturating_sub(old_tokens.reasoning);
                d.tokens.cache_read = d.tokens.cache_read.saturating_sub(old_tokens.cache_read);
                d.tokens.cache_write = d.tokens.cache_write.saturating_sub(old_tokens.cache_write);

                if let Some(s_arc) = d.sessions.get_mut(&session_id) {
                    let s = Arc::make_mut(s_arc);
                    s.cost -= old_cost;
                    s.tokens.input = s.tokens.input.saturating_sub(old_tokens.input);
                    s.tokens.output = s.tokens.output.saturating_sub(old_tokens.output);
                    s.tokens.reasoning = s.tokens.reasoning.saturating_sub(old_tokens.reasoning);
                    s.tokens.cache_read = s.tokens.cache_read.saturating_sub(old_tokens.cache_read);
                    s.tokens.cache_write =
                        s.tokens.cache_write.saturating_sub(old_tokens.cache_write);
                    s.active_duration_ms = s.active_duration_ms.saturating_sub(old_duration);

                    if let Some(agent) = s.agents.iter_mut().find(|a| *a.name == *agent_name) {
                        agent.tokens.input = agent.tokens.input.saturating_sub(old_tokens.input);
                        agent.tokens.output = agent.tokens.output.saturating_sub(old_tokens.output);
                        agent.tokens.reasoning =
                            agent.tokens.reasoning.saturating_sub(old_tokens.reasoning);
                        agent.tokens.cache_read = agent
                            .tokens
                            .cache_read
                            .saturating_sub(old_tokens.cache_read);
                        agent.tokens.cache_write = agent
                            .tokens
                            .cache_write
                            .saturating_sub(old_tokens.cache_write);
                        agent.active_duration_ms =
                            agent.active_duration_ms.saturating_sub(old_duration);
                    }
                }
            }
        } else {
            stats.totals.messages += 1;
            if is_user {
                stats.totals.prompts += 1;
            }
        }

        cached
            .message_contributions
            .insert(message_id_str, (cost, tokens_add, duration_add));
        stats.processed_message_ids.insert(message_id);

        if !original_session_id.is_empty() {
            stats
                .session_message_files
                .entry(original_session_id.clone())
                .or_insert_with(|| FxHashSet::default())
                .insert(PathBuf::from(path));
        }

        stats.totals.tokens.input += tokens_add.input;
        stats.totals.tokens.output += tokens_add.output;
        stats.totals.tokens.reasoning += tokens_add.reasoning;
        stats.totals.tokens.cache_read += tokens_add.cache_read;
        stats.totals.tokens.cache_write += tokens_add.cache_write;
        stats.totals.cost += cost;
        stats
            .totals
            .sessions
            .insert(session_id.clone().into_boxed_str());

        if is_assistant {
            if let Some(m) = stats.model_usage.iter_mut().find(|m| *m.name == *model_id) {
                if is_new_message {
                    m.messages += 1;
                }
                m.cost += cost;
                m.tokens.input += tokens_add.input;
                m.tokens.output += tokens_add.output;
                m.tokens.reasoning += tokens_add.reasoning;
                m.tokens.cache_read += tokens_add.cache_read;
                m.tokens.cache_write += tokens_add.cache_write;
                m.sessions.insert(session_id.clone().into_boxed_str());
                if is_new_message {
                    *m.agents.entry(agent_name.clone()).or_insert(0) += 1;
                }
            } else {
                let name_str: &str = &model_id;
                let parts: Vec<&str> = name_str.split('/').collect();
                let (p, n) = if parts.len() >= 2 {
                    (parts[0], parts[1])
                } else {
                    ("unknown", name_str)
                };
                let mut agents = HashMap::new();
                agents.insert(agent_name.clone(), 1);
                stats.model_usage.push(crate::stats::ModelUsage {
                    name: model_id.clone(),
                    short_name: n.into(),
                    provider: p.into(),
                    display_name: format!("{}/{}", p, n).into_boxed_str(),
                    messages: 1,
                    sessions: [session_id.clone().into_boxed_str()].into(),
                    tokens: tokens_add,
                    tools: HashMap::new(),
                    agents,
                    cost,
                });
            }
        }

        {
            let d = stats.per_day.entry(day.clone()).or_default();
            if is_new_message {
                d.messages += 1;
                if is_user {
                    d.prompts += 1;
                }
            }
            d.cost += cost;
            d.tokens.input += tokens_add.input;
            d.tokens.output += tokens_add.output;
            d.tokens.reasoning += tokens_add.reasoning;
            d.tokens.cache_read += tokens_add.cache_read;
            d.tokens.cache_write += tokens_add.cache_write;

            let s_arc = d
                .sessions
                .entry(session_id.clone())
                .or_insert_with(|| Arc::new(crate::stats::SessionStat::new(session_id.clone())));
            let s = Arc::make_mut(s_arc);

            if is_new_message {
                s.messages += 1;
                if is_user {
                    s.prompts += 1;
                }
            }
            s.cost += cost;
            s.active_duration_ms += duration_add;

            if is_assistant {
                s.models.insert(model_id.clone());
            }
            s.tokens.input += tokens_add.input;
            s.tokens.output += tokens_add.output;
            s.tokens.reasoning += tokens_add.reasoning;
            s.tokens.cache_read += tokens_add.cache_read;
            s.tokens.cache_write += tokens_add.cache_write;
            if let Some(t) = ts {
                if t < s.first_activity {
                    s.first_activity = t;
                }
            }
            let end_ts = msg
                .time
                .as_ref()
                .and_then(|t| t.completed.map(|v| *v))
                .or(ts);
            if let Some(t) = end_ts {
                if t > s.last_activity {
                    s.last_activity = t;
                }
            }
            if let Some(p) = &msg.path {
                if let Some(cwd) = &p.cwd {
                    s.path_cwd = cwd.clone().into();
                }
                if let Some(root) = &p.root {
                    s.path_root = root.clone().into();
                }
            }

            {
                let agent_entry = s.agents.iter_mut().find(|a| *a.name == *agent_name);
                if let Some(agent) = agent_entry {
                    if is_new_message {
                        agent.messages += 1;
                    }
                    agent.tokens.input += tokens_add.input;
                    agent.tokens.output += tokens_add.output;
                    agent.tokens.reasoning += tokens_add.reasoning;
                    agent.tokens.cache_read += tokens_add.cache_read;
                    agent.tokens.cache_write += tokens_add.cache_write;
                    if is_assistant {
                        agent.models.insert(model_id.clone());
                    }
                    if let Some(t) = ts {
                        if t < agent.first_activity {
                            agent.first_activity = t;
                        }
                    }
                    if let Some(t) = end_ts {
                        if t > agent.last_activity {
                            agent.last_activity = t;
                        }
                    }
                    agent.active_duration_ms += duration_add;
                } else if is_new_message {
                    let mut models = fxhash::FxHashSet::default();
                    if is_assistant {
                        models.insert(model_id.clone());
                    }
                    s.agents.push(crate::stats::AgentInfo {
                        name: agent_name.clone(),
                        is_main: !is_subagent_msg,
                        models,
                        messages: 1,
                        tokens: tokens_add,
                        first_activity: ts.unwrap_or(i64::MAX),
                        last_activity: end_ts.unwrap_or(0),
                        active_duration_ms: duration_add,
                    });
                }
            }
        }

        let cumulative_diffs = Self::extract_cumulative_diffs(&msg);
        if !session_id.is_empty() && !cumulative_diffs.is_empty() {
            let key = format!("{}|{}", session_id, day);
            let file_map = cached.session_day_union_diffs.entry(key).or_default();
            for d in &cumulative_diffs {
                if d.path.is_empty() {
                    continue;
                }
                file_map.insert(d.path.to_string(), d.clone());
            }

            // Update session_diff_map with cumulative diffs from this message
            // This ensures the session has the most up-to-date diff data
            cached
                .session_diff_map
                .insert(session_id.clone(), cumulative_diffs.clone());

            // Update session_diff_totals and global totals
            let adds: u64 = cumulative_diffs.iter().map(|d| d.additions).sum();
            let dels: u64 = cumulative_diffs.iter().map(|d| d.deletions).sum();

            // Update global totals: subtract old session totals, add new ones
            if let Some(&(old_adds, old_dels)) = cached.session_diff_totals.get(&session_id) {
                stats.totals.diffs.additions =
                    stats.totals.diffs.additions.saturating_sub(old_adds);
                stats.totals.diffs.deletions =
                    stats.totals.diffs.deletions.saturating_sub(old_dels);
            }
            stats.totals.diffs.additions += adds;
            stats.totals.diffs.deletions += dels;

            cached
                .session_diff_totals
                .insert(session_id.clone(), (adds, dels));
        }

        if !session_id.is_empty() {
            let days = cached
                .session_sorted_days
                .entry(session_id.clone())
                .or_default();
            if days.binary_search(&day).is_err() {
                days.push(day.clone());
                days.sort_unstable();
            }

            if let Some(sorted_days) = cached.session_sorted_days.get(&session_id).cloned() {
                let start_pos = sorted_days.iter().position(|d| d == &day).unwrap_or(0);
                let first_day = sorted_days.first().cloned().unwrap_or_else(|| day.clone());
                let is_last_day = start_pos + 1 >= sorted_days.len();
                let day_iter: Box<dyn Iterator<Item = (usize, &String)>> = if is_last_day {
                    Box::new(std::iter::once((
                        start_pos,
                        sorted_days.get(start_pos).unwrap_or(&day),
                    )))
                } else {
                    Box::new(sorted_days.iter().enumerate().skip(start_pos))
                };

                for (idx, day_str) in day_iter {
                    let lookup_key = format!("{}|{}", session_id, day_str);
                    let current_day_diffs: Vec<crate::stats::FileDiff> = cached
                        .session_day_union_diffs
                        .get(&lookup_key)
                        .map(|m| m.values().cloned().collect())
                        .unwrap_or_default();

                    let d_stat = stats.per_day.entry(day_str.clone()).or_default();
                    let s_arc = d_stat
                        .sessions
                        .entry(session_id.clone())
                        .or_insert_with(|| {
                            Arc::new(crate::stats::SessionStat::new(session_id.clone()))
                        });
                    let s = Arc::make_mut(s_arc);

                    let is_continuation = *day_str != first_day;
                    s.is_continuation = is_continuation;
                    s.first_created_date = if is_continuation {
                        Some(first_day.clone().into_boxed_str())
                    } else {
                        None
                    };
                    s.original_session_id = if is_continuation {
                        Some(session_id.clone().into_boxed_str())
                    } else {
                        None
                    };

                    if !is_continuation {
                        if let Some(session_diffs) = cached.session_diff_map.get(&session_id) {
                            s.file_diffs = session_diffs.clone();
                            if let Some(&(adds, dels)) = cached.session_diff_totals.get(&session_id)
                            {
                                s.diffs.additions = adds;
                                s.diffs.deletions = dels;
                            } else {
                                let adds: u64 = s.file_diffs.iter().map(|d| d.additions).sum();
                                let dels: u64 = s.file_diffs.iter().map(|d| d.deletions).sum();
                                s.diffs.additions = adds;
                                s.diffs.deletions = dels;
                            }
                        } else {
                            let mut diffs = current_day_diffs;
                            Self::sort_file_diffs(&mut diffs);
                            let adds: u64 = diffs.iter().map(|d| d.additions).sum();
                            let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
                            s.file_diffs = diffs;
                            s.diffs.additions = adds;
                            s.diffs.deletions = dels;
                        }
                    } else {
                        let mut diffs = current_day_diffs;
                        if idx > 0 {
                            let prev_day = &sorted_days[idx - 1];
                            let prev_key = format!("{}|{}", session_id, prev_day);
                            if let Some(prev_map) = cached.session_day_union_diffs.get(&prev_key) {
                                let prev_vec: Vec<crate::stats::FileDiff> =
                                    prev_map.values().cloned().collect();
                                diffs = Self::compute_incremental_diffs(&diffs, &prev_vec);
                            }
                        }
                        Self::sort_file_diffs(&mut diffs);
                        let adds: u64 = diffs.iter().map(|d| d.additions).sum();
                        let dels: u64 = diffs.iter().map(|d| d.deletions).sum();
                        s.file_diffs = diffs;
                        s.diffs.additions = adds;
                        s.diffs.deletions = dels;
                    }

                    d_stat.diffs.additions =
                        d_stat.sessions.values().map(|ss| ss.diffs.additions).sum();
                    d_stat.diffs.deletions =
                        d_stat.sessions.values().map(|ss| ss.diffs.deletions).sum();
                }
            }
        }

        Some(session_id)
    }

    fn incrementally_update_session_diff(
        &self,
        cached: &mut CachedStats,
        path: &str,
    ) -> Option<String> {
        let p = std::path::Path::new(path);
        let session_id = p.file_stem()?.to_str()?.to_string();

        let bytes = fs::read(path).ok()?;

        #[derive(serde::Deserialize)]
        struct DiffEntry {
            file: Option<crate::stats::LenientString>,
            additions: Option<crate::stats::LenientU64>,
            deletions: Option<crate::stats::LenientU64>,
            status: Option<crate::stats::LenientString>,
        }

        let entries: Vec<DiffEntry> = serde_json::from_slice(&bytes).ok()?;
        let mut diffs: Vec<crate::stats::FileDiff> = entries
            .into_iter()
            .map(|item| crate::stats::FileDiff {
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
        Self::sort_file_diffs(&mut diffs);

        let adds: u64 = diffs.iter().map(|d| d.additions).sum();
        let dels: u64 = diffs.iter().map(|d| d.deletions).sum();

        // Update global totals: subtract old session totals, add new ones
        if let Some(&(old_adds, old_dels)) = cached.session_diff_totals.get(&session_id) {
            cached.stats.totals.diffs.additions =
                cached.stats.totals.diffs.additions.saturating_sub(old_adds);
            cached.stats.totals.diffs.deletions =
                cached.stats.totals.diffs.deletions.saturating_sub(old_dels);
        }
        cached.stats.totals.diffs.additions += adds;
        cached.stats.totals.diffs.deletions += dels;

        cached
            .session_diff_map
            .insert(session_id.clone(), diffs.clone());
        cached
            .session_diff_totals
            .insert(session_id.clone(), (adds, dels));

        for day_stat in cached.stats.per_day.values_mut() {
            if let Some(s_arc) = day_stat.sessions.get_mut(&session_id) {
                let s = std::sync::Arc::make_mut(s_arc);
                if !s.is_continuation {
                    s.file_diffs = diffs.clone();
                    s.diffs.additions = adds;
                    s.diffs.deletions = dels;
                }
                day_stat.diffs.additions = day_stat
                    .sessions
                    .values()
                    .map(|ss| ss.diffs.additions)
                    .sum();
                day_stat.diffs.deletions = day_stat
                    .sessions
                    .values()
                    .map(|ss| ss.diffs.deletions)
                    .sum();
            }
        }

        Some(session_id)
    }

    fn incrementally_update_session_title(
        &self,
        cached: &mut CachedStats,
        path: &str,
    ) -> Option<String> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => return None,
        };

        #[derive(serde::Deserialize)]
        struct SessionData {
            id: Option<crate::stats::LenientString>,
            title: Option<crate::stats::LenientString>,
            #[serde(rename = "parentID")]
            parent_id: Option<crate::stats::LenientString>,
        }

        let session_data: SessionData = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let session_id = session_data
            .id
            .map(|s| s.0)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                // Fallback to filename if ID not available
                std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .into_boxed_str();

        let title = session_data.title.map(|s| s.0).unwrap_or_default();

        if !session_id.is_empty() {
            cached
                .stats
                .session_titles
                .insert(session_id.clone(), title);

            if let Some(pid) = session_data.parent_id.as_ref().filter(|p| !p.0.is_empty()) {
                let parent_id: Box<str> = pid.0.clone().into_boxed_str();
                if !cached.parent_map.contains_key(&session_id) {
                    cached
                        .parent_map
                        .insert(session_id.clone(), parent_id.clone());
                    cached
                        .stats
                        .parent_map
                        .insert(session_id.clone(), parent_id.clone());
                    let children = cached.children_map.entry(parent_id.clone()).or_default();
                    if !children.contains(&session_id) {
                        children.push(session_id.clone());
                    }
                    let stats_children = cached.stats.children_map.entry(parent_id).or_default();
                    if !stats_children.contains(&session_id) {
                        stats_children.push(session_id.clone());
                    }
                }
            }

            cached.version += 1;
        }

        Some(session_id.into_string())
    }

    fn incrementally_update_parts(&self, stats: &mut crate::stats::Stats, path: &str) {
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let Ok(part) = serde_json::from_slice::<crate::stats::PartData>(&bytes) else {
            return;
        };
        if let Some(text) = &part.text {
            let _a = text.lines().filter(|l| l.starts_with('+')).count() as u64;
            let _d = text.lines().filter(|l| l.starts_with('-')).count() as u64;
            // Removed global total updates from parts to stay consistent with authoritative session_diff
            // stats.totals.diffs.additions += a;
            // stats.totals.diffs.deletions += d;
        }

        if part.part_type.as_deref() == Some("tool") {
            if let Some(tool) = &part.tool {
                *stats.totals.tools.entry(tool.clone().into()).or_insert(0) += 1;
            }
        }
    }
}

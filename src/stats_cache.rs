use bincode::{deserialize, serialize};
use parking_lot::RwLock;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

// Type alias for complex return type to reduce complexity
type SessionDiffs = HashMap<String, HashMap<String, crate::stats::FileDiff>>;
type SessionSortedDays = HashMap<String, Vec<String>>;

const CACHE_FORMAT_VERSION: u64 = 4;

/// Cached statistics with version tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedStats {
    pub stats: crate::stats::Stats,
    pub version: u64,
    pub file_hashes: HashMap<String, u64>,
    #[serde(default)]
    pub format_version: u64,
    #[serde(default)]
    pub session_day_union_diffs: HashMap<String, HashMap<String, crate::stats::FileDiff>>,
    #[serde(default)]
    pub session_sorted_days: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub session_diff_map: HashMap<String, Vec<crate::stats::FileDiff>>,
    #[serde(default)]
    pub session_diff_totals: HashMap<String, (u64, u64)>,
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
                file_hashes: HashMap::new(),
                format_version: CACHE_FORMAT_VERSION,
                session_day_union_diffs: HashMap::new(),
                session_sorted_days: HashMap::new(),
                session_diff_map: HashMap::new(),
                session_diff_totals: HashMap::new(),
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
                                stats_lock.file_hashes.clone_from(&cached.file_hashes);
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
        let cache_meta = match fs::metadata(&self.cache_path) {
            Ok(m) => m,
            Err(_) => return false,
        };

        let cache_age = cache_meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or(Duration::from_secs(121));

        if cache_age > Duration::from_secs(120) {
            return false;
        }

        let message_count = self.count_message_files_fast();
        if cached.stats.processed_message_ids.len() != message_count {
            return false;
        }

        let all_files = match self.list_all_files() {
            Ok(files) => files,
            Err(_) => return false,
        };
        if all_files.len() != cached.file_hashes.len() {
            return false;
        }

        let sample_files: Vec<_> = cached.file_hashes.keys().take(20).collect();
        for path in sample_files {
            let Some(current_hash) = self.compute_file_hash(path) else {
                return false;
            };
            if cached.file_hashes.get(path).copied() != Some(current_hash) {
                return false;
            }
        }

        true
    }

    fn count_message_files_fast(&self) -> usize {
        let message_path = self._storage_path.join("message");
        if !message_path.exists() {
            return 0;
        }

        let Ok(entries) = fs::read_dir(&message_path) else {
            return 0;
        };

        let dirs: Vec<_> = entries.flatten().collect();
        dirs.par_iter()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    fs::read_dir(&path)
                        .map(|sub| {
                            sub.flatten()
                                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                                .count()
                        })
                        .unwrap_or(0)
                } else if path.extension().is_some_and(|e| e == "json") {
                    1
                } else {
                    0
                }
            })
            .sum()
    }

    pub fn update_files(&self, paths: Vec<String>) -> HashSet<String> {
        let mut stats_lock = self.stats.write();
        self.update_files_internal(&mut stats_lock, paths)
    }

    fn update_files_internal(
        &self,
        cached: &mut CachedStats,
        paths: Vec<String>,
    ) -> HashSet<String> {
        let mut affected_sessions = HashSet::new();
        let has_session_changes = paths
            .iter()
            .any(|p| p.contains("session.json") || p.contains("session_diff/"));
        let has_message_changes = paths.iter().any(|p| p.contains("message/"));

        if has_session_changes || has_message_changes {
            cached.stats = crate::stats::collect_stats();
            for day_stat in cached.stats.per_day.values() {
                for id in day_stat.sessions.keys() {
                    affected_sessions.insert(id.clone());
                }
            }
        } else {
            for p in &paths {
                if p.contains("message/") {
                    if let Some(session_id) = self.incrementally_update_messages(cached, p) {
                        affected_sessions.insert(session_id);
                    }
                } else if p.contains("part/") {
                    self.incrementally_update_parts(&mut cached.stats, p);
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
            if let Some(h) = self.compute_file_hash(p) {
                cached.file_hashes.insert(p.clone(), h);
            }
        }

        if let Ok(data) = serialize(&*cached) {
            let _ = fs::write(&self.cache_path, data);
        }

        affected_sessions
    }

    fn update_cache(&self, stats: &crate::stats::Stats) {
        let mut cached = self.stats.write();
        cached.stats.clone_from(stats);
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
        let (union_diffs, sorted_days) = self.build_session_day_union_diffs(&message_files);
        cached.session_day_union_diffs = union_diffs;
        cached.session_sorted_days = sorted_days;
        cached.version += 1;
        cached.format_version = CACHE_FORMAT_VERSION;
        cached.file_hashes.clear();

        if let Ok(files) = self.list_all_files() {
            let hashes: HashMap<String, u64> = files
                .par_iter()
                .filter_map(|f| self.compute_file_hash(f).map(|h| (f.clone(), h)))
                .collect();
            cached.file_hashes = hashes;
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
    ) -> (SessionDiffs, SessionSortedDays) {
        let mut union: SessionDiffs = HashMap::new();
        let mut session_sorted_days: SessionSortedDays = HashMap::new();
        let mut processed_ids: HashSet<Box<str>> = HashSet::with_capacity(files.len());

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
            if !processed_ids.insert(message_id) {
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

            let days = session_sorted_days.entry(session_id).or_default();
            if days.binary_search(&day).is_err() {
                days.push(day);
                days.sort_unstable();
            }
        }

        (union, session_sorted_days)
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
        let prev_map: HashMap<&str, &crate::stats::FileDiff> =
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

    fn compute_file_hash(&self, path: &str) -> Option<u64> {
        let m = fs::metadata(path).ok()?;
        let mod_time = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some(mod_time.wrapping_mul(31).wrapping_add(m.len()))
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
        if stats.processed_message_ids.contains(&message_id) {
            return None;
        }
        stats.processed_message_ids.insert(message_id);

        let session_id_lenient = msg.session_id.clone().unwrap_or_default();
        let session_id = session_id_lenient.0.clone();

        if !session_id.is_empty() {
            stats
                .session_message_files
                .entry(session_id.clone())
                .or_default()
                .push(PathBuf::from(path));
        }

        let ts = msg.time.as_ref().and_then(|t| t.created.map(|v| *v));
        let day = crate::stats::get_day(ts);
        let role = msg.role.as_ref().map(|s| s.0.as_str()).unwrap_or("");
        let is_user = role == "user";
        let is_assistant = role == "assistant";
        let model_id = crate::stats::get_model_id(&msg);
        let cost = msg.cost.as_ref().map(|c| **c).unwrap_or(0.0);

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

        stats.totals.tokens.input += tokens_add.input;
        stats.totals.tokens.output += tokens_add.output;
        stats.totals.tokens.reasoning += tokens_add.reasoning;
        stats.totals.tokens.cache_read += tokens_add.cache_read;
        stats.totals.tokens.cache_write += tokens_add.cache_write;
        stats.totals.messages += 1;
        if is_user {
            stats.totals.prompts += 1;
        }
        stats.totals.cost += cost;
        stats
            .totals
            .sessions
            .insert(session_id.clone().into_boxed_str());

        if is_assistant {
            if let Some(m) = stats.model_usage.iter_mut().find(|m| *m.name == *model_id) {
                m.messages += 1;
                m.cost += cost;
                m.tokens.input += tokens_add.input;
                m.tokens.output += tokens_add.output;
                m.tokens.reasoning += tokens_add.reasoning;
                m.tokens.cache_read += tokens_add.cache_read;
                m.tokens.cache_write += tokens_add.cache_write;
                m.sessions.insert(session_id.clone().into_boxed_str());
            } else {
                let name_str: &str = &model_id;
                let parts: Vec<&str> = name_str.split('/').collect();
                let (p, n) = if parts.len() >= 2 {
                    (parts[0], parts[1])
                } else {
                    ("unknown", name_str)
                };
                stats.model_usage.push(crate::stats::ModelUsage {
                    name: model_id.clone(),
                    short_name: n.into(),
                    provider: p.into(),
                    display_name: format!("{}/{}", p, n).into_boxed_str(),
                    messages: 1,
                    sessions: [session_id.clone().into_boxed_str()].into(),
                    tokens: tokens_add,
                    tools: HashMap::new(),
                    agents: HashMap::new(),
                    cost,
                });
            }
            if let Some(agent) = msg
                .agent
                .as_ref()
                .map(|s| s.0.as_str())
                .filter(|s| !s.is_empty())
            {
                if let Some(m) = stats.model_usage.iter_mut().find(|m| *m.name == *model_id) {
                    *m.agents
                        .entry(agent.to_string().into_boxed_str())
                        .or_insert(0) += 1;
                }
            }
        }

        {
            let d = stats.per_day.entry(day.clone()).or_default();
            d.messages += 1;
            if is_user {
                d.prompts += 1;
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
            s.messages += 1;
            if is_user {
                s.prompts += 1;
            }
            s.cost += cost;
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
        }

        let cumulative_diffs = Self::extract_cumulative_diffs(&msg);
        if !session_id.is_empty() && !cumulative_diffs.is_empty() {
            let key = format!("{}|{}", session_id, day);
            let file_map = cached.session_day_union_diffs.entry(key).or_default();
            for d in cumulative_diffs {
                if d.path.is_empty() {
                    continue;
                }
                file_map.insert(d.path.to_string(), d);
            }
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
                for (idx, day_str) in sorted_days.iter().enumerate().skip(start_pos) {
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

    fn incrementally_update_parts(&self, stats: &mut crate::stats::Stats, path: &str) {
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let Ok(part) = serde_json::from_slice::<crate::stats::PartData>(&bytes) else {
            return;
        };
        if let Some(text) = &part.text {
            let a = text.lines().filter(|l| l.starts_with('+')).count() as u64;
            let d = text.lines().filter(|l| l.starts_with('-')).count() as u64;
            stats.totals.diffs.additions += a;
            stats.totals.diffs.deletions += d;
        }
        if part.part_type.as_deref() == Some("tool") {
            if let Some(tool) = &part.tool {
                *stats.totals.tools.entry(tool.clone().into()).or_insert(0) += 1;
            }
        }
    }

    pub fn get_stats(&self) -> crate::stats::Stats {
        self.stats.read().stats.clone()
    }
}

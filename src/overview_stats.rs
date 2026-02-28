use crate::cost::estimate_cost;
use crate::stats::{DayStat, ModelUsage};
use chrono::{Datelike, NaiveDate};
use rustc_hash::FxHashMap;
use std::cell::RefCell;

#[derive(Clone)]
pub struct OverviewStats {
    pub peak_day: String,
    pub longest_session: String,
    pub total_active_time: String,
    pub total_savings: String,
    pub start_day: String,
    pub active_days: String,
    pub avg_sessions: String,
    pub avg_cost: String,
    pub avg_tokens: String,
    pub chronotype: String,
    pub favorite_day: String,
    pub total_models: String,
    pub top_languages: Vec<(String, f64)>,
    pub has_more_langs: bool,
}

pub struct OverviewStatsCache {
    stats: RefCell<Option<OverviewStats>>,
    key: RefCell<Option<OverviewCacheKey>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct OverviewCacheKey {
    per_day_ptr: usize,
    models_ptr: usize,
    days: usize,
    models: usize,
    cost_bits: u64,
}

impl OverviewStatsCache {
    pub fn new() -> Self {
        Self {
            stats: RefCell::new(None),
            key: RefCell::new(None),
        }
    }

    pub fn get(
        &self,
        per_day: &FxHashMap<String, DayStat>,
        models: &[ModelUsage],
        cost: f64,
    ) -> OverviewStats {
        let key = OverviewCacheKey {
            per_day_ptr: per_day as *const _ as usize,
            models_ptr: models.as_ptr() as usize,
            days: per_day.len(),
            models: models.len(),
            cost_bits: cost.to_bits(),
        };

        if *self.key.borrow() == Some(key) {
            if let Some(cached) = self.stats.borrow().as_ref() {
                return cached.clone();
            }
        }

        let stats = calculate(per_day, models, cost);
        *self.stats.borrow_mut() = Some(stats.clone());
        *self.key.borrow_mut() = Some(key);
        stats
    }

    pub fn invalidate(&self) {
        *self.stats.borrow_mut() = None;
        *self.key.borrow_mut() = None;
    }
}

pub fn calculate(
    per_day: &FxHashMap<String, DayStat>,
    models: &[ModelUsage],
    cost: f64,
) -> OverviewStats {
    if per_day.is_empty() {
        return OverviewStats {
            peak_day: "—".into(),
            longest_session: "0h 0m".into(),
            total_active_time: "0h 0m".into(),
            total_savings: "$0.00".into(),
            start_day: "—".into(),
            active_days: "0".into(),
            avg_sessions: "0 sess/day".into(),
            avg_cost: "$0.00/day".into(),
            avg_tokens: "0/day".into(),
            chronotype: "Unknown".into(),
            favorite_day: "—".into(),
            total_models: "0".into(),
            top_languages: Vec::new(),
            has_more_langs: false,
        };
    }

    let days = per_day.len();
    let mut peak_tokens: u64 = 0;
    let mut longest: i64 = 0;
    let mut total_ms: i64 = 0;
    let mut sessions: usize = 0;
    let mut cost_sum: f64 = 0.0;
    let mut tokens: u64 = 0;
    let mut period_buckets = [0u64; 4];
    let mut day_buckets = [0u64; 7];
    let mut lang_counts: FxHashMap<&'static str, u64> = FxHashMap::default();

    let mut peak_day: Option<&str> = None;
    let mut start_day: Option<&str> = None;

    for (day_key, day_stat) in per_day.iter() {
        let day_tokens = day_stat.tokens.total();
        if day_tokens > peak_tokens {
            peak_tokens = day_tokens;
            peak_day = Some(day_key.as_str());
        }

        if start_day.is_none_or(|k| day_key.as_str() < k) {
            start_day = Some(day_key.as_str());
        }

        sessions += day_stat.sessions.len();
        cost_sum += day_stat.cost;
        tokens += day_tokens;

        for session in day_stat.sessions.values() {
            let dur = session.active_duration_ms;
            if dur > longest {
                longest = dur;
            }
            total_ms += dur;

            let secs = session.first_activity.div_euclid(1000);
            let hour = secs.rem_euclid(86_400) / 3_600;
            period_buckets[match hour {
                6..=11 => 1,
                12..=17 => 2,
                18..=23 => 3,
                _ => 0,
            }] += 1;

            for d in &session.file_diffs {
                if let Some((_, ext)) = d.path.rsplit_once('.') {
                    if let Some(l) = lang(ext) {
                        *lang_counts.entry(l).or_insert(0) += (d.additions + d.deletions).max(1);
                    }
                }
            }
        }

        if let Ok(d) = NaiveDate::parse_from_str(day_key, "%Y-%m-%d") {
            day_buckets[d.weekday().num_days_from_monday() as usize] +=
                day_stat.sessions.len() as u64;
        }
    }

    let est: f64 = models
        .iter()
        .filter_map(|m| {
            m.name
                .split('/')
                .next_back()
                .and_then(|n| estimate_cost(n, &m.tokens))
        })
        .sum();

    let (top_langs, has_more_langs) = if lang_counts.is_empty() {
        (Vec::new(), false)
    } else {
        let total: u64 = lang_counts.values().sum();
        let mut v: Vec<_> = lang_counts.into_iter().collect();
        let has_more = v.len() > 5;
        if has_more {
            v.select_nth_unstable_by(4, |a, b| b.1.cmp(&a.1));
            v[..5].sort_unstable_by(|a, b| b.1.cmp(&a.1));
        } else {
            v.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        }
        let top: Vec<_> = v
            .iter()
            .take(if has_more { 4 } else { 5 })
            .map(|(l, c)| (l.to_string(), (*c as f64 / total as f64) * 100.0))
            .collect();
        (top, has_more)
    };

    OverviewStats {
        peak_day: peak_day.map(fmt_date).unwrap_or_else(|| "—".into()),
        longest_session: fmt_duration(longest),
        total_active_time: fmt_duration(total_ms),
        total_savings: format!("${:.2}", est - cost),
        start_day: start_day.map(fmt_date).unwrap_or_else(|| "—".into()),
        active_days: days.to_string(),
        avg_sessions: format!("{:.1} sess/day", sessions as f64 / days as f64),
        avg_cost: format!("${:.2}/day", cost_sum / days as f64),
        avg_tokens: fmt_tokens(tokens as f64 / days as f64),
        chronotype: match period_buckets
            .iter()
            .enumerate()
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i)
            .unwrap_or(1)
        {
            0 => "Night",
            1 => "Morning",
            2 => "Afternoon",
            _ => "Evening",
        }
        .into(),
        favorite_day: match day_buckets
            .iter()
            .enumerate()
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i)
            .unwrap_or(0)
        {
            0 => "Mondays",
            1 => "Tuesdays",
            2 => "Wednesdays",
            3 => "Thursdays",
            4 => "Fridays",
            5 => "Saturdays",
            _ => "Sundays",
        }
        .into(),
        total_models: models.len().to_string(),
        top_languages: top_langs,
        has_more_langs,
    }
}

fn fmt_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0h 0m".into();
    }
    let s = (ms / 1000) as u64;
    format!("{}h {}m", s / 3600, (s % 3600) / 60)
}

fn fmt_date(d: &str) -> String {
    NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .map(|d| format!("{} {:02}, {}", month(d.month()), d.day(), d.year()))
        .unwrap_or_else(|_| d.into())
}

fn month(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

fn fmt_tokens(avg: f64) -> String {
    if avg >= 1e6 {
        format!("{:.1}M/day", avg / 1e6)
    } else if avg >= 1e3 {
        format!("{:.1}K/day", avg / 1e3)
    } else {
        format!("{:.0}/day", avg)
    }
}

fn lang(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "Rust",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" => "C++",
        "rb" => "Ruby",
        "swift" => "Swift",
        "kt" => "Kotlin",
        "lua" => "Lua",
        "sh" | "bash" | "zsh" => "Shell",
        "css" | "scss" | "sass" => "CSS",
        "html" | "htm" => "HTML",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "md" | "mdx" => "Markdown",
        "sql" => "SQL",
        "svelte" => "Svelte",
        "vue" => "Vue",
        "dart" => "Dart",
        "zig" => "Zig",
        "ex" | "exs" => "Elixir",
        "jl" => "Julia",
        _ => return None,
    })
}

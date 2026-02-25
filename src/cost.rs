//! OpenRouter pricing lookup for cost estimation.

use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Pricing rates for a model.
#[derive(Clone, Copy)]
pub struct ModelPricing {
    pub prompt: f64,
    pub completion: f64,
    pub reasoning: f64,
    pub input_cache_read: f64,
    pub input_cache_write: f64,
}

static PRICING_CACHE: OnceLock<FxHashMap<String, ModelPricing>> = OnceLock::new();

// Thread-local scratch buffer for fuzzy matching to avoid allocations per call
thread_local! {
    static FUZZY_SCRATCH: RefCell<Vec<u16>> = RefCell::new(Vec::with_capacity(256));
}

/// Initialize pricing cache (call once at startup).
pub fn init_pricing() {
    PRICING_CACHE.get_or_init(fetch_pricing);
}

/// Look up pricing for a model name.
pub fn lookup_pricing(model_name: &str) -> Option<ModelPricing> {
    let cache = PRICING_CACHE.get_or_init(fetch_pricing);
    let found = lookup_in_map(cache, model_name);
    if found.is_some() {
        return found;
    }

    // On miss, try a one-time live fetch for newly added models
    let live = fetch_pricing();
    if live.is_empty() {
        return None;
    }
    lookup_in_map(&live, model_name)
}

fn lookup_in_map(map: &FxHashMap<String, ModelPricing>, model_name: &str) -> Option<ModelPricing> {
    if map.is_empty() {
        return None;
    }

    let input = model_name.trim().to_ascii_lowercase();
    let slug = input.rsplit('/').next().unwrap_or(&input).to_string();

    // Exact full-id match
    if let Some(p) = map.get(input.as_str()) {
        return Some(*p);
    }

    // Exact slug match
    if let Some(p) = map.get(slug.as_str()) {
        return Some(*p);
    }

    // Strip date suffix and retry
    let stripped = strip_date_suffix(&slug);
    if stripped != slug {
        if let Some(p) = map.get(stripped) {
            return Some(*p);
        }
    }

    // Fuzzy match
    let local_norm = normalize(stripped);
    if local_norm.is_empty() {
        return None;
    }
    let mut best_score: usize = 0;
    let mut best: Option<ModelPricing> = None;

    for (key, pricing) in map.iter() {
        if key.contains('/') {
            continue;
        }
        let key_norm = normalize(strip_date_suffix(key));
        let s = fuzzy_score(&local_norm, &key_norm);
        if s > best_score {
            best_score = s;
            best = Some(*pricing);
        }
    }

    // Require minimum 60% match
    if best_score > 0 {
        let min_required = (local_norm.len().max(3) * 6) / 10;
        if best_score >= min_required {
            return best;
        }
    }

    None
}

/// Estimate cost from model name and token usage
pub fn estimate_cost(model_name: &str, tokens: &crate::stats::Tokens) -> Option<f64> {
    let p = lookup_pricing(model_name)?;
    Some(
        tokens.input as f64 * p.prompt
            + tokens.output as f64 * p.completion
            + tokens.reasoning as f64 * p.reasoning
            + tokens.cache_read as f64 * p.input_cache_read
            + tokens.cache_write as f64 * p.input_cache_write,
    )
}

/// Normalize slug for comparison
fn normalize(slug: &str) -> String {
    slug.chars()
        .filter(|c| *c != '-' && *c != '.' && *c != ':')
        .collect()
}

/// Fuzzy match score using LCS with scratch buffer
fn fuzzy_score(a: &str, b: &str) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    if a == b {
        return a.len() * 2;
    }
    if b.starts_with(a) || a.starts_with(b) {
        return a.len().min(b.len()) * 2;
    }

    // Use thread-local scratch buffer
    FUZZY_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let b_len = b.len() + 1;
        let needed = b_len * 2;

        // Ensure capacity
        if scratch.len() < needed {
            scratch.resize(needed, 0);
        }

        // Split into prev and curr slices
        let (prev, curr) = scratch.split_at_mut(b_len);

        // Reset used portions
        prev[..b_len].fill(0);
        curr[..b_len].fill(0);

        // LCS on bytes
        let a = a.as_bytes();
        let b = b.as_bytes();
        for &ac in a {
            for (j, &bc) in b.iter().enumerate() {
                curr[j + 1] = if ac == bc {
                    prev[j] + 1
                } else {
                    prev[j + 1].max(curr[j])
                };
            }
            // Swap prev and curr by copying
            prev[..b_len].copy_from_slice(&curr[..b_len]);
            curr[..b_len].fill(0);
        }
        prev[b.len()] as usize
    })
}

/// Strip trailing date suffix (MMDD or YYYYMMDD).
fn strip_date_suffix(slug: &str) -> &str {
    let Some(pos) = slug.rfind('-') else {
        return slug;
    };
    let tail = &slug[pos + 1..];
    if looks_like_yyyymmdd(tail) || looks_like_mmdd(tail) {
        return &slug[..pos];
    }
    slug
}

fn looks_like_mmdd(tail: &str) -> bool {
    if tail.len() != 4 || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mm: u32 = tail[0..2].parse().unwrap_or(0);
    let dd: u32 = tail[2..4].parse().unwrap_or(0);
    (1..=12).contains(&mm) && (1..=31).contains(&dd)
}

fn looks_like_yyyymmdd(tail: &str) -> bool {
    if tail.len() != 8 || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let yyyy: u32 = tail[0..4].parse().unwrap_or(0);
    let mm: u32 = tail[4..6].parse().unwrap_or(0);
    let dd: u32 = tail[6..8].parse().unwrap_or(0);
    (2020..=2100).contains(&yyyy) && (1..=12).contains(&mm) && (1..=31).contains(&dd)
}

/// Returns the path to the cache file.
fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".cache")
        .join("opencode-stats-tui")
        .join("openrouter-pricing.json")
}

/// Returns true if the cache file exists and is less than 24 hours.
fn cache_is_fresh() -> bool {
    let Ok(meta) = std::fs::metadata(cache_path()) else {
        return false;
    };
    meta.modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(86400))
}

fn parse_body(body: &serde_json::Value) -> FxHashMap<String, ModelPricing> {
    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return FxHashMap::default();
    };
    let mut map = FxHashMap::default();
    for m in data {
        let Some(id) = m.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(pr) = m.get("pricing").and_then(|v| v.as_object()) else {
            continue;
        };
        let p = |k: &str| -> f64 {
            pr.get(k)
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse().ok())
                        .or_else(|| v.as_f64())
                })
                .unwrap_or(0.0)
                .max(0.0)
        };
        let prompt = p("prompt");
        let completion = p("completion");
        let reasoning = {
            let r = p("reasoning");
            if r == 0.0 {
                completion
            } else {
                r
            }
        };
        let pricing = ModelPricing {
            prompt,
            completion,
            reasoning,
            input_cache_read: p("input_cache_read"),
            input_cache_write: p("input_cache_write"),
        };

        let slug = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
        let full = id.to_ascii_lowercase();

        map.entry(full).or_insert(pricing);
        map.entry(slug).or_insert(pricing);
    }
    map
}

fn fetch_pricing() -> FxHashMap<String, ModelPricing> {
    let path = cache_path();

    // Use disk cache if fresh (< 1 day old)
    if cache_is_fresh() {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let map = parse_body(&body);
                if !map.is_empty() {
                    return map;
                }
            }
        }
    }

    // Fetch from API
    let body = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .get("https://openrouter.ai/api/v1/models")
        .call()
        .ok()
        .and_then(|r| r.into_json::<serde_json::Value>().ok());

    if let Some(ref b) = body {
        let map = parse_body(b);
        if !map.is_empty() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, serde_json::to_string(b).unwrap_or_default());
            return map;
        }
    }

    // Stale cache fallback
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(b) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let map = parse_body(&b);
            if !map.is_empty() {
                return map;
            }
        }
    }

    FxHashMap::default()
}

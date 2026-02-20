use fxhash::FxHashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub struct ModelPricing {
    pub prompt: f64,
    pub completion: f64,
    pub reasoning: f64,
    pub input_cache_read: f64,
    pub input_cache_write: f64,
}

static PRICING_CACHE: OnceLock<FxHashMap<String, ModelPricing>> = OnceLock::new();

pub fn init_pricing() {
    PRICING_CACHE.get_or_init(fetch_pricing);
}

/// Look up pricing for a model name like "prox/minimax-m2" or "anthropic/claude-sonnet-4".
/// The provider prefix (before '/') is stripped — only the model slug matters.
/// Returns None if no match found.
pub fn lookup_pricing(model_name: &str) -> Option<ModelPricing> {
    let cache = PRICING_CACHE.get_or_init(fetch_pricing);
    let found = lookup_in_map(cache, model_name);
    if found.is_some() {
        return found;
    }

    // On miss, try a one-time live fetch to avoid stale 24h disk cache misses
    // for newly added models (e.g., minimax variants).
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

    // 1) Exact full-id match (when available)
    if let Some(p) = map.get(input.as_str()) {
        return Some(*p);
    }

    // 2) Exact slug match (provider-agnostic)
    if let Some(p) = map.get(slug.as_str()) {
        return Some(*p);
    }

    // 3) Strip date suffix and retry (e.g. "claude-sonnet-4-20250514" -> "claude-sonnet-4")
    let stripped = strip_date_suffix(&slug);
    if stripped != slug {
        if let Some(p) = map.get(stripped) {
            return Some(*p);
        }
    }

    // 4) Normalize and find best fuzzy match
    let local_norm = normalize(stripped);
    if local_norm.is_empty() {
        return None;
    }
    let mut best_score: usize = 0;
    let mut best: Option<ModelPricing> = None;

    // Only fuzzy-scan slug keys (no '/'): we store both full-id and slug,
    // so this avoids duplicate work and improves hot-path lookup speed.
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

    // Require minimum 60% of the longer side matched
    if best_score > 0 {
        let min_required = (local_norm.len().max(3) * 6) / 10; // 60% threshold
        if best_score >= min_required {
            return best;
        }
    }

    None
}

/// Returns `Some(cost)` when pricing is found, `None` when the model is unknown.
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

// ---------------------------------------------------------------------------
// Matching helpers
// ---------------------------------------------------------------------------

/// Normalize a slug to a comparable string: lowercase, remove all `-`, `.`, `:`.
/// "qwen-3.5-plus" and "qwen3.5-plus-02-15" both become "qwen35plus0215".
/// This makes "qwen-3.5" == "qwen3.5" naturally.
fn normalize(slug: &str) -> String {
    slug.chars()
        .filter(|c| *c != '-' && *c != '.' && *c != ':')
        .collect()
}

/// Score how well two normalized strings match.
/// Uses longest common subsequence length as score.
/// Returns 0 for no meaningful match.
fn fuzzy_score(a: &str, b: &str) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    // Quick check: if one contains the other, it's a strong match
    if a == b {
        return a.len() * 2;
    }
    if b.starts_with(a) || a.starts_with(b) {
        return a.len().min(b.len()) * 2;
    }

    // LCS (longest common subsequence) on bytes
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut prev = vec![0u16; b.len() + 1];
    let mut curr = vec![0u16; b.len() + 1];
    for &ac in a {
        for (j, &bc) in b.iter().enumerate() {
            curr[j + 1] = if ac == bc {
                prev[j] + 1
            } else {
                prev[j + 1].max(curr[j])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.iter_mut().for_each(|v| *v = 0);
    }
    prev[b.len()] as usize
}

/// Strip a trailing date suffix: only MMDD (4 digits) or YYYYMMDD (8 digits).
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

// ---------------------------------------------------------------------------
// Cache & fetch
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".cache")
        .join("opencode-stats-tui")
        .join("openrouter-pricing.json")
}

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

        // Key by slug only (part after '/') — provider doesn't matter
        let slug = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
        let full = id.to_ascii_lowercase();

        // Keep both keys:
        // - full id for exact match when available
        // - slug for provider-agnostic lookup (e.g., prox/minimax-m2 -> minimax-m2)
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

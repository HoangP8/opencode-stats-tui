use crate::live_watcher::LiveWatcher;
use crate::session::SessionModal;
use crate::stats::{
    format_duration_ms, format_number, format_number_full, load_session_chat_incremental,
    load_session_chat_with_max_ts, ChatMessage, DayStat, MessageContent, ModelUsage, ToolUsage,
    Totals, MAX_MESSAGES_TO_LOAD,
};
use crate::stats_cache::StatsCache;
use chrono::Datelike;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use parking_lot::Mutex;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

// Constants for optimized mouse handling

/// Cached chat session data including pre-calculated scroll info
struct CachedChat {
    messages: Arc<Vec<ChatMessage>>,
    total_lines: u16,
    last_file_count: usize,
    last_max_ts: i64,
}

/// Helper to create cache key from session_id and day
fn cache_key(session_id: &str, day: Option<&str>) -> String {
    if let Some(d) = day {
        format!("{}|{}", session_id, d)
    } else {
        session_id.to_string()
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Left,
    Right,
}

#[derive(PartialEq, Clone, Copy)]
enum LeftPanel {
    Stats,
    Days,
    Models,
}

#[derive(PartialEq, Clone, Copy)]
enum RightPanel {
    Detail,
    List,
    Tools,
}

/// Cached panel rectangles for efficient mouse hit-testing
/// Updated during render to match exactly what's displayed
#[derive(Default, Clone)]
struct PanelRects {
    // Left panels
    stats: Option<Rect>,
    days: Option<Rect>,
    models: Option<Rect>,
    // Right panels (context-dependent based on left_panel)
    detail: Option<Rect>, // SESSION INFO or MODEL INFO
    list: Option<Rect>,   // SESSIONS or MODEL RANKING
    tools: Option<Rect>,  // TOOLS USED (only in Models view)
}

impl PanelRects {
    /// Optimized hit-test that returns early once a match is found
    #[inline(always)]
    fn find_panel(&self, x: u16, y: u16) -> Option<&'static str> {
        // Check in order of most common usage for early return
        if Self::contains_point(self.list, x, y) {
            return Some("list");
        }
        if Self::contains_point(self.days, x, y) {
            return Some("days");
        }
        if Self::contains_point(self.models, x, y) {
            return Some("models");
        }
        if Self::contains_point(self.detail, x, y) {
            return Some("detail");
        }
        if Self::contains_point(self.tools, x, y) {
            return Some("tools");
        }
        if Self::contains_point(self.stats, x, y) {
            return Some("stats");
        }
        None
    }

    #[inline(always)]
    fn contains_point(rect: Option<Rect>, x: u16, y: u16) -> bool {
        rect.is_some_and(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
    }
}

pub struct App {
    totals: Totals,
    per_day: HashMap<String, DayStat>,
    session_titles: HashMap<Box<str>, String>,
    session_message_files: HashMap<String, Vec<PathBuf>>,
    day_list: Vec<String>,
    day_list_state: ListState,
    session_list: Vec<Arc<crate::stats::SessionStat>>,
    session_list_state: ListState,
    cached_session_items: Vec<ListItem<'static>>, // Cached rendered list items
    cached_session_width: u16,
    chat_cache: HashMap<String, CachedChat>,
    chat_cache_order: Vec<String>,
    chat_scroll: u16,
    model_usage: Vec<ModelUsage>,
    model_list_state: ListState,
    tool_usage: Vec<ToolUsage>,
    tool_scroll: u16,
    tool_max_scroll: u16,
    detail_scroll: u16,
    detail_max_scroll: u16,
    model_tool_scroll: u16,
    model_tool_max_scroll: u16,
    ranking_scroll: usize,
    ranking_max_scroll: usize,

    // Phase 2: Render Caching
    cached_day_strings: HashMap<String, String>, // Pre-formatted day strings with weekday names
    

    chat_max_scroll: u16,
    focus: Focus,
    left_panel: LeftPanel,
    right_panel: RightPanel,
    is_active: bool,
    models_active: bool,
    exit: bool,
    selected_model_index: Option<usize>,
    current_chat_session_id: Option<String>,

    modal: SessionModal,

    // Optimized mouse tracking
    last_mouse_panel: Option<&'static str>, // Cache last panel for faster hit-testing
    last_session_click: Option<(std::time::Instant, usize)>, // Double-click detection for sessions

    // Cached panel rectangles for optimized mouse hit-testing
    cached_rects: PanelRects,

    // Phase 1 optimizations
    cached_git_branch: Option<(Box<str>, Option<String>)>, // (path_root, branch) - avoid fs I/O per frame
    cached_max_cost_width: usize,

    // Live stats: Cache and file watching
    stats_cache: Option<StatsCache>,
    _storage_path: PathBuf,
    live_watcher: Option<LiveWatcher>,
    needs_refresh: Arc<Mutex<Vec<PathBuf>>>,
    pending_refresh_paths: Vec<PathBuf>,
    last_refresh: Option<std::time::Instant>,
    should_redraw: bool,
}

/// Helper: Create a stat paragraph with label and value
fn stat_widget(label: &str, value: String, color: Color) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(Span::styled(
            label.to_string(),
            Style::default().fg(Color::Rgb(180, 180, 180)),
        )),
        Line::from(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
    ])
    .alignment(Alignment::Center)
}

/// Helper: Formatting configuration for usage list rows
struct UsageRowFormat {
    name_width: usize,
    cost_width: usize,
    sess_width: usize,
}

/// Helper: Create a list row with consistent formatting for usage lists
/// Optimized with pre-allocated Vec capacity
fn usage_list_row(
    name: String,
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    session_count: usize,
    format: &UsageRowFormat,
) -> Line<'static> {
    let in_val = format_number(input_tokens);
    let out_val = format_number(output_tokens);

    // Optimized: use format! with padding instead of manual loop
    let name_display = format!(
        "{:<width$}",
        name.chars().take(format.name_width).collect::<String>(),
        width = format.name_width
    );

    // Optimized: combine nested format! calls into single format
    let spans = vec![
        Span::styled(name_display, Style::default().fg(Color::White)),
        Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
        Span::styled(format!("{:>7}", in_val), Style::default().fg(Color::Blue)),
        Span::styled(" in ", Style::default().fg(Color::Rgb(180, 180, 180))),
        Span::styled(
            format!("{:>7}", out_val),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled(" out", Style::default().fg(Color::Rgb(180, 180, 180))),
        Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
        Span::styled(
            format!("${:>width$.2}", cost, width = format.cost_width),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
        Span::styled(
            format!("{:>width$} sess", session_count, width = format.sess_width),
            Style::default().fg(Color::Cyan),
        ),
    ];
    Line::from(spans)
}

/// Helper: Safely truncate a string to max characters without breaking UTF-8 (no ellipsis)
/// Returns Cow to avoid allocation when no truncation needed
fn safe_truncate_plain(s: &str, max_chars: usize) -> Cow<'_, str> {
    let mut count = 0;
    for (idx, _) in s.char_indices() {
        count += 1;
        if count > max_chars {
            return Cow::Owned(s[..idx].to_string());
        }
    }
    Cow::Borrowed(s)
}

/// Helper: Truncate a string to max characters and add ellipsis if truncated
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut count = 0;
    for (idx, _) in s.char_indices() {
        count += 1;
        if count > max_chars {
            let visible_chars = max_chars.saturating_sub(3);
            let byte_end = s
                .char_indices()
                .nth(visible_chars)
                .map(|(i, _)| i)
                .unwrap_or(idx);
            let mut result = String::with_capacity(byte_end + 3);
            result.push_str(&s[..byte_end]);
            result.push_str("...");
            return result;
        }
    }
    s.into()
}

/// Calculate the actual number of rendered lines for a chat message
fn calculate_message_rendered_lines(msg: &ChatMessage) -> u16 {
    let mut lines = 1u16; // Header line

    for part in &msg.parts {
        match part {
            MessageContent::Text(text) => {
                let (_max_line_chars, max_lines) = match &*msg.role {
                    "user" => (150, 5),
                    "assistant" => (250, 8),
                    _ => (200, 6),
                };

                let line_count = text.lines().count();
                lines += line_count.min(max_lines) as u16;

                // Add indicator if truncated
                if line_count > max_lines {
                    lines += 1;
                }
            }
            MessageContent::ToolCall(_) => {
                lines += 1;
            }
            MessageContent::Thinking(_) => {
                lines += 1;
            }
        }
    }

    lines
}

impl App {
    pub fn new() -> Self {
        // Initialize logger
        // env_logger::init();

        // Get storage path
        let storage_path = std::env::var("XDG_DATA_HOME")
            .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap()))
            .to_string();
        let storage_path = PathBuf::from(storage_path).join("opencode").join("storage");

        // Initialize cache
        let stats_cache = StatsCache::new(storage_path.clone()).ok();
        log::info!("Initialized stats cache for: {}", storage_path.display());

        let (totals, per_day, session_titles, model_usage, session_message_files) =
            if let Some(cache) = &stats_cache {
                let s = cache.load_or_compute();
                (
                    s.totals,
                    s.per_day,
                    s.session_titles,
                    s.model_usage,
                    s.session_message_files,
                )
            } else {
                let s = crate::stats::collect_stats();
                (
                    s.totals,
                    s.per_day,
                    s.session_titles,
                    s.model_usage,
                    s.session_message_files,
                )
            };

        // Set up live watcher for real-time updates
        let needs_refresh = Arc::new(Mutex::new(Vec::new()));
        let needs_refresh_clone = needs_refresh.clone();
        let mut live_watcher = LiveWatcher::new(
            storage_path.clone(),
            Arc::new(move |files| {
                needs_refresh_clone.lock().extend(files);
            }),
        )
        .ok();

        if let Some(watcher) = &mut live_watcher {
            if let Err(e) = watcher.start() {
                log::error!("Failed to start live watcher: {}", e);
            }
        }

        let mut day_list: Vec<String> = per_day.keys().cloned().collect();
        day_list.sort_unstable_by(|a, b| b.cmp(a));

        let mut day_list_state = ListState::default();
        if !day_list.is_empty() {
            day_list_state.select(Some(0));
        }

        let mut model_list_state = ListState::default();
        let mut selected_model_index = None;
        if !model_usage.is_empty() {
            model_list_state.select(Some(0));
            selected_model_index = Some(0);
        }

        let mut tool_usage: Vec<ToolUsage> = totals
            .tools
            .iter()
            .map(|(name, count)| ToolUsage {
                name: name.clone(),
                count: *count,
            })
            .collect();
        tool_usage.sort_unstable_by(|a, b| b.count.cmp(&a.count));

        let mut app = Self {
            totals,
            per_day,
            session_titles,
            session_message_files,
            day_list,
            day_list_state,
            session_list: Vec::new(),
            session_list_state: ListState::default(),
            chat_cache: HashMap::new(),
            chat_cache_order: Vec::new(),
            chat_scroll: 0,
            model_usage,
            model_list_state,
            tool_usage,
            tool_scroll: 0,
            tool_max_scroll: 0,
            detail_scroll: 0,
            detail_max_scroll: 0,
            model_tool_scroll: 0,
            model_tool_max_scroll: 0,
            ranking_scroll: 0,
            ranking_max_scroll: 0,
            cached_session_items: Vec::new(),
            cached_session_width: 0,
            cached_day_strings: HashMap::with_capacity(32),


            chat_max_scroll: 0,
            focus: Focus::Left,
            left_panel: LeftPanel::Days,
            right_panel: RightPanel::List,
            is_active: false,
            models_active: false,
            exit: false,
            selected_model_index,
            current_chat_session_id: None,

            modal: SessionModal::new(),

            last_mouse_panel: None,
            last_session_click: None,

            cached_rects: PanelRects::default(),

            cached_git_branch: None,
            cached_max_cost_width: 0,

            stats_cache,
            _storage_path: storage_path,
            live_watcher,
            needs_refresh,
            pending_refresh_paths: Vec::new(),
            last_refresh: None,
            should_redraw: true,
        };
        app.update_session_list();
        app.precompute_day_strings();
        app.recompute_max_cost_width();
        app
    }

    fn recompute_max_cost_width(&mut self) {
        let mut max_len = 8usize;
        for day in &self.day_list {
            if let Some(stat) = self.per_day.get(day) {
                let s = format!("{:.2}", stat.display_cost());
                max_len = max_len.max(s.len());
            }
        }
        for m in self.model_usage.iter() {
            let s = format!("{:.2}", m.cost);
            max_len = max_len.max(s.len());
        }
        self.cached_max_cost_width = max_len;
    }

    #[inline]
    fn max_cost_width(&self) -> usize {
        self.cached_max_cost_width
    }

    fn update_session_list(&mut self) {
        let prev_selected_id = self.session_list_state.selected()
            .and_then(|i| self.session_list.get(i))
            .map(|s| s.id.clone());

        self.session_list.clear();
        if let Some(day) = self.selected_day() {
            if let Some(stat) = self.per_day.get(&day) {
                let mut sessions: Vec<_> = stat.sessions.values().cloned().collect();
                sessions.sort_unstable_by(|a, b| b.last_activity.cmp(&a.last_activity));
                self.session_list = sessions;
            }
        }
        if !self.session_list.is_empty() {
            if let Some(prev_id) = prev_selected_id.as_ref() {
                if let Some(idx) = self.session_list.iter().position(|s| s.id == *prev_id) {
                    self.session_list_state.select(Some(idx));
                } else {
                    self.session_list_state.select(Some(0));
                }
            } else {
                self.session_list_state.select(Some(0));
            }
        } else {
            self.session_list_state.select(None);
        }
        // Clear cached chat session since session list changed
        self.current_chat_session_id = None;
        // Clear cached items; rebuild on render with correct width
        self.cached_session_items.clear();
        self.cached_session_width = 0;
        // Invalidate git branch cache since selected session may have changed
        self.cached_git_branch = None;
    }

    /// Rebuild cached session list items to avoid heavy computation on every render
    fn rebuild_cached_session_items(&mut self, width: u16) {
        self.cached_session_width = width;
        let max_cost_len = self
            .session_list
            .iter()
            .map(|s| format!("{:.2}", s.display_cost()).len())
            .max()
            .unwrap_or(0)
            .max(8);
        let max_models_len = self
            .session_list
            .iter()
            .map(|s| {
                let c = s.models.len();
                if c == 1 {
                    "1 model".len()
                } else {
                    format!("{} models", c).len()
                }
            })
            .max()
            .unwrap_or(7);
        let fixed_width = 3 + 8 + 3 + 8 + 3 + (max_cost_len + 1) + 3 + 8 + 3 + max_models_len + 2;
        let title_width =
            width.saturating_sub((fixed_width).min(u16::MAX as usize) as u16) as usize;

        self.cached_session_items = self
            .session_list
            .iter()
            .map(|s| {
                // No [Continued] badge - continuation info shown in panel title above
                let title = self
                    .session_titles
                    .get(&s.id)
                    .map(|t| t.strip_prefix("New session - ").unwrap_or(t).to_string())
                    .unwrap_or_else(|| s.id.chars().take(14).collect());

                let model_count = s.models.len();
                let model_text = if model_count == 1 {
                    "1 model".into()
                } else {
                    format!("{} models", model_count)
                };
                let model_text = format!("{:>width$}", model_text, width = max_models_len);
                let additions = s.diffs.additions;
                let deletions = s.diffs.deletions;

                // Gray title for continued sessions to highlight them
                let title_color = if s.is_continuation {
                    Color::Rgb(150, 150, 150)
                } else {
                    Color::White
                };

                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(
                            "{:<width$}",
                            title.chars().take(title_width.max(8)).collect::<String>(),
                            width = title_width.max(8)
                        ),
                        Style::default().fg(title_color),
                    ),
                    Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
                    Span::styled(
                        format!("{}{:>7}", "+", format_number(additions)),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
                    Span::styled(
                        format!("{}{:>7}", "-", format_number(deletions)),
                        Style::default().fg(Color::Red),
                    ),
                    Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
                    Span::styled(
                        format!("${:>width$.2}", s.display_cost(), width = max_cost_len),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
                    Span::styled(
                        format!("{:>4} msg", s.messages),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(" │ ", Style::default().fg(Color::Rgb(180, 180, 180))),
                    Span::styled(model_text, Style::default().fg(Color::Magenta)),
                ]))
            })
            .collect();
    }

    /// Precompute formatted day strings with weekday names (Phase 2 optimization)
    fn precompute_day_strings(&mut self) {
        // Only compute if not already cached
        for day in &self.day_list {
            if self.cached_day_strings.contains_key(day) {
                continue;
            }
            if let Ok(parsed) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
                let weekday = parsed.weekday();
                let day_abbr = match weekday {
                    chrono::Weekday::Mon => "Mon",
                    chrono::Weekday::Tue => "Tue",
                    chrono::Weekday::Wed => "Wed",
                    chrono::Weekday::Thu => "Thu",
                    chrono::Weekday::Fri => "Fri",
                    chrono::Weekday::Sat => "Sat",
                    chrono::Weekday::Sun => "Sun",
                };
                let formatted = format!("{} ({})", day, day_abbr);
                self.cached_day_strings.insert(day.clone(), formatted);
            } else {
                self.cached_day_strings.insert(day.clone(), day.clone());
            }
        }
    }

    fn open_session_modal(&mut self, area_height: u16) {
        let session_stat = match self.session_list_state.selected()
            .and_then(|i| self.session_list.get(i))
            .cloned()
        {
            Some(s) => s,
            None => return,
        };

        let session_id = session_stat.id.clone();

        // Get the current day for filtering messages
        let current_day = self.selected_day();

        self.chat_scroll = 0;
        let session_id_str = session_id.to_string();
        self.current_chat_session_id = Some(session_id_str.clone());

        // Use composite key (session_id + day) for caching
        let cache_key = cache_key(&session_id_str, current_day.as_deref());

        let total_lines = if let Some(cached) = self.chat_cache.get(&cache_key) {
            let messages_arc = Arc::clone(&cached.messages);
            if let Some(pos) = self.chat_cache_order.iter().position(|s| s == &cache_key) {
                self.chat_cache_order.remove(pos);
            }
            self.chat_cache_order.push(cache_key.clone());

            // Open modal with cached messages (Arc clone, no deep copy)
            self.modal.open_session(
                &session_id_str,
                messages_arc,
                &session_stat,
                self.session_message_files
                    .get(&session_id_str)
                    .map(|v| v.as_slice()),
                current_day.as_deref(),
            );
            cached.total_lines
        } else {
            let files = self
                .session_message_files
                .get(&session_id_str)
                .map(|v| v.as_slice());
            // Pass current day to filter messages to only show this day's messages
            let (messages, max_ts) =
                load_session_chat_with_max_ts(&session_id_str, files, current_day.as_deref());
            let total_lines: u16 = messages.iter().map(calculate_message_rendered_lines).sum();
            let blank_lines = if !messages.is_empty() {
                messages.len() - 1
            } else {
                0
            };
            let total_lines = total_lines + blank_lines as u16;
            let file_count = files.map(|f| f.len()).unwrap_or(0);

            // Implement LRU cache eviction if cache is too large
            const MAX_CACHE_SIZE: usize = 5;
            if self.chat_cache.len() >= MAX_CACHE_SIZE {
                if let Some(oldest) = self.chat_cache_order.first() {
                    self.chat_cache.remove(oldest);
                    self.chat_cache_order.remove(0);
                }
            }

            let messages_arc = Arc::new(messages);

            self.chat_cache.insert(
                cache_key.clone(),
                CachedChat {
                    messages: Arc::clone(&messages_arc),
                    total_lines,
                    last_file_count: file_count,
                    last_max_ts: max_ts,
                },
            );
            self.chat_cache_order.push(cache_key.clone());

            // Open modal with Arc (no deep copy)
            self.modal.open_session(
                &session_id_str,
                messages_arc,
                &session_stat,
                files,
                current_day.as_deref(),
            );
            total_lines
        };

        self.chat_max_scroll = total_lines.saturating_sub(area_height.saturating_sub(4));
    }

    fn day_next(&mut self) {
        if self.day_list.is_empty() {
            return;
        }
        let i = self.day_list_state.selected().unwrap_or(0);
        self.day_list_state
            .select(Some((i + 1).min(self.day_list.len() - 1)));
    }

    fn day_previous(&mut self) {
        let i = self.day_list_state.selected().unwrap_or(0);
        self.day_list_state.select(Some(i.saturating_sub(1)));
    }

    fn model_next(&mut self) {
        if self.model_usage.is_empty() {
            return;
        }
        let i = self.model_list_state.selected().unwrap_or(0);
        self.model_list_state
            .select(Some((i + 1).min(self.model_usage.len() - 1)));
    }

    fn model_previous(&mut self) {
        let i = self.model_list_state.selected().unwrap_or(0);
        self.model_list_state.select(Some(i.saturating_sub(1)));
    }

    fn session_next(&mut self) {
        if self.session_list.is_empty() {
            return;
        }
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state
            .select(Some((i + 1).min(self.session_list.len() - 1)));
        // Clear cached chat session since selection changed
        self.current_chat_session_id = None;
    }

    fn session_previous(&mut self) {
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state.select(Some(i.saturating_sub(1)));
        // Clear cached chat session since selection changed
        self.current_chat_session_id = None;
    }

    fn selected_day(&self) -> Option<String> {
        self.day_list_state
            .selected()
            .and_then(|i| self.day_list.get(i).cloned())
    }


    /// Refresh stats from cache (for live updates)
    pub fn refresh_stats(&mut self, changed_files: Vec<PathBuf>) {
        if let Some(cache) = &self.stats_cache {
            let is_full_refresh = changed_files.is_empty();
            let mut affected_sessions = std::collections::HashSet::new();

            let (totals, per_day, session_titles, model_usage, session_message_files) =
                if is_full_refresh {
                    let s = cache.load_or_compute();
                    (
                        s.totals,
                        s.per_day,
                        s.session_titles,
                        s.model_usage,
                        s.session_message_files,
                    )
                } else {
                    let files: Vec<String> = changed_files
                        .iter()
                        .filter_map(|p| p.to_str().map(ToString::to_string))
                        .collect();
                    affected_sessions = cache.update_files(files);
                    let s = cache.get_stats();
                    (
                        s.totals,
                        s.per_day,
                        s.session_titles,
                        s.model_usage,
                        s.session_message_files,
                    )
                };

            // Update all stats
            self.totals = totals;
            self.per_day = per_day;
            self.session_titles = session_titles;
            self.model_usage = model_usage;
            self.session_message_files = session_message_files;

            // Rebuild derived data if full refresh, otherwise partial
            if is_full_refresh {
                let prev_selected_day = self.selected_day();
                self.day_list.clear();
                self.day_list.extend(self.per_day.keys().cloned());
                self.day_list.sort_unstable_by(|a, b| b.cmp(a));
                if let Some(prev) = prev_selected_day.as_ref() {
                    if let Some(idx) = self.day_list.iter().position(|d| d == prev) {
                        self.day_list_state.select(Some(idx));
                    } else if !self.day_list.is_empty() {
                        self.day_list_state.select(Some(0));
                    }
                } else if !self.day_list.is_empty() && self.day_list_state.selected().is_none() {
                    self.day_list_state.select(Some(0));
                }
                self.update_session_list();
            } else {
                // For incremental updates, we don't need to rebuild the entire day_list
                // just ensure it contains any new days (though messages usually arrive on existing days)
                let mut day_list_changed = false;
                for day in self.per_day.keys() {
                    if !self.day_list.contains(day) {
                        self.day_list.push(day.clone());
                        day_list_changed = true;
                    }
                }
                if day_list_changed {
                    let prev_selected_day = self.selected_day();
                    self.day_list.sort_unstable_by(|a, b| b.cmp(a));
                    if let Some(prev) = prev_selected_day.as_ref() {
                        if let Some(idx) = self.day_list.iter().position(|d| d == prev) {
                            self.day_list_state.select(Some(idx));
                        }
                    } else if self.day_list_state.selected().is_none() {
                        self.day_list_state.select(Some(0));
                    }
                }

                // Only rebuild session list if the currently selected day was affected
                let mut current_day_affected = false;
                if let Some(current_day) = self.selected_day() {
                    if let Some(day_stat) = self.per_day.get(&current_day) {
                        for session_id in &affected_sessions {
                            if day_stat.sessions.contains_key(session_id) {
                                current_day_affected = true;
                                break;
                            }
                        }
                    }
                }

                if current_day_affected {
                    self.update_session_list();
                }
            }

            // Update tool usage
            let mut tool_usage: Vec<ToolUsage> = self
                .totals
                .tools
                .iter()
                .map(|(name, count)| ToolUsage {
                    name: name.clone(),
                    count: *count,
                })
                .collect();
            tool_usage.sort_unstable_by(|a, b| b.count.cmp(&a.count));
            self.tool_usage = tool_usage;

            // Update model list state if needed
            if !self.model_usage.is_empty() && self.model_list_state.selected().is_none() {
                self.model_list_state.select(Some(0));
                self.selected_model_index = Some(0);
            }

            // Incrementally update open chat modal if its session changed.
            if let Some(current) = self.current_chat_session_id.as_deref() {
                if affected_sessions.contains(current) {
                    let current_day = self.selected_day();
                    let cache_key = cache_key(current, current_day.as_deref());
                    if let Some(files) = self.session_message_files.get(current) {
                        if let Some(cached) = self.chat_cache.get(&cache_key) {
                            let cached_messages = Arc::clone(&cached.messages);
                            let cached_total_lines = cached.total_lines;
                            let cached_last_file_count = cached.last_file_count;
                            let cached_last_max_ts = cached.last_max_ts;
                            if cached_last_file_count < files.len() {
                                let new_files = &files[cached_last_file_count..];
                                let (mut new_msgs, new_max_ts) =
                                    load_session_chat_incremental(
                                        new_files,
                                        current_day.as_deref(),
                                        Some(cached_last_max_ts),
                                    );

                                let mut merged: Vec<ChatMessage> =
                                    Vec::with_capacity(cached_messages.len() + new_msgs.len());
                                merged.extend((*cached_messages).clone());
                                if !new_msgs.is_empty() {
                                    if let Some(last) = merged.last_mut() {
                                        if let Some(first) = new_msgs.first() {
                                            if last.role == first.role {
                                                last.parts.extend(first.parts.clone());
                                                new_msgs.remove(0);
                                            }
                                        }
                                    }
                                    merged.extend(new_msgs);
                                }

                                if merged.len() > MAX_MESSAGES_TO_LOAD {
                                    let start = merged.len() - MAX_MESSAGES_TO_LOAD;
                                    merged.drain(..start);
                                }

                                let total_lines: u16 =
                                    merged.iter().map(calculate_message_rendered_lines).sum();
                                let blank_lines = if !merged.is_empty() {
                                    merged.len() - 1
                                } else {
                                    0
                                };
                                let total_lines = total_lines + blank_lines as u16;

                                let messages_arc = Arc::new(merged);
                                self.chat_cache.insert(
                                    cache_key.clone(),
                                    CachedChat {
                                        messages: Arc::clone(&messages_arc),
                                        total_lines,
                                        last_file_count: files.len(),
                                        last_max_ts: cached_last_max_ts.max(new_max_ts),
                                    },
                                );
                                if let Some(pos) = self
                                    .chat_cache_order
                                    .iter()
                                    .position(|s| s == &cache_key)
                                {
                                    self.chat_cache_order.remove(pos);
                                }
                                self.chat_cache_order.push(cache_key.clone());
                                if self.modal.open {
                                    self.modal.chat_messages = messages_arc;
                                }
                                affected_sessions.remove(current);
                            } else if cached_last_file_count != files.len() {
                                self.chat_cache.insert(
                                    cache_key.clone(),
                                    CachedChat {
                                        messages: cached_messages,
                                        total_lines: cached_total_lines,
                                        last_file_count: files.len(),
                                        last_max_ts: cached_last_max_ts,
                                    },
                                );
                            }
                        }
                    }
                }
            }

            // Invalidate chat cache for remaining affected sessions.
            if !affected_sessions.is_empty() {
                if affected_sessions.len() == 1 {
                    let only = affected_sessions.iter().next().unwrap();
                    self.chat_cache.retain(|key, _| {
                        if key == only {
                            return false;
                        }
                        key.strip_prefix(only)
                            .and_then(|rest| rest.strip_prefix('|'))
                            .is_none()
                    });
                } else {
                    self.chat_cache.retain(|key, _| {
                        let session_id = key.split_once('|').map(|(s, _)| s).unwrap_or(key);
                        !affected_sessions.contains(session_id)
                    });
                }
                self.chat_cache_order
                    .retain(|key| self.chat_cache.contains_key(key));
                if let Some(current) = self.current_chat_session_id.as_deref() {
                    if affected_sessions.contains(current) {
                        self.current_chat_session_id = None;
                    }
                }
            }

            self.precompute_day_strings();
            self.recompute_max_cost_width();

            log::debug!("Stats refreshed successfully (live update)");
        }
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        // Render immediately on startup
        self.should_redraw = true;

        while !self.exit {
            // Wait for events with a timeout
            // This parks the thread and keeps it responsive to OS signals and new input
            if event::poll(std::time::Duration::from_millis(250))? {
                // DRAIN ALL EVENTS first to ensure keyboard input (like 'q') is handled immediately
                while event::poll(std::time::Duration::from_millis(0))? {
                    match event::read()? {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key_event(key, terminal.size()?.height)?;
                                self.should_redraw = true;
                                if self.exit {
                                    return Ok(());
                                }
                            }
                        }
                        Event::Resize(_, _) => {
                            self.should_redraw = true;
                        }
                        Event::Mouse(mouse) => {
                            let size = terminal.size()?;
                            let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);

                            if self.modal.open {
                                if self.modal.handle_mouse_event(mouse, area) {
                                    self.chat_scroll = self.modal.chat_scroll;
                                    self.should_redraw = true;
                                }
                            } else if self.handle_mouse_event(mouse, area) {
                                self.should_redraw = true;
                            }
                        }
                        Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
                    }
                }
            }

            // Check for live updates regardless of user input
            // This ensures background file changes are processed even if the user is idle
            if let Some(watcher) = &self.live_watcher {
                watcher.process_changes();
            }

            // Check if refresh is needed from files collected by watcher
            {
                let mut lock = self.needs_refresh.lock();
                if !lock.is_empty() {
                    self.pending_refresh_paths.append(&mut lock);
                }
            }

            let should_refresh = !self.pending_refresh_paths.is_empty()
                && self
                    .last_refresh
                    .map(|t| t.elapsed() >= std::time::Duration::from_millis(100))
                    .unwrap_or(true);

            if should_refresh {
                let paths = std::mem::take(&mut self.pending_refresh_paths);
                self.refresh_stats(paths);
                self.last_refresh = Some(std::time::Instant::now());
                self.should_redraw = true;
            }

            // Draw only if needed
            if self.should_redraw {
                terminal.draw(|frame| self.render(frame))?;
                self.should_redraw = false;
            }
        }

        Ok(())
    }

    fn handle_key_event(
        &mut self,
        key: crossterm::event::KeyEvent,
        term_height: u16,
    ) -> io::Result<()> {
        // Global quit commands
        if (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            || (key.code == KeyCode::Char('q')
                && !self.is_active
                && !self.models_active
                && !self.modal.open)
        {
            self.exit = true;
            return Ok(());
        }

        if self.modal.open {
            if self.modal.handle_key_event(key.code, term_height) {
                self.chat_scroll = self.modal.chat_scroll;
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.is_active || self.models_active {
                    self.is_active = false;
                    self.models_active = false;
                } else {
                    self.exit = true;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus == Focus::Right && self.left_panel == LeftPanel::Models {
                    match self.right_panel {
                        RightPanel::List => self.right_panel = RightPanel::Tools,
                        _ => self.focus = Focus::Left,
                    }
                } else if self.focus == Focus::Right
                    && self.left_panel == LeftPanel::Days
                    && self.is_active
                {
                    self.focus = Focus::Left;
                    self.right_panel = RightPanel::List;
                } else {
                    self.focus = Focus::Left;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.focus == Focus::Right && self.left_panel == LeftPanel::Models {
                    if self.right_panel == RightPanel::Tools {
                        self.right_panel = RightPanel::List;
                    }
                } else if self.focus == Focus::Left
                    && self.left_panel == LeftPanel::Days
                    && self.is_active
                {
                    self.focus = Focus::Right;
                    self.right_panel = RightPanel::List;
                } else {
                    self.focus = Focus::Right;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.is_active || self.models_active {
                    // Active mode: Scroll within the focused panel
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {} // Not interactive
                            LeftPanel::Days => {
                                self.day_previous();
                                self.update_session_list(); // Auto-update preview
                            }
                            LeftPanel::Models => {
                                self.model_previous();
                                self.selected_model_index = self.model_list_state.selected();
                                // Auto-update preview
                            }
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => {
                                self.tool_scroll = self.tool_scroll.saturating_sub(1);
                            }
                            LeftPanel::Days => match self.right_panel {
                                RightPanel::Detail => {
                                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                                }
                                RightPanel::List => {
                                    if self.session_list_state.selected() == Some(0) {
                                        self.right_panel = RightPanel::Detail;
                                    } else {
                                        self.session_previous();
                                    }
                                }
                                _ => {}
                            },
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::Detail => {}
                                RightPanel::Tools => {
                                    self.model_tool_scroll =
                                        self.model_tool_scroll.saturating_sub(1);
                                }
                                RightPanel::List => {
                                    self.model_previous();
                                    self.selected_model_index = self.model_list_state.selected();
                                }
                            },
                        },
                    }
                } else {
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => self.left_panel = LeftPanel::Stats,
                            LeftPanel::Models => self.left_panel = LeftPanel::Days,
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::Detail => {}
                                RightPanel::Tools | RightPanel::List => {
                                    self.right_panel = RightPanel::Detail
                                }
                            },
                            LeftPanel::Stats | LeftPanel::Days => match self.right_panel {
                                RightPanel::Detail => {}
                                _ => self.right_panel = RightPanel::Detail,
                            },
                        },
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.is_active || self.models_active {
                    // Active mode: Scroll within the focused panel
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => {
                                self.day_next();
                                self.update_session_list(); // Auto-update preview
                            }
                            LeftPanel::Models => {
                                self.model_next();
                                self.selected_model_index = self.model_list_state.selected();
                                // Auto-update preview
                            }
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => {
                                if self.tool_scroll < self.tool_max_scroll {
                                    self.tool_scroll += 1;
                                }
                            }
                            LeftPanel::Days => match self.right_panel {
                                RightPanel::Detail => {
                                    if self.detail_scroll < self.detail_max_scroll {
                                        self.detail_scroll += 1;
                                    } else {
                                        self.right_panel = RightPanel::List;
                                    }
                                }
                                RightPanel::List => self.session_next(),
                                _ => {}
                            },
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::Detail => self.right_panel = RightPanel::Tools,
                                RightPanel::Tools => {
                                    if self.model_tool_scroll < self.model_tool_max_scroll {
                                        self.model_tool_scroll += 1;
                                    } else {
                                        self.right_panel = RightPanel::List;
                                    }
                                }
                                RightPanel::List => {
                                    self.model_next();
                                    self.selected_model_index = self.model_list_state.selected();
                                }
                            },
                        },
                    }
                } else {
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => self.left_panel = LeftPanel::Days,
                            LeftPanel::Days => self.left_panel = LeftPanel::Models,
                            LeftPanel::Models => {}
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Models => {
                                if self.right_panel == RightPanel::Detail {
                                    self.right_panel = RightPanel::Tools;
                                }
                            }
                            LeftPanel::Stats | LeftPanel::Days => match self.right_panel {
                                RightPanel::Detail => self.right_panel = RightPanel::List,
                                RightPanel::List => {}
                                _ => {}
                            },
                        },
                    }
                }
            }
            KeyCode::PageUp => {
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Days => {
                            for _ in 0..10 {
                                self.session_previous();
                            }
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::List {
                                for _ in 0..10 {
                                    self.model_previous();
                                }
                                self.selected_model_index = self.model_list_state.selected();
                            }
                        }
                        _ => {}
                    }
                } else {
                    match self.left_panel {
                        LeftPanel::Days => {
                            for _ in 0..10 {
                                self.day_previous();
                            }
                            self.update_session_list();
                        }
                        LeftPanel::Models => {
                            for _ in 0..10 {
                                self.model_previous();
                            }
                            self.selected_model_index = self.model_list_state.selected();
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::PageDown => {
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Days => {
                            for _ in 0..10 {
                                self.session_next();
                            }
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::List {
                                for _ in 0..10 {
                                    self.model_next();
                                }
                                self.selected_model_index = self.model_list_state.selected();
                            }
                        }
                        _ => {}
                    }
                } else {
                    match self.left_panel {
                        LeftPanel::Days => {
                            for _ in 0..10 {
                                self.day_next();
                            }
                            self.update_session_list();
                        }
                        LeftPanel::Models => {
                            for _ in 0..10 {
                                self.model_next();
                            }
                            self.selected_model_index = self.model_list_state.selected();
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Home => {
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Days => {
                            self.session_list_state.select(Some(0));
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::List {
                                self.model_list_state.select(Some(0));
                                self.selected_model_index = Some(0);
                            }
                        }
                        _ => {}
                    }
                } else {
                    match self.left_panel {
                        LeftPanel::Days => {
                            self.day_list_state.select(Some(0));
                            self.update_session_list();
                        }
                        LeftPanel::Models => {
                            self.model_list_state.select(Some(0));
                            self.selected_model_index = Some(0);
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::End => {
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Days => {
                            if !self.session_list.is_empty() {
                                self.session_list_state
                                    .select(Some(self.session_list.len() - 1));
                            }
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::List {
                                if !self.model_usage.is_empty() {
                                    let last = self.model_usage.len() - 1;
                                    self.model_list_state.select(Some(last));
                                    self.selected_model_index = Some(last);
                                }
                            }
                        }
                        _ => {}
                    }
                } else {
                    match self.left_panel {
                        LeftPanel::Days => {
                            if !self.day_list.is_empty() {
                                self.day_list_state.select(Some(self.day_list.len() - 1));
                                self.update_session_list();
                            }
                        }
                        LeftPanel::Models => {
                            if !self.model_usage.is_empty() {
                                let last = self.model_usage.len() - 1;
                                self.model_list_state.select(Some(last));
                                self.selected_model_index = Some(last);
                            }
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Enter => {
                if !self.is_active && !self.models_active {
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => {
                                self.is_active = true;
                                self.models_active = false;
                            }
                            LeftPanel::Models => {
                                self.models_active = true;
                                self.is_active = false;
                            }
                        },
                        Focus::Right => {
                            if self.left_panel == LeftPanel::Days
                                && self.right_panel == RightPanel::List
                            {
                                self.is_active = true;
                            }
                        }
                    }
                } else if self.focus == Focus::Right
                    && self.left_panel == LeftPanel::Days
                    && self.right_panel == RightPanel::List
                {
                    self.open_session_modal(term_height);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent, area: Rect) -> bool {
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let (x, y) = (mouse.column, mouse.row);

                // Use optimized hit-testing with cached panel
                let panel = self.cached_rects.find_panel(x, y);
                self.last_mouse_panel = panel;

                match panel {
                    Some("days") => {
                        // Only scroll if already focused/active as requested
                        if self.left_panel == LeftPanel::Days && self.is_active {
                            if mouse.kind == MouseEventKind::ScrollUp {
                                self.day_previous();
                            } else {
                                self.day_next();
                            }
                            self.update_session_list();
                        }
                        true
                    }
                    Some("models") => {
                        // Only scroll if already focused/active as requested
                        if self.left_panel == LeftPanel::Models && self.models_active {
                            if mouse.kind == MouseEventKind::ScrollUp {
                                self.model_previous();
                            } else {
                                self.model_next();
                            }
                            self.selected_model_index = self.model_list_state.selected();
                        }
                        true
                    }
                    Some("stats") => {
                        // GENERAL USAGE is not scrollable, do nothing
                        true
                    }
                    Some("detail") => {
                        // Only scroll if the detail panel is currently focused
                        if self.focus == Focus::Right && self.right_panel == RightPanel::Detail {
                            if self.left_panel == LeftPanel::Stats {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.tool_scroll = self.tool_scroll.saturating_sub(1);
                                } else if self.tool_scroll < self.tool_max_scroll {
                                    self.tool_scroll += 1;
                                }
                            } else if self.left_panel == LeftPanel::Days {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                                } else if self.detail_scroll < self.detail_max_scroll {
                                    self.detail_scroll += 1;
                                }
                            }
                        }
                        true
                    }
                    Some("tools") => {
                        // Only scroll if Tools are currently highlighted
                        if self.focus == Focus::Right && self.right_panel == RightPanel::Tools {
                            if mouse.kind == MouseEventKind::ScrollUp {
                                self.model_tool_scroll = self.model_tool_scroll.saturating_sub(1);
                            } else if self.model_tool_scroll < self.model_tool_max_scroll {
                                self.model_tool_scroll += 1;
                            }
                        }
                        true
                    }
                    Some("list") => {
                        if self.left_panel == LeftPanel::Models {
                            // MODEL RANKING: Scroll only if Models are active
                            if self.left_panel == LeftPanel::Models && self.models_active {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.model_previous();
                                } else {
                                    self.model_next();
                                }
                                self.selected_model_index = self.model_list_state.selected();
                            }
                        } else {
                            // SESSIONS: Scroll only if Session List is active
                            if self.focus == Focus::Right
                                && self.right_panel == RightPanel::List
                                && self.is_active
                            {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.session_previous();
                                } else {
                                    self.session_next();
                                }
                            }
                        }
                        true
                    }
                    _ => false,
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = (mouse.column, mouse.row);
                self.handle_mouse_single_click_optimized(pos, area.height)
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.is_active || self.models_active {
                    self.is_active = false;
                    self.models_active = false;
                } else {
                    self.exit = true;
                }
                true
            }
            _ => false,
        }
    }

    /// Optimized single-click handler using efficient hit-testing
    #[inline(always)]
    fn handle_mouse_single_click_optimized(&mut self, pos: (u16, u16), term_height: u16) -> bool {
        let (x, y) = pos;

        // Use optimized panel finder
        if let Some(panel) = self.cached_rects.find_panel(x, y) {
            self.last_mouse_panel = Some(panel);
            match panel {
                "stats" => {
                    self.focus = Focus::Left;
                    self.left_panel = LeftPanel::Stats;
                    self.is_active = false;
                    self.models_active = false;
                }
                "days" => {
                    self.focus = Focus::Left;
                    self.left_panel = LeftPanel::Days;
                    self.is_active = true;
                    self.models_active = false;

                    if let Some(rect) = self.cached_rects.days {
                        let inner_top = rect.y.saturating_add(1);
                        let inner_bottom = rect.y + rect.height.saturating_sub(1);
                        if y >= inner_top && y < inner_bottom {
                            let clicked_row = (y - inner_top) as usize;
                            let offset = self.day_list_state.offset();
                            let idx = offset + clicked_row;
                            if idx < self.day_list.len() {
                                self.day_list_state.select(Some(idx));
                                self.update_session_list();
                            }
                        }
                    }
                }
                "models" => {
                    self.focus = Focus::Left;
                    self.left_panel = LeftPanel::Models;
                    self.models_active = true;
                    self.is_active = false;

                    if let Some(rect) = self.cached_rects.models {
                        let inner_top = rect.y.saturating_add(1);
                        let inner_bottom = rect.y + rect.height.saturating_sub(1);
                        if y >= inner_top && y < inner_bottom {
                            let clicked_row = (y - inner_top) as usize;
                            let offset = self.model_list_state.offset();
                            let idx = offset + clicked_row;
                            if idx < self.model_usage.len() {
                                self.model_list_state.select(Some(idx));
                                self.selected_model_index = Some(idx);
                            }
                        }
                    }
                }
                "detail" => {
                    self.focus = Focus::Right;
                    self.right_panel = RightPanel::Detail;
                }
                "tools" => {
                    self.focus = Focus::Right;
                    self.right_panel = RightPanel::Tools;
                }
                "list" => {
                    self.focus = Focus::Right;
                    self.right_panel = RightPanel::List;

                    if self.left_panel == LeftPanel::Days {
                        self.is_active = true;
                        self.models_active = false;

                        if let Some(rect) = self.cached_rects.list {
                            let inner_top = rect.y.saturating_add(1);
                            let inner_bottom = rect.y + rect.height.saturating_sub(1);
                            if y >= inner_top && y < inner_bottom {
                                let clicked_row = (y - inner_top) as usize;
                                let offset = self.session_list_state.offset();
                                let idx = offset + clicked_row;
                                if idx < self.session_list.len() {
                                    self.session_list_state.select(Some(idx));
                                    self.current_chat_session_id = None;

                                    let now = std::time::Instant::now();
                                    let is_double =
                                        self.last_session_click.is_some_and(|(t, last_idx)| {
                                            last_idx == idx
                                                && now.duration_since(t)
                                                    <= std::time::Duration::from_millis(400)
                                        });
                                    self.last_session_click = Some((now, idx));

                                    if is_double {
                                        self.open_session_modal(term_height);
                                    }
                                }
                            }
                        }
                    } else if self.left_panel == LeftPanel::Models {
                        self.models_active = true;
                        self.is_active = false;
                    }
                }
                _ => return false,
            }
            true
        } else {
            false
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        // Render either the main dashboard OR the modal view - not both
        if self.modal.open {
            // Render ONLY the modal view - completely new clean screen
            // Clear cached rects when modal is open
            self.cached_rects = PanelRects::default();

            let session_id = self
                .session_list_state
                .selected()
                .and_then(|i| self.session_list.get(i).map(|s| s.id.clone()));
            if let Some(id) = session_id {
                if let Some(session) = self.session_list.iter().find(|s| s.id == id) {
                    self.modal
                        .render(frame, frame.area(), session, &self.session_titles);
                }
            }
        } else {
            // Render the main dashboard and cache panel rectangles
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(1)])
                .split(frame.area());

            let horizontal_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
                .split(main_chunks[0]);

            self.render_left_panel(frame, horizontal_chunks[0]);
            self.render_right_panel(frame, horizontal_chunks[1]);
            self.render_status_bar(frame, main_chunks[1]);
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let k = Style::default()
            .fg(Color::Rgb(140, 140, 160))
            .add_modifier(Modifier::BOLD);
        let t = Style::default().fg(Color::DarkGray);
        let sep = Span::styled(" │ ", Style::default().fg(Color::Rgb(50, 50, 70)));

        let mut spans: Vec<Span> = Vec::with_capacity(16);

        if self.modal.open {
            spans.extend_from_slice(&[
                Span::styled("←→/Click", k),
                Span::styled(" column", t),
                sep.clone(),
                Span::styled("↑↓/Scroll", k),
                Span::styled(" scroll", t),
                sep.clone(),
                Span::styled("PgUp/Dn", k),
                Span::styled(" page", t),
                sep.clone(),
                Span::styled("Esc/q/Right-click", k),
                Span::styled(" close", t),
            ]);
        } else if self.is_active || self.models_active {
            spans.extend_from_slice(&[
                Span::styled("↑↓/Scroll", k),
                Span::styled(" scroll", t),
                sep.clone(),
                Span::styled("←→/Click", k),
                Span::styled(" focus", t),
            ]);
            if self.is_active && self.left_panel == LeftPanel::Days {
                spans.extend_from_slice(&[
                    sep.clone(),
                    Span::styled("Enter/Double-click", k),
                    Span::styled(" open", t),
                ]);
            }
            spans.extend_from_slice(&[
                sep.clone(),
                Span::styled("Esc/q/Right-click", k),
                Span::styled(" back", t),
            ]);
        } else {
            spans.extend_from_slice(&[
                Span::styled("↑↓", k),
                Span::styled(" navigate", t),
                sep.clone(),
                Span::styled("←→/Click", k),
                Span::styled(" focus", t),
                sep.clone(),
                Span::styled("Enter/Scroll", k),
                Span::styled(" activate", t),
                sep.clone(),
                Span::styled("Esc/q/Right-click", k),
                Span::styled(" quit", t),
            ]);
        }

        let status_bar = Paragraph::new(Line::from(spans))
            .style(Style::default().bg(Color::Rgb(15, 15, 25)))
            .alignment(Alignment::Center);
        frame.render_widget(status_bar, area);
    }

    fn render_left_panel(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus == Focus::Left;
        let border_style = if is_focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let stats_height = 6;
        let model_height = 6.min(self.model_usage.len() as u16 + 2);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(stats_height),
                Constraint::Min(9),
                Constraint::Length(model_height),
            ])
            .split(area);

        // Cache panel rectangles for mouse hit-testing
        self.cached_rects.stats = Some(chunks[0]);
        self.cached_rects.days = Some(chunks[1]);
        self.cached_rects.models = Some(chunks[2]);

        self.render_stats_panel(
            frame,
            chunks[0],
            border_style,
            self.focus == Focus::Left && self.left_panel == LeftPanel::Stats,
            self.is_active,
        );
        self.render_day_list(
            frame,
            chunks[1],
            border_style,
            self.focus == Focus::Left && self.left_panel == LeftPanel::Days,
            self.is_active, // Linked with Sessions
        );
        self.render_model_list(
            frame,
            chunks[2],
            border_style,
            self.focus == Focus::Left && self.left_panel == LeftPanel::Models,
            self.models_active, // Independent - only when Enter on Models
        );
    }

    fn render_stats_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_highlighted {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    " GENERAL USAGE ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(24),
                Constraint::Length(1),
                Constraint::Percentage(18),
                Constraint::Percentage(18),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        let sep_style = Style::default().fg(Color::Rgb(180, 180, 180));
        for &i in &[1, 4] {
            let sep_area = cols[i];
            let sep = Paragraph::new(vec![
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
            ]);
            frame.render_widget(sep, sep_area);
        }

        let total_responses = self.totals.messages.saturating_sub(self.totals.prompts);

        // Col 1: Sessions / Cost
        let c1 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[0]);
        frame.render_widget(
            stat_widget("Sessions", format!("{}", self.totals.sessions.len()), Color::Cyan),
            c1[0],
        );
        frame.render_widget(
            stat_widget("Cost", format!("${:.2}", self.totals.display_cost()), Color::Yellow),
            c1[1],
        );

        // Col 2: Input / Output
        let c2 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[2]);
        frame.render_widget(
            stat_widget("Input", format_number(self.totals.tokens.input), Color::Blue),
            c2[0],
        );
        frame.render_widget(
            stat_widget("Output", format_number(self.totals.tokens.output), Color::Magenta),
            c2[1],
        );

        // Col 3: Thinking / Cache
        let c3 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[3]);
        frame.render_widget(
            stat_widget("Thinking", format_number(self.totals.tokens.reasoning), Color::Rgb(255, 165, 0)),
            c3[0],
        );
        frame.render_widget(
            stat_widget("Cache", format_number(self.totals.tokens.cache_read + self.totals.tokens.cache_write), Color::Yellow),
            c3[1],
        );

        // Col 4: Lines / User · Agent
        let c4 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[5]);

        let lines_widget = Paragraph::new(vec![
            Line::from(Span::styled(
                "Line Changes",
                Style::default().fg(Color::Rgb(180, 180, 180)),
            )),
            Line::from(vec![
                Span::styled(
                    format!("+{}", format_number(self.totals.diffs.additions)),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" / ", Style::default().fg(Color::Rgb(100, 100, 120))),
                Span::styled(
                    format!("-{}", format_number(self.totals.diffs.deletions)),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(lines_widget, c4[0]);

        let msg_widget = Paragraph::new(vec![
            Line::from(Span::styled(
                "User / Agent Messages",
                Style::default().fg(Color::Rgb(180, 180, 180)),
            )),
            Line::from(vec![
                Span::styled(
                    format!("{}", self.totals.prompts),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" / ", Style::default().fg(Color::Rgb(100, 100, 120))),
                Span::styled(
                    format!("{}", total_responses),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(msg_widget, c4[1]);
    }

    fn render_day_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        // Pre-allocate Vec with capacity to avoid reallocations
        let mut items = Vec::with_capacity(self.day_list.len());
        let cost_width = self.max_cost_width();
        let sess_width = 4usize;
        let fixed_width = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + (sess_width + 5);
        let inner_width = area.width.saturating_sub(2);
        let available =
            inner_width.saturating_sub((fixed_width + 2).min(u16::MAX as usize) as u16) as usize;
        let name_width = available.max(8);

        for day in &self.day_list {
            // Optimized: single HashMap lookup instead of multiple map calls
            let (sess, input, output, cost) = if let Some(stat) = self.per_day.get(day) {
                (
                    stat.sessions.len(),
                    stat.tokens.input,
                    stat.tokens.output,
                    stat.display_cost(),
                )
            } else {
                (0, 0, 0, 0.0)
            };

            let _in_val = format_number(input);
            let _out_val = format_number(output);

            // Use cached day string (Phase 2 optimization - avoids date parsing on every render)
            let day_with_name = self
                .cached_day_strings
                .get(day)
                .cloned()
                .unwrap_or_else(|| day.clone());

            items.push(ListItem::new(usage_list_row(
                day_with_name,
                input,
                output,
                cost,
                sess,
                &UsageRowFormat {
                    name_width,
                    cost_width,
                    sess_width,
                },
            )));
        }

        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if is_highlighted {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    })
                    .title(
                        Line::from(Span::styled(
                            " DAILY USAGE ",
                            Style::default()
                                .fg(title_color)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(Alignment::Center),
                    )
                    .title_bottom(
                        Line::from(Span::styled(
                            if is_active {
                                " ↑↓: scroll │ Esc: back "
                            } else {
                                " "
                            },
                            Style::default().fg(Color::DarkGray),
                        ))
                        .alignment(Alignment::Center),
                    ),
            )
            .highlight_style(if is_active {
                Style::default()
                    .bg(Color::Rgb(60, 60, 90))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
            .highlight_symbol(if is_active { "▶ " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.day_list_state);
    }

    fn render_model_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        // Pre-allocate Vec with capacity to avoid reallocations
        let mut items = Vec::with_capacity(self.model_usage.len());
        let cost_width = self.max_cost_width();
        let sess_width = 4usize;
        let fixed_width = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + (sess_width + 5);
        let inner_width = area.width.saturating_sub(2);
        let available =
            inner_width.saturating_sub((fixed_width + 2).min(u16::MAX as usize) as u16) as usize;
        let name_width = available.max(8);

        for m in &self.model_usage {
            // Show full model name with provider (e.g., "prox/glm-4.7")
            let full_name = m.name.to_string();
            items.push(ListItem::new(usage_list_row(
                full_name,
                m.tokens.input,
                m.tokens.output,
                m.cost,
                m.sessions.len(),
                &UsageRowFormat {
                    name_width,
                    cost_width,
                    sess_width,
                },
            )));
        }

        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if is_highlighted {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    })
                    .title(
                        Line::from(Span::styled(
                            " MODEL USAGE ",
                            Style::default()
                                .fg(title_color)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(Alignment::Center),
                    )
                    .title_bottom(
                        Line::from(Span::styled(
                            if is_active {
                                " ↑↓: scroll │ Esc: back "
                            } else {
                                " "
                            },
                            Style::default().fg(Color::DarkGray),
                        ))
                        .alignment(Alignment::Center),
                    ),
            )
            .highlight_style(if is_active {
                Style::default()
                    .bg(Color::Rgb(60, 60, 90))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
            .highlight_symbol(if is_active { "▶ " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.model_list_state);
    }

    fn render_right_panel(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus == Focus::Right;
        let border_style = if is_focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        match self.left_panel {
            LeftPanel::Stats => {
                // Tool usage takes entire right panel
                self.cached_rects.detail = Some(area);
                self.cached_rects.list = None;
                self.cached_rects.tools = None;
                self.render_tool_usage_panel(frame, area, border_style, is_focused, self.is_active)
            }
            LeftPanel::Days => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(10), Constraint::Min(0)])
                    .split(area);

                // Cache right panel rects for Days view
                self.cached_rects.detail = Some(chunks[0]);
                self.cached_rects.list = Some(chunks[1]);
                self.cached_rects.tools = None;

                let detail_highlighted = is_focused && self.right_panel == RightPanel::Detail;
                self.render_session_detail(
                    frame,
                    chunks[0],
                    if detail_highlighted {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                    detail_highlighted,
                );

                let list_highlighted = is_focused && self.right_panel == RightPanel::List;
                // Sessions panel should be highlighted when active or when left panel isDays
                let list_active = list_highlighted && self.is_active;
                self.render_session_list(
                    frame,
                    chunks[1],
                    if list_highlighted {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                    list_highlighted,
                    list_active,
                );
            }
            LeftPanel::Models => {
                // Cache right panel rects for Models view (done in render_model_detail)
                self.render_model_detail(frame, area, border_style, is_focused, self.models_active)
            }
        }
    }

    fn render_tool_usage_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_highlighted {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    " TOOL USAGE ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if is_highlighted {
                        " ↑↓: scroll "
                    } else {
                        " "
                    },
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.tool_usage.is_empty() {
            let empty = Paragraph::new("No tool usage data")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty, inner);
            return;
        }

        let total_count: u64 = self.tool_usage.iter().map(|t| t.count).sum();
        let bar_max_width = inner.width.saturating_sub(30) as u64;

        let visible_height = inner.height as usize;
        self.tool_max_scroll = (self.tool_usage.len().saturating_sub(visible_height)) as u16;
        self.tool_scroll = self.tool_scroll.min(self.tool_max_scroll);

        let mut lines: Vec<Line> = Vec::with_capacity(self.tool_usage.len());
        for tool in &self.tool_usage {
            let percentage = if total_count > 0 {
                (tool.count as f64 / total_count as f64) * 100.0
            } else {
                0.0
            };
            let bar_width = if total_count > 0 {
                ((tool.count as f64 / total_count as f64) * bar_max_width as f64) as usize
            } else {
                0
            };
            let filled = "█".repeat(bar_width);
            let empty = "░".repeat(bar_max_width.saturating_sub(bar_width as u64) as usize);

            // Truncate tool name to max 12 chars with ellipsis if needed
            let tool_name_display = truncate_with_ellipsis(&tool.name, 12);

            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:>12} ", tool_name_display),
                    Style::default().fg(Color::White),
                ),
                Span::styled(filled, Style::default().fg(Color::Cyan)),
                Span::styled(empty, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!(" {:>5} ", tool.count),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("({:>5.1}%)", percentage),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        frame.render_widget(Paragraph::new(lines).scroll((self.tool_scroll, 0)), inner);
    }

    fn render_model_detail(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let selected_model = self
            .selected_model_index
            .and_then(|i| self.model_usage.get(i));

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7), // Info (5 lines content + borders)
                Constraint::Min(0),    // Bottom section
            ])
            .split(area);

        let bottom_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50), // Tools
                Constraint::Percentage(50), // Ranking
            ])
            .split(main_chunks[1]);

        // Cache right panel rects for Models view
        self.cached_rects.detail = Some(main_chunks[0]);
        self.cached_rects.tools = Some(bottom_chunks[0]);
        self.cached_rects.list = Some(bottom_chunks[1]);

        // --- 1. MODEL INFO ---
        let info_focused = is_highlighted && self.right_panel == RightPanel::Detail;
        let info_title = selected_model
            .map(|m| format!(" {} ", m.name))
            .unwrap_or_else(|| " MODEL INFO ".to_string());
        let info_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if info_focused {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    info_title,
                    Style::default()
                        .fg(if info_focused {
                            Color::Cyan
                        } else {
                            Color::Yellow
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let info_inner = info_block.inner(main_chunks[0]);
        frame.render_widget(info_block, main_chunks[0]);

        if let Some(model) = selected_model {
            let mut agent_pairs: Vec<(&Box<str>, &u64)> = model.agents.iter().collect();
            agent_pairs.sort_unstable_by(|a, b| b.1.cmp(a.1));

            // Responsive layout for model info
            let inner_width = info_inner.width;
            let (show_agents, show_tokens) = if inner_width < 45 {
                (false, false)
            } else if inner_width < 80 {
                (false, true)
            } else {
                (true, true)
            };

            let constraints = if show_agents && show_tokens {
                vec![
                    Constraint::Percentage(25),
                    Constraint::Percentage(37),
                    Constraint::Percentage(38),
                ]
            } else if show_tokens {
                vec![Constraint::Percentage(45), Constraint::Percentage(55)]
            } else {
                vec![Constraint::Percentage(100)]
            };

            let info_columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(constraints)
                .split(info_inner);

            let label_color = Style::default().fg(Color::Rgb(180, 180, 180));
            let col_width = info_columns.get(1).map(|c| c.width).unwrap_or(0) as usize;
            let name_fit_ellipsis = |label_len: usize, text: &str, max_width: usize| -> String {
                let avail = max_width.saturating_sub(label_len + 1);
                truncate_with_ellipsis(text, avail.max(1))
            };

            let non_cache = (model.tokens.input + model.tokens.output + model.tokens.reasoning).max(1) as f64;
            let est_cost = model.cost + (model.tokens.cache_read as f64 * model.cost / non_cache);
            let savings = est_cost - model.cost;

            let left_lines = vec![
                Line::from(vec![
                    Span::styled("Sessions  ", label_color),
                    Span::styled(
                        format!("{}", model.sessions.len()),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Messages  ", label_color),
                    Span::styled(
                        format!("{}", model.messages),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cost      ", label_color),
                    Span::styled(
                        format!("${:.2}", model.cost),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Savings   ", label_color),
                    Span::styled(
                        format!("${:.2}", savings),
                        Style::default()
                            .fg(if savings > 0.0 { Color::Green } else { Color::DarkGray })
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            frame.render_widget(Paragraph::new(left_lines), info_columns[0]);

            if show_agents {
                let mut agent_lines: Vec<Line> = Vec::with_capacity(5);
                let label = "Agents: ";
                let indent = "        ";
                if agent_pairs.is_empty() {
                    agent_lines.push(Line::from(vec![
                        Span::styled(label, label_color),
                        Span::styled("n/a", Style::default().fg(Color::DarkGray)),
                    ]));
                } else {
                    let mut iter = agent_pairs.iter();
                    if let Some((a, c)) = iter.next() {
                        let first = format!("{} ({} msg)", a.as_ref(), c);
                        agent_lines.push(Line::from(vec![
                            Span::styled(label, label_color),
                            Span::styled(
                                name_fit_ellipsis(label.len(), &first, col_width),
                                Style::default().fg(Color::Magenta),
                            ),
                        ]));
                    }
                    for (a, c) in iter {
                        if agent_lines.len() >= 5 {
                            agent_lines.pop();
                            agent_lines.push(Line::from(vec![
                                Span::styled(indent, label_color),
                                Span::styled("...", Style::default().fg(Color::Magenta)),
                            ]));
                            break;
                        }
                        let line = format!("{} ({} msg)", a.as_ref(), c);
                        agent_lines.push(Line::from(vec![
                            Span::styled(indent, label_color),
                            Span::styled(
                                name_fit_ellipsis(indent.len(), &line, col_width),
                                Style::default().fg(Color::Magenta),
                            ),
                        ]));
                    }
                }
                frame.render_widget(Paragraph::new(agent_lines), info_columns[1]);
            }

            if show_tokens {
                let right_lines = vec![
                    Line::from(vec![
                        Span::styled(
                            "Input         ",
                            Style::default().fg(Color::Rgb(180, 180, 180)),
                        ),
                        Span::styled(
                            format_number_full(model.tokens.input),
                            Style::default().fg(Color::Blue),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "Output        ",
                            Style::default().fg(Color::Rgb(180, 180, 180)),
                        ),
                        Span::styled(
                            format_number_full(model.tokens.output),
                            Style::default().fg(Color::Magenta),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "Thinking      ",
                            Style::default().fg(Color::Rgb(180, 180, 180)),
                        ),
                        Span::styled(
                            format_number_full(model.tokens.reasoning),
                            Style::default().fg(Color::Rgb(255, 165, 0)),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "Cache Read    ",
                            Style::default().fg(Color::Rgb(180, 180, 180)),
                        ),
                        Span::styled(
                            format_number_full(model.tokens.cache_read),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "Cache Write   ",
                            Style::default().fg(Color::Rgb(180, 180, 180)),
                        ),
                        Span::styled(
                            format_number_full(model.tokens.cache_write),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]),
                ];
                let token_col_idx = if show_agents { 2 } else { 1 };
                frame.render_widget(Paragraph::new(right_lines), info_columns[token_col_idx]);
            }
        }

        // --- 2. TOOLS USED ---
        let tools_focused = is_highlighted && self.right_panel == RightPanel::Tools;
        let tools_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if tools_focused {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    " TOOLS USED ",
                    Style::default()
                        .fg(if tools_focused {
                            Color::Cyan
                        } else {
                            Color::Yellow
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let tools_inner = tools_block.inner(bottom_chunks[0]);
        frame.render_widget(tools_block, bottom_chunks[0]);

        if let Some(model) = selected_model {
            if !model.tools.is_empty() {
                // Optimized: pre-allocate with known capacity for sorting
                let mut tools: Vec<_> = model.tools.iter().collect();
                tools.sort_unstable_by(|a, b| b.1.cmp(a.1));
                let total: u64 = tools.iter().map(|(_, c)| **c).sum();
                let bar_max = tools_inner.width.saturating_sub(16) as u64;

                self.model_tool_max_scroll =
                    (tools.len().saturating_sub(tools_inner.height as usize)) as u16;
                self.model_tool_scroll = self.model_tool_scroll.min(self.model_tool_max_scroll);

                // Optimized: pre-allocate with known capacity for lines
                let lines: Vec<Line> = tools
                    .into_iter()
                    .map(|(name, count)| {
                        let width = ((*count as f64 / total as f64) * bar_max as f64) as usize;
                        let filled = "█".repeat(width);
                        let empty = "░".repeat(bar_max as usize - width);
                        Line::from(vec![
                            Span::styled(
                                format!("{:<12}", safe_truncate_plain(name, 12)),
                                Style::default().fg(Color::White),
                            ),
                            Span::styled(filled, Style::default().fg(Color::Magenta)),
                            Span::styled(empty, Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("{:>3}", count),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    })
                    .collect();
                frame.render_widget(
                    Paragraph::new(lines).scroll((self.model_tool_scroll, 0)),
                    tools_inner,
                );
            } else {
                let empty = Paragraph::new("No tools used")
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(Alignment::Center);
                frame.render_widget(empty, tools_inner);
            }
        }

        // --- 3. MODEL RANKING ---
        let ranking_focused = is_highlighted && self.right_panel == RightPanel::List;
        let ranking_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if ranking_focused {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    " MODEL RANKING ",
                    Style::default()
                        .fg(if ranking_focused {
                            Color::Cyan
                        } else {
                            Color::Yellow
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let ranking_inner = ranking_block.inner(bottom_chunks[1]);
        frame.render_widget(ranking_block, bottom_chunks[1]);

        let mut ranked_models: Vec<_> = self.model_usage.iter().enumerate().collect();
        ranked_models.sort_unstable_by(|a, b| b.1.tokens.total().cmp(&a.1.tokens.total()));
        self.ranking_max_scroll = ranked_models
            .len()
            .saturating_sub(ranking_inner.height as usize);
        self.ranking_scroll = self.ranking_scroll.min(self.ranking_max_scroll);

        let grand_total: u64 = self.model_usage.iter().map(|m| m.tokens.total()).sum();

        let bar_available_width = ranking_inner.width.saturating_sub(2);
        let max_token_len = self
            .model_usage
            .iter()
            .map(|m| format_number(m.tokens.total()).len())
            .max()
            .unwrap_or(1);
        let ranking_lines: Vec<Line> = ranked_models
            .iter()
            .map(|(idx, model)| {
                let is_selected = self.selected_model_index == Some(*idx);
                let percentage = if grand_total > 0 {
                    (model.tokens.total() as f64 / grand_total as f64) * 100.0
                } else {
                    0.0
                };
                let percent_text = format!("{:>5.1}%", percentage);
                let token_text = format_number(model.tokens.total());
                let suffix = format!(
                    " {} ({:>width$})",
                    percent_text,
                    token_text,
                    width = max_token_len
                );
                let suffix_len = suffix.chars().count() as u16;
                let bar_max_width = bar_available_width.saturating_sub(suffix_len) as usize;
                let bar_width = if grand_total > 0 {
                    ((model.tokens.total() as f64 / grand_total as f64) * bar_max_width as f64)
                        as usize
                } else {
                    0
                };
                let filled = "█".repeat(bar_width.min(bar_max_width));
                let empty = "░".repeat(bar_max_width.saturating_sub(bar_width));

                Line::from(vec![
                    Span::styled(
                        filled,
                        Style::default().fg(if is_selected {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::styled(empty, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        suffix,
                        Style::default()
                            .fg(if is_selected {
                                Color::Yellow
                            } else {
                                Color::DarkGray
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        // Auto-scroll to keep selected model visible
        if let Some(selected_idx) = self.selected_model_index {
            if let Some(selected_rank) = ranked_models
                .iter()
                .position(|(idx, _)| *idx == selected_idx)
            {
                let visible_height = ranking_inner.height as usize;
                if visible_height > 0 {
                    if selected_rank >= self.ranking_scroll + visible_height
                        || selected_rank < self.ranking_scroll
                    {
                        self.ranking_scroll = selected_rank.saturating_sub(visible_height / 2);
                    }
                    self.ranking_scroll = self
                        .ranking_scroll
                        .min(ranking_lines.len().saturating_sub(visible_height));
                } else {
                    self.ranking_scroll = 0;
                }
            }
        }

        let visible_lines: Vec<Line> = ranking_lines
            .into_iter()
            .skip(self.ranking_scroll)
            .take(ranking_inner.height as usize)
            .collect();

        frame.render_widget(Paragraph::new(visible_lines), ranking_inner);
    }

    fn render_session_detail(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let session = self
            .session_list_state
            .selected()
            .and_then(|i| self.session_list.get(i).cloned());

        let panel_title = if let Some(s) = &session {
            if s.is_continuation {
                if let Some(first_date) = &s.first_created_date {
                    format!(" {} [Continue from {}] ", s.id, first_date)
                } else {
                    format!(" {} [Continued] ", s.id)
                }
            } else {
                format!(" {} ", s.id)
            }
        } else {
            " SESSION INFO ".to_string()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_highlighted {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    panel_title,
                    Style::default()
                        .fg(if is_highlighted {
                            Color::Cyan
                        } else {
                            Color::Yellow
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let info_inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(s) = session {
            let title = self
                .session_titles
                .get(&s.id)
                .map(|t| t.strip_prefix("New session - ").unwrap_or(t))
                .unwrap_or("Untitled");
            let project_str: Box<str> = if !s.path_root.is_empty() {
                s.path_root.clone()
            } else {
                s.path_cwd.clone()
            };
            let project: &str = &project_str;

            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
                .split(info_inner);

            let label_style = Style::default().fg(Color::Rgb(180, 180, 180));
            let left_val_width = cols[0].width.saturating_sub(14) as usize;

            let mut left_lines: Vec<Line> = Vec::with_capacity(8);

            left_lines.push(Line::from(vec![
                Span::styled("Title        ", label_style),
                Span::styled(
                    truncate_with_ellipsis(title, left_val_width),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            left_lines.push(Line::from(vec![
                Span::styled("Project      ", label_style),
                Span::styled(
                    truncate_with_ellipsis(project, left_val_width),
                    Style::default().fg(Color::Blue),
                ),
            ]));

            let branch = match &self.cached_git_branch {
                Some((cached_root, cached_branch)) if &**cached_root == project => {
                    cached_branch.clone()
                }
                _ => {
                    use crate::session::detect_git_branch;
                    let b = detect_git_branch(project);
                    self.cached_git_branch = Some((project_str.clone(), b.clone()));
                    b
                }
            };
            left_lines.push(Line::from(vec![
                Span::styled("Branch       ", label_style),
                Span::styled(
                    branch
                        .as_deref()
                        .map(|b| truncate_with_ellipsis(b, left_val_width))
                        .unwrap_or_else(|| "n/a".into()),
                    Style::default().fg(if branch.is_some() {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }),
                ),
            ]));

            left_lines.push(Line::from(vec![
                Span::styled("Last Active  ", label_style),
                Span::styled(
                    chrono::DateTime::from_timestamp(s.last_activity / 1000, 0)
                        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

            left_lines.push(Line::from(vec![
                Span::styled("Duration     ", label_style),
                Span::styled(
                    format_duration_ms(s.first_activity, s.last_activity)
                        .unwrap_or_else(|| "n/a".into()),
                    Style::default().fg(Color::Rgb(100, 200, 255)),
                ),
            ]));

            let mut models: Vec<_> = s.models.iter().collect();
            models.sort();
            let model_val_width = left_val_width;


            if models.is_empty() {
                left_lines.push(Line::from(vec![
                    Span::styled("Models       ", label_style),
                    Span::styled("n/a", Style::default().fg(Color::DarkGray)),
                ]));
            } else if models.len() <= 3 {
                for (i, m) in models.iter().enumerate() {
                    let prefix = if i == 0 { "Models       " } else { "             " };
                    left_lines.push(Line::from(vec![
                        Span::styled(prefix, label_style),
                        Span::styled(
                            truncate_with_ellipsis(m, model_val_width),
                            Style::default()
                                .fg(Color::Magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
            } else {
                for (i, m) in models.iter().take(2).enumerate() {
                    let prefix = if i == 0 { "Models       " } else { "             " };
                    left_lines.push(Line::from(vec![
                        Span::styled(prefix, label_style),
                        Span::styled(
                            truncate_with_ellipsis(m, model_val_width),
                            Style::default()
                                .fg(Color::Magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
                left_lines.push(Line::from(vec![
                    Span::styled("             ", label_style),
                    Span::styled(
                        format!("+{}", models.len() - 2),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }

            frame.render_widget(Paragraph::new(left_lines), cols[0]);

            let s_responses = s.messages.saturating_sub(s.prompts);
            let right_lines = vec![
                Line::from(vec![
                    Span::styled("Input         ", label_style),
                    Span::styled(
                        format_number_full(s.tokens.input),
                        Style::default().fg(Color::Blue),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Output        ", label_style),
                    Span::styled(
                        format_number_full(s.tokens.output),
                        Style::default().fg(Color::Magenta),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Thinking      ", label_style),
                    Span::styled(
                        format_number_full(s.tokens.reasoning),
                        Style::default().fg(Color::Rgb(255, 165, 0)),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cache Read    ", label_style),
                    Span::styled(
                        format_number_full(s.tokens.cache_read),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cache Write   ", label_style),
                    Span::styled(
                        format_number_full(s.tokens.cache_write),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Prompts       ", label_style),
                    Span::styled(
                        format!("{}", s.prompts),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Responses     ", label_style),
                    Span::styled(
                        format!("{}", s_responses),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cost          ", label_style),
                    Span::styled(
                        format!("${:.2}", s.display_cost()),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            frame.render_widget(Paragraph::new(right_lines), cols[1]);
        }
    }

    fn render_session_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        let inner_width = area.width.saturating_sub(2);
        if self.cached_session_width != inner_width || self.cached_session_items.is_empty() {
            self.rebuild_cached_session_items(inner_width);
        }

        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::Yellow
        };

        let items: Vec<ListItem> = self.cached_session_items.clone();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(
                        Line::from(Span::styled(
                            " SESSIONS ",
                            Style::default()
                                .fg(title_color)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .alignment(Alignment::Center),
                    )
                    .title_bottom(
                        Line::from(Span::styled(
                            if is_active {
                                " ↑↓: scroll │ Enter: Open Chat │ Esc: back "
                            } else {
                                " "
                            },
                            Style::default().fg(Color::DarkGray),
                        ))
                        .alignment(Alignment::Center),
                    ),
            )
            .highlight_style(if is_active {
                Style::default()
                    .bg(Color::Rgb(60, 60, 90))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
            .highlight_symbol(if is_active { "▶ " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.session_list_state);
    }
}

use crate::live_watcher::LiveWatcher;
use crate::session::SessionModal;
use crate::stats::{
    format_active_duration, format_number, format_number_full, load_session_chat_with_max_ts,
    ChatMessage, DayStat, MessageContent, ModelUsage, ToolUsage, Totals,
};
use crate::stats_cache::StatsCache;
use chrono::{Datelike, NaiveDate};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use fxhash::{FxHashMap, FxHashSet};
use parking_lot::Mutex;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::borrow::Cow;
use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

// Constants for optimized mouse handling

/// Cached chat session data including pre-calculated scroll info
struct CachedChat {
    messages: Arc<Vec<ChatMessage>>,
    total_lines: u16,
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
    Detail,   // OVERVIEW panel (top right in Stats view)
    Activity, // ACTIVITY heatmap panel
    List,     // SESSIONS/PROJECTS
    Tools,    // TOOLS USED
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
    detail: Option<Rect>,   // SESSION INFO or MODEL INFO
    activity: Option<Rect>, // ACTIVITY heatmap (Stats view)
    list: Option<Rect>,     // SESSIONS or MODEL RANKING
    tools: Option<Rect>,    // TOOLS USED (only in Models view)
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
        if Self::contains_point(self.activity, x, y) {
            return Some("activity");
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

#[derive(Clone, Copy)]
struct HeatmapLayout {
    inner: Rect,
    label_w: u16,
    weeks: usize,
    grid_start: NaiveDate,
    week_w: u16,
    extra_cols: u16,
}

pub struct App {
    totals: Totals,
    per_day: FxHashMap<String, DayStat>,
    session_titles: FxHashMap<Box<str>, String>,
    session_message_files: FxHashMap<String, FxHashSet<PathBuf>>,
    parent_map: FxHashMap<Box<str>, Box<str>>,
    children_map: FxHashMap<Box<str>, Vec<Box<str>>>,
    day_list: Vec<String>,
    day_list_state: ListState,
    session_list: Vec<Arc<crate::stats::SessionStat>>,
    session_list_state: ListState,
    cached_session_items: Vec<ListItem<'static>>,
    cached_session_width: u16,
    cached_day_items: Vec<ListItem<'static>>,
    cached_day_width: u16,
    cached_model_items: Vec<ListItem<'static>>,
    cached_model_width: u16,
    chat_cache: FxHashMap<String, CachedChat>,
    chat_cache_order: Vec<String>,
    chat_scroll: u16,
    model_usage: Vec<ModelUsage>,
    model_list_state: ListState,
    tool_usage: Vec<ToolUsage>,

    detail_scroll: u16,
    detail_max_scroll: u16,
    model_tool_scroll: u16,
    model_tool_max_scroll: u16,
    ranking_scroll: usize,
    ranking_max_scroll: usize,

    // Phase 2: Render Caching
    cached_day_strings: FxHashMap<String, String>, // Pre-formatted day strings with weekday names

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

    // Terminal size cache
    terminal_size: Rect,

    // Cached panel rectangles for optimized mouse hit-testing
    cached_rects: PanelRects,

    // Phase 1 optimizations
    cached_git_branch: Option<(Box<str>, Option<String>)>, // (path_root, branch) - avoid fs I/O per frame
    cached_max_cost_width: usize,

    // Overview panel data (General Usage right panel)
    overview_projects: Vec<(String, usize)>, // (project_name, session_count) sorted desc
    overview_project_scroll: usize,
    overview_project_max_scroll: usize,
    overview_tool_scroll: usize,
    overview_tool_max_scroll: usize,
    overview_heatmap_layout: Option<HeatmapLayout>,
    overview_heatmap_inspect: bool,
    overview_heatmap_selected_day: Option<String>,
    overview_heatmap_selected_tokens: u64,
    overview_heatmap_selected_sessions: usize,
    overview_heatmap_selected_cost: f64,
    overview_heatmap_selected_active_ms: i64,

    // Live stats: Cache and file watching
    stats_cache: Option<StatsCache>,
    _storage_path: PathBuf,
    live_watcher: Option<LiveWatcher>,
    needs_refresh: Arc<Mutex<Vec<PathBuf>>>,
    pending_refresh_paths: Vec<PathBuf>,
    last_refresh: Option<std::time::Instant>,
    should_redraw: bool,
    wake_rx: mpsc::Receiver<()>,
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

/// Helper: Smart truncate for host names.
/// If the full name fits, show it. If not, show just the short name (before space/parenthesis) without ellipsis.
fn truncate_host_name(full_name: &str, short_name: &str, max_chars: usize) -> String {
    if full_name.chars().count() <= max_chars {
        full_name.to_string()
    } else {
        // Not enough space - show clean short name, no ellipsis as requested
        safe_truncate_plain(short_name, max_chars).into_owned()
    }
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

        // Get data source root path
        let storage_path = if crate::stats::is_db_mode() {
            crate::stats::get_opencode_root_path()
        } else {
            let storage_path = std::env::var("XDG_DATA_HOME")
                .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap()))
                .to_string();
            PathBuf::from(storage_path).join("opencode").join("storage")
        };

        // Initialize cache
        let stats_cache = StatsCache::new(storage_path.clone()).ok();
        log::info!("Initialized stats cache for: {}", storage_path.display());

        let (
            totals,
            per_day,
            session_titles,
            model_usage,
            session_message_files,
            parent_map,
            children_map,
        ) = if let Some(cache) = &stats_cache {
            let s = cache.load_or_compute();
            (
                s.totals,
                s.per_day,
                s.session_titles,
                s.model_usage,
                s.session_message_files,
                s.parent_map,
                s.children_map,
            )
        } else {
            let s = crate::stats::collect_stats();
            (
                s.totals,
                s.per_day,
                s.session_titles,
                s.model_usage,
                s.session_message_files,
                s.parent_map,
                s.children_map,
            )
        };

        // Set up live watcher with channel-based wake for instant updates
        let needs_refresh = Arc::new(Mutex::new(Vec::new()));
        let needs_refresh_clone = needs_refresh.clone();
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut live_watcher = LiveWatcher::new(
            storage_path.clone(),
            Arc::new(move |files| {
                needs_refresh_clone.lock().extend(files);
            }),
            wake_tx,
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
            parent_map,
            children_map,
            day_list,
            day_list_state,
            session_list: Vec::new(),
            session_list_state: ListState::default(),
            chat_cache_order: Vec::new(),
            chat_scroll: 0,
            model_usage,
            model_list_state,
            tool_usage,
            detail_scroll: 0,
            detail_max_scroll: 0,
            model_tool_scroll: 0,
            model_tool_max_scroll: 0,
            ranking_scroll: 0,
            ranking_max_scroll: 0,
            cached_session_items: Vec::new(),
            cached_session_width: 0,
            cached_day_items: Vec::new(),
            cached_day_width: 0,
            cached_model_items: Vec::new(),
            cached_model_width: 0,
            cached_day_strings: FxHashMap::default(),
            chat_cache: FxHashMap::default(),

            chat_max_scroll: 0,

            focus: Focus::Left,
            left_panel: LeftPanel::Stats,
            right_panel: RightPanel::Detail,
            is_active: false,
            models_active: false,
            exit: false,
            selected_model_index,
            current_chat_session_id: None,

            overview_projects: Vec::new(),
            overview_project_scroll: 0,
            overview_project_max_scroll: 0,
            overview_tool_scroll: 0,
            overview_tool_max_scroll: 0,
            overview_heatmap_layout: None,
            overview_heatmap_inspect: false,
            overview_heatmap_selected_day: None,
            overview_heatmap_selected_tokens: 0,
            overview_heatmap_selected_sessions: 0,
            overview_heatmap_selected_cost: 0.0,
            overview_heatmap_selected_active_ms: 0,

            modal: SessionModal::new(),

            last_mouse_panel: None,
            last_session_click: None,

            terminal_size: Rect::default(),

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
            wake_rx,
        };
        // Initialize all cached data and derived values
        app.update_session_list();
        app.precompute_day_strings();
        app.recompute_max_cost_width();
        app.compute_overview_data();

        // Ensure all displays are current
        app.should_redraw = true;
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

    fn compute_overview_data(&mut self) {
        // Aggregate projects from all sessions across all days
        let mut project_counts: FxHashMap<String, usize> = FxHashMap::default();
        for day_stat in self.per_day.values() {
            for session in day_stat.sessions.values() {
                let path = session.path_root.as_ref();
                let name = if path.is_empty() {
                    "home".to_string()
                } else {
                    path.rsplit('/')
                        .find(|s| !s.is_empty())
                        .unwrap_or("home")
                        .to_string()
                };
                *project_counts.entry(name).or_insert(0) += 1;
            }
        }
        let mut projects: Vec<(String, usize)> = project_counts.into_iter().collect();
        projects.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        self.overview_projects = projects;
    }

    #[inline]
    fn max_cost_width(&self) -> usize {
        self.cached_max_cost_width
    }

    fn update_session_list(&mut self) {
        let prev_selected_id = self
            .session_list_state
            .selected()
            .and_then(|i| self.session_list.get(i))
            .map(|s| s.id.clone());

        // Always clear and rebuild session list from current data
        self.session_list.clear();
        if let Some(day) = self.selected_day() {
            if let Some(stat) = self.per_day.get(&day) {
                let mut sessions: Vec<_> = stat.sessions.values().cloned().collect();
                sessions.sort_unstable_by(|a, b| b.last_activity.cmp(&a.last_activity));
                self.session_list = sessions;
            }
        }

        // Restore previous selection or select first session
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

        // Only clear current_chat_session_id if modal is NOT open
        if !self.modal.open {
            self.current_chat_session_id = None;
        }

        self.cached_session_items.clear();
        self.cached_session_width = 0;
        self.cached_day_items.clear();
        self.cached_day_width = 0;

        // Invalidate git branch cache since selected session may have changed
        self.cached_git_branch = None;

        log::debug!("Session list updated: {} sessions", self.session_list.len());
    }

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
                let month_abbr = match parsed.month() {
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
                };
                let formatted = format!(
                    "{} {:02}, {} {}",
                    month_abbr,
                    parsed.day(),
                    parsed.year(),
                    day_abbr
                );
                self.cached_day_strings.insert(day.clone(), formatted);
            } else {
                self.cached_day_strings.insert(day.clone(), day.clone());
            }
        }
    }

    fn combined_session_files(&self, session_id: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .session_message_files
            .get(session_id)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default();
        if let Some(child_ids) = self.children_map.get(session_id) {
            for child_id in child_ids {
                if let Some(child_files) = self.session_message_files.get(child_id.as_ref()) {
                    files.extend(child_files.iter().cloned());
                }
            }
        }
        files
    }

    fn open_session_modal(&mut self, area_height: u16) {
        let session_stat = match self
            .session_list_state
            .selected()
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

            let files_vec = self.combined_session_files(&session_id_str);

            // Open modal with cached messages (Arc clone, no deep copy)
            self.modal.open_session(
                &session_id_str,
                messages_arc,
                &session_stat,
                Some(&files_vec),
                current_day.as_deref(),
            );
            cached.total_lines
        } else {
            let files_vec = self.combined_session_files(&session_id_str);
            // Pass current day to filter messages to only show this day's messages
            let messages = if let Some(child_ids) = self.children_map.get(session_id.as_ref()) {
                let children: Vec<(Box<str>, Box<str>)> = child_ids
                    .iter()
                    .map(|cid| {
                        let agent_name = self
                            .session_titles
                            .get(cid)
                            .map(|t| crate::stats::extract_agent_name(t))
                            .unwrap_or_else(|| "subagent".into());
                        (cid.clone(), agent_name)
                    })
                    .collect();
                let (msgs, _max_ts) = crate::stats::load_combined_session_chat(
                    &session_id_str,
                    &children,
                    &self.session_message_files,
                    current_day.as_deref(),
                );
                msgs
            } else {
                let (msgs, _max_ts) = load_session_chat_with_max_ts(
                    &session_id_str,
                    Some(&files_vec),
                    current_day.as_deref(),
                );
                msgs
            };
            let total_lines: u16 = messages.iter().map(calculate_message_rendered_lines).sum();
            let blank_lines = if !messages.is_empty() {
                messages.len() - 1
            } else {
                0
            };
            let total_lines = total_lines + blank_lines as u16;

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
                },
            );
            self.chat_cache_order.push(cache_key.clone());

            // Open modal with Arc (no deep copy)
            self.modal.open_session(
                &session_id_str,
                messages_arc,
                &session_stat,
                Some(&files_vec),
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
        self.update_session_list();
        self.should_redraw = true;
    }

    fn day_previous(&mut self) {
        let i = self.day_list_state.selected().unwrap_or(0);
        self.day_list_state.select(Some(i.saturating_sub(1)));
        self.update_session_list();
        self.should_redraw = true;
    }

    fn model_next(&mut self) {
        if self.model_usage.is_empty() {
            return;
        }
        let i = self.model_list_state.selected().unwrap_or(0);
        self.model_list_state
            .select(Some((i + 1).min(self.model_usage.len() - 1)));
        self.should_redraw = true;
    }

    fn model_previous(&mut self) {
        let i = self.model_list_state.selected().unwrap_or(0);
        self.model_list_state.select(Some(i.saturating_sub(1)));
        self.should_redraw = true;
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
        self.should_redraw = true;
    }

    fn session_previous(&mut self) {
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state.select(Some(i.saturating_sub(1)));
        // Clear cached chat session since selection changed
        self.current_chat_session_id = None;
        self.should_redraw = true;
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
            let mut affected_sessions = FxHashSet::default();

            let (
                totals,
                per_day,
                session_titles,
                model_usage,
                session_message_files,
                parent_map,
                children_map,
            ) = if is_full_refresh {
                let s = cache.load_or_compute();
                (
                    s.totals,
                    s.per_day,
                    s.session_titles,
                    s.model_usage,
                    s.session_message_files,
                    s.parent_map,
                    s.children_map,
                )
            } else {
                let files: Vec<String> = changed_files
                    .iter()
                    .filter_map(|p| p.to_str().map(ToString::to_string))
                    .collect();
                let update = cache.update_files(files);
                affected_sessions = update.affected_sessions;
                (
                    update.totals,
                    update.per_day,
                    update.session_titles,
                    update.model_usage,
                    update.session_message_files,
                    update.parent_map,
                    update.children_map,
                )
            };

            // Update all stats
            self.totals = totals;
            self.per_day = per_day;
            self.session_titles = session_titles;
            self.model_usage = model_usage;
            self.session_message_files = session_message_files;
            self.parent_map = parent_map;
            self.children_map = children_map;

            // Always rebuild day list and sessions for consistency
            self.rebuild_day_and_session_lists(is_full_refresh);

            // Update derived data that affects display
            self.update_derived_data();

            // Live-refresh the open modal: reload chat + session details fresh.
            // Simple and reliable — just reload instead of complex incremental merging.
            if self.modal.open {
                if let Some(current) = self.current_chat_session_id.clone() {
                    self.refresh_open_modal(&current);
                    affected_sessions.remove(&current);
                }
            }

            // Invalidate chat cache for affected sessions (not the open modal)
            if !affected_sessions.is_empty() {
                self.invalidate_affected_chat_cache(&affected_sessions);
            }

            log::debug!("Stats refreshed successfully (live update)");
            self.should_redraw = true;
        }
    }

    /// Rebuild day list and session lists based on current data
    fn rebuild_day_and_session_lists(&mut self, _is_full_refresh: bool) {
        let prev_selected_day = self.selected_day();

        // Always rebuild day list to ensure consistency
        self.day_list.clear();
        self.day_list.extend(self.per_day.keys().cloned());
        self.day_list.sort_unstable_by(|a, b| b.cmp(a));

        // Restore previous selection or select first day
        if let Some(prev) = prev_selected_day.as_ref() {
            if let Some(idx) = self.day_list.iter().position(|d| d == prev) {
                self.day_list_state.select(Some(idx));
            } else if !self.day_list.is_empty() {
                self.day_list_state.select(Some(0));
            }
        } else if !self.day_list.is_empty() && self.day_list_state.selected().is_none() {
            self.day_list_state.select(Some(0));
        }

        // Always update session list to reflect current data
        self.update_session_list();
    }

    /// Update all derived data that affects display formatting
    fn update_derived_data(&mut self) {
        // Always update tool usage to reflect current totals
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

        // Always recalculate cached values that depend on current data
        self.precompute_day_strings();
        self.recompute_max_cost_width();
        self.compute_overview_data();

        self.cached_session_items.clear();
        self.cached_session_width = 0;
        self.cached_day_items.clear();
        self.cached_day_width = 0;
        self.cached_model_items.clear();
        self.cached_model_width = 0;
    }

    /// Refresh the currently open modal with latest data
    fn refresh_open_modal(&mut self, session_id: &str) {
        if self.stats_cache.is_some() {
            let current_day = self.selected_day();
            let ck = cache_key(session_id, current_day.as_deref());
            let files = self.session_message_files.get(session_id);

            if let Some(f) = files {
                let vec: Vec<PathBuf> = f.iter().cloned().collect();
                let msgs = if let Some(child_ids) = self.children_map.get(session_id) {
                    let children: Vec<(Box<str>, Box<str>)> = child_ids
                        .iter()
                        .map(|cid| {
                            let agent_name = self
                                .session_titles
                                .get(cid)
                                .map(|t| crate::stats::extract_agent_name(t))
                                .unwrap_or_else(|| "subagent".into());
                            (cid.clone(), agent_name)
                        })
                        .collect();
                    let (msgs, _) = crate::stats::load_combined_session_chat(
                        session_id,
                        &children,
                        &self.session_message_files,
                        current_day.as_deref(),
                    );
                    msgs
                } else {
                    let (msgs, _) = load_session_chat_with_max_ts(
                        session_id,
                        Some(&vec),
                        current_day.as_deref(),
                    );
                    msgs
                };

                // If the number of messages increased, only update what's needed
                let total_lines: u16 = msgs
                    .iter()
                    .map(calculate_message_rendered_lines)
                    .sum::<u16>()
                    + msgs.len().saturating_sub(1) as u16;
                let messages_arc = Arc::new(msgs);

                self.chat_cache.insert(
                    ck.clone(),
                    CachedChat {
                        messages: Arc::clone(&messages_arc),
                        total_lines,
                    },
                );
                self.modal.chat_messages = messages_arc;
            }

            if let Some(session) = self.session_list.iter().find(|s| &*s.id == session_id) {
                self.modal.current_session = Some((**session).clone());
                let files_vec = self.combined_session_files(session_id);
                let current_day = self.selected_day();
                let details = crate::stats::load_session_details(
                    session_id,
                    Some(&files_vec),
                    current_day.as_deref(),
                );
                self.modal.session_details = Some(details);
            }
        }
    }

    /// Invalidate chat cache for affected sessions
    fn invalidate_affected_chat_cache(&mut self, affected_sessions: &FxHashSet<String>) {
        self.chat_cache.retain(|key, _| {
            let session_id = key.split_once('|').map(|(s, _)| s).unwrap_or(key);
            !affected_sessions.contains(session_id)
        });
        self.chat_cache_order
            .retain(|key| self.chat_cache.contains_key(key));
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        self.should_redraw = true;
        let size = terminal.size()?;
        self.terminal_size = Rect::new(0, 0, size.width, size.height);

        while !self.exit {
            // Short poll: 30ms keeps UI responsive while saving CPU.
            // The wake channel from the file watcher will also wake us.
            if event::poll(std::time::Duration::from_millis(30))? {
                while event::poll(std::time::Duration::from_millis(0))? {
                    match event::read()? {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key_event(key, self.terminal_size.height)?;
                                self.should_redraw = true;
                                if self.exit {
                                    return Ok(());
                                }
                            }
                        }
                        Event::Resize(w, h) => {
                            self.terminal_size = Rect::new(0, 0, w, h);
                            self.should_redraw = true;
                        }
                        Event::Mouse(mouse) => {
                            if self.modal.open {
                                if self.modal.handle_mouse_event(mouse, self.terminal_size) {
                                    self.chat_scroll = self.modal.chat_scroll;
                                    self.should_redraw = true;
                                }
                            } else if self.handle_mouse_event(mouse, self.terminal_size) {
                                self.should_redraw = true;
                            }
                        }
                        Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
                    }
                }
            }

            // Drain wake signals from file watcher (non-blocking)
            while self.wake_rx.try_recv().is_ok() {}

            // Process coalesced file changes
            if let Some(watcher) = &self.live_watcher {
                watcher.process_changes();
            }

            // Apply pending refresh with minimal throttle (30ms)
            {
                let mut lock = self.needs_refresh.lock();
                if !lock.is_empty() {
                    self.pending_refresh_paths.append(&mut lock);
                }
            }

            let should_refresh = !self.pending_refresh_paths.is_empty()
                && self
                    .last_refresh
                    .map(|t| t.elapsed() >= std::time::Duration::from_millis(30))
                    .unwrap_or(true);

            if should_refresh {
                let paths = std::mem::take(&mut self.pending_refresh_paths);
                self.refresh_stats(paths);
                self.last_refresh = Some(std::time::Instant::now());
                // should_redraw is now set in refresh_stats method itself
            }

            // Ensure we always redraw if needed, including after window resize
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
                if self.is_active || self.models_active || self.overview_heatmap_inspect {
                    self.is_active = false;
                    self.models_active = false;
                    self.overview_heatmap_inspect = false;
                } else {
                    self.exit = true;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Stats => {
                            if self.right_panel == RightPanel::Tools {
                                self.right_panel = RightPanel::List;
                            } else {
                                self.focus = Focus::Left;
                            }
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::List {
                                self.right_panel = RightPanel::Tools;
                            } else {
                                self.focus = Focus::Left;
                            }
                        }
                        _ => self.focus = Focus::Left,
                    }
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.focus == Focus::Left {
                    self.focus = Focus::Right;
                    match self.left_panel {
                        LeftPanel::Stats => self.right_panel = RightPanel::Detail,
                        LeftPanel::Days => self.right_panel = RightPanel::List,
                        LeftPanel::Models => self.right_panel = RightPanel::Tools,
                    }
                } else {
                    match self.left_panel {
                        LeftPanel::Stats => {
                            if self.right_panel == RightPanel::List {
                                self.right_panel = RightPanel::Tools;
                            }
                        }
                        LeftPanel::Models => {
                            if self.right_panel == RightPanel::Tools {
                                self.right_panel = RightPanel::List;
                            }
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.is_active || self.models_active {
                    // ACTIVE MODE: Scroll within the focused panel
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => {
                                self.day_previous();
                                self.update_session_list();
                            }
                            LeftPanel::Models => {
                                self.model_previous();
                                self.selected_model_index = self.model_list_state.selected();
                            }
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => match self.right_panel {
                                RightPanel::List => {
                                    self.overview_project_scroll =
                                        self.overview_project_scroll.saturating_sub(1);
                                }
                                RightPanel::Tools => {
                                    self.overview_tool_scroll =
                                        self.overview_tool_scroll.saturating_sub(1);
                                }
                                _ => {}
                            },
                            LeftPanel::Days => match self.right_panel {
                                RightPanel::List => self.session_previous(),
                                RightPanel::Detail => {
                                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                                }
                                _ => {}
                            },
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::List => {
                                    self.model_previous();
                                    self.selected_model_index = self.model_list_state.selected();
                                }
                                RightPanel::Tools => {
                                    self.model_tool_scroll =
                                        self.model_tool_scroll.saturating_sub(1);
                                }
                                _ => {}
                            },
                        },
                    }
                } else {
                    // INACTIVE MODE: Navigate between panels
                    match self.focus {
                        Focus::Left => {
                            // Navigate between left panels: Stats <- Days <- Models
                            match self.left_panel {
                                LeftPanel::Stats => {}
                                LeftPanel::Days => self.left_panel = LeftPanel::Stats,
                                LeftPanel::Models => self.left_panel = LeftPanel::Days,
                            }
                        }
                        Focus::Right => {
                            // Navigate between right panels vertically
                            match self.left_panel {
                                LeftPanel::Stats => match self.right_panel {
                                    RightPanel::List | RightPanel::Tools => {
                                        self.right_panel = RightPanel::Activity;
                                    }
                                    RightPanel::Activity => {
                                        self.right_panel = RightPanel::Detail;
                                    }
                                    _ => {}
                                },
                                LeftPanel::Days => match self.right_panel {
                                    RightPanel::List => self.right_panel = RightPanel::Detail,
                                    _ => {}
                                },
                                LeftPanel::Models => match self.right_panel {
                                    RightPanel::List | RightPanel::Tools => {
                                        self.right_panel = RightPanel::Detail;
                                    }
                                    _ => {}
                                },
                            }
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.is_active || self.models_active {
                    // ACTIVE MODE: Scroll within the focused panel
                    match self.focus {
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => {
                                self.day_next();
                                self.update_session_list();
                            }
                            LeftPanel::Models => {
                                self.model_next();
                                self.selected_model_index = self.model_list_state.selected();
                            }
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => match self.right_panel {
                                RightPanel::List => {
                                    if self.overview_project_scroll
                                        < self.overview_project_max_scroll
                                    {
                                        self.overview_project_scroll += 1;
                                    }
                                }
                                RightPanel::Tools => {
                                    if self.overview_tool_scroll < self.overview_tool_max_scroll {
                                        self.overview_tool_scroll += 1;
                                    }
                                }
                                _ => {}
                            },
                            LeftPanel::Days => match self.right_panel {
                                RightPanel::List => self.session_next(),
                                RightPanel::Detail => {
                                    if self.detail_scroll < self.detail_max_scroll {
                                        self.detail_scroll += 1;
                                    }
                                }
                                _ => {}
                            },
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::List => {
                                    self.model_next();
                                    self.selected_model_index = self.model_list_state.selected();
                                }
                                RightPanel::Tools => {
                                    if self.model_tool_scroll < self.model_tool_max_scroll {
                                        self.model_tool_scroll += 1;
                                    }
                                }
                                _ => {}
                            },
                        },
                    }
                } else {
                    // INACTIVE MODE: Navigate between panels
                    match self.focus {
                        Focus::Left => {
                            // Navigate between left panels: Stats -> Days -> Models
                            match self.left_panel {
                                LeftPanel::Stats => self.left_panel = LeftPanel::Days,
                                LeftPanel::Days => self.left_panel = LeftPanel::Models,
                                LeftPanel::Models => {}
                            }
                        }
                        Focus::Right => {
                            // Navigate between right panels vertically
                            match self.left_panel {
                                LeftPanel::Stats => match self.right_panel {
                                    RightPanel::Detail => {
                                        self.right_panel = RightPanel::Activity;
                                    }
                                    RightPanel::Activity => {
                                        self.right_panel = RightPanel::List;
                                    }
                                    _ => {}
                                },
                                LeftPanel::Days => match self.right_panel {
                                    RightPanel::Detail => self.right_panel = RightPanel::List,
                                    _ => {}
                                },
                                LeftPanel::Models => match self.right_panel {
                                    RightPanel::Detail => self.right_panel = RightPanel::Tools,
                                    _ => {}
                                },
                            }
                        }
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
                            // update_session_list is called by day_previous()
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
                            // update_session_list is called by day_next()
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
                            self.should_redraw = true;
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
                                self.should_redraw = true;
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
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => {
                                if self.right_panel == RightPanel::Activity {
                                    self.overview_heatmap_inspect = !self.overview_heatmap_inspect;
                                } else if self.right_panel == RightPanel::List
                                    || self.right_panel == RightPanel::Tools
                                {
                                    self.is_active = true;
                                }
                            }
                            LeftPanel::Days => {
                                if self.right_panel == RightPanel::List {
                                    self.is_active = true;
                                }
                            }
                            LeftPanel::Models => {
                                if self.right_panel == RightPanel::List
                                    || self.right_panel == RightPanel::Tools
                                {
                                    self.models_active = true;
                                }
                            }
                        },
                    }
                } else if self.focus == Focus::Right
                    && self.left_panel == LeftPanel::Stats
                    && self.right_panel == RightPanel::Activity
                {
                    self.overview_heatmap_inspect = !self.overview_heatmap_inspect;
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
                    Some("activity") => {
                        // Activity heatmap does not use wheel scrolling currently
                        true
                    }
                    Some("detail") => {
                        // Only scroll if the detail panel is currently focused
                        if self.focus == Focus::Right && self.right_panel == RightPanel::Detail {
                            if self.left_panel == LeftPanel::Days {
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
                            if self.left_panel == LeftPanel::Stats {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.overview_tool_scroll =
                                        self.overview_tool_scroll.saturating_sub(1);
                                } else if self.overview_tool_scroll < self.overview_tool_max_scroll
                                {
                                    self.overview_tool_scroll += 1;
                                }
                            } else {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.model_tool_scroll =
                                        self.model_tool_scroll.saturating_sub(1);
                                } else if self.model_tool_scroll < self.model_tool_max_scroll {
                                    self.model_tool_scroll += 1;
                                }
                            }
                        }
                        true
                    }
                    Some("list") => {
                        if self.left_panel == LeftPanel::Stats {
                            // TOP PROJECTS: Scroll only if active
                            if self.focus == Focus::Right
                                && self.right_panel == RightPanel::List
                                && self.is_active
                            {
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    self.overview_project_scroll =
                                        self.overview_project_scroll.saturating_sub(1);
                                } else if self.overview_project_scroll
                                    < self.overview_project_max_scroll
                                {
                                    self.overview_project_scroll += 1;
                                }
                            }
                        } else if self.left_panel == LeftPanel::Models {
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
                if self.is_active || self.models_active || self.overview_heatmap_inspect {
                    self.is_active = false;
                    self.models_active = false;
                    self.overview_heatmap_inspect = false;
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
                "activity" => {
                    self.focus = Focus::Right;
                    self.left_panel = LeftPanel::Stats;
                    self.right_panel = RightPanel::Activity;
                    if self.overview_heatmap_inspect {
                        self.select_heatmap_day_from_mouse(x, y);
                    }
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

    fn select_heatmap_day_from_mouse(&mut self, x: u16, y: u16) {
        let Some(layout) = self.overview_heatmap_layout else {
            return;
        };

        // Row 0 is month labels; day rows are 1..=7
        if y <= layout.inner.y {
            return;
        }
        let day_row = (y - layout.inner.y - 1) as usize;
        if day_row >= 7 {
            return;
        }

        let start_x = layout.inner.x.saturating_add(layout.label_w);
        if x < start_x {
            return;
        }
        let mut rel_x = x - start_x;
        let mut col = 0usize;
        while col < layout.weeks {
            let w = layout.week_w
                + if (col as u16) < layout.extra_cols {
                    1
                } else {
                    0
                };
            if rel_x <= w {
                break;
            }
            rel_x = rel_x.saturating_sub(w);
            col += 1;
        }
        if col >= layout.weeks {
            return;
        }

        let date = layout.grid_start + chrono::Duration::days((col * 7 + day_row) as i64);

        // Use max date from actual data instead of system date
        let today = self
            .per_day
            .keys()
            .filter_map(|day_str| chrono::NaiveDate::parse_from_str(day_str, "%Y-%m-%d").ok())
            .max()
            .unwrap_or_else(|| chrono::Local::now().date_naive());

        let start_365 = today - chrono::Duration::days(364);
        if date < start_365 || date > today {
            return;
        }

        let key = date.format("%Y-%m-%d").to_string();
        let (sessions, tokens, cost, active_ms) = self
            .per_day
            .get(&key)
            .map(|ds| {
                let active: i64 = ds.sessions.values().map(|s| s.active_duration_ms).sum();
                (ds.sessions.len(), ds.tokens.total(), ds.cost, active)
            })
            .unwrap_or((0, 0, 0.0, 0));

        self.overview_heatmap_selected_day = Some(key);
        self.overview_heatmap_selected_sessions = sessions;
        self.overview_heatmap_selected_tokens = tokens;
        self.overview_heatmap_selected_cost = cost;
        self.overview_heatmap_selected_active_ms = active_ms;
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
            // Don't show "Enter" for GENERAL USAGE panel (Stats) as it doesn't activate
            let show_enter = !(self.focus == Focus::Left && self.left_panel == LeftPanel::Stats);
            spans.extend_from_slice(&[
                Span::styled("↑↓", k),
                Span::styled(" navigate", t),
                sep.clone(),
                Span::styled("←→/Click", k),
                Span::styled(" focus", t),
            ]);
            if show_enter {
                spans.extend_from_slice(&[
                    sep.clone(),
                    Span::styled("Enter", k),
                    Span::styled(" activate", t),
                ]);
            }
            spans.extend_from_slice(&[
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
            false,
        );
        self.render_day_list(
            frame,
            chunks[1],
            border_style,
            self.focus == Focus::Left && self.left_panel == LeftPanel::Days,
            self.is_active && self.left_panel == LeftPanel::Days,
        );
        self.render_model_list(
            frame,
            chunks[2],
            border_style,
            self.focus == Focus::Left && self.left_panel == LeftPanel::Models,
            self.models_active && self.left_panel == LeftPanel::Models,
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
            stat_widget(
                "Sessions",
                format!("{}", self.totals.sessions.len()),
                Color::Cyan,
            ),
            c1[0],
        );
        frame.render_widget(
            stat_widget(
                "Cost",
                format!("${:.2}", self.totals.display_cost()),
                Color::Yellow,
            ),
            c1[1],
        );

        // Col 2: Input / Output
        let c2 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[2]);
        frame.render_widget(
            stat_widget(
                "Input",
                format_number(self.totals.tokens.input),
                Color::Blue,
            ),
            c2[0],
        );
        frame.render_widget(
            stat_widget(
                "Output",
                format_number(self.totals.tokens.output),
                Color::Magenta,
            ),
            c2[1],
        );

        // Col 3: Thinking / Cache
        let c3 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[3]);
        frame.render_widget(
            stat_widget(
                "Thinking",
                format_number(self.totals.tokens.reasoning),
                Color::Rgb(255, 165, 0),
            ),
            c3[0],
        );
        frame.render_widget(
            stat_widget(
                "Cache",
                format_number(self.totals.tokens.cache_read + self.totals.tokens.cache_write),
                Color::Yellow,
            ),
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
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
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
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" / ", Style::default().fg(Color::Rgb(100, 100, 120))),
                Span::styled(
                    format!("{}", total_responses),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
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
        let inner_width = area.width.saturating_sub(2);
        if self.cached_day_items.is_empty() || self.cached_day_width != inner_width {
            self.rebuild_day_list_cache(inner_width);
        }

        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let list = List::new(self.cached_day_items.clone())
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

    fn rebuild_day_list_cache(&mut self, width: u16) {
        self.cached_day_width = width;
        let cost_width = self.max_cost_width();
        let sess_width = 4usize;
        let fixed_width = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + (sess_width + 5);
        let available =
            width.saturating_sub((fixed_width + 2).min(u16::MAX as usize) as u16) as usize;
        let name_width = available.max(8);

        self.cached_day_items = self
            .day_list
            .iter()
            .map(|day| {
                let (sess, input, output, cost, duration) =
                    if let Some(stat) = self.per_day.get(day) {
                        let dur: i64 = stat.sessions.values().map(|s| s.active_duration_ms).sum();
                        (
                            stat.sessions.len(),
                            stat.tokens.input,
                            stat.tokens.output,
                            stat.display_cost(),
                            dur,
                        )
                    } else {
                        (0, 0, 0, 0.0, 0)
                    };

                let day_with_name = self
                    .cached_day_strings
                    .get(day)
                    .cloned()
                    .unwrap_or_else(|| day.clone());

                let total_secs = (duration / 1000) as u64;
                let dur_str = if total_secs >= 3600 {
                    let h = total_secs / 3600;
                    let m = (total_secs % 3600) / 60;
                    format!(" · {}h{}m", h, m)
                } else if total_secs >= 60 {
                    let m = total_secs / 60;
                    let s = total_secs % 60;
                    format!(" · {}m{}s", m, s)
                } else if total_secs > 0 {
                    format!(" · {}s", total_secs)
                } else {
                    String::new()
                };

                let name_with_dur = format!("{}{}", day_with_name, dur_str);

                ListItem::new(usage_list_row(
                    name_with_dur,
                    input,
                    output,
                    cost,
                    sess,
                    &UsageRowFormat {
                        name_width,
                        cost_width,
                        sess_width,
                    },
                ))
            })
            .collect();
    }

    fn render_model_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        let inner_width = area.width.saturating_sub(2);
        if self.cached_model_items.is_empty() || self.cached_model_width != inner_width {
            self.rebuild_model_list_cache(inner_width);
        }

        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let list = List::new(self.cached_model_items.clone())
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

    fn rebuild_model_list_cache(&mut self, width: u16) {
        self.cached_model_width = width;
        let cost_width = self.max_cost_width();
        let sess_width = 4usize;
        let fixed_width = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + (sess_width + 5);
        let available =
            width.saturating_sub((fixed_width + 2).min(u16::MAX as usize) as u16) as usize;
        let name_width = available.max(8);

        self.cached_model_items = self
            .model_usage
            .iter()
            .map(|m| {
                let full_name = m.name.to_string();
                ListItem::new(usage_list_row(
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
                ))
            })
            .collect();
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
                // Simplified layout for Stats view
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(8),  // Overview (4 lines content + borders)
                        Constraint::Length(10), // Activity (8 lines content + borders)
                        Constraint::Min(0),     // Projects | Tools takes all remaining space
                    ])
                    .split(area);

                let bottom_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[2]);

                // Cache rects for mouse hit-testing
                self.cached_rects.detail = Some(chunks[0]);
                self.cached_rects.activity = Some(chunks[1]);
                self.cached_rects.list = Some(bottom_chunks[0]);
                self.cached_rects.tools = Some(bottom_chunks[1]);

                let overview_hl = is_focused && self.right_panel == RightPanel::Detail;
                self.render_overview_panel(frame, chunks[0], border_style, overview_hl);

                let activity_hl = is_focused && self.right_panel == RightPanel::Activity;
                self.render_activity_heatmap(frame, chunks[1], border_style, activity_hl);

                let projects_hl = is_focused && self.right_panel == RightPanel::List;
                self.render_projects_panel(
                    frame,
                    bottom_chunks[0],
                    if projects_hl {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                    projects_hl,
                );

                let tools_hl = is_focused && self.right_panel == RightPanel::Tools;
                self.render_overview_tools_panel(
                    frame,
                    bottom_chunks[1],
                    if tools_hl {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                    tools_hl,
                );
            }
            LeftPanel::Days => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(10), Constraint::Min(0)])
                    .split(area);

                // Cache right panel rects for Days view
                self.cached_rects.detail = Some(chunks[0]);
                self.cached_rects.activity = None;
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
                self.render_session_list(
                    frame,
                    chunks[1],
                    if list_highlighted {
                        border_style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                    list_highlighted,
                    self.is_active,
                );
            }
            LeftPanel::Models => {
                self.cached_rects.activity = None;
                // Cache right panel rects for Models view (done in render_model_detail)
                self.render_model_detail(frame, area, border_style, is_focused, self.models_active)
            }
        }
    }

    fn render_overview_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
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
                    " OVERVIEW ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Compute stats
        let total_sessions = self.totals.sessions.len();
        let total_days = self.day_list.len();
        let start_day = self.day_list.last().cloned().unwrap_or_else(|| "—".into());
        let active_days = total_days;

        let days_since_start = if let Some(first) = self.day_list.last() {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d") {
                let today = chrono::Local::now().date_naive();
                (today - d).num_days().max(1) as usize
            } else {
                total_days.max(1)
            }
        } else {
            1
        };

        let avg_sess_per_day = if active_days > 0 {
            total_sessions as f64 / active_days as f64
        } else {
            0.0
        };

        let avg_cost_per_sess = if total_sessions > 0 {
            self.totals.display_cost() / total_sessions as f64
        } else {
            0.0
        };

        let (peak_day, peak_count) = self
            .per_day
            .iter()
            .map(|(d, s)| (d.clone(), s.sessions.len()))
            .max_by_key(|(_, c)| *c)
            .unwrap_or_else(|| ("—".into(), 0));
        let peak_display = self
            .cached_day_strings
            .get(&peak_day)
            .cloned()
            .unwrap_or(peak_day);

        let longest_ms: i64 = self
            .per_day
            .values()
            .flat_map(|d| d.sessions.values())
            .map(|s| s.active_duration_ms)
            .max()
            .unwrap_or(0);

        let total_active_ms: i64 = self
            .per_day
            .values()
            .flat_map(|d| d.sessions.values())
            .map(|s| s.active_duration_ms)
            .sum();

        let fav_lang = {
            let mut ext_counts: FxHashMap<&str, u64> = FxHashMap::default();
            for day_stat in self.per_day.values() {
                for session in day_stat.sessions.values() {
                    for fd in &session.file_diffs {
                        let ext = fd.path.rsplit('.').next().unwrap_or("");
                        let lang = match ext {
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
                            _ => "",
                        };
                        if !lang.is_empty() {
                            *ext_counts.entry(lang).or_insert(0) +=
                                (fd.additions + fd.deletions).max(1);
                        }
                    }
                }
            }
            ext_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(l, _)| l.to_string())
                .unwrap_or_else(|| "—".into())
        };

        let start_display = if let Ok(d) = chrono::NaiveDate::parse_from_str(&start_day, "%Y-%m-%d")
        {
            let month = match d.month() {
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
            };
            format!("{} {:02}, {}", month, d.day(), d.year())
        } else {
            start_day
        };

        let label_style = Style::default().fg(Color::Rgb(140, 140, 160));
        let val_col = 18usize;

        if inner.width < 50 {
            // 1-column layout for narrow screens
            let all_lines = vec![
                Line::from(vec![
                    Span::styled("Peak: ", label_style),
                    Span::styled(peak_display, Style::default().fg(Color::Yellow)),
                ]),
                Line::from(vec![
                    Span::styled("Long: ", label_style),
                    Span::styled(
                        format_active_duration(longest_ms),
                        Style::default().fg(Color::Rgb(100, 200, 255)),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Avg:  ", label_style),
                    Span::styled(
                        format!("{:.1}", avg_sess_per_day),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Fav:  ", label_style),
                    Span::styled(fav_lang, Style::default().fg(Color::Magenta)),
                ]),
            ];
            frame.render_widget(Paragraph::new(all_lines), inner);
        } else {
            // 2-column layout (standard)
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner);

            let left_lines = vec![
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Peak Day", w = val_col), label_style),
                    Span::styled(
                        format!("{} ({}s)", peak_display, peak_count),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Longest Session", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format_active_duration(longest_ms),
                        Style::default().fg(Color::Rgb(100, 200, 255)),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Avg Sessions/Day", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format!("{:.1}", avg_sess_per_day),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Total Active", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format_active_duration(total_active_ms),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            let right_lines = vec![
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Start Day", w = val_col), label_style),
                    Span::styled(start_display, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Active Days", w = val_col), label_style),
                    Span::styled(
                        format!("{} / {}", active_days, days_since_start),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Avg Cost/Session", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format!("${:.2}", avg_cost_per_sess),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Fav Language", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        fav_lang,
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            frame.render_widget(Paragraph::new(left_lines), cols[0]);
            frame.render_widget(Paragraph::new(right_lines), cols[1]);
        }
    }

    /// Activity heatmap: last 365 days, Mon-Sun rows, adaptive to terminal width.
    fn render_activity_heatmap(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_focused: bool,
    ) {
        let title_color = if is_focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_focused {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
            })
            .title(
                Line::from(Span::styled(
                    " ACTIVITY ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if self.overview_heatmap_inspect {
                        " Inspect: ON (click day) │ Enter/Esc: off "
                    } else {
                        " "
                    },
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 16 || inner.height < 6 {
            self.overview_heatmap_layout = None;
            return;
        }

        // Use max date from actual data instead of system date
        let today = self
            .per_day
            .keys()
            .filter_map(|day_str| chrono::NaiveDate::parse_from_str(day_str, "%Y-%m-%d").ok())
            .max()
            .unwrap_or_else(|| chrono::Local::now().date_naive());

        let start_365 = today - chrono::Duration::days(364);
        let start_offset = start_365.weekday().num_days_from_monday() as i64;
        let grid_start = start_365 - chrono::Duration::days(start_offset);

        let total_days_365 = (today - grid_start).num_days().max(0) as usize + 1;
        let total_weeks_365 = total_days_365.div_ceil(7);

        let label_w = 6u16;
        let avail_w = inner.width.saturating_sub(label_w + 1);
        if avail_w < 2 {
            self.overview_heatmap_layout = None;
            return;
        }

        // Fit up to full 365-day window, otherwise show latest weeks that fit.
        let max_weeks_fit = (avail_w / 2) as usize;
        if max_weeks_fit == 0 {
            self.overview_heatmap_layout = None;
            return;
        }

        let weeks = total_weeks_365.min(max_weeks_fit).max(1);
        let start_week = total_weeks_365.saturating_sub(weeks);
        let render_start = grid_start + chrono::Duration::days((start_week * 7) as i64);

        // Use full available width exactly by distributing remainder columns.
        let week_w = (avail_w / weeks as u16).max(2);
        let extra_cols = avail_w.saturating_sub(week_w * weeks as u16);

        let mut grid: Vec<[Option<u64>; 7]> = vec![[None; 7]; weeks];
        let mut max_tokens: u64 = 1;

        for (w, col) in grid.iter_mut().enumerate() {
            for (d, cell) in col.iter_mut().enumerate() {
                let date = render_start + chrono::Duration::days((w * 7 + d) as i64);
                if date < start_365 || date > today {
                    continue;
                }
                let key = date.format("%Y-%m-%d").to_string();
                let tokens = self
                    .per_day
                    .get(&key)
                    .map(|ds| ds.tokens.total())
                    .unwrap_or(0);
                *cell = Some(tokens);
                max_tokens = max_tokens.max(tokens);
            }
        }

        self.overview_heatmap_layout = Some(HeatmapLayout {
            inner,
            label_w,
            weeks,
            grid_start: render_start,
            week_w,
            extra_cols,
        });

        let week_width_at = |idx: usize| week_w + if (idx as u16) < extra_cols { 1 } else { 0 };

        // Month labels centered over each visible month range.
        let mut month_row: Vec<char> = vec![' '; avail_w as usize];
        let mut month_ranges: Vec<(u32, u16, u16)> = Vec::new(); // month, x_start, x_end
        let mut x_cursor: u16 = 0;
        let mut cur_month: Option<u32> = None;
        let mut range_start: u16 = 0;
        for w in 0..weeks {
            let d0 = render_start + chrono::Duration::days((w * 7) as i64);
            let m = d0.month();
            if cur_month.is_none() {
                cur_month = Some(m);
                range_start = x_cursor;
            } else if cur_month != Some(m) {
                month_ranges.push((cur_month.unwrap_or(m), range_start, x_cursor));
                cur_month = Some(m);
                range_start = x_cursor;
            }
            x_cursor = x_cursor.saturating_add(week_width_at(w));
        }
        if let Some(m) = cur_month {
            month_ranges.push((m, range_start, x_cursor));
        }

        let mut last_label_end: i32 = -2;
        for (m, x0, x1) in month_ranges {
            let name = match m {
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
            };
            let span_w = x1.saturating_sub(x0) as usize;
            if span_w < name.len() {
                continue;
            }
            let center = (x0 as usize + x1 as usize) / 2;
            let start = center.saturating_sub(name.len() / 2) as i32;
            let end = start + name.len() as i32 - 1;
            if start <= last_label_end + 1 {
                continue;
            }
            if start < 0 || end >= month_row.len() as i32 {
                continue;
            }
            for (i, ch) in name.chars().enumerate() {
                month_row[(start as usize) + i] = ch;
            }
            last_label_end = end;
        }

        let day_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

        let mut lines: Vec<Line> = Vec::with_capacity(11);
        if inner.height > 8 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<w$}", "", w = label_w as usize),
                    Style::default(),
                ),
                Span::styled(
                    month_row.iter().collect::<String>(),
                    Style::default().fg(Color::Rgb(140, 140, 160)),
                ),
            ]));
        }

        let selected_key = self.overview_heatmap_selected_day.as_deref();

        // 7 day rows (show all labels)
        for d in 0..7usize {
            let mut spans: Vec<Span> = Vec::with_capacity(weeks + 1);
            let label = format!(" {:<w$}", day_labels[d], w = (label_w - 1) as usize);
            spans.push(Span::styled(
                label,
                Style::default().fg(Color::Rgb(100, 100, 120)),
            ));

            for (w, week) in grid.iter().enumerate().take(weeks) {
                let col_w = week_width_at(w) as usize;
                let date = render_start + chrono::Duration::days((w * 7 + d) as i64);
                let key = date.format("%Y-%m-%d").to_string();
                let is_selected = selected_key.is_some_and(|k| k == key);

                match week[d] {
                    None => {
                        spans.push(Span::styled(" ".repeat(col_w), Style::default()));
                    }
                    Some(0) => {
                        let ch = if is_selected { '░' } else { '█' };
                        spans.push(Span::styled(
                            ch.to_string().repeat(col_w),
                            Style::default().fg(Color::Rgb(28, 32, 38)),
                        ));
                    }
                    Some(day_tokens) => {
                        let ratio = day_tokens as f64 / max_tokens as f64;
                        let color = if ratio <= 0.20 {
                            Color::Rgb(24, 66, 44)
                        } else if ratio <= 0.40 {
                            Color::Rgb(28, 102, 58)
                        } else if ratio <= 0.60 {
                            Color::Rgb(42, 138, 74)
                        } else if ratio <= 0.80 {
                            Color::Rgb(64, 181, 96)
                        } else if ratio <= 0.95 {
                            Color::Rgb(94, 230, 126)
                        } else {
                            Color::Rgb(118, 255, 149)
                        };

                        let ch = if is_selected { '▓' } else { '█' };
                        spans.push(Span::styled(
                            ch.to_string().repeat(col_w),
                            Style::default().fg(color),
                        ));
                    }
                }
            }
            lines.push(Line::from(spans));
        }

        if inner.height > 9 {
            lines.push(Line::from(""));
        }
        let mut legend = vec![
            Span::styled(
                format!("{:<w$}", "", w = label_w as usize),
                Style::default(),
            ),
            Span::styled("Less ", Style::default().fg(Color::Rgb(100, 100, 120))),
            Span::styled("██", Style::default().fg(Color::Rgb(28, 32, 38))),
            Span::styled("██", Style::default().fg(Color::Rgb(24, 66, 44))),
            Span::styled("██", Style::default().fg(Color::Rgb(28, 102, 58))),
            Span::styled("██", Style::default().fg(Color::Rgb(42, 138, 74))),
            Span::styled("██", Style::default().fg(Color::Rgb(64, 181, 96))),
            Span::styled("██", Style::default().fg(Color::Rgb(94, 230, 126))),
            Span::styled(" More ", Style::default().fg(Color::Rgb(100, 100, 120))),
        ];
        if let Some(day) = &self.overview_heatmap_selected_day {
            legend.push(Span::styled(
                format!(
                    "   [{}] tok:{}  sess:{}  cost:${:.2}  active:{}",
                    day,
                    format_number(self.overview_heatmap_selected_tokens),
                    self.overview_heatmap_selected_sessions,
                    self.overview_heatmap_selected_cost,
                    format_active_duration(self.overview_heatmap_selected_active_ms)
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(legend));

        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn render_projects_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(
                Line::from(Span::styled(
                    " TOP PROJECTS ",
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

        if self.overview_projects.is_empty() {
            frame.render_widget(
                Paragraph::new("No project data")
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(Alignment::Center),
                inner,
            );
            return;
        }

        let visible = inner.height as usize;
        self.overview_project_max_scroll = self.overview_projects.len().saturating_sub(visible);
        self.overview_project_scroll = self
            .overview_project_scroll
            .min(self.overview_project_max_scroll);

        let max_count = self
            .overview_projects
            .first()
            .map(|(_, c)| *c)
            .unwrap_or(1)
            .max(1);
        let name_width = 14.min(inner.width.saturating_sub(16) as usize).max(6);
        let bar_max = inner.width.saturating_sub((name_width + 12) as u16) as usize;

        let lines: Vec<Line> = self
            .overview_projects
            .iter()
            .enumerate()
            .skip(self.overview_project_scroll)
            .take(visible)
            .map(|(i, (name, count))| {
                let bar_len = (*count as f64 / max_count as f64 * bar_max as f64) as usize;
                let filled = "█".repeat(bar_len);
                let empty = "░".repeat(bar_max.saturating_sub(bar_len));
                Line::from(vec![
                    Span::styled(
                        format!(" {:>2}. ", i + 1),
                        Style::default().fg(Color::Rgb(100, 100, 120)),
                    ),
                    Span::styled(
                        format!(
                            "{:<width$} ",
                            safe_truncate_plain(name, name_width),
                            width = name_width
                        ),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(filled, Style::default().fg(Color::Cyan)),
                    Span::styled(empty, Style::default().fg(Color::Rgb(40, 40, 50))),
                    Span::styled(
                        format!(" {:>3}", count),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn render_overview_tools_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
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
            frame.render_widget(
                Paragraph::new("No tool data")
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(Alignment::Center),
                inner,
            );
            return;
        }

        let visible = inner.height as usize;
        self.overview_tool_max_scroll = self.tool_usage.len().saturating_sub(visible);
        self.overview_tool_scroll = self.overview_tool_scroll.min(self.overview_tool_max_scroll);

        let total_count: u64 = self.tool_usage.iter().map(|t| t.count).sum();
        let name_w = 12.min(inner.width.saturating_sub(14) as usize).max(4);
        let bar_max = inner.width.saturating_sub((name_w + 14) as u16) as usize;

        let lines: Vec<Line> = self
            .tool_usage
            .iter()
            .skip(self.overview_tool_scroll)
            .take(visible)
            .map(|tool| {
                let pct = if total_count > 0 {
                    tool.count as f64 / total_count as f64
                } else {
                    0.0
                };
                let bar_len = (pct * bar_max as f64) as usize;
                let filled = "█".repeat(bar_len);
                let empty = "░".repeat(bar_max.saturating_sub(bar_len));
                Line::from(vec![
                    Span::styled(
                        format!(
                            " {:>width$} ",
                            truncate_with_ellipsis(&tool.name, name_w),
                            width = name_w
                        ),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(filled, Style::default().fg(Color::Magenta)),
                    Span::styled(empty, Style::default().fg(Color::Rgb(40, 40, 50))),
                    Span::styled(
                        format!(" {:>5}", tool.count),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
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
                Constraint::Length(7), // Info (6 lines content + borders)
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
                            Color::DarkGray
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

            let est_cost = crate::cost::estimate_cost(&model.name, &model.tokens);
            let savings = est_cost.map(|e| e - model.cost);

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
                    Span::styled("Est. Cost ", label_color),
                    Span::styled(
                        match est_cost {
                            Some(c) => format!("${:.2}", c),
                            None => "$0.00".to_string(),
                        },
                        Style::default()
                            .fg(match est_cost {
                                Some(c) if c > 0.0 => Color::Rgb(255, 165, 0),
                                _ => Color::DarkGray,
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Savings   ", label_color),
                    Span::styled(
                        match savings {
                            Some(s) => format!("${:.2}", s),
                            None => "$0.00".to_string(),
                        },
                        Style::default()
                            .fg(match savings {
                                Some(s) if s > 0.0 => Color::Green,
                                _ => Color::DarkGray,
                            })
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
                            Color::DarkGray
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
                            Color::DarkGray
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
                            Color::DarkGray
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
                        .map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "n/a".to_string()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

            left_lines.push(Line::from(vec![
                Span::styled("Duration     ", label_style),
                Span::styled(
                    format_active_duration(s.active_duration_ms),
                    Style::default().fg(Color::Rgb(100, 200, 255)),
                ),
            ]));

            // Agents row - inline comma-separated, truncate with +X if needed
            if s.agents.is_empty() {
                left_lines.push(Line::from(vec![
                    Span::styled("Agents       ", label_style),
                    Span::styled("n/a", Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                let mut agent_refs: Vec<(&str, bool, u64)> = s
                    .agents
                    .iter()
                    .map(|a| (a.name.as_ref(), a.is_main, a.tokens.total()))
                    .collect();
                agent_refs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
                agent_refs.dedup_by(|a, b| a.0 == b.0);
                let agent_names: Vec<&str> = agent_refs.iter().map(|(n, _, _)| *n).collect();
                let avail = left_val_width;
                let mut display = String::new();
                let mut shown = 0usize;
                for (i, name) in agent_names.iter().enumerate() {
                    let candidate = if i == 0 {
                        name.to_string()
                    } else {
                        format!(", {}", name)
                    };
                    let remaining = agent_names.len() - i;
                    let plus_suffix = if remaining > 1 {
                        format!(", +{}", remaining - 1)
                    } else {
                        String::new()
                    };
                    if display.len() + candidate.len() + plus_suffix.len() > avail && shown > 0 {
                        display.push_str(&format!(", +{}", agent_names.len() - shown));
                        break;
                    }
                    display.push_str(&candidate);
                    shown += 1;
                }
                if shown == agent_names.len() && display.len() > avail {
                    display = truncate_with_ellipsis(&display, avail);
                }
                left_lines.push(Line::from(vec![
                    Span::styled("Agents       ", label_style),
                    Span::styled(
                        display,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }

            // Models: single line, comma-separated with +N overflow
            {
                let mut models: Vec<_> = s.models.iter().map(|m| m.as_ref()).collect();
                models.sort_unstable();
                let avail = left_val_width;
                if models.is_empty() {
                    left_lines.push(Line::from(vec![
                        Span::styled("Models       ", label_style),
                        Span::styled("n/a", Style::default().fg(Color::DarkGray)),
                    ]));
                } else {
                    let mut display = String::new();
                    let mut shown = 0usize;
                    for (i, model) in models.iter().enumerate() {
                        let candidate = if display.is_empty() {
                            (*model).to_string()
                        } else {
                            format!(", {}", model)
                        };
                        let remaining = models.len() - i - 1;
                        let suffix_len = if remaining > 0 {
                            format!(", +{}", remaining).len()
                        } else {
                            0
                        };
                        if display.len() + candidate.len() + suffix_len <= avail || i == 0 {
                            display.push_str(&candidate);
                            shown += 1;
                        } else {
                            break;
                        }
                    }
                    let overflow = models.len() - shown;
                    if overflow > 0 {
                        display.push_str(&format!(", +{}", overflow));
                    }
                    left_lines.push(Line::from(vec![
                        Span::styled("Models       ", label_style),
                        Span::styled(
                            truncate_with_ellipsis(&display, avail),
                            Style::default()
                                .fg(Color::Magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
            }

            // Host line (Local or Server)
            {
                let device = crate::device::get_device_info();
                let type_color = if device.kind == "server" {
                    Color::Rgb(255, 165, 0)
                } else {
                    Color::Rgb(100, 200, 255)
                };
                let label = device.display_label();
                // 13 for "Host:        ", label length, 3 for " | ", and 1 for margin
                let host_avail = (cols[0].width as usize).saturating_sub(13 + label.len() + 3 + 1);

                left_lines.push(Line::from(vec![
                    Span::styled("Host:        ", label_style),
                    Span::styled(label, Style::default().fg(type_color)),
                    Span::raw(" | "),
                    Span::styled(
                        truncate_host_name(
                            &device.display_name(),
                            &device.short_name(),
                            host_avail,
                        ),
                        Style::default().fg(type_color),
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
            Color::DarkGray
        };

        let list = List::new(self.cached_session_items.clone())
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

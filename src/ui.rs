//! Main UI module with three panels: Stats, Days, Models.

mod days_panel;
mod helpers;
mod models_panel;
mod stats_panel;

use crate::live_watcher::LiveWatcher;
use crate::session::SessionModal;
use crate::stats::{load_session_chat_with_max_ts, DayStat, ModelUsage, ToolUsage, Totals};
use crate::stats_cache::StatsCache;
use crate::theme::Theme;
use chrono::Datelike;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use helpers::{
    cache_key, calculate_message_rendered_lines, CachedChat, Focus, HeatmapLayout, LeftPanel,
    ModelTimelineLayout, PanelRects, RightPanel,
};
use parking_lot::Mutex;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{ListState, Paragraph},
    Frame,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

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
    cached_session_items: Vec<ratatui::widgets::ListItem<'static>>,
    cached_session_width: u16,
    cached_day_items: Vec<ratatui::widgets::ListItem<'static>>,
    cached_day_width: u16,
    cached_model_items: Vec<ratatui::widgets::ListItem<'static>>,
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

    cached_day_strings: FxHashMap<String, String>,

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
    theme: Theme,

    //Mmouse tracking
    last_mouse_panel: Option<&'static str>,
    last_session_click: Option<(std::time::Instant, usize)>,

    terminal_size: Rect,
    cached_rects: PanelRects,

    cached_git_branch: Option<(Box<str>, Option<String>)>,
    cached_max_cost_width: usize,

    // Overview panel data
    overview_projects: Vec<(String, usize)>,
    overview_project_scroll: usize,
    overview_project_max_scroll: usize,
    overview_tool_scroll: usize,
    overview_tool_max_scroll: usize,
    overview_heatmap_layout: Option<HeatmapLayout>,
    overview_heatmap_selected_day: Option<String>,
    overview_heatmap_selected_tokens: u64,
    overview_heatmap_selected_sessions: usize,
    overview_heatmap_selected_cost: f64,
    overview_heatmap_selected_active_ms: i64,
    overview_heatmap_flash_time: Option<std::time::Instant>,

    // Models panel timeline data
    model_timeline_layout: Option<ModelTimelineLayout>,
    model_timeline_selected_day: Option<String>,
    model_timeline_selected_tokens: u64,
    model_timeline_selected_pct: f64,
    model_timeline_flash_time: Option<std::time::Instant>,

    // Live stats: cache and file watching
    stats_cache: Option<StatsCache>,
    _storage_path: PathBuf,
    live_watcher: Option<LiveWatcher>,
    needs_refresh: Arc<Mutex<Vec<PathBuf>>>,
    pending_refresh_paths: Vec<PathBuf>,
    last_refresh: Option<std::time::Instant>,
    should_redraw: bool,
    wake_rx: mpsc::Receiver<()>,
}

/// The main application state.
impl App {
    pub fn new() -> Self {
        let storage_path = if crate::stats::is_db_mode() {
            crate::stats::get_opencode_root_path()
        } else {
            let storage_path = std::env::var("XDG_DATA_HOME")
                .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap()))
                .to_string();
            PathBuf::from(storage_path).join("opencode").join("storage")
        };

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
            overview_heatmap_selected_day: None,
            overview_heatmap_selected_tokens: 0,
            overview_heatmap_selected_sessions: 0,
            overview_heatmap_selected_cost: 0.0,
            overview_heatmap_selected_active_ms: 0,
            overview_heatmap_flash_time: None,

            model_timeline_layout: None,
            model_timeline_selected_day: None,
            model_timeline_selected_tokens: 0,
            model_timeline_selected_pct: 0.0,
            model_timeline_flash_time: None,

            modal: SessionModal::new(),
            theme: Theme,

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

        app.should_redraw = true;
        app
    }

    /// Recompute the maximum width of the cost column.
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

        // Clear and rebuild session list from current data
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

        // Clear current_chat_session_id if modal is NOT open
        if !self.modal.open {
            self.current_chat_session_id = None;
        }

        self.cached_session_items.clear();
        self.cached_session_width = 0;
        self.cached_day_items.clear();
        self.cached_day_width = 0;

        self.cached_git_branch = None;

        log::debug!("Session list updated: {} sessions", self.session_list.len());
    }

    /// Precompute formatted day strings
    fn precompute_day_strings(&mut self) {
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

    /// Get all message files for a session and its subagents
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
                &self.parent_map,
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
                &self.parent_map,
            );

            total_lines
        };

        self.chat_max_scroll = total_lines.saturating_sub(area_height.saturating_sub(4));
    }

    /// Move to the next day in the day list
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

    /// Move to the previous day in the day list
    fn day_previous(&mut self) {
        let i = self.day_list_state.selected().unwrap_or(0);
        self.day_list_state.select(Some(i.saturating_sub(1)));
        self.update_session_list();
        self.should_redraw = true;
    }

    #[inline]
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

    #[inline]
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

    #[inline]
    fn selected_day(&self) -> Option<String> {
        self.day_list_state
            .selected()
            .and_then(|i| self.day_list.get(i).cloned())
    }

    /// Refresh stats from cache
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

            self.totals = totals;
            self.per_day = per_day;
            self.session_titles = session_titles;
            self.model_usage = model_usage;
            self.session_message_files = session_message_files;
            self.parent_map = parent_map;
            self.children_map = children_map;

            self.rebuild_day_and_session_lists(is_full_refresh);
            self.update_derived_data();

            if self.modal.open {
                if let Some(current) = self.current_chat_session_id.clone() {
                    self.refresh_open_modal(&current);
                    affected_sessions.remove(&current);
                }
            }

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

        if !self.model_usage.is_empty() && self.model_list_state.selected().is_none() {
            self.model_list_state.select(Some(0));
            self.selected_model_index = Some(0);
        }

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
                    &self.parent_map,
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

    /// Main event loop for the UI
    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        self.should_redraw = true;
        let size = terminal.size()?;
        self.terminal_size = Rect::new(0, 0, size.width, size.height);

        while !self.exit {
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

            while self.wake_rx.try_recv().is_ok() {}

            if let Some(watcher) = &self.live_watcher {
                watcher.process_changes();
            }

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
            }

            let needs_flicker_redraw = (self.overview_heatmap_flash_time.is_some()
                && self.left_panel == LeftPanel::Stats)
                || (self.model_timeline_flash_time.is_some()
                    && self.left_panel == LeftPanel::Models);

            if self.should_redraw || needs_flicker_redraw {
                terminal.draw(|frame| self.render(frame))?;
                self.should_redraw = false;
            }
        }

        Ok(())
    }

    /// Input handling for the UI.
    fn handle_key_event(
        &mut self,
        key: crossterm::event::KeyEvent,
        term_height: u16,
    ) -> io::Result<()> {
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
                if self.focus == Focus::Right {
                    match self.left_panel {
                        LeftPanel::Stats => {
                            // Layout: Tools (LEFT) | List (RIGHT)
                            // From List → Tools, from Tools or Detail/Activity → Left
                            if self.right_panel == RightPanel::List {
                                self.right_panel = RightPanel::Tools;
                            } else {
                                self.focus = Focus::Left;
                            }
                        }
                        LeftPanel::Models => {
                            // Layout: Tools (LEFT) | List (RIGHT)
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
                            // Layout: Tools (LEFT) | List (RIGHT)
                            // From Tools → List, List has nothing to the right
                            if self.right_panel == RightPanel::Tools {
                                self.right_panel = RightPanel::List;
                            }
                        }
                        LeftPanel::Models => {
                            // Layout: Tools (LEFT) | List (RIGHT)
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
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => {}
                            LeftPanel::Days => self.left_panel = LeftPanel::Stats,
                            LeftPanel::Models => self.left_panel = LeftPanel::Days,
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => match self.right_panel {
                                RightPanel::List | RightPanel::Tools => {
                                    self.right_panel = RightPanel::Activity;
                                }
                                RightPanel::Activity => {
                                    self.right_panel = RightPanel::Detail;
                                }
                                _ => {}
                            },
                            LeftPanel::Days => {
                                if self.right_panel == RightPanel::List {
                                    self.right_panel = RightPanel::Detail;
                                }
                            }
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::List | RightPanel::Tools => {
                                    self.right_panel = RightPanel::Activity;
                                }
                                RightPanel::Activity => {
                                    self.right_panel = RightPanel::Detail;
                                }
                                _ => {}
                            },
                        },
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
                        Focus::Left => match self.left_panel {
                            LeftPanel::Stats => self.left_panel = LeftPanel::Days,
                            LeftPanel::Days => self.left_panel = LeftPanel::Models,
                            LeftPanel::Models => {}
                        },
                        Focus::Right => match self.left_panel {
                            LeftPanel::Stats => match self.right_panel {
                                RightPanel::Detail => {
                                    self.right_panel = RightPanel::Activity;
                                }
                                RightPanel::Activity => {
                                    self.right_panel = RightPanel::List;
                                }
                                _ => {}
                            },
                            LeftPanel::Days => {
                                if self.right_panel == RightPanel::Detail {
                                    self.right_panel = RightPanel::List;
                                }
                            }
                            LeftPanel::Models => match self.right_panel {
                                RightPanel::Detail => {
                                    self.right_panel = RightPanel::Activity;
                                }
                                RightPanel::Activity => {
                                    self.right_panel = RightPanel::Tools;
                                }
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
                            if self.right_panel == RightPanel::List && !self.model_usage.is_empty()
                            {
                                let last = self.model_usage.len() - 1;
                                self.model_list_state.select(Some(last));
                                self.selected_model_index = Some(last);
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
                                if self.right_panel == RightPanel::List
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
                        if self.focus == Focus::Right
                            && self.right_panel == RightPanel::Detail
                            && self.left_panel == LeftPanel::Days
                        {
                            if mouse.kind == MouseEventKind::ScrollUp {
                                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                            } else if self.detail_scroll < self.detail_max_scroll {
                                self.detail_scroll += 1;
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
                            } else if self.left_panel == LeftPanel::Models {
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
                            // TOP PROJECTS: Scroll only if focused
                            if self.focus == Focus::Right && self.right_panel == RightPanel::List {
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
                "activity" => {
                    self.focus = Focus::Right;
                    if self.left_panel == LeftPanel::Models {
                        self.right_panel = RightPanel::Activity;
                        self.select_model_timeline_day_from_mouse(x, y);
                    } else {
                        self.left_panel = LeftPanel::Stats;
                        self.right_panel = RightPanel::Activity;
                        // Always allow clicking to select a day
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

        // Month labels only exist when inner.height > 8; adjust offset accordingly
        let has_month_row = layout.inner.height > 8;
        let day_row_offset: u16 = if has_month_row { 1 } else { 0 };
        if y < layout.inner.y + day_row_offset {
            return;
        }
        let day_row = (y - layout.inner.y - day_row_offset) as usize;
        if day_row >= 7 {
            return;
        }

        let start_x = layout
            .inner
            .x
            .saturating_add(layout.label_w)
            .saturating_add(layout.grid_pad);
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
            if rel_x < w {
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

        if date > today {
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
        self.overview_heatmap_flash_time = Some(std::time::Instant::now());
    }

    fn select_model_timeline_day_from_mouse(&mut self, x: u16, y: u16) {
        let Some(layout) = self.model_timeline_layout else {
            return;
        };
        if y < layout.chart_y || y >= layout.chart_y + layout.chart_h {
            return;
        }
        if x < layout.inner.x {
            return;
        }

        let rel_x = x - layout.inner.x;
        let col = (rel_x / layout.bar_w) as usize;
        if col >= layout.bars {
            return;
        }

        let day = layout.start_date + chrono::Duration::days(col as i64 * layout.bucket_days);
        self.model_timeline_selected_day = Some(day.format("%Y-%m-%d").to_string());
        self.model_timeline_flash_time = Some(std::time::Instant::now());
    }

    fn render(&mut self, frame: &mut Frame) {
        let colors = self.theme.colors();

        // Set terminal background to match theme
        frame.render_widget(
            ratatui::widgets::Block::default()
                .style(ratatui::style::Style::default().bg(colors.bg_primary)),
            frame.area(),
        );

        if self.modal.open {
            self.cached_rects = PanelRects::default();

            let session_id = self
                .session_list_state
                .selected()
                .and_then(|i| self.session_list.get(i).map(|s| s.id.clone()));
            if let Some(id) = session_id {
                if let Some(session) = self.session_list.iter().find(|s| s.id == id) {
                    self.modal
                        .render(frame, frame.area(), session, &self.session_titles, colors);
                }
            }
        } else {
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
        let colors = self.theme.colors();
        let k = Style::default()
            .fg(colors.text_secondary)
            .add_modifier(Modifier::BOLD);
        let t = Style::default().fg(colors.text_muted);
        let sep = Span::styled(" │ ", Style::default().fg(colors.border_muted));

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
            let show_enter = !((self.focus == Focus::Left && self.left_panel == LeftPanel::Stats)
                || (self.right_panel == RightPanel::Detail
                    && (self.left_panel == LeftPanel::Stats
                        || self.left_panel == LeftPanel::Models)));
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
            .style(Style::default().bg(colors.bg_primary))
            .alignment(Alignment::Center);
        frame.render_widget(status_bar, area);
    }

    fn render_left_panel(&mut self, frame: &mut Frame, area: Rect) {
        let colors = self.theme.colors();
        let is_focused = self.focus == Focus::Left;
        let border_style = if is_focused {
            Style::default()
                .fg(colors.border_focus)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colors.border_default)
        };

        let stats_height = 6;
        let remaining = area.height.saturating_sub(stats_height);
        let model_height = if remaining > 18 {
            let extra = remaining - 18;
            (6 + extra / 3).min(self.model_usage.len() as u16 + 2)
        } else {
            6.min(self.model_usage.len() as u16 + 2)
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(stats_height),
                Constraint::Min(12),
                Constraint::Length(model_height),
            ])
            .split(area);

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

    fn render_right_panel(&mut self, frame: &mut Frame, area: Rect) {
        let colors = self.theme.colors();
        let is_focused = self.focus == Focus::Right;
        let border_style = if is_focused {
            Style::default()
                .fg(colors.border_focus)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colors.border_default)
        };

        match self.left_panel {
            LeftPanel::Stats => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(8),
                        Constraint::Length(11),
                        Constraint::Min(0),
                    ])
                    .split(area);

                let bottom_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[2]);

                self.cached_rects.detail = Some(chunks[0]);
                self.cached_rects.activity = Some(chunks[1]);
                self.cached_rects.tools = Some(bottom_chunks[0]);
                self.cached_rects.list = Some(bottom_chunks[1]);

                let overview_hl = is_focused && self.right_panel == RightPanel::Detail;
                self.render_overview_panel(frame, chunks[0], border_style, overview_hl);

                let activity_hl = is_focused && self.right_panel == RightPanel::Activity;
                self.render_activity_heatmap(frame, chunks[1], border_style, activity_hl);

                let tools_hl = is_focused && self.right_panel == RightPanel::Tools;
                self.render_overview_tools_panel(
                    frame,
                    bottom_chunks[0],
                    if tools_hl {
                        border_style
                    } else {
                        Style::default().fg(colors.border_muted)
                    },
                    tools_hl,
                );

                let projects_hl = is_focused && self.right_panel == RightPanel::List;
                self.render_projects_panel(
                    frame,
                    bottom_chunks[1],
                    if projects_hl {
                        border_style
                    } else {
                        Style::default().fg(colors.border_muted)
                    },
                    projects_hl,
                );
            }
            LeftPanel::Days => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(10), Constraint::Min(0)])
                    .split(area);

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
                        Style::default().fg(colors.border_muted)
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
                        Style::default().fg(colors.border_muted)
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
}

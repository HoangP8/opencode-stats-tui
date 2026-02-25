//! Session modal for detailed session view

use crate::cost::estimate_cost;
use crate::stats::{
    format_active_duration, format_number, load_session_details, ChatMessage, MessageContent,
    SessionDetails, SessionStat,
};
use crate::theme::{FixedColors, ThemeColors};
use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SCROLL_INCREMENT: u16 = 3;

#[derive(Default, Clone, Copy)]
struct ModalRects {
    info: Option<Rect>,
    chat: Option<Rect>,
    info_max_scroll: u16,
}

pub struct SessionModal {
    pub open: bool,
    pub session_details: Option<SessionDetails>,
    pub current_session: Option<SessionStat>,
    pub info_scroll: u16,
    pub chat_messages: Arc<Vec<ChatMessage>>,
    pub chat_scroll: u16,
    pub chat_max_scroll: u16,
    pub selected_column: ModalColumn,
    cached_rects: ModalRects,
    pub expanded_agents: FxHashSet<Box<str>>,
    pub expanded_messages: FxHashSet<usize>,
    pub expanded_tools: FxHashSet<Box<str>>,
    chat_click_targets: Vec<(u16, ChatClickTarget)>,
    expanded_info_agents: FxHashSet<Box<str>>,
    expanded_info_models: FxHashSet<Box<str>>,
    info_click_targets: Vec<(u16, InfoClickTarget)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatClickTarget {
    Agent(Box<str>),
    Message(usize),
    ToolBox(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoClickTarget {
    Agent(Box<str>),
    Model(Box<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalColumn {
    Info,
    Chat,
}

// ============================================================================
// SessionModal Implementation
// ============================================================================

impl SessionModal {
    #[inline]
    fn reset_expansion_state(&mut self) {
        self.expanded_agents.clear();
        self.expanded_messages.clear();
        self.expanded_tools.clear();
        self.chat_click_targets.clear();
        self.expanded_info_agents.clear();
        self.expanded_info_models.clear();
        self.info_click_targets.clear();
    }

    #[inline]
    pub fn new() -> Self {
        Self {
            open: false,
            session_details: None,
            current_session: None,
            info_scroll: 0,
            chat_messages: Arc::new(Vec::new()),
            chat_scroll: 0,
            chat_max_scroll: 0,
            selected_column: ModalColumn::Info,
            cached_rects: ModalRects::default(),
            expanded_agents: FxHashSet::default(),
            expanded_messages: FxHashSet::default(),
            expanded_tools: FxHashSet::default(),
            chat_click_targets: Vec::new(),
            expanded_info_agents: FxHashSet::default(),
            expanded_info_models: FxHashSet::default(),
            info_click_targets: Vec::new(),
        }
    }

    pub fn open_session(
        &mut self,
        session_id: &str,
        chat_messages: Arc<Vec<ChatMessage>>,
        session_stat: &crate::stats::SessionStat,
        files: Option<&[std::path::PathBuf]>,
        day_filter: Option<&str>,
    ) {
        let details = load_session_details(session_id, files, day_filter);
        self.session_details = Some(details);
        self.current_session = Some(session_stat.clone());
        self.chat_messages = chat_messages;
        self.chat_scroll = 0;
        self.info_scroll = 0;
        self.chat_max_scroll = 0; // Will be calculated during render
        self.open = true;
        self.selected_column = ModalColumn::Info;
        self.reset_expansion_state();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.session_details = None;
        self.current_session = None;
        self.chat_messages = Arc::new(Vec::new());
        self.chat_scroll = 0;
        self.info_scroll = 0;
        self.chat_max_scroll = 0;
        self.selected_column = ModalColumn::Info;
        self.cached_rects = ModalRects::default();
        self.reset_expansion_state();
    }

    pub fn handle_key_event(&mut self, key: KeyCode, _area_height: u16) -> bool {
        if !self.open {
            return false;
        }

        let info_max = self.cached_rects.info_max_scroll;

        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.close();
                true
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.selected_column = ModalColumn::Info;
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.selected_column = ModalColumn::Chat;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = self.info_scroll.saturating_sub(1);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = self.chat_scroll.saturating_sub(1);
                    }
                }
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = (self.info_scroll + 1).min(info_max);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = (self.chat_scroll + 1).min(self.chat_max_scroll);
                    }
                }
                true
            }
            KeyCode::PageUp => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = self.info_scroll.saturating_sub(10);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = self.chat_scroll.saturating_sub(10);
                    }
                }
                true
            }
            KeyCode::PageDown => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = (self.info_scroll + 10).min(info_max);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = (self.chat_scroll + 10).min(self.chat_max_scroll);
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent, _area: Rect) -> bool {
        if !self.open {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let (x, y) = (mouse.column, mouse.row);
                if Self::contains_point(self.cached_rects.info, x, y) {
                    self.selected_column = ModalColumn::Info;
                } else if Self::contains_point(self.cached_rects.chat, x, y) {
                    self.selected_column = ModalColumn::Chat;
                }

                let info_max = self.cached_rects.info_max_scroll;
                match self.selected_column {
                    ModalColumn::Info => {
                        if mouse.kind == MouseEventKind::ScrollUp {
                            self.info_scroll = self.info_scroll.saturating_sub(SCROLL_INCREMENT);
                        } else {
                            self.info_scroll = self
                                .info_scroll
                                .saturating_add(SCROLL_INCREMENT)
                                .min(info_max);
                        }
                    }
                    ModalColumn::Chat => {
                        if mouse.kind == MouseEventKind::ScrollUp {
                            self.chat_scroll = self.chat_scroll.saturating_sub(SCROLL_INCREMENT);
                        } else {
                            self.chat_scroll = self
                                .chat_scroll
                                .saturating_add(SCROLL_INCREMENT)
                                .min(self.chat_max_scroll);
                        }
                    }
                }
                true
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                let (x, y) = (mouse.column, mouse.row);
                if Self::contains_point(self.cached_rects.info, x, y) {
                    self.selected_column = ModalColumn::Info;
                    if let Some(info_rect) = self.cached_rects.info {
                        let content_y =
                            (y.saturating_sub(info_rect.y + 1)) as u16 + self.info_scroll;
                        if let Ok(pos) = self
                            .info_click_targets
                            .binary_search_by_key(&content_y, |(line_idx, _)| *line_idx)
                        {
                            let target = &self.info_click_targets[pos].1;
                            match target {
                                InfoClickTarget::Agent(name) => {
                                    let name = name.clone();
                                    if !self.expanded_info_agents.remove(&name) {
                                        self.expanded_info_agents.insert(name);
                                    }
                                }
                                InfoClickTarget::Model(name) => {
                                    let name = name.clone();
                                    if !self.expanded_info_models.remove(&name) {
                                        self.expanded_info_models.insert(name);
                                    }
                                }
                            }
                        }
                    }
                    return true;
                }
                if Self::contains_point(self.cached_rects.chat, x, y) {
                    self.selected_column = ModalColumn::Chat;
                    if let Some(chat_rect) = self.cached_rects.chat {
                        let content_y =
                            (y.saturating_sub(chat_rect.y + 1)) as u16 + self.chat_scroll;
                        // Binary search since targets are sorted by line index
                        if let Ok(pos) = self
                            .chat_click_targets
                            .binary_search_by_key(&content_y, |(line_idx, _)| *line_idx)
                        {
                            let target = &self.chat_click_targets[pos].1;
                            match target {
                                ChatClickTarget::Agent(name) => {
                                    let name = name.clone();
                                    if !self.expanded_agents.remove(&name) {
                                        self.expanded_agents.insert(name);
                                    }
                                }
                                ChatClickTarget::Message(idx) => {
                                    let idx = *idx;
                                    if !self.expanded_messages.remove(&idx) {
                                        self.expanded_messages.insert(idx);
                                    }
                                }
                                ChatClickTarget::ToolBox(id) => {
                                    let id = id.clone();
                                    if !self.expanded_tools.remove(&id) {
                                        self.expanded_tools.insert(id);
                                    }
                                }
                            }
                            return true;
                        }
                    }
                    return true;
                }
                false
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                self.close();
                true
            }
            _ => false,
        }
    }

    #[inline(always)]
    fn contains_point(rect: Option<Rect>, x: u16, y: u16) -> bool {
        rect.is_some_and(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        session: &SessionStat,
        session_titles: &FxHashMap<Box<str>, String>,
        colors: ThemeColors,
    ) {
        let modal_block = Block::default().style(Style::default().bg(colors.bg_primary));
        frame.render_widget(modal_block, area);
        let modal_area = area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(modal_area);
        let content_area = chunks[0];
        let instruction_area = chunks[1];
        let column_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(content_area);

        self.cached_rects.info = Some(column_chunks[0]);
        self.cached_rects.chat = Some(column_chunks[1]);

        let (info_border_style, chat_border_style) = match self.selected_column {
            ModalColumn::Info => (
                Style::default()
                    .fg(colors.border_focus)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(colors.border_muted),
            ),
            ModalColumn::Chat => (
                Style::default().fg(colors.border_muted),
                Style::default()
                    .fg(colors.border_focus)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        self.render_modal_info(
            frame,
            column_chunks[0],
            session,
            session_titles,
            info_border_style,
            colors,
        );
        self.render_modal_chat(frame, column_chunks[1], chat_border_style, colors);
        self.render_instructions(frame, instruction_area, colors);
    }

    // ========================================================================
    // Info Panel Rendering
    // ========================================================================

    fn render_modal_info(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        session: &SessionStat,
        session_titles: &FxHashMap<Box<str>, String>,
        border_style: Style,
        colors: ThemeColors,
    ) {
        let fixed = FixedColors::DEFAULT;
        self.info_click_targets.clear();
        let mut lines = Vec::with_capacity(50);
        let device = crate::device::get_device_info();
        let device_display = device.display_name();
        let title = session_titles
            .get(&session.id)
            .map(|t| t.strip_prefix("New session - ").unwrap_or(t))
            .unwrap_or("Untitled");
        lines.push(Line::from(""));
        let project = session.path_root.as_ref();
        if !project.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  INFO",
                Style::default()
                    .fg(colors.accent_yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            let prefix_len: usize = 14;
            let inner_width = area.width.saturating_sub(2) as usize;
            let value_width = inner_width.saturating_sub(prefix_len);
            let wrapped_title =
                wrap_text_with_indent(title, value_width.max(1), value_width.max(1));
            for (i, line) in wrapped_title.into_iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::raw("    Title:    "),
                        Span::styled(line, Style::default().fg(colors.text_primary)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(prefix_len)),
                        Span::styled(line, Style::default().fg(colors.text_primary)),
                    ]));
                }
            }
            lines.push(Line::from(vec![
                Span::raw("    Project:  "),
                Span::styled(
                    safe_truncate_plain(project, value_width),
                    Style::default().fg(colors.text_primary),
                ),
            ]));
            if let Some(branch) = detect_git_branch(project) {
                let branch_display = safe_truncate_plain(&branch, value_width).into_owned();
                lines.push(Line::from(vec![
                    Span::raw("    Branch:   "),
                    Span::styled(branch_display, Style::default().fg(colors.accent_cyan)),
                ]));
            }
            {
                let type_color = if device.kind == "server" {
                    colors.accent_orange
                } else {
                    colors.accent_cyan
                };
                lines.push(Line::from(vec![
                    Span::raw("    Host:     "),
                    Span::styled(device.display_label(), Style::default().fg(type_color)),
                    Span::raw(" | "),
                    Span::styled(
                        safe_truncate_plain(&device_display, value_width),
                        Style::default().fg(type_color),
                    ),
                ]));
            }
            let active_dur = format_active_duration(session.active_duration_ms);
            lines.push(Line::from(vec![
                Span::raw("    Duration: "),
                Span::styled(active_dur, Style::default().fg(colors.accent_cyan)),
            ]));
            {
                let mut all_models: Vec<&str> = session.models.iter().map(|m| m.as_ref()).collect();
                all_models.sort_unstable();
                for (i, model) in all_models.iter().enumerate() {
                    let label = if i == 0 {
                        "    Models:   "
                    } else {
                        "              "
                    };
                    lines.push(Line::from(vec![
                        Span::raw(label),
                        Span::styled(
                            safe_truncate_plain(model, value_width),
                            Style::default().fg(colors.accent_magenta),
                        ),
                    ]));
                }
            }
            lines.push(Line::from(""));
        }
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(colors.border_muted),
        )]));
        lines.push(Line::from(""));
        if !session.agents.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                format!("  AGENTS ({})", session.agents.len()),
                Style::default()
                    .fg(colors.accent_yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));
            for agent in &session.agents {
                let name_color = if agent.is_main {
                    colors.accent_cyan
                } else {
                    colors.accent_orange
                };
                let is_expanded = self.expanded_info_agents.contains(&agent.name);
                let toggle_label = if is_expanded {
                    " ▾ collapse"
                } else {
                    " ▸ expand"
                };
                let mut model_list: Vec<&str> = agent.models.iter().map(|m| m.as_ref()).collect();
                model_list.sort_unstable();
                let model_suffix = if !model_list.is_empty() {
                    format!(" ({})", model_list.join(", "))
                } else {
                    String::new()
                };
                let prefix_len = 6; // "    ● "
                let toggle_len = toggle_label.chars().count();
                let max_content =
                    (area.width.saturating_sub(2) as usize).saturating_sub(prefix_len + toggle_len);
                let agent_name = &agent.name;
                let full_text = format!("{}{}", agent_name, model_suffix);
                let display = safe_truncate_plain(&full_text, max_content);
                let name_len = agent_name.chars().count();
                self.info_click_targets.push((
                    lines.len() as u16,
                    InfoClickTarget::Agent(agent.name.clone()),
                ));
                if display.chars().count() <= name_len {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled("● ", Style::default().fg(name_color)),
                        Span::styled(
                            display.into_owned(),
                            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(toggle_label, Style::default().fg(colors.text_muted)),
                    ]));
                } else {
                    let name_part: String = display.chars().take(name_len).collect();
                    let suffix_part: String = display.chars().skip(name_len).collect();
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled("● ", Style::default().fg(name_color)),
                        Span::styled(
                            name_part,
                            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(suffix_part, Style::default().fg(colors.accent_magenta)),
                        Span::styled(toggle_label, Style::default().fg(colors.text_muted)),
                    ]));
                }
                if is_expanded {
                    let agent_active_dur = format_active_duration(agent.active_duration_ms);
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("      {:<9}", "Duration"),
                            Style::default().fg(colors.text_muted),
                        ),
                        Span::styled(
                            format!("{:>8}", agent_active_dur),
                            Style::default().fg(colors.accent_cyan),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("      {:<9}", "Messages"),
                            Style::default().fg(colors.text_muted),
                        ),
                        Span::styled(
                            format!("{:>8}", agent.messages),
                            Style::default().fg(colors.text_primary),
                        ),
                    ]));
                    let token_rows = [
                        (
                            "Input",
                            format_number(agent.tokens.input),
                            colors.token_input(),
                        ),
                        (
                            "Output",
                            format_number(agent.tokens.output),
                            colors.token_output(),
                        ),
                        (
                            "Thinking",
                            format_number(agent.tokens.reasoning),
                            colors.thinking(),
                        ),
                        (
                            "Cache R",
                            format_number(agent.tokens.cache_read),
                            colors.cost(),
                        ),
                        (
                            "Cache W",
                            format_number(agent.tokens.cache_write),
                            colors.cost(),
                        ),
                    ];
                    for (label, value, color) in &token_rows {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("      {:<9}", label),
                                Style::default().fg(*color),
                            ),
                            Span::styled(
                                format!("{:>8}", value),
                                Style::default().fg(colors.text_primary),
                            ),
                        ]));
                    }
                }
                lines.push(Line::from(""));
            }
        }
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(colors.border_muted),
        )]));
        lines.push(Line::from(""));
        let details = self.session_details.as_ref();
        if let Some(d) = details {
            if !d.model_stats.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!("  MODELS ({})", d.model_stats.len()),
                    Style::default()
                        .fg(colors.accent_yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                for model in &d.model_stats {
                    let is_expanded = self.expanded_info_models.contains(&model.name);
                    let toggle_label = if is_expanded {
                        " (▾ collapse)"
                    } else {
                        " (▸ expand)"
                    };
                    let prefix_len = 6; // "    ● "
                    let toggle_len = toggle_label.chars().count();
                    let model_max = (area.width.saturating_sub(2) as usize)
                        .saturating_sub(prefix_len + toggle_len);
                    self.info_click_targets.push((
                        lines.len() as u16,
                        InfoClickTarget::Model(model.name.clone()),
                    ));
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled("● ", Style::default().fg(colors.accent_magenta)),
                        Span::styled(
                            safe_truncate_plain(&model.name, model_max),
                            Style::default()
                                .fg(colors.accent_magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(toggle_label, Style::default().fg(colors.text_muted)),
                    ]));
                    if is_expanded {
                        let left_labels = [
                            (
                                "Input",
                                format_number(model.tokens.input),
                                colors.token_input(),
                            ),
                            (
                                "Output",
                                format_number(model.tokens.output),
                                colors.token_output(),
                            ),
                            (
                                "Thinking",
                                format_number(model.tokens.reasoning),
                                colors.thinking(),
                            ),
                            (
                                "Cache R",
                                format_number(model.tokens.cache_read),
                                colors.cost(),
                            ),
                            (
                                "Cache W",
                                format_number(model.tokens.cache_write),
                                colors.cost(),
                            ),
                        ];
                        let responses = model.messages.saturating_sub(model.prompts);
                        let model_cost = model.cost;
                        let model_est =
                            estimate_cost(&model.name, &model.tokens).unwrap_or(model_cost);
                        let model_savings = model_est - model_cost;
                        let right_labels = [
                            ("Prompts", model.prompts.to_string(), colors.info),
                            ("Responses", responses.to_string(), colors.success),
                            ("Cost", format!("${:.2}", model_cost), colors.text_primary),
                            ("Est. Cost", format!("${:.2}", model_est), colors.text_muted),
                            (
                                "Savings",
                                format!("${:.2}", model_savings),
                                if model_savings > 0.0 {
                                    colors.success
                                } else {
                                    colors.text_muted
                                },
                            ),
                        ];
                        let inner_width = area.width.saturating_sub(2) as usize;
                        let show_right_column = inner_width >= 48;
                        for i in 0..5 {
                            let mut spans = Vec::with_capacity(7);
                            spans.push(Span::raw("      "));
                            if i < left_labels.len() {
                                let (label, value, color) = &left_labels[i];
                                spans.push(Span::styled(
                                    format!("{:<9}", label),
                                    Style::default().fg(*color),
                                ));
                                spans.push(Span::styled(
                                    format!("{:>8}", value),
                                    Style::default().fg(colors.text_primary),
                                ));
                            } else if show_right_column {
                                spans.push(Span::raw(" ".repeat(17)));
                            }
                            if show_right_column {
                                spans.push(Span::styled(
                                    "   │   ",
                                    Style::default().fg(colors.border_muted),
                                ));
                                if i < right_labels.len() {
                                    let (label, value, color) = &right_labels[i];
                                    spans.push(Span::styled(
                                        format!("{:<9}", label),
                                        Style::default().fg(*color),
                                    ));
                                    spans.push(Span::styled(
                                        format!("{:>8}", value),
                                        Style::default().fg(colors.text_primary),
                                    ));
                                }
                            }
                            lines.push(Line::from(spans));
                        }
                    }
                    lines.push(Line::from(""));
                }
            }
        }
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(colors.border_muted),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "  TOTAL USAGE",
            Style::default()
                .fg(colors.accent_yellow)
                .add_modifier(Modifier::BOLD),
        )]));
        let total_labels: Vec<(&str, String, Color)> = vec![
            (
                "Tokens",
                format_number(session.tokens.total()),
                colors.text_primary,
            ),
            ("Prompts", session.prompts.to_string(), colors.info),
            (
                "Responses",
                session.messages.saturating_sub(session.prompts).to_string(),
                colors.success,
            ),
            ("Cost", format!("${:.2}", session.cost), colors.text_primary),
        ];
        for (label, value, color) in &total_labels {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("      {:<9}", label),
                    Style::default().fg(colors.text_muted),
                ),
                Span::styled(
                    format!("{:>8}", value),
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        let total_cost = session.cost;
        // Use OpenRouter pricing for estimated cost (what you would pay at standard rates)
        let est_cost = session
            .models
            .iter()
            .next()
            .and_then(|m| estimate_cost(m, &session.tokens))
            .unwrap_or(total_cost);
        let savings = est_cost - total_cost;
        let (savings_text, savings_color) = if savings < 0.0 {
            (format!("-${:.2}", savings.abs()), colors.error)
        } else {
            (
                format!("${:.2}", savings),
                if savings > 0.0 {
                    colors.success
                } else {
                    colors.text_muted
                },
            )
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("      {:<9}", "Savings"),
                Style::default().fg(colors.text_muted),
            ),
            Span::styled(
                format!("{:>8}", savings_text),
                Style::default()
                    .fg(savings_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(colors.border_muted),
        )]));
        lines.push(Line::from(""));
        if !session.file_diffs.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  FILE CHANGES",
                Style::default()
                    .fg(colors.accent_yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            let max_val_len = 6usize;
            let needed_width = 4 + 10 + 1 + 2 + (max_val_len + 1) * 2 + 3 + 5;
            let max_avail = (area.width.saturating_sub(2) as usize).saturating_sub(needed_width);
            let longest_path = session
                .file_diffs
                .iter()
                .map(|f| f.path.chars().count())
                .max()
                .unwrap_or(0);
            let path_display_width = longest_path.max(8).min(35).min(max_avail);
            let show_diffs = (area.width.saturating_sub(2) as usize) >= needed_width;
            for f in &session.file_diffs {
                let status_text = match f.status.as_ref() {
                    "added" => "   added",
                    "modified" => "modified",
                    "deleted" => " deleted",
                    _ => " unknown",
                };
                let mut file_spans = vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("[{}]", status_text),
                        Style::default().fg(colors.text_muted),
                    ),
                ];
                if show_diffs {
                    file_spans.push(Span::raw(" "));
                    let short_path = if f.path.chars().count() > path_display_width {
                        format!(
                            "{}…",
                            f.path
                                .chars()
                                .take(path_display_width.saturating_sub(1))
                                .collect::<String>()
                        )
                    } else {
                        format!("{:<width$}", f.path, width = path_display_width)
                    };
                    file_spans.extend(vec![
                        Span::styled(short_path, Style::default().fg(colors.text_primary)),
                        Span::raw("  "),
                        Span::styled(
                            format!(
                                "{:>width$}",
                                format!("+{}", format_number(f.additions)),
                                width = max_val_len + 1
                            ),
                            Style::default().fg(fixed.diff_add),
                        ),
                        Span::styled(" │ ", Style::default().fg(colors.border_muted)),
                        Span::styled(
                            format!(
                                "{:>width$}",
                                format!("-{}", format_number(f.deletions)),
                                width = max_val_len + 1
                            ),
                            Style::default().fg(fixed.diff_remove),
                        ),
                    ]);
                }
                lines.push(Line::from(file_spans));
            }
            if show_diffs {
                let diff_start = 4 + 10 + 1 + path_display_width + 2;
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(diff_start)),
                    Span::styled(
                        format!(
                            "{}─┼─{}",
                            "─".repeat(max_val_len + 1),
                            "─".repeat(max_val_len + 1)
                        ),
                        Style::default().fg(colors.border_muted),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(diff_start)),
                    Span::styled(
                        format!(
                            "{:>width$}",
                            format!("+{}", format_number(session.diffs.additions)),
                            width = max_val_len + 1
                        ),
                        Style::default()
                            .fg(fixed.diff_add)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" │ ", Style::default().fg(colors.border_muted)),
                    Span::styled(
                        format!(
                            "{:>width$}",
                            format!("-{}", format_number(session.diffs.deletions)),
                            width = max_val_len + 1
                        ),
                        Style::default()
                            .fg(fixed.diff_remove)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        } else {
            lines.push(Line::from(vec![Span::styled(
                "  FILE CHANGES ",
                Style::default()
                    .fg(colors.accent_yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "    NO FILE CHANGES",
                Style::default().fg(colors.text_muted),
            )]));
        }
        let inner_height = area.height.saturating_sub(2) as usize;
        let info_max_scroll = (lines.len().saturating_sub(inner_height)) as u16;
        self.cached_rects.info_max_scroll = info_max_scroll;
        self.info_scroll = self.info_scroll.min(info_max_scroll);
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(self.info_scroll as usize)
            .take(inner_height)
            .collect();
        let title_text = if session.is_continuation {
            if let Some(first_date) = &session.first_created_date {
                format!("  {}  [Continue from {}]  ", session.id, first_date)
            } else {
                format!("  {}  [Continued]  ", session.id)
            }
        } else {
            format!("  {}  ", session.id)
        };
        let title_color = border_style.fg.unwrap_or(colors.border_default);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(colors.bg_primary))
            .title(
                Line::from(Span::styled(
                    title_text,
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
        frame.render_widget(
            Paragraph::new(visible)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    // ========================================================================
    // Chat Panel Rendering
    // ========================================================================

    fn render_modal_chat(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        colors: ThemeColors,
    ) {
        let mut lines: Vec<Line> = Vec::with_capacity(self.chat_messages.len() * 10);
        let inner_w = area.width.saturating_sub(2) as usize;
        let box_w = inner_w.saturating_sub(2);

        // ── Phase 1: group messages into blocks ──
        enum ChatBlock {
            Single(usize),
            SubagentGroup(Vec<(Box<str>, Vec<usize>)>),
        }
        let msgs = &*self.chat_messages;
        let mut blocks: Vec<ChatBlock> = Vec::with_capacity(msgs.len());
        let mut i = 0;
        while i < msgs.len() {
            if msgs[i].is_subagent {
                let start = i;
                while i < msgs.len() && msgs[i].is_subagent {
                    i += 1;
                }
                let mut agent_order: Vec<Box<str>> = Vec::with_capacity(4);
                let mut agent_msgs: FxHashMap<Box<str>, Vec<usize>> = FxHashMap::default();
                for idx in start..i {
                    let name: Box<str> = msgs[idx]
                        .agent_label
                        .clone()
                        .unwrap_or_else(|| "subagent".into());
                    if let Some(indices) = agent_msgs.get_mut(&name) {
                        indices.push(idx);
                    } else {
                        agent_order.push(name.clone());
                        agent_msgs.insert(name, vec![idx]);
                    }
                }
                let groups: Vec<(Box<str>, Vec<usize>)> = agent_order
                    .into_iter()
                    .map(|n| (n.clone(), agent_msgs.remove(&n).unwrap_or_default()))
                    .collect();
                blocks.push(ChatBlock::SubagentGroup(groups));
            } else {
                blocks.push(ChatBlock::Single(i));
                i += 1;
            }
        }

        // ── Phase 2: render blocks ──
        self.chat_click_targets.clear();
        let mut user_count = 0usize;
        let mut agent_count = 0usize;
        for block in &blocks {
            match block {
                ChatBlock::Single(idx) => {
                    let msg = &msgs[*idx];
                    let is_expanded = self.expanded_messages.contains(idx);
                    self.chat_click_targets
                        .push((lines.len() as u16, ChatClickTarget::Message(*idx)));
                    if &*msg.role == "user" {
                        user_count += 1;
                        render_user_box(&mut lines, msg, box_w, is_expanded, user_count, colors);
                    } else {
                        agent_count += 1;
                        render_agent_box(
                            &mut lines,
                            msg,
                            box_w,
                            is_expanded,
                            agent_count,
                            *idx,
                            &mut self.chat_click_targets,
                            &self.expanded_tools,
                            colors,
                        );
                    }
                    lines.push(Line::from(""));
                }
                ChatBlock::SubagentGroup(agents) => {
                    let outer_color = colors.accent_orange;
                    let header = format!(" SUBAGENTS ({}) ", agents.len());
                    let dash_len = box_w.saturating_sub(header.chars().count() + 2);
                    lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled("┌", Style::default().fg(outer_color)),
                        Span::styled(
                            header,
                            Style::default()
                                .fg(colors.accent_yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("╌".repeat(dash_len), Style::default().fg(outer_color)),
                    ]));
                    for (ag_idx, (agent_name, msg_indices)) in agents.iter().enumerate() {
                        let ag_color = subagent_color(ag_idx, colors);
                        let ag_dim = dim_color(ag_color, &colors);
                        let is_expanded = self.expanded_agents.contains(agent_name);
                        let toggle_label = if is_expanded {
                            "▾ collapse"
                        } else {
                            "▸ expand"
                        };
                        let (total_tools, tool_stats) = aggregate_tools_in_group(msgs, msg_indices);
                        let card_w = box_w.saturating_sub(4);
                        self.chat_click_targets.push((
                            lines.len() as u16,
                            ChatClickTarget::Agent(agent_name.clone()),
                        ));
                        let model_str = msg_indices
                            .first()
                            .and_then(|&mi| msgs[mi].model.as_deref())
                            .unwrap_or("");
                        let card_label = if model_str.is_empty() {
                            format!(" {} ", agent_name)
                        } else {
                            format!(" {} ({}) ", agent_name, model_str)
                        };
                        let card_dash = card_w.saturating_sub(
                            card_label.chars().count() + 2 + toggle_label.len() + 1,
                        );
                        lines.push(Line::from(vec![
                            Span::raw("   "),
                            Span::styled("┌", Style::default().fg(ag_dim)),
                            Span::styled(
                                card_label,
                                Style::default().fg(ag_color).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled("╌".repeat(card_dash), Style::default().fg(ag_dim)),
                            Span::styled(
                                format!(" {}", toggle_label),
                                Style::default().fg(colors.text_muted),
                            ),
                        ]));
                        if is_expanded {
                            let mut all_repr_text = String::new();
                            let mut all_task_text = String::new();
                            for &mi in msg_indices {
                                let m = &msgs[mi];
                                let is_user = &*m.role == "user";
                                for part in &m.parts {
                                    if let MessageContent::Text(t) = part {
                                        let text = if is_user {
                                            filter_user_text(t)
                                        } else {
                                            t.to_string()
                                        };
                                        if !text.trim().is_empty() {
                                            if is_user {
                                                if !all_task_text.is_empty() {
                                                    all_task_text.push('\n');
                                                }
                                                all_task_text.push_str(&text);
                                            } else {
                                                if !all_repr_text.is_empty() {
                                                    all_repr_text.push('\n');
                                                }
                                                all_repr_text.push_str(&text);
                                            }
                                        }
                                    }
                                }
                            }
                            let wrap_w = card_w.saturating_sub(8);
                            
                            if !all_task_text.is_empty() {
                                for (i, line) in
                                    wrap_text_plain(&all_task_text, wrap_w).iter().enumerate()
                                {
                                    let tag = if i == 0 { "TASK:" } else { "     " };
                                    lines.push(Line::from(vec![
                                        Span::styled("   ┊ ", Style::default().fg(ag_dim)),
                                        Span::styled(tag, Style::default().fg(colors.info)),
                                        Span::raw(" "),
                                        Span::styled(
                                            line.clone(),
                                            Style::default().fg(colors.text_secondary),
                                        ),
                                    ]));
                                }
                            }

                            if !all_repr_text.is_empty() {
                                for (i, line) in
                                    wrap_text_plain(&all_repr_text, wrap_w).iter().enumerate()
                                {
                                    let tag = if i == 0 { "REPR:" } else { "     " };
                                    lines.push(Line::from(vec![
                                        Span::styled("   ┊ ", Style::default().fg(ag_dim)),
                                        Span::styled(tag, Style::default().fg(ag_color)),
                                        Span::raw(" "),
                                        Span::styled(
                                            line.clone(),
                                            Style::default().fg(colors.text_secondary),
                                        ),
                                    ]));
                                }
                            }
                            if total_tools > 0 {
                                lines.push(Line::from(vec![Span::styled(
                                    "   ┊ ",
                                    Style::default().fg(ag_dim),
                                )]));
                                let target_id =
                                    format!("tools:agent:{}", agent_name).into_boxed_str();
                                let tools_expanded = self.expanded_tools.contains(&target_id);
                                render_tool_stats_box(
                                    &mut lines,
                                    "   ┊  ",
                                    ag_dim,
                                    card_w,
                                    total_tools,
                                    &tool_stats,
                                    tools_expanded,
                                    &mut self.chat_click_targets,
                                    target_id,
                                    colors,
                                );
                            }
                        } else {
                            // Collapsed: 300-char preview with early exit
                            const COLLAPSE_LIMIT: usize = 300;
                            let mut preview = String::new();
                            'collect: for &mi in msg_indices {
                                for part in &msgs[mi].parts {
                                    if let MessageContent::Text(t) = part {
                                        let filtered = filter_user_text(t);
                                        if !filtered.trim().is_empty() {
                                            if !preview.is_empty() {
                                                preview.push(' ');
                                            }
                                            preview.push_str(filtered.trim());
                                            if preview.len() >= COLLAPSE_LIMIT {
                                                break 'collect;
                                            }
                                        }
                                    }
                                }
                            }
                            if !preview.is_empty() {
                                // Truncate cleanly at char boundary
                                let truncated = preview.len() > COLLAPSE_LIMIT;
                                if truncated {
                                    let byte_pos = preview
                                        .char_indices()
                                        .take_while(|(i, _)| *i < COLLAPSE_LIMIT)
                                        .last()
                                        .map(|(i, c)| i + c.len_utf8())
                                        .unwrap_or(preview.len());
                                    preview.truncate(byte_pos);
                                    preview.push('…');
                                }
                                for line in wrap_text_plain(&preview, card_w.saturating_sub(8)) {
                                    lines.push(Line::from(vec![
                                        Span::styled("   ┊  ", Style::default().fg(ag_dim)),
                                        Span::styled(line, Style::default().fg(colors.text_muted)),
                                    ]));
                                }
                            }
                            lines.push(Line::from(vec![
                                Span::styled("   ┊  ", Style::default().fg(ag_dim)),
                                Span::styled(
                                    format!("tools: {}", total_tools),
                                    Style::default().fg(colors.text_muted),
                                ),
                            ]));
                            if total_tools > 0 {
                                let target_id =
                                    format!("tools:agent:{}", agent_name).into_boxed_str();
                                let tools_expanded = self.expanded_tools.contains(&target_id);
                                render_tool_stats_box(
                                    &mut lines,
                                    "   ┊  ",
                                    ag_dim,
                                    card_w,
                                    total_tools,
                                    &tool_stats,
                                    tools_expanded,
                                    &mut self.chat_click_targets,
                                    target_id,
                                    colors,
                                );
                            }
                        }
                        lines.push(Line::from(vec![
                            Span::raw("   "),
                            Span::styled(
                                format!("└{}", "╌".repeat(card_w.saturating_sub(1))),
                                Style::default().fg(ag_dim),
                            ),
                        ]));
                    }
                    lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            format!("└{}", "╌".repeat(box_w.saturating_sub(1))),
                            Style::default().fg(outer_color),
                        ),
                    ]));
                    lines.push(Line::from(""));
                }
            }
        }
        let inner_h = area.height.saturating_sub(2) as usize;
        self.chat_max_scroll = (lines.len().saturating_sub(inner_h)) as u16;
        self.chat_scroll = self.chat_scroll.min(self.chat_max_scroll);
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(self.chat_scroll as usize)
            .take(inner_h)
            .collect();
        let title_color = border_style.fg.unwrap_or(colors.border_default);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(colors.bg_primary))
            .title(
                Line::from(Span::styled(
                    " CHAT ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );
        frame.render_widget(
            Paragraph::new(visible)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    /// Render instruction bar at the bottom
    fn render_instructions(&self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        let k = Style::default()
            .fg(colors.text_secondary)
            .add_modifier(Modifier::BOLD);
        let t = Style::default().fg(colors.text_muted);
        let sep = Span::styled(" │ ", Style::default().fg(colors.border_muted));
        let instructions = vec![Line::from(vec![
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
        ])];
        let status_bar = Paragraph::new(instructions)
            .style(Style::default().bg(colors.bg_tertiary))
            .alignment(Alignment::Center);
        frame.render_widget(status_bar, area);
    }
}

struct ToolInvocation {
    file_path: Option<String>,
    input: Option<String>,
}

struct ToolStatsEntry {
    count: usize,
    invocations: Vec<ToolInvocation>,
}

fn aggregate_tools_in_group(
    msgs: &[ChatMessage],
    indices: &[usize],
) -> (usize, FxHashMap<String, ToolStatsEntry>) {
    let mut stats: FxHashMap<String, ToolStatsEntry> = FxHashMap::default();
    let mut total = 0;
    for &mi in indices {
        for part in &msgs[mi].parts {
            if let MessageContent::ToolCall(tc) = part {
                total += 1;
                let entry = stats
                    .entry(normalize_tool_name(&tc.name))
                    .or_insert_with(|| ToolStatsEntry {
                        count: 0,
                        invocations: Vec::new(),
                    });
                entry.count += 1;
                entry.invocations.push(ToolInvocation {
                    file_path: tc.file_path.as_deref().map(|s| s.to_string()),
                    input: tc.input.as_deref().map(|s| s.to_string()),
                });
            }
        }
    }
    (total, stats)
}

fn render_tool_stats_box<'a>(
    lines: &mut Vec<Line<'a>>,
    prefix: &'a str,
    dim_color: Color,
    card_w: usize,
    total_tools: usize,
    tool_stats: &FxHashMap<String, ToolStatsEntry>,
    is_expanded: bool,
    click_targets: &mut Vec<(u16, ChatClickTarget)>,
    target_id: Box<str>,
    colors: ThemeColors,
) {
    let inner_w = card_w.saturating_sub(6);
    let frame_color = colors.border_muted;
    let tool_header_color = colors.text_secondary;
    let tool_text_color = colors.text_muted;
    let toggle_label = if is_expanded {
        "▾ collapse"
    } else {
        "▸ expand"
    };

    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled(
            format!("┌{}┐", "─".repeat(inner_w)),
            Style::default().fg(frame_color),
        ),
    ]));

    click_targets.push((lines.len() as u16, ChatClickTarget::ToolBox(target_id)));

    let header = format!("⚙︎ tools used ({})", total_tools);
    let dash_len = inner_w.saturating_sub(header.chars().count() + toggle_label.len() + 3);
    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled("│ ", Style::default().fg(frame_color)),
        Span::styled(
            header,
            Style::default()
                .fg(tool_header_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(dash_len), Style::default().fg(frame_color)),
        Span::styled(
            format!(" {}", toggle_label),
            Style::default().fg(colors.text_muted),
        ),
        Span::styled(" │", Style::default().fg(frame_color)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled("├", Style::default().fg(frame_color)),
        Span::styled("─".repeat(inner_w), Style::default().fg(frame_color)),
        Span::styled("┤", Style::default().fg(frame_color)),
    ]));

    let mut tools: Vec<(&String, &ToolStatsEntry)> = tool_stats.iter().collect();
    tools.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));

    let tools_len = tools.len();
    for (idx, (name, entry)) in tools.iter().enumerate() {
        let tool_line = format!("  {} (x{})", name, entry.count);
        push_tool_line(
            lines,
            prefix,
            dim_color,
            frame_color,
            inner_w,
            &tool_line,
            tool_text_color,
        );

        if is_expanded {
            let is_read = *name == "read";
            let is_file_tool = matches!(name.as_str(), "read" | "edit" | "write" | "apply_patch");
            if is_file_tool {
                // Group invocations by file path
                let mut file_groups: Vec<(String, Vec<&ToolInvocation>)> = Vec::new();
                let mut file_order: Vec<String> = Vec::new();
                let mut file_map: FxHashMap<String, Vec<&ToolInvocation>> = FxHashMap::default();
                for inv in &entry.invocations {
                    let key = inv.file_path.as_deref().unwrap_or("(unknown)").to_string();
                    if let Some(list) = file_map.get_mut(&key) {
                        list.push(inv);
                    } else {
                        file_order.push(key.clone());
                        file_map.insert(key, vec![inv]);
                    }
                }
                for key in file_order {
                    let val = file_map.remove(&key).unwrap_or_default();
                    file_groups.push((key, val));
                }
                let group_count = file_groups.len();
                for (g_idx, (fp, invs)) in file_groups.iter().enumerate() {
                    let is_last_group = g_idx == group_count - 1;
                    let tree_char = if is_last_group { "└" } else { "├" };
                    let file_line = format!(
                        "    {} {} (x{})",
                        tree_char,
                        short_file_path(Some(fp)),
                        invs.len()
                    );
                    push_tool_line(
                        lines,
                        prefix,
                        dim_color,
                        frame_color,
                        inner_w,
                        &file_line,
                        tool_text_color,
                    );
                    if is_read {
                        // For read, show each invocation as a subtree (dedup'd)
                        let mut seen_details: FxHashSet<String> = FxHashSet::default();
                        let detail_max = inner_w.saturating_sub(1).saturating_sub(6);
                        let mut invs_deduped: Vec<&ToolInvocation> = Vec::new();
                        for inv in invs {
                            let detail = tool_invocation_secondary_detail(name, inv, detail_max)
                                .or_else(|| tool_invocation_primary_detail(name, inv, detail_max));
                            if let Some(d) = detail {
                                if seen_details.insert(d) {
                                    invs_deduped.push(inv);
                                }
                            }
                        }
                        let total_rendered = invs_deduped.len();
                        if total_rendered == 0 {
                            let fallback = if fp == "(unknown)" {
                                "      └ no file metadata"
                            } else {
                                "      └ no per-call detail"
                            };
                            push_tool_line(
                                lines,
                                prefix,
                                dim_color,
                                frame_color,
                                inner_w,
                                fallback,
                                tool_text_color,
                            );
                        } else {
                            for (idx, inv) in invs_deduped.iter().enumerate() {
                                let is_last = idx == total_rendered - 1;
                                let tree_char = if is_last { "└" } else { "├" };
                                let detail =
                                    tool_invocation_secondary_detail(name, inv, detail_max)
                                        .or_else(|| {
                                            tool_invocation_primary_detail(name, inv, detail_max)
                                        })
                                        .unwrap_or_else(|| "unknown".to_string());
                                let bullet_line = format!("      {} {}", tree_char, detail);
                                push_tool_line(
                                    lines,
                                    prefix,
                                    dim_color,
                                    frame_color,
                                    inner_w,
                                    &bullet_line,
                                    tool_text_color,
                                );
                            }
                        }
                    }
                }
            } else {
                // Non-file-centric tools: each invocation as a separate line (no bullet)
                let inv_count = entry.invocations.len();
                let desc_max = inner_w.saturating_sub(1).saturating_sub(4);
                for (inv_idx, inv) in entry.invocations.iter().enumerate() {
                    let is_last_inv = inv_idx == inv_count - 1;
                    let tree_char = if is_last_inv { "└" } else { "├" };
                    let description = tool_invocation_primary_detail(name, inv, desc_max)
                        .unwrap_or_else(|| format!("{} call", name));
                    let line_text = format!("    {} {}", tree_char, description);
                    push_tool_line(
                        lines,
                        prefix,
                        dim_color,
                        frame_color,
                        inner_w,
                        &line_text,
                        tool_text_color,
                    );
                }
            }
        }

        // Add padding between tools (except last) when expanded
        if is_expanded && idx < tools_len - 1 {
            push_tool_padding(lines, prefix, dim_color, frame_color, inner_w);
        }
    }

    // Final padding before bottom border
    if is_expanded {
        push_tool_padding(lines, prefix, dim_color, frame_color, inner_w);
    }

    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled(
            format!("└{}┘", "─".repeat(inner_w)),
            Style::default().fg(frame_color),
        ),
    ]));
}

fn tool_invocation_primary_detail(
    tool_name: &str,
    inv: &ToolInvocation,
    max_w: usize,
) -> Option<String> {
    if let Some(inp) = inv.input.as_deref() {
        if !inp.trim().is_empty() {
            let detail = format_tool_detail(tool_name, inp, max_w);
            if !detail.trim().is_empty() {
                return Some(detail);
            }
        }
    }
    inv.file_path
        .as_deref()
        .map(|fp| fit_display_width(&short_file_path(Some(fp)), max_w))
}

fn tool_invocation_secondary_detail(
    tool_name: &str,
    inv: &ToolInvocation,
    max_w: usize,
) -> Option<String> {
    if let Some(inp) = inv.input.as_deref() {
        if !inp.trim().is_empty() {
            let short = format_tool_invocation_short(tool_name, inp, max_w);
            if !short.trim().is_empty() {
                return Some(short);
            }
            let detail = format_tool_detail(tool_name, inp, max_w);
            if !detail.trim().is_empty() {
                return Some(detail);
            }
        }
    }
    // For read tool without explicit params, show (full file)
    if tool_name == "read" {
        if let Some(fp) = inv.file_path.as_deref() {
            // Check if input contains offset/limit parameters
            let has_range_params = inv
                .input
                .as_deref()
                .map_or(false, |inp| inp.contains("offset") || inp.contains("limit"));
            if !has_range_params {
                return Some(format!(
                    "{} (full file)",
                    safe_truncate_plain(&short_file_path(Some(fp)), max_w)
                ));
            }
        }
    }
    inv.file_path
        .as_deref()
        .map(|fp| safe_truncate_plain(&short_file_path(Some(fp)), max_w).into_owned())
}

/// Truncate text to max chars with "..." suffix
fn truncate_text(text: &str, max_chars: usize) -> Cow<'_, str> {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        Cow::Borrowed(trimmed)
    } else {
        let target = max_chars.saturating_sub(1);
        let byte_pos = trimmed
            .char_indices()
            .nth(target)
            .map(|(i, _)| i)
            .unwrap_or(trimmed.len());
        Cow::Owned(format!("{}…", &trimmed[..byte_pos]))
    }
}

/// Clean text and add line breaks after **section** headers for readability
fn clean_text_with_breaks(text: &str) -> String {
    // Replace **Title** with **Title**\n for better line breaks
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            // Find closing **
            let mut j = i + 2;
            while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '*') {
                j += 1;
            }
            if j + 1 < chars.len() {
                // Add the **title** and a newline after
                for k in i..=j + 1 {
                    result.push(chars[k]);
                }
                // Check if next char is not already newline
                if j + 2 < chars.len() && !chars[j + 2].is_whitespace() {
                    result.push('\n');
                }
                i = j + 2;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Filter out tool call annotations from user text to show only raw input
/// Removes lines like "Called the Read tool with..." and similar patterns
fn filter_user_text(text: &str) -> String {
    let mut result = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Skip tool call annotation lines
        if trimmed.starts_with("Called the ")
            || trimmed.starts_with("Used the ")
            || trimmed.starts_with("Ran the ")
            || trimmed.starts_with("Invoked the ")
            // Skip JSON-like parameter blocks
            || trimmed.starts_with("{\"")
            || trimmed.starts_with("{ \"")
            // Skip lines that are just tool output markers
            || trimmed.starts_with("Tool:")
            || trimmed.starts_with("Result:")
            // Skip path annotations like "<path>/foo/bar</path>"
            || (trimmed.starts_with("<path>") && trimmed.ends_with("</path>"))
            // Skip type annotations like "<type>file</type>"
            || (trimmed.starts_with("<type>") && trimmed.ends_with("</type>"))
            // Skip content markers
            || trimmed.starts_with("<content>")
            || trimmed == "```"
            || trimmed.starts_with("```json")
        {
            continue;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
    }
    result.trim().to_string()
}

fn render_user_box<'a>(
    lines: &mut Vec<Line<'a>>,
    msg: &ChatMessage,
    box_w: usize,
    is_expanded: bool,
    user_num: usize,
    colors: ThemeColors,
) {
    let border_color = colors.accent_cyan;
    let toggle_label = if is_expanded {
        "▾ collapse"
    } else {
        "▸ expand"
    };
    let label = format!(" USER #{} ", user_num);
    let dash_len = box_w.saturating_sub(label.chars().count() + 2 + toggle_label.len() + 1);
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("┌", Style::default().fg(border_color)),
        Span::styled(
            label,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("─".repeat(dash_len), Style::default().fg(border_color)),
        Span::styled(
            format!(" {}", toggle_label),
            Style::default().fg(colors.text_muted),
        ),
    ]));
    let content_w = box_w.saturating_sub(4);
    let all_text: String = msg
        .parts
        .iter()
        .filter_map(|p| {
            if let MessageContent::Text(t) = p {
                Some(filter_user_text(t).trim().to_string())
            } else {
                None
            }
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = clean_text_with_breaks(&all_text);
    if cleaned.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" │", Style::default().fg(border_color)),
            Span::styled("  (empty)", Style::default().fg(colors.text_muted)),
        ]));
    } else {
        if is_expanded {
            for line in wrap_text_plain(&cleaned, content_w) {
                lines.push(Line::from(vec![
                    Span::styled(" │", Style::default().fg(border_color)),
                    Span::raw("  "),
                    Span::styled(line, Style::default().fg(colors.text_primary)),
                ]));
            }
        } else {
            let truncated = truncate_text(&cleaned, 300);
            for line in wrap_text_plain(&truncated, content_w) {
                lines.push(Line::from(vec![
                    Span::styled(" │", Style::default().fg(border_color)),
                    Span::raw("  "),
                    Span::styled(line, Style::default().fg(colors.text_primary)),
                ]));
            }
        }
    }
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("└{}", "─".repeat(box_w.saturating_sub(1))),
            Style::default().fg(border_color),
        ),
    ]));
}

fn render_agent_box<'a>(
    lines: &mut Vec<Line<'a>>,
    msg: &ChatMessage,
    box_w: usize,
    is_expanded: bool,
    agent_num: usize,
    msg_idx: usize,
    click_targets: &mut Vec<(u16, ChatClickTarget)>,
    expanded_tools: &FxHashSet<Box<str>>,
    colors: ThemeColors,
) {
    let border_color = colors.success;
    let toggle_label = if is_expanded {
        "▾ collapse"
    } else {
        "▸ expand"
    };
    let model_str = msg.model.as_deref().unwrap_or("");
    let label = if model_str.is_empty() {
        format!(" AGENT #{} ", agent_num)
    } else {
        format!(" AGENT #{} ({}) ", agent_num, model_str)
    };
    let dash_len = box_w.saturating_sub(label.chars().count() + 2 + toggle_label.len() + 1);
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("╔", Style::default().fg(border_color)),
        Span::styled(
            label,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("═".repeat(dash_len), Style::default().fg(border_color)),
        Span::styled(
            format!(" {}", toggle_label),
            Style::default().fg(colors.text_muted),
        ),
    ]));
    let content_w = box_w.saturating_sub(4);
    let all_text: String = msg
        .parts
        .iter()
        .filter_map(|p| {
            if let MessageContent::Text(t) = p {
                Some(t.trim())
            } else {
                None
            }
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = clean_text_with_breaks(&all_text);
    let has_text = !cleaned.is_empty();
    if has_text {
        if is_expanded {
            for line in wrap_text_plain(&cleaned, content_w) {
                lines.push(Line::from(vec![
                    Span::styled(" ║", Style::default().fg(border_color)),
                    Span::raw("  "),
                    Span::styled(line, Style::default().fg(colors.text_secondary)),
                ]));
            }
        } else {
            let truncated = truncate_text(&cleaned, 300);
            for line in wrap_text_plain(&truncated, content_w) {
                lines.push(Line::from(vec![
                    Span::styled(" ║", Style::default().fg(border_color)),
                    Span::raw("  "),
                    Span::styled(line, Style::default().fg(colors.text_secondary)),
                ]));
            }
        }
    }
    let (total_tools, tool_stats) = aggregate_tools_in_group(std::slice::from_ref(msg), &[0]);
    if total_tools > 0 {
        let target_id = format!("tools:msg:{}", msg_idx).into_boxed_str();
        let tools_expanded = expanded_tools.contains(&target_id);
        render_tool_stats_box(
            lines,
            " ║  ",
            border_color,
            content_w,
            total_tools,
            &tool_stats,
            tools_expanded,
            click_targets,
            target_id,
            colors,
        );
    }
    if !has_text && total_tools == 0 {
        lines.push(Line::from(vec![
            Span::styled(" ║", Style::default().fg(border_color)),
            Span::styled("  (empty)", Style::default().fg(colors.text_muted)),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("╚{}", "═".repeat(box_w.saturating_sub(1))),
            Style::default().fg(border_color),
        ),
    ]));
}

fn wrap_text_plain(s: &str, max_w: usize) -> Vec<String> {
    if max_w == 0 {
        return vec![s.to_string()];
    }
    let mut result = Vec::new();
    // Handle embedded newlines: split first, then wrap each line
    for raw_line in s.split('\n') {
        if raw_line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_w = 0usize;
        for word in raw_line.split_whitespace() {
            let word_w = UnicodeWidthStr::width(word);
            if current_w + word_w + usize::from(!current.is_empty()) > max_w {
                if !current.is_empty() {
                    result.push(current);
                    current = String::new();
                    current_w = 0;
                }
                if word_w > max_w {
                    // Break long word on char boundary using display width
                    let mut chunk = String::new();
                    let mut chunk_w = 0usize;
                    for ch in word.chars() {
                        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                        if chunk_w + cw > max_w && !chunk.is_empty() {
                            result.push(chunk);
                            chunk = String::new();
                            chunk_w = 0;
                        }
                        chunk.push(ch);
                        chunk_w += cw;
                    }
                    current = chunk;
                    current_w = chunk_w;
                    continue;
                }
            }
            if !current.is_empty() {
                current.push(' ');
                current_w += 1;
            }
            current.push_str(word);
            current_w += word_w;
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

#[inline]
fn subagent_color(index: usize, colors: ThemeColors) -> Color {
    colors.subagent_color(index)
}

#[inline]
fn dim_color(c: Color, colors: &ThemeColors) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(r / 3, g / 3, b / 3),
        _ => colors.text_muted,
    }
}

fn normalize_tool_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let base = lower.strip_prefix("opencode_").unwrap_or(&lower);
    match base {
        "find" | "finder" => "grep".to_string(),
        "list_directory" | "list" | "ls" => "glob".to_string(),
        "edit_file" => "edit".to_string(),
        "create_file" | "create" => "write".to_string(),
        "patch" | "apply" | "apply_diff" => "apply_patch".to_string(),
        "shell" | "exec" | "terminal" => "bash".to_string(),
        s if s.starts_with("exa_") => "exa".to_string(),
        s if s.starts_with("ref_") => "ref".to_string(),
        s if s.starts_with("context7_") => "context7".to_string(),
        s if s.starts_with("memory_") => "memory".to_string(),
        _ => base.to_string(),
    }
}

/// Push a single padded line inside the tool stats box.
fn push_tool_line<'a>(
    lines: &mut Vec<Line<'a>>,
    prefix: &'a str,
    dim_color: Color,
    frame_color: Color,
    inner_w: usize,
    text: &str,
    text_color: Color,
) {
    let fitted = fit_display_width(text, inner_w.saturating_sub(1));
    let style = Style::default().fg(text_color);

    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled("│ ", Style::default().fg(frame_color)),
        Span::styled(fitted, style),
        Span::styled("│", Style::default().fg(frame_color)),
    ]));
}

fn push_tool_padding<'a>(
    lines: &mut Vec<Line<'a>>,
    prefix: &'a str,
    dim_color: Color,
    frame_color: Color,
    inner_w: usize,
) {
    lines.push(Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color)),
        Span::styled("│ ", Style::default().fg(frame_color)),
        Span::styled(" ".repeat(inner_w.saturating_sub(1)), Style::default()),
        Span::styled("│", Style::default().fg(frame_color)),
    ]));
}

/// For file-centric tools with multiple invocations on the same file,
/// extract just the distinguishing part (e.g. line range for Read).
fn format_tool_invocation_short(tool_name: &str, input: &str, max_w: usize) -> String {
    if !input.contains('{') {
        return if input.chars().count() > max_w {
            safe_truncate_plain(input, max_w).into_owned()
        } else {
            input.to_string()
        };
    }
    let lower = tool_name.to_ascii_lowercase();
    let detail = match lower.as_str() {
        "read" => {
            let range = extract_json_field(input, "read_range").or_else(|| {
                let off = extract_json_field(input, "offset");
                let lim = extract_json_field(input, "limit");
                match (off, lim) {
                    (Some(o), Some(l)) => Some(format!("offset {}, limit {}", o, l)),
                    (Some(o), None) => Some(format!("offset {}", o)),
                    (None, Some(l)) => Some(format!("limit {}", l)),
                    (None, None) => None,
                }
            });
            match range {
                Some(r) => r,
                None => "(full file)".to_string(),
            }
        }
        "edit" | "edit_file" | "write" | "create" | "create_file" => {
            // Show a snippet of old_str or description if available
            let old = extract_json_field(input, "old_str");
            match old {
                Some(o) => {
                    let first = o.lines().next().unwrap_or(&o);
                    let trimmed = first.trim();
                    if trimmed.is_empty() {
                        "edit".to_string()
                    } else {
                        format!("\"{}\"", trimmed)
                    }
                }
                None => "write".to_string(),
            }
        }
        _ => compact_oneline(input),
    };
    if detail.chars().count() > max_w {
        safe_truncate_plain(&detail, max_w).into_owned()
    } else {
        detail
    }
}

/// Format tool detail for display in the UI.
fn format_tool_detail(tool_name: &str, input: &str, max_w: usize) -> String {
    if !input.contains('{') {
        return if input.chars().count() > max_w {
            safe_truncate_plain(input, max_w).into_owned()
        } else {
            input.to_string()
        };
    }
    let lower = tool_name.to_ascii_lowercase();
    let detail = match lower.as_str() {
        "read" => {
            // Try to extract path and optional read_range from JSON-ish input
            let path = extract_json_field(input, "path")
                .or_else(|| extract_json_field(input, "file_path"))
                .or_else(|| extract_json_field(input, "filePath"));
            let range = extract_json_field(input, "read_range").or_else(|| {
                let off = extract_json_field(input, "offset");
                let lim = extract_json_field(input, "limit");
                match (off, lim) {
                    (Some(o), Some(l)) => Some(format!("offset {}, limit {}", o, l)),
                    (Some(o), None) => Some(format!("offset {}", o)),
                    (None, Some(l)) => Some(format!("limit {}", l)),
                    (None, None) => None,
                }
            });
            match (path, range) {
                (Some(p), Some(r)) => format!("{} ({})", short_path_display(&p), r),
                (Some(p), None) => format!("{} (full file)", short_path_display(&p)),
                _ => compact_oneline(input),
            }
        }
        "grep" | "find" | "finder" => {
            let pattern =
                extract_json_field(input, "pattern").or_else(|| extract_json_field(input, "query"));
            let path = extract_json_field(input, "path");
            match (pattern, path) {
                (Some(pat), Some(p)) => format!("`{}` in {}", pat, short_path_display(&p)),
                (Some(pat), None) => format!("`{}`", pat),
                _ => compact_oneline(input),
            }
        }
        "bash" | "shell" | "exec" | "terminal" => {
            let cmd =
                extract_json_field(input, "cmd").or_else(|| extract_json_field(input, "command"));
            match cmd {
                Some(c) => {
                    // Show first line of command
                    let first = c.lines().next().unwrap_or(&c);
                    first.to_string()
                }
                None => compact_oneline(input),
            }
        }
        "edit" | "edit_file" | "write" | "create" | "create_file" => {
            let path = extract_json_field(input, "path")
                .or_else(|| extract_json_field(input, "file_path"))
                .or_else(|| extract_json_field(input, "filePath"));
            match path {
                Some(p) => short_path_display(&p),
                None => compact_oneline(input),
            }
        }
        "glob" | "list" | "ls" | "list_directory" => {
            let pat = extract_json_field(input, "filePattern")
                .or_else(|| extract_json_field(input, "pattern"));
            match pat {
                Some(p) => p,
                None => compact_oneline(input),
            }
        }
        _ => compact_oneline(input),
    };
    if detail.chars().count() > max_w {
        safe_truncate_plain(&detail, max_w).into_owned()
    } else {
        detail
    }
}

/// Extract a simple JSON field value (handles `"key": "value"` and `"key": [...]`)
fn extract_json_field(input: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let key_pos = input.find(&key)?;
    let after_key = &input[key_pos + key.len()..];
    // skip `: ` or `:`
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let trimmed = after_colon.trim_start();
    if trimmed.starts_with('"') {
        // String value — find closing quote (handle escaped quotes)
        let content = &trimmed[1..];
        let mut end = 0;
        let mut escaped = false;
        for ch in content.chars() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            }
            end += ch.len_utf8();
        }
        Some(content[..end].to_string())
    } else if trimmed.starts_with('[') {
        // Array value — find closing bracket
        let end = trimmed.find(']').unwrap_or(trimmed.len());
        let inner = trimmed[1..end].trim();
        Some(inner.to_string())
    } else {
        // Number or other
        let end = trimmed
            .find(|c: char| c == ',' || c == '}' || c == '\n')
            .unwrap_or(trimmed.len());
        Some(trimmed[..end].trim().to_string())
    }
}

/// Show path shortened to last N components
/// If components >= depth+1, shows prefix + last components
fn short_path(path: &str, depth: usize, prefix: &str) -> String {
    let parts: Vec<&str> = path.rsplit('/').take(depth + 1).collect();
    if parts.len() > depth {
        let reversed: Vec<&str> = parts.into_iter().rev().collect();
        format!("{}{}", prefix, reversed.join("/"))
    } else {
        path.to_string()
    }
}

/// Show last 2-3 path components with ellipsis prefix
fn short_path_display(path: &str) -> String {
    short_path(path, 2, "…/")
}

/// Show last 2 components for file paths (handles Option)
fn short_file_path(fp: Option<&str>) -> String {
    match fp {
        Some(p) => short_path(p, 1, ""),
        None => "file".to_string(),
    }
}

/// Collapse multi-line input into a single line
fn compact_oneline(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn safe_truncate_plain(s: &str, max_len: usize) -> Cow<'_, str> {
    let char_count = s.chars().count();
    if char_count <= max_len {
        Cow::Borrowed(s)
    } else {
        let target = max_len.saturating_sub(1);
        let byte_pos = s
            .char_indices()
            .nth(target)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        Cow::Owned(format!("{}…", &s[..byte_pos]))
    }
}

fn fit_display_width(s: &str, target_width: usize) -> String {
    if target_width == 0 {
        return String::new();
    }
    let current_w = UnicodeWidthStr::width(s);
    if current_w <= target_width {
        let mut out = String::with_capacity(s.len() + (target_width - current_w));
        out.push_str(s);
        out.push_str(&" ".repeat(target_width - current_w));
        return out;
    }
    let ellipsis = '…';
    let ellipsis_w = UnicodeWidthChar::width(ellipsis).unwrap_or(1);
    let keep_w = target_width.saturating_sub(ellipsis_w);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > keep_w {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push(ellipsis);
    used += ellipsis_w;
    if used < target_width {
        out.push_str(&" ".repeat(target_width - used));
    }
    out
}

fn wrap_text_with_indent(
    text: &str,
    first_line_width: usize,
    continuation_width: usize,
) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }
    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;
    let mut is_first_line = true;
    for word in &words {
        let max_width = if is_first_line {
            first_line_width
        } else {
            continuation_width
        };
        if current_line.is_empty() {
            if word.len() <= max_width {
                current_line.push_str(word);
                current_width = word.len();
            } else {
                let mut remaining = *word;
                while !remaining.is_empty() {
                    let w = if is_first_line {
                        first_line_width
                    } else {
                        continuation_width
                    };
                    if remaining.len() <= w {
                        current_line = remaining.to_string();
                        current_width = remaining.len();
                        break;
                    }
                    let break_at = w.saturating_sub(1).max(1);
                    let byte_pos = remaining
                        .char_indices()
                        .nth(break_at)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    result.push(format!("{}-", &remaining[..byte_pos]));
                    remaining = &remaining[byte_pos..];
                    is_first_line = false;
                }
            }
        } else {
            let needed = 1 + word.len();
            if current_width + needed <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
                current_width += needed;
            } else {
                result.push(current_line);
                is_first_line = false;
                let new_max = continuation_width;
                if word.len() <= new_max {
                    current_line = word.to_string();
                    current_width = word.len();
                } else {
                    current_line = String::new();
                    current_width = 0;
                    let mut remaining = *word;
                    while !remaining.is_empty() {
                        let w = continuation_width;
                        if remaining.len() <= w {
                            current_line = remaining.to_string();
                            current_width = remaining.len();
                            break;
                        }
                        let break_at = w.saturating_sub(1).max(1);
                        let byte_pos = remaining
                            .char_indices()
                            .nth(break_at)
                            .map(|(i, _)| i)
                            .unwrap_or(remaining.len());
                        result.push(format!("{}-", &remaining[..byte_pos]));
                        remaining = &remaining[byte_pos..];
                    }
                }
            }
        }
    }
    if !current_line.is_empty() {
        result.push(current_line);
    }
    result
}

pub fn detect_git_branch(root: &str) -> Option<String> {
    let root_path = Path::new(root);
    if root_path.as_os_str().is_empty() {
        return None;
    }
    let git_path = root_path.join(".git");
    let head_path = if git_path.is_dir() {
        git_path.join("HEAD")
    } else if git_path.is_file() {
        let Ok(contents) = fs::read_to_string(&git_path) else {
            return None;
        };
        let gitdir = contents
            .lines()
            .find_map(|l| l.strip_prefix("gitdir:"))
            .map(|s| s.trim())?;
        let gitdir_path = PathBuf::from(gitdir);
        let resolved = if gitdir_path.is_absolute() {
            gitdir_path
        } else {
            root_path.join(gitdir_path)
        };
        resolved.join("HEAD")
    } else {
        return None;
    };
    let Ok(head) = fs::read_to_string(head_path) else {
        return None;
    };
    let head = head.trim();
    if let Some(ref_line) = head.strip_prefix("ref:") {
        let ref_path = ref_line.trim();
        let branch = ref_path
            .strip_prefix("refs/heads/")
            .unwrap_or(ref_path)
            .to_string();
        if branch.is_empty() {
            None
        } else {
            Some(branch)
        }
    } else if !head.is_empty() {
        Some(format!("detached {}", &head[..head.len().min(7)]))
    } else {
        None
    }
}

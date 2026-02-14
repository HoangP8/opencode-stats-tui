use crate::stats::{
    format_active_duration, format_number, load_session_details, ChatMessage, MessageContent,
    SessionDetails, SessionStat,
};
use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use fxhash::FxHashMap;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Scroll increment for smooth scrolling experience (1 line = smoothest)
const SCROLL_INCREMENT: u16 = 1;

/// Cached column rectangles and scroll bounds for optimized modal rendering
#[derive(Default, Clone, Copy)]
struct ModalRects {
    info: Option<Rect>,
    chat: Option<Rect>,
    info_max_scroll: u16,
}

/// Session modal view for displaying detailed session information
pub struct SessionModal {
    pub open: bool,
    pub session_details: Option<SessionDetails>,
    pub current_session: Option<SessionStat>,
    pub info_scroll: u16,
    pub chat_messages: Arc<Vec<ChatMessage>>,
    pub chat_scroll: u16,
    pub chat_max_scroll: u16,
    pub selected_column: ModalColumn, // Track which column is focused
    // Cached modal rectangles for optimized mouse hit-testing
    cached_rects: ModalRects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalColumn {
    Info,
    Chat,
}

impl SessionModal {
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
        }
    }

    /// Open modal for a specific session
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
    }

    /// Close modal and reset state
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
    }

    /// Handle keyboard events when modal is open
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

    /// Handle mouse events when modal is open - optimized with cached layout
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
                // Use cached rectangles for optimal hit-testing
                let (x, y) = (mouse.column, mouse.row);

                // Check Info column (most common click target)
                if Self::contains_point(self.cached_rects.info, x, y) {
                    self.selected_column = ModalColumn::Info;
                    return true;
                }

                // Check Chat column
                if Self::contains_point(self.cached_rects.chat, x, y) {
                    self.selected_column = ModalColumn::Chat;
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

    /// Render the modal
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        session: &SessionStat,
        session_titles: &FxHashMap<Box<str>, String>,
    ) {
        // Solid clean background - no blur effect
        let modal_block = Block::default().style(Style::default().bg(Color::Rgb(0, 0, 0)));

        // Fill the entire area with solid black background
        frame.render_widget(modal_block, area);

        let modal_area = area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        });

        // Add instruction bar at the bottom
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(modal_area);

        let content_area = chunks[0];
        let instruction_area = chunks[1];

        // Split content area into two columns
        let column_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(content_area);

        // Cache column rectangles for mouse hit-testing
        self.cached_rects.info = Some(column_chunks[0]);
        self.cached_rects.chat = Some(column_chunks[1]);

        // Determine border styles based on selection
        let (info_border_style, chat_border_style) = match self.selected_column {
            ModalColumn::Info => (
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::DarkGray),
            ),
            ModalColumn::Chat => (
                Style::default().fg(Color::DarkGray),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        self.render_modal_info(
            frame,
            column_chunks[0],
            session,
            session_titles,
            info_border_style,
        );
        self.render_modal_chat(frame, column_chunks[1], chat_border_style);

        // Render instruction bar
        self.render_instructions(frame, instruction_area);
    }

    /// Render modal info panel
    fn render_modal_info(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        session: &SessionStat,
        session_titles: &FxHashMap<Box<str>, String>,
        border_style: Style,
    ) {
        // Pre-allocate with estimated capacity to avoid reallocations
        let mut lines = Vec::with_capacity(50);
        let device = crate::device::get_device_info();
        let device_display = device.display_name();

        let title = session_titles
            .get(&session.id)
            .map(|t| t.strip_prefix("New session - ").unwrap_or(t))
            .unwrap_or("Untitled");

        // Separator line after title
        lines.push(Line::from(""));

        // Project section (best-effort git branch detection)
        let project = session.path_root.as_ref();
        if !project.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  INFOR",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            // Title with wrapping - prefix is "    Title:   " = 13 chars
            let prefix_len: usize = 13; // "    Title:   "
            let inner_width = area.width.saturating_sub(2) as usize; // borders
            let first_line_width = inner_width.saturating_sub(prefix_len);
            let continuation_width = inner_width.saturating_sub(prefix_len);
            let wrapped_title =
                wrap_text_with_indent(title, first_line_width.max(1), continuation_width.max(1));
            for (i, line) in wrapped_title.into_iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::raw("    Title:   "),
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(prefix_len)),
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                }
            }
            let value_width = inner_width.saturating_sub(prefix_len);
            lines.push(Line::from(vec![
                Span::raw("    Project: "),
                Span::styled(
                    safe_truncate_plain(project, value_width),
                    Style::default().fg(Color::White),
                ),
            ]));

            if let Some(branch) = detect_git_branch(project) {
                let branch_display = safe_truncate_plain(&branch, value_width).into_owned();
                lines.push(Line::from(vec![
                    Span::raw("    Branch:  "),
                    Span::styled(branch_display, Style::default().fg(Color::Cyan)),
                ]));
            }

            let active_dur = format_active_duration(session.active_duration_ms);
            lines.push(Line::from(vec![
                Span::raw("    Duration:"),
                Span::styled(
                    format!(" {}", active_dur),
                    Style::default().fg(Color::Rgb(100, 200, 255)),
                ),
            ]));

            // Models: inline comma-separated with +N overflow
            {
                let mut all_models: Vec<&str> =
                    session.models.iter().map(|m| m.as_ref()).collect();
                all_models.sort_unstable();
                if !all_models.is_empty() {
                    let label = "    Models:  ";
                    let avail = value_width;
                    let mut display = String::new();
                    let mut shown = 0usize;
                    for (i, model) in all_models.iter().enumerate() {
                        let candidate = if display.is_empty() {
                            (*model).to_string()
                        } else {
                            format!(", {}", model)
                        };
                        // Reserve space for possible "+N" suffix
                        let remaining = all_models.len() - i - 1;
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
                    let overflow = all_models.len() - shown;
                    if overflow > 0 {
                        display.push_str(&format!(", +{}", overflow));
                    }
                    lines.push(Line::from(vec![
                        Span::raw(label),
                        Span::styled(display, Style::default().fg(Color::Magenta)),
                    ]));
                }
            }

            // Device / Server line
            {
                let (label, color) = if device.kind == "server" {
                    ("    Server:  ", Color::Rgb(255, 165, 0))
                } else {
                    ("    Device:  ", Color::Rgb(100, 200, 255))
                };
                lines.push(Line::from(vec![
                    Span::raw(label),
                    Span::styled(
                        safe_truncate_plain(&device_display, value_width),
                        Style::default().fg(color),
                    ),
                ]));
            }

            lines.push(Line::from(""));
        }

        // Separator line after PROJECT section
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(Color::Rgb(50, 50, 70)),
        )]));
        lines.push(Line::from(""));

        // AGENTS section
        if !session.agents.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                format!("  AGENTS ({})", session.agents.len()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));

            for agent in &session.agents {
                let name_color = if agent.is_main {
                    Color::Cyan
                } else {
                    Color::Rgb(255, 165, 0)
                };
                let mut model_list: Vec<&str> = agent.models.iter().map(|m| m.as_ref()).collect();
                model_list.sort_unstable();
                let model_suffix = if !model_list.is_empty() {
                    format!(" ({})", model_list.join(", "))
                } else {
                    String::new()
                };
                let prefix_len = 6; // "    ● "
                let max_content =
                    (area.width.saturating_sub(2) as usize).saturating_sub(prefix_len);
                let agent_name = &agent.name;
                let full_text = format!("{}{}", agent_name, model_suffix);
                let display = safe_truncate_plain(&full_text, max_content);
                let name_len = agent_name.chars().count();
                if display.chars().count() <= name_len {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled("● ", Style::default().fg(name_color)),
                        Span::styled(
                            display.into_owned(),
                            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                        ),
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
                        Span::styled(suffix_part, Style::default().fg(Color::Magenta)),
                    ]));
                }

                let agent_active_dur = format_active_duration(agent.active_duration_ms);
                lines.push(Line::from(vec![
                    Span::styled("      Duration   ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>10}", agent_active_dur),
                        Style::default().fg(Color::Rgb(100, 200, 255)),
                    ),
                ]));

                // Messages count
                lines.push(Line::from(vec![
                    Span::styled("      Messages   ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>10}", agent.messages),
                        Style::default().fg(Color::White),
                    ),
                ]));

                // Token details - single column, aligned
                let token_rows = [
                    ("Input", format_number(agent.tokens.input), Color::Blue),
                    ("Output", format_number(agent.tokens.output), Color::Green),
                    (
                        "Thinking",
                        format_number(agent.tokens.reasoning),
                        Color::Rgb(255, 165, 0),
                    ),
                    (
                        "Cache R",
                        format_number(agent.tokens.cache_read),
                        Color::Yellow,
                    ),
                    (
                        "Cache W",
                        format_number(agent.tokens.cache_write),
                        Color::Yellow,
                    ),
                ];

                for (label, value, color) in &token_rows {
                    lines.push(Line::from(vec![
                        Span::styled(format!("      {:<11}", label), Style::default().fg(*color)),
                        Span::styled(format!("{:>10}", value), Style::default().fg(Color::White)),
                    ]));
                }

                lines.push(Line::from(""));
            }
        }

        // Separator before MODEL
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(Color::Rgb(50, 50, 70)),
        )]));
        lines.push(Line::from(""));

        // MODEL section (per-model token breakdown)
        let details = self.session_details.as_ref();
        if let Some(d) = details {
            for (idx, model) in d.model_stats.iter().enumerate() {
                let prefix = if d.model_stats.len() > 1 {
                    format!("MODEL {}:", idx + 1)
                } else {
                    "MODEL:".to_string()
                };
                let header_prefix = format!("  {} ", prefix);
                let header_prefix_len = header_prefix.chars().count();
                let model_max =
                    (area.width.saturating_sub(2) as usize).saturating_sub(header_prefix_len);
                let mut model_header_spans = Vec::with_capacity(2);
                model_header_spans.push(Span::styled(
                    header_prefix,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
                model_header_spans.push(Span::styled(
                    safe_truncate_plain(&model.name, model_max),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(model_header_spans));

                let left_labels = [
                    ("Input", format_number(model.tokens.input), Color::Blue),
                    ("Output", format_number(model.tokens.output), Color::Green),
                    (
                        "Thinking",
                        format_number(model.tokens.reasoning),
                        Color::Rgb(255, 165, 0),
                    ),
                    (
                        "Cache R",
                        format_number(model.tokens.cache_read),
                        Color::Yellow,
                    ),
                    (
                        "Cache W",
                        format_number(model.tokens.cache_write),
                        Color::Yellow,
                    ),
                ];

                let responses = model.messages.saturating_sub(model.prompts);
                let model_cost = model.cost;
                let model_est = model_cost
                    + (model.tokens.cache_read as f64 * model_cost
                        / (model.tokens.input + model.tokens.output + model.tokens.reasoning).max(1)
                            as f64);
                let model_savings = model_est - model_cost;
                let right_labels = [
                    ("Prompts", model.prompts.to_string(), Color::Cyan),
                    ("Responses", responses.to_string(), Color::Green),
                    ("Cost", format!("${:.2}", model_cost), Color::White),
                    (
                        "Est. Cost",
                        format!("${:.2}", model_est),
                        Color::Rgb(150, 150, 150),
                    ),
                    (
                        "Savings",
                        format!("${:.2}", model_savings),
                        if model_savings > 0.0 {
                            Color::Green
                        } else {
                            Color::DarkGray
                        },
                    ),
                ];

                let inner_width = area.width.saturating_sub(2) as usize;
                // 6 (indent) + 11 (label) + 10 (value) + 7 (sep) + 11 (label) + 10 (value) = 55
                let show_right_column = inner_width >= 55;

                for i in 0..5 {
                    let mut spans = Vec::with_capacity(7);
                    spans.push(Span::raw("      "));

                    if i < left_labels.len() {
                        let (label, value, color) = &left_labels[i];
                        spans.push(Span::styled(
                            format!("{:<11}", label),
                            Style::default().fg(*color),
                        ));
                        spans.push(Span::styled(
                            format!("{:>10}", value),
                            Style::default().fg(Color::White),
                        ));
                    } else if show_right_column {
                        spans.push(Span::raw(" ".repeat(21)));
                    }

                    if show_right_column {
                        spans.push(Span::styled(
                            "   │   ",
                            Style::default().fg(Color::Rgb(40, 40, 50)),
                        ));
                        if i < right_labels.len() {
                            let (label, value, color) = &right_labels[i];
                            spans.push(Span::styled(
                                format!("{:<11}", label),
                                Style::default().fg(*color),
                            ));
                            spans.push(Span::styled(
                                format!("{:>10}", value),
                                Style::default().fg(Color::White),
                            ));
                        }
                    }

                    lines.push(Line::from(spans));
                }

                lines.push(Line::from(""));
            }
        }

        // Separator before TOTAL USAGE
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(Color::Rgb(50, 50, 70)),
        )]));
        lines.push(Line::from(""));

        // Total usage
        lines.push(Line::from(vec![Span::styled(
            "  TOTAL USAGE",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));

        let total_tokens = session.tokens.total();
        lines.push(Line::from(vec![
            Span::styled("      Tokens     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", format_number(total_tokens)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let total_responses = session.messages.saturating_sub(session.prompts);
        lines.push(Line::from(vec![
            Span::styled("      Prompts    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", session.prompts),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("      Responses  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", total_responses),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("      Cost       ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", format!("${:.2}", session.cost)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let total_cost = session.cost;
        let total_non_cache =
            (session.tokens.input + session.tokens.output + session.tokens.reasoning).max(1) as f64;
        let est_cost =
            total_cost + (session.tokens.cache_read as f64 * total_cost / total_non_cache);
        let savings = est_cost - total_cost;
        let (savings_text, savings_color) = if savings < 0.0 {
            (format!("-${:.2}", savings.abs()), Color::Red)
        } else {
            (
                format!("${:.2}", savings),
                if savings > 0.0 {
                    Color::Green
                } else {
                    Color::DarkGray
                },
            )
        };
        lines.push(Line::from(vec![
            Span::styled("      Savings    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10}", savings_text),
                Style::default()
                    .fg(savings_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        // Separator before file changes
        lines.push(Line::from(vec![Span::styled(
            "─".repeat((area.width - 2) as usize),
            Style::default().fg(Color::Rgb(50, 50, 70)),
        )]));
        lines.push(Line::from(""));

        // File changes section
        if !session.file_diffs.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  FILE CHANGES",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));

            let max_val_len = 6usize;
            let diff_block_width = (max_val_len + 1) * 2 + 3; // 17 (7+3+7)
            let fixed_prefix = 14; // 4 + 10
            let status_sep = 1;
            let path_sep = 2;
            let right_margin = 5; // Add space at the end of the panel

            let inner_width = area.width.saturating_sub(2) as usize;
            let needed_width =
                fixed_prefix + status_sep + path_sep + diff_block_width + right_margin;
            let path_display_width = inner_width.saturating_sub(needed_width);
            let show_diffs = inner_width >= needed_width;

            for f in &session.file_diffs {
                let status_text = match f.status.as_ref() {
                    "added" => "   added",
                    "modified" => "modified",
                    "deleted" => " deleted",
                    _ => " unknown",
                };

                let mut file_spans = Vec::with_capacity(10);
                file_spans.push(Span::raw("    "));
                file_spans.push(Span::styled(
                    format!("[{}]", status_text),
                    Style::default().fg(Color::DarkGray),
                ));

                if show_diffs {
                    file_spans.push(Span::raw(" ")); // status_sep

                    let path_count = f.path.chars().count();
                    let short_path = if path_count > path_display_width {
                        let visible_chars = path_display_width.saturating_sub(3);
                        let truncated: String = f.path.chars().take(visible_chars).collect();
                        format!("{}...", truncated)
                    } else {
                        format!("{:<width$}", f.path, width = path_display_width)
                    };

                    file_spans.push(Span::styled(short_path, Style::default().fg(Color::White)));
                    file_spans.push(Span::raw("  ")); // path_sep

                    let add_str = format_number(f.additions);
                    let add_sign_str = format!("+{}", add_str);
                    file_spans.push(Span::styled(
                        format!("{:>width$}", add_sign_str, width = max_val_len + 1),
                        Style::default().fg(Color::Green),
                    ));
                    file_spans.push(Span::styled(
                        " │ ",
                        Style::default().fg(Color::Rgb(40, 40, 50)),
                    ));
                    let del_str = format_number(f.deletions);
                    let del_sign_str = format!("-{}", del_str);
                    file_spans.push(Span::styled(
                        format!("{:>width$}", del_sign_str, width = max_val_len + 1),
                        Style::default().fg(Color::Red),
                    ));
                }
                lines.push(Line::from(file_spans));
            }

            // Add separator line and total (without "total" label)
            if show_diffs {
                let mut sep_spans = Vec::new();
                let diff_start = fixed_prefix + status_sep + path_display_width + path_sep;
                sep_spans.push(Span::raw(" ".repeat(diff_start)));

                let dash_part = "─".repeat(max_val_len + 1); // 8
                sep_spans.push(Span::styled(
                    format!("{}─┼─{}", dash_part, dash_part),
                    Style::default().fg(Color::Rgb(40, 40, 50)),
                ));
                lines.push(Line::from(sep_spans));

                let mut total_spans = Vec::with_capacity(4);
                total_spans.push(Span::raw(" ".repeat(diff_start)));

                let add_str = format_number(session.diffs.additions);
                let add_sign_str = format!("+{}", add_str);
                total_spans.push(Span::styled(
                    format!("{:>width$}", add_sign_str, width = max_val_len + 1),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ));
                total_spans.push(Span::styled(
                    " │ ",
                    Style::default().fg(Color::Rgb(40, 40, 50)),
                ));
                let del_str = format_number(session.diffs.deletions);
                let del_sign_str = format!("-{}", del_str);
                total_spans.push(Span::styled(
                    format!("{:>width$}", del_sign_str, width = max_val_len + 1),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(total_spans));
            }
        } else {
            // No file changes
            lines.push(Line::from(vec![Span::styled(
                "  FILE CHANGES ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "    NO FILE CHANGES",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        // Compute max scroll from actual content vs inner height (borders = 2)
        let inner_height = area.height.saturating_sub(2) as usize;
        let info_max_scroll = (lines.len().saturating_sub(inner_height)) as u16;
        self.cached_rects.info_max_scroll = info_max_scroll;
        let scroll = self.info_scroll.min(info_max_scroll);
        self.info_scroll = scroll;
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(scroll as usize)
            .take(inner_height)
            .collect();

        // Build title: show session ID and continuation info
        let title_text = if session.is_continuation {
            if let Some(first_date) = &session.first_created_date {
                format!("  {}  [Continue from {}]  ", session.id, first_date)
            } else {
                format!("  {}  [Continued]  ", session.id)
            }
        } else {
            format!("  {}  ", session.id)
        };

        let title_color = if border_style.fg == Some(Color::Cyan) {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(Color::Rgb(10, 10, 15)))
            .title(
                Line::from(Span::styled(
                    title_text,
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(ratatui::layout::Alignment::Center),
            );

        frame.render_widget(
            Paragraph::new(visible)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    /// Render modal chat panel
    fn render_modal_chat(&mut self, frame: &mut Frame, area: Rect, border_style: Style) {
        // Pre-allocate with estimated capacity to avoid reallocations
        let mut lines = Vec::with_capacity(self.chat_messages.len() * 15);
        let inner_width = area.width.saturating_sub(2) as usize;

        let mut current_subagent: Option<&str> = None;

        for msg in self.chat_messages.iter() {
            // Check for sub-agent transitions
            let msg_agent = msg.agent_label.as_deref();

            if msg.is_subagent && current_subagent != msg_agent {
                // Close previous sub-agent if any
                if current_subagent.is_some() {
                    let end_label = format!(" ══ end {} ", current_subagent.unwrap());
                    let end_dashes = "═".repeat(inner_width.saturating_sub(end_label.len()));
                    lines.push(Line::from(vec![
                        Span::styled(
                            end_label,
                            Style::default()
                                .fg(Color::Rgb(255, 165, 0))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(end_dashes, Style::default().fg(Color::Rgb(80, 60, 0))),
                    ]));
                    lines.push(Line::from(""));
                }
                // Open new sub-agent
                let agent_name = msg_agent.unwrap_or("subagent");
                let start_label = format!(" ══ {} ", agent_name);
                let start_dashes = "═".repeat(inner_width.saturating_sub(start_label.len()));
                lines.push(Line::from(vec![
                    Span::styled(
                        start_label,
                        Style::default()
                            .fg(Color::Rgb(255, 165, 0))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(start_dashes, Style::default().fg(Color::Rgb(80, 60, 0))),
                ]));
                lines.push(Line::from(""));
                current_subagent = msg_agent;
            } else if !msg.is_subagent && current_subagent.is_some() {
                // Leaving sub-agent context
                let end_label = format!(" ══ end {} ", current_subagent.unwrap());
                let end_dashes = "═".repeat(inner_width.saturating_sub(end_label.len()));
                lines.push(Line::from(vec![
                    Span::styled(
                        end_label,
                        Style::default()
                            .fg(Color::Rgb(255, 165, 0))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(end_dashes, Style::default().fg(Color::Rgb(80, 60, 0))),
                ]));
                lines.push(Line::from(""));
                current_subagent = None;
            }

            // Role header (the AGENT/SUBAGENT logic)
            let (role_icon, role_label, role_color) = if &*msg.role == "assistant" {
                if msg.is_subagent {
                    ("🤖", "SUBAGENT".to_string(), Color::Rgb(255, 165, 0))
                } else {
                    ("🤖", "AGENT".to_string(), Color::Green)
                }
            } else {
                let (icon, label, color) = get_role_info(&msg.role);
                (icon, label, color)
            };

            let display_role = if &*msg.role == "assistant" {
                if let Some(model) = &msg.model {
                    format!("{} ({})", role_label, model)
                } else {
                    role_label
                }
            } else {
                role_label
            };

            let header_label = format!(" {} {} ", role_icon, display_role);
            let dash_len = inner_width.saturating_sub(header_label.len());
            let dash_line = "─".repeat(dash_len);

            let header_spans = vec![
                Span::styled(
                    header_label,
                    Style::default().fg(role_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(dash_line, Style::default().fg(Color::Rgb(50, 50, 60))),
            ];
            lines.push(Line::from(header_spans));

            // Message content
            for part in &msg.parts {
                match part {
                    MessageContent::Text(text) => {
                        let (max_line_chars, max_lines) = match &*msg.role {
                            "user" => (150, 5),
                            "assistant" => (250, 10),
                            _ => (200, 6),
                        };
                        let line_count = text.lines().count();
                        for (line_idx, l) in text.lines().enumerate() {
                            if line_idx >= max_lines {
                                let mut more_spans = Vec::with_capacity(2);
                                more_spans.push(Span::raw("  "));
                                more_spans.push(Span::styled(
                                    format!("... ({} more)", line_count - max_lines),
                                    Style::default().fg(Color::Rgb(100, 100, 100)),
                                ));
                                lines.push(Line::from(more_spans));
                                break;
                            }
                            let text_spans =
                                vec![Span::raw("  "), Span::raw(safe_truncate(l, max_line_chars))];
                            lines.push(Line::from(text_spans));
                        }
                    }
                    MessageContent::ToolCall(tool_info) => {
                        let display = match (&tool_info.file_path, &tool_info.title) {
                            (Some(fp), _) => {
                                let parts: Vec<&str> = fp.rsplit('/').take(2).collect();
                                let short_path = if parts.len() >= 2 {
                                    format!("{}/{}", parts[1], parts[0])
                                } else {
                                    parts[0].to_string()
                                };
                                format!("{} → {}", tool_info.name, short_path)
                            }
                            (None, Some(title)) => {
                                format!("{}: {}", tool_info.name, safe_truncate_plain(title, 40))
                            }
                            (None, None) => tool_info.name.to_string(),
                        };
                        let tool_spans = vec![
                            Span::raw("    "),
                            Span::styled("▸ ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                display,
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ];
                        lines.push(Line::from(tool_spans));
                    }
                    MessageContent::Thinking(_) => {
                        let think_spans = vec![
                            Span::raw("  "),
                            Span::styled(
                                "> Thinking...",
                                Style::default().fg(Color::Rgb(80, 80, 80)),
                            ),
                        ];
                        lines.push(Line::from(think_spans));
                    }
                }
            }

            lines.push(Line::from(""));
        }

        // Close final sub-agent banner if still open
        if current_subagent.is_some() {
            let end_label = format!(" ══ end {} ", current_subagent.unwrap());
            let end_dashes = "═".repeat(inner_width.saturating_sub(end_label.len()));
            lines.push(Line::from(vec![
                Span::styled(
                    end_label,
                    Style::default()
                        .fg(Color::Rgb(255, 165, 0))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(end_dashes, Style::default().fg(Color::Rgb(80, 60, 0))),
            ]));
            lines.push(Line::from(""));
        }

        let inner_height = area.height.saturating_sub(2) as usize;
        self.chat_max_scroll = (lines.len().saturating_sub(inner_height)) as u16;
        self.chat_scroll = self.chat_scroll.min(self.chat_max_scroll);
        let scroll = self.chat_scroll as usize;
        let visible: Vec<Line> = lines.into_iter().skip(scroll).take(inner_height).collect();

        let title_color = if border_style.fg == Some(Color::Cyan) {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(Color::Rgb(10, 10, 15)))
            .title(
                Line::from(Span::styled(
                    " CHAT ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(ratatui::layout::Alignment::Center),
            );

        frame.render_widget(
            Paragraph::new(visible)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    /// Render instruction bar at the bottom
    fn render_instructions(&self, frame: &mut Frame, area: Rect) {
        let k = Style::default()
            .fg(Color::Rgb(140, 140, 160))
            .add_modifier(Modifier::BOLD);
        let t = Style::default().fg(Color::DarkGray);
        let sep = Span::styled(" │ ", Style::default().fg(Color::Rgb(50, 50, 70)));

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
            .style(Style::default().bg(Color::Rgb(15, 15, 25)))
            .alignment(Alignment::Center);

        frame.render_widget(status_bar, area);
    }
}

/// Helper: Truncate string with ellipsis if too long
/// Returns a static string slice - optimized to avoid allocations
#[inline]
fn safe_truncate(s: &str, max_len: usize) -> &str {
    if s.chars().count() <= max_len {
        return s;
    }
    let target = max_len.saturating_sub(3);
    let byte_end = s
        .char_indices()
        .nth(target)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s[..byte_end]
        .rfind(' ')
        .map(|pos| &s[..pos + 1])
        .unwrap_or(&s[..byte_end])
}

fn safe_truncate_plain(s: &str, max_len: usize) -> Cow<'_, str> {
    let mut char_count = 0;
    // Fast check for short strings
    for _ in s.chars() {
        char_count += 1;
        if char_count > max_len {
            break;
        }
    }
    if char_count <= max_len {
        return Cow::Borrowed(s);
    }

    // Truncate and add ellipsis - total length will be max_len
    let target = max_len.saturating_sub(1);
    let mut current_count = 0;
    for (idx, _) in s.char_indices() {
        if current_count == target {
            let mut result = s[..idx].to_string();
            result.push('…');
            return Cow::Owned(result);
        }
        current_count += 1;
    }
    Cow::Borrowed(s)
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

/// Helper: Get role display information (icon, label, color)
fn get_role_info(role: &str) -> (&'static str, String, Color) {
    match role {
        "user" => ("👤", "USER".to_string(), Color::Cyan),
        "assistant" => ("🤖", "ASSISTANT".to_string(), Color::Green),
        "system" => ("⚙", "SYSTEM".to_string(), Color::Yellow),
        _ => ("?", role.to_uppercase(), Color::White),
    }
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

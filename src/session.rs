use crate::stats::{
    format_number, load_session_details, ChatMessage, MessageContent, SessionDetails, SessionStat,
};
use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::borrow::Cow;
use std::collections::HashMap;

/// Scroll increment for smooth scrolling experience (1 line = smoothest)
const SCROLL_INCREMENT: u16 = 1;

/// Cached column rectangles for optimized modal mouse hit-testing
#[derive(Default, Clone, Copy)]
struct ModalRects {
    info: Option<Rect>,
    chat: Option<Rect>,
}

/// Session modal view for displaying detailed session information
pub struct SessionModal {
    pub open: bool,
    pub session_details: Option<SessionDetails>,
    pub current_session: Option<SessionStat>,
    pub info_scroll: u16,
    pub chat_messages: Vec<ChatMessage>,
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
            chat_messages: Vec::new(),
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
        chat_messages: Vec<ChatMessage>,
        session_stat: &crate::stats::SessionStat,
        files: Option<&[std::path::PathBuf]>,
    ) {
        let details = load_session_details(session_id, files);
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
        self.chat_messages.clear();
        self.chat_scroll = 0;
        self.info_scroll = 0;
        self.chat_max_scroll = 0;
        self.selected_column = ModalColumn::Info;
        self.cached_rects = ModalRects::default();
    }

    /// Handle keyboard events when modal is open
    pub fn handle_key_event(&mut self, key: KeyCode, area_height: u16) -> bool {
        if !self.open {
            return false;
        }

        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.close();
                true
            }
            // Column selection (left/right to switch between Info and Chat)
            KeyCode::Left | KeyCode::Char('h') => {
                self.selected_column = ModalColumn::Info;
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.selected_column = ModalColumn::Chat;
                true
            }
            // Vertical scrolling (up/down)
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
                        // Calculate max scroll for info panel based on content height
                        let info_max_scroll = if let Some(session) = &self.current_session {
                            self.calculate_info_max_scroll(area_height, session)
                        } else {
                            0
                        };
                        self.info_scroll = (self.info_scroll + 1).min(info_max_scroll);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = (self.chat_scroll + 1).min(self.chat_max_scroll);
                    }
                }
                true
            }
            // Page up/down
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
                        // Calculate max scroll for info panel based on content height
                        let info_max_scroll = if let Some(session) = &self.current_session {
                            self.calculate_info_max_scroll(area_height, session)
                        } else {
                            0
                        };
                        self.info_scroll = (self.info_scroll + 10).min(info_max_scroll);
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

    /// Calculate the maximum scroll value for the info panel based on content
    fn calculate_info_max_scroll(&self, area_height: u16, session: &SessionStat) -> u16 {
        let details = self.session_details.as_ref();
        let mut total_lines = 2u16; // Header + blank

        if let Some(d) = details {
            // Model section: 7 lines per model (1 header + 5 data rows + 1 blank)
            total_lines += (d.model_stats.len() as u16) * 7;

            // Combined models total (3 lines)
            total_lines += 3;

            // Separator before file changes (2 lines)
            total_lines += 2;

            // File changes section
            if !session.file_diffs.is_empty() {
                // Section header (1 line) + Blank (1 line)
                total_lines += 2;

                // File entries (one per file)
                total_lines += session.file_diffs.len() as u16;

                // Blank + Total line (2 lines)
                total_lines += 2;
            }
        }

        // Subtract 4 for borders and title
        let available_height = area_height.saturating_sub(4);

        if total_lines > available_height {
            total_lines.saturating_sub(available_height)
        } else {
            0
        }
    }

    /// Calculate the actual number of rendered lines for the chat panel
    fn calculate_chat_max_scroll(&self, area_height: u16) -> u16 {
        let mut total_lines = 0u16;

        for msg in &self.chat_messages {
            // Header line
            total_lines += 1;

            for part in &msg.parts {
                match part {
                    MessageContent::Text(text) => {
                        let (_max_line_chars, max_lines) = match &*msg.role {
                            "user" => (150, 5),
                            "assistant" => (250, 10),
                            _ => (200, 6),
                        };

                        let line_count = text.lines().count();
                        total_lines += line_count.min(max_lines) as u16;

                        // Add indicator if truncated
                        if line_count > max_lines {
                            total_lines += 1;
                        }
                    }
                    MessageContent::ToolCall(_) => {
                        total_lines += 1;
                    }
                    MessageContent::Thinking(_) => {
                        total_lines += 1;
                    }
                }
            }

            // Blank line between messages
            total_lines += 1;
        }

        // Subtract 4 for borders and title
        let available_height = area_height.saturating_sub(4);

        if total_lines > available_height {
            total_lines.saturating_sub(available_height)
        } else {
            0
        }
    }

    /// Handle mouse events when modal is open - optimized with cached layout
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent, _area: Rect) -> bool {
        if !self.open {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = self.info_scroll.saturating_sub(SCROLL_INCREMENT);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = self.chat_scroll.saturating_sub(SCROLL_INCREMENT);
                    }
                }
                true
            }
            MouseEventKind::ScrollDown => {
                match self.selected_column {
                    ModalColumn::Info => {
                        self.info_scroll = self.info_scroll.saturating_add(SCROLL_INCREMENT);
                    }
                    ModalColumn::Chat => {
                        self.chat_scroll = self.chat_scroll.saturating_add(SCROLL_INCREMENT);
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
        session_titles: &HashMap<String, String>,
    ) {
        // Calculate max scroll for both panels based on content height
        let info_max_scroll = self.calculate_info_max_scroll(area.height, session);
        self.chat_max_scroll = self.calculate_chat_max_scroll(area.height);

        // Ensure current scroll doesn't exceed max
        self.info_scroll = self.info_scroll.min(info_max_scroll);
        self.chat_scroll = self.chat_scroll.min(self.chat_max_scroll);

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

        // Cache column rectangles for optimized mouse hit-testing
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
        &self,
        frame: &mut Frame,
        area: Rect,
        session: &SessionStat,
        session_titles: &HashMap<String, String>,
        border_style: Style,
    ) {
        // Pre-allocate with estimated capacity to avoid reallocations
        let mut lines = Vec::with_capacity(50);
        let details = self.session_details.as_ref();

        let title = session_titles
            .get(&session.id.to_string())
            .map(|t| t.replace("New session - ", ""))
            .unwrap_or_else(|| "Untitled".to_string());

        // Title row with full session ID
        let mut title_spans = Vec::with_capacity(2);
        title_spans.push(Span::styled(
            safe_truncate(
                &title,
                (area.width.saturating_sub(session.id.len() as u16 + 5)) as usize,
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        title_spans.push(Span::styled(
            format!(" [{}]", &session.id),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::from(title_spans));
        lines.push(Line::from(""));

        // Model usage section
        if let Some(d) = details {
            let mut total_tokens = 0u64;

            for (idx, model) in d.model_stats.iter().enumerate() {
                let prefix = if d.model_stats.len() > 1 {
                    format!("MODEL {}:", idx + 1)
                } else {
                    "MODEL:".to_string()
                };
                let mut model_header_spans = Vec::with_capacity(2);
                model_header_spans.push(Span::styled(
                    format!("  {} ", prefix),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
                model_header_spans.push(Span::styled(
                    safe_truncate_plain(&model.name, (area.width - 12) as usize),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(model_header_spans));

                // Two column layout for model details
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

                let right_labels = [
                    ("Messages", model.messages.to_string(), Color::Cyan),
                    ("Cost", "$0.00".to_string(), Color::White),
                    ("Est. Cost", "$0.00".to_string(), Color::Rgb(150, 150, 150)),
                    ("Savings", "$0.00".to_string(), Color::Green),
                ];

                total_tokens += model.tokens.input
                    + model.tokens.output
                    + model.tokens.reasoning
                    + model.tokens.cache_read
                    + model.tokens.cache_write;

                for i in 0..5 {
                    let mut spans = Vec::with_capacity(7);
                    spans.push(Span::raw("      "));

                    // Left column
                    if i < left_labels.len() {
                        let (label, value, color) = &left_labels[i];
                        spans.push(Span::styled(
                            format!("{:<10}", label),
                            Style::default().fg(*color),
                        ));
                        spans.push(Span::styled(
                            format!("{:>8}", value),
                            Style::default().fg(Color::White),
                        ));
                    } else {
                        spans.push(Span::raw(" ".repeat(18)));
                    }

                    spans.push(Span::styled(
                        "   │   ",
                        Style::default().fg(Color::Rgb(40, 40, 50)),
                    ));

                    // Right column
                    if i < right_labels.len() {
                        let (label, value, _color) = &right_labels[i];
                        spans.push(Span::styled(
                            format!("{:<10}", label),
                            Style::default().fg(Color::White),
                        ));
                        spans.push(Span::styled(
                            format!("{:>8}", value),
                            Style::default().fg(Color::White),
                        ));
                    }

                    lines.push(Line::from(spans));
                }

                lines.push(Line::from(""));
            }

            // Models Combined Total below models
            lines.push(Line::from(vec![Span::styled(
                "  TOTAL USAGE",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(vec![
                Span::styled(
                    "      Combined Tokens: ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format_number(total_tokens),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    "      Combined Savings: ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    "$0.00",
                    Style::default()
                        .fg(Color::Green)
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
                    "  FILE CHANGES ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));

                let path_display_width = (area.width.saturating_sub(42)) as usize;

                for f in &session.file_diffs {
                    let status_text = match f.status.as_ref() {
                        "added" => "   added",
                        "modified" => "modified",
                        "deleted" => " deleted",
                        _ => " unknown",
                    };

                    let short_path = if f.path.len() > path_display_width
                        && f.path.chars().count() > path_display_width
                    {
                        let visible_chars = path_display_width.saturating_sub(3);
                        let truncated: String = f.path.chars().take(visible_chars).collect();
                        format!("{}...", truncated)
                    } else {
                        format!("{:<width$}", f.path, width = path_display_width)
                    };

                    let mut file_spans = Vec::with_capacity(10);
                    file_spans.push(Span::raw("    "));
                    file_spans.push(Span::styled(
                        format!("[{}]", status_text),
                        Style::default().fg(Color::DarkGray),
                    ));
                    file_spans.push(Span::raw(" "));
                    file_spans.push(Span::styled(short_path, Style::default().fg(Color::White)));
                    file_spans.push(Span::raw(" "));
                    file_spans.push(Span::styled(
                        format!("{:>7}", format_number(f.additions)),
                        Style::default().fg(Color::Green),
                    ));
                    file_spans.push(Span::styled("+", Style::default().fg(Color::Green)));
                    file_spans.push(Span::styled(
                        " │ ",
                        Style::default().fg(Color::Rgb(40, 40, 50)),
                    ));
                    file_spans.push(Span::styled(
                        format!("{:>7}", format_number(f.deletions)),
                        Style::default().fg(Color::Red),
                    ));
                    file_spans.push(Span::styled("-", Style::default().fg(Color::Red)));
                    lines.push(Line::from(file_spans));
                }

                // Sum it up for file changes
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    "    TOTAL FILE CHANGES",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled("Added Lines:   ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>7}", format_number(session.diffs.additions)),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "+",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   │   ", Style::default().fg(Color::Rgb(40, 40, 50))),
                    Span::styled("Deleted Lines: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>7}", format_number(session.diffs.deletions)),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "-",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                ]));
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
        }

        let scroll = self.info_scroll;
        let info_max_scroll = self.calculate_info_max_scroll(area.height, session);
        let scroll = scroll.min(info_max_scroll);
        let visible_height = area.height.saturating_sub(4) as usize;
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(scroll as usize)
            .take(visible_height)
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(Color::Rgb(10, 10, 15)))
            .title(
                Line::from(Span::styled(
                    " SESSION INFO ",
                    Style::default()
                        .fg(Color::Cyan)
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
    fn render_modal_chat(&self, frame: &mut Frame, area: Rect, border_style: Style) {
        // Pre-allocate with estimated capacity to avoid reallocations
        let mut lines = Vec::with_capacity(self.chat_messages.len() * 15);
        let inner_width = area.width.saturating_sub(2) as usize;

        for msg in self.chat_messages.iter() {
            let (role_icon, role_label, role_color) = get_role_info(&msg.role);

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

        let scroll = self.chat_scroll as usize;
        // Take only what's needed for the visible area to reduce memory usage
        let visible_height = area.height.saturating_sub(4) as usize;
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(Color::Rgb(10, 10, 15)))
            .title(
                Line::from(Span::styled(
                    " CHAT ",
                    Style::default()
                        .fg(Color::Cyan)
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
        let instructions = vec![Line::from(vec![
            Span::styled(
                "←→",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Select column", Style::default().fg(Color::DarkGray)),
            Span::styled("  ", Style::default()),
            Span::styled(
                "↑↓",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Scroll", Style::default().fg(Color::DarkGray)),
            Span::styled("  ", Style::default()),
            Span::styled(
                "PgUp/PgDn",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Page scroll", Style::default().fg(Color::DarkGray)),
            Span::styled("  ", Style::default()),
            Span::styled(
                "Q/Esc",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Quit", Style::default().fg(Color::DarkGray)),
        ])];

        let status_bar = Paragraph::new(instructions)
            .style(
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(Color::Rgb(20, 20, 30)),
            )
            .alignment(Alignment::Center);

        frame.render_widget(status_bar, area);
    }
}

/// Helper: Truncate string with ellipsis if too long
/// Returns a static string slice - optimized to avoid allocations
#[inline]
fn safe_truncate(s: &str, max_len: usize) -> &str {
    // Optimized: early return without char count if already short enough
    if s.len() <= max_len {
        return s;
    }

    // Fast path for ASCII strings (most common case)
    if s.is_ascii() {
        // For ASCII, byte length equals char count
        // Safe to truncate at byte boundary
        return &s[..max_len.min(s.len())];
    }

    // Slow path for non-ASCII: count chars properly
    let end = s
        .char_indices()
        .nth(max_len)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// Helper: Truncate string and return Cow<'_, str>
/// Uses Cow to avoid allocation when no truncation needed
#[inline]
fn safe_truncate_plain(s: &str, max_len: usize) -> Cow<'_, str> {
    // Optimized: check byte length first as a fast path
    if s.len() <= max_len {
        return Cow::Borrowed(s);
    }

    // Fast path for ASCII strings (most common case)
    if s.is_ascii() {
        // For ASCII, byte length equals char count
        // Safe to truncate at byte boundary
        return Cow::Borrowed(&s[..max_len]);
    }

    // Slow path for non-ASCII: count chars properly
    if s.chars().count() <= max_len {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(s.chars().take(max_len).collect())
    }
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

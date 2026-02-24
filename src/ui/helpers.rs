//! Helper functions and shared types for UI rendering

use crate::stats::{format_number, ChatMessage, MessageContent};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::borrow::Cow;

/// Cached chat session data including pre-calculated scroll info
pub struct CachedChat {
    pub messages: std::sync::Arc<Vec<ChatMessage>>,
    pub total_lines: u16,
}

/// Helper to create cache key from session_id and day
pub fn cache_key(session_id: &str, day: Option<&str>) -> String {
    if let Some(d) = day {
        format!("{}|{}", session_id, d)
    } else {
        session_id.to_string()
    }
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Left,
    Right,
}

#[derive(PartialEq, Clone, Copy)]
pub enum LeftPanel {
    Stats,
    Days,
    Models,
}

#[derive(PartialEq, Clone, Copy)]
pub enum RightPanel {
    Detail,   // OVERVIEW panel (top right in Stats view)
    Activity, // ACTIVITY heatmap panel
    List,     // SESSIONS/PROJECTS
    Tools,    // TOOLS USED
}

/// Cached panel rectangles for efficient mouse hit-testing
/// Updated during render to match exactly what's displayed
#[derive(Default, Clone)]
pub struct PanelRects {
    // Left panels
    pub stats: Option<Rect>,
    pub days: Option<Rect>,
    pub models: Option<Rect>,
    // Right panels (context-dependent based on left_panel)
    pub detail: Option<Rect>,   // SESSION INFO or MODEL INFO
    pub activity: Option<Rect>, // ACTIVITY heatmap (Stats view)
    pub list: Option<Rect>,     // SESSIONS or MODEL RANKING
    pub tools: Option<Rect>,    // TOOLS USED (only in Models view)
}

impl PanelRects {
    /// Optimized hit-test that returns early once a match is found
    #[inline(always)]
    pub fn find_panel(&self, x: u16, y: u16) -> Option<&'static str> {
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
pub struct HeatmapLayout {
    pub inner: Rect,
    pub label_w: u16,
    pub weeks: usize,
    pub grid_start: chrono::NaiveDate,
    pub week_w: u16,
    pub extra_cols: u16,
    pub grid_pad: u16,
}

/// Helper: Create a stat paragraph with label and value
pub fn stat_widget(label: &str, value: String, color: Color) -> Paragraph<'static> {
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
pub struct UsageRowFormat {
    pub name_width: usize,
    pub cost_width: usize,
    pub sess_width: usize,
}

/// Helper: Create a list row with consistent formatting for usage lists
/// Optimized with pre-allocated Vec capacity
pub fn usage_list_row(
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
pub fn safe_truncate_plain(s: &str, max_chars: usize) -> Cow<'_, str> {
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
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut count = 0;
    for (idx, _) in s.char_indices() {
        count += 1;
        if count > max_chars {
            let visible_chars = max_chars.saturating_sub(1);
            let byte_end = s
                .char_indices()
                .nth(visible_chars)
                .map(|(i, _)| i)
                .unwrap_or(idx);
            let mut result = String::with_capacity(byte_end + 3);
            result.push_str(&s[..byte_end]);
            result.push('…');
            return result;
        }
    }
    s.into()
}

/// Helper: Smart truncate for host names.
/// If the full name fits, show it. If not, show just the short name (before space/parenthesis) without ellipsis.
pub fn truncate_host_name(full_name: &str, short_name: &str, max_chars: usize) -> String {
    if full_name.chars().count() <= max_chars {
        full_name.to_string()
    } else {
        // Not enough space - show clean short name, no ellipsis as requested
        safe_truncate_plain(short_name, max_chars).into_owned()
    }
}

/// Calculate the actual number of rendered lines for a chat message
pub fn calculate_message_rendered_lines(msg: &ChatMessage) -> u16 {
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

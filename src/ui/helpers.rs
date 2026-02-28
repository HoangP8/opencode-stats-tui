//! UI helper functions and shared types.

use crate::stats::{format_number, ChatMessage, MessageContent};
use crate::theme::ThemeColors;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::borrow::Cow;

/// Cached chat session data.
pub struct CachedChat {
    pub messages: std::sync::Arc<Vec<ChatMessage>>,
    pub total_lines: u16,
}

/// Cache key from session_id and optional day.
pub fn cache_key(session_id: &str, day: Option<&str>) -> String {
    day.map_or_else(
        || session_id.to_string(),
        |d| format!("{}|{}", session_id, d),
    )
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
    Detail,
    Activity,
    List,
    Tools,
}

/// Cached panel rects for mouse hit-testing.
#[derive(Default, Clone)]
pub struct PanelRects {
    pub stats: Option<Rect>,
    pub days: Option<Rect>,
    pub models: Option<Rect>,
    pub detail: Option<Rect>,
    pub activity: Option<Rect>,
    pub list: Option<Rect>,
    pub tools: Option<Rect>,
}

impl PanelRects {
    #[inline(always)]
    pub fn find_panel(&self, x: u16, y: u16) -> Option<&'static str> {
        if self.contains(self.list, x, y) {
            return Some("list");
        }
        if self.contains(self.days, x, y) {
            return Some("days");
        }
        if self.contains(self.models, x, y) {
            return Some("models");
        }
        if self.contains(self.activity, x, y) {
            return Some("activity");
        }
        if self.contains(self.detail, x, y) {
            return Some("detail");
        }
        if self.contains(self.tools, x, y) {
            return Some("tools");
        }
        if self.contains(self.stats, x, y) {
            return Some("stats");
        }
        None
    }

    #[inline(always)]
    fn contains(&self, rect: Option<Rect>, x: u16, y: u16) -> bool {
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

#[derive(Clone, Copy)]
pub struct ModelTimelineLayout {
    pub inner: Rect,
    pub chart_y: u16,
    pub chart_h: u16,
    pub bars: usize,
    pub bar_w: u16,
    pub start_date: chrono::NaiveDate,
    pub bucket_days: i64,
}

/// Stat paragraph with label and value.
pub fn stat_widget(
    label: &str,
    value: String,
    color: Color,
    colors: &ThemeColors,
) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(Span::styled(
            label.to_string(),
            Style::default().fg(colors.text_muted),
        )),
        Line::from(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
    ])
    .alignment(Alignment::Center)
}

/// Usage list row formatting config.
pub struct UsageRowFormat {
    pub name_width: usize,
    pub cost_width: usize,
    pub sess_width: usize,
}

/// Create a usage list row.
pub fn usage_list_row(
    name: String,
    input: u64,
    output: u64,
    cost: f64,
    sessions: usize,
    fmt: &UsageRowFormat,
    colors: &ThemeColors,
    is_active: bool,
) -> Line<'static> {
    let name_display: String = name.chars().take(fmt.name_width).collect();
    let sep = if is_active {
        Style::default().fg(colors.border_focus)
    } else {
        Style::default().fg(colors.text_muted)
    };
    let label = Style::default().fg(colors.text_muted);

    Line::from(vec![
        Span::styled(
            format!("{:<1$}", name_display, fmt.name_width),
            Style::default().fg(colors.text_primary),
        ),
        Span::styled(" │ ", sep),
        Span::styled(
            format!("{:>7}", format_number(input)),
            Style::default().fg(colors.token_input()),
        ),
        Span::styled(" in ", label),
        Span::styled(
            format!("{:>7}", format_number(output)),
            Style::default().fg(colors.token_output()),
        ),
        Span::styled(" out", label),
        Span::styled(" │ ", sep),
        Span::styled(
            format!("${:>1$.2}", cost, fmt.cost_width),
            Style::default().fg(colors.cost()),
        ),
        Span::styled(" │ ", sep),
        Span::styled(
            format!("{:>1$} sess", sessions, fmt.sess_width),
            Style::default().fg(colors.info),
        ),
    ])
}

/// Safe truncate without breaking UTF-8.
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

/// Truncate with ellipsis.
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut count = 0;
    for (idx, _) in s.char_indices() {
        count += 1;
        if count > max_chars {
            let visible = max_chars.saturating_sub(1);
            let byte_end = s.char_indices().nth(visible).map(|(i, _)| i).unwrap_or(idx);
            let mut result = String::with_capacity(byte_end + 3);
            result.push_str(&s[..byte_end]);
            result.push('…');
            return result;
        }
    }
    s.into()
}

/// Smart truncate for host names.
pub fn truncate_host_name(full: &str, short: &str, max: usize) -> String {
    if full.chars().count() <= max {
        full.to_string()
    } else {
        safe_truncate_plain(short, max).into_owned()
    }
}

/// Calculate rendered lines for a chat message.
pub fn calculate_message_rendered_lines(msg: &ChatMessage) -> u16 {
    let mut lines = 1u16;
    for part in &msg.parts {
        match part {
            MessageContent::Text(text) => {
                let max_lines = match &*msg.role {
                    "user" => 5,
                    "assistant" => 8,
                    _ => 6,
                };
                let n = text.lines().count();
                lines += n.min(max_lines) as u16;
                if n > max_lines {
                    lines += 1;
                }
            }
            MessageContent::ToolCall(_) | MessageContent::Thinking(_) => lines += 1,
        }
    }
    lines
}

/// Month abbreviation (1-12).
#[inline]
pub fn month_abbr(m: u32) -> &'static str {
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

/// Weekday abbreviation.
#[inline]
pub fn weekday_abbr(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

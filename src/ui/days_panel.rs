//! Daily usage panel rendering.

use super::helpers::{truncate_host_name, truncate_with_ellipsis, usage_list_row, UsageRowFormat};
use crate::stats::{format_active_duration, format_number, format_number_full};
use crate::theme::FixedColors;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, Paragraph},
    Frame,
};

impl super::App {
    /// DAILY USAGE left panel.
    pub fn render_day_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        let inner_width = area.width.saturating_sub(2);
        if self.cached_day_items.is_empty()
            || self.cached_day_width != inner_width
            || self.cached_day_is_active != is_active
        {
            self.rebuild_day_list_cache(inner_width, is_active);
        }

        let colors = self.theme.colors();
        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
        };

        let list = List::new(self.cached_day_items.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if is_highlighted {
                        border_style
                    } else {
                        Style::default().fg(colors.border_default)
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
                            Style::default().fg(colors.text_muted),
                        ))
                        .alignment(Alignment::Center),
                    ),
            )
            .highlight_style(if is_active {
                Style::default()
                    .bg(colors.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .bg(colors.bg_primary)
                    .remove_modifier(Modifier::REVERSED | Modifier::BOLD)
            })
            .highlight_symbol(if is_active { "● " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.day_list_state);
    }

    /// Rebuild cached day list items.
    pub fn rebuild_day_list_cache(&mut self, width: u16, is_active: bool) {
        let colors = self.theme.colors();
        self.cached_day_width = width;
        self.cached_day_is_active = is_active;
        let cost_width = self.max_cost_width();
        let fixed = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + 9;
        let name_width = width.saturating_sub((fixed + 2).min(u16::MAX as usize) as u16) as usize;

        self.cached_day_items = self
            .day_list
            .iter()
            .map(|day| {
                let (sess, input, output, cost, duration) = self
                    .per_day
                    .get(day)
                    .map(|s| {
                        (
                            s.sessions.len(),
                            s.tokens.input,
                            s.tokens.output,
                            s.display_cost(),
                            s.sessions
                                .values()
                                .map(|s| s.active_duration_ms)
                                .sum::<i64>(),
                        )
                    })
                    .unwrap_or((0, 0, 0, 0.0, 0));

                let day_name = self
                    .cached_day_strings
                    .get(day)
                    .cloned()
                    .unwrap_or_else(|| day.clone());
                let dur_str = format_duration_short(duration / 1000);
                let name_with_dur = format!("{}{}", day_name, dur_str);

                ListItem::new(usage_list_row(
                    name_with_dur,
                    input,
                    output,
                    cost,
                    sess,
                    &UsageRowFormat {
                        name_width: name_width.max(8),
                        cost_width,
                        sess_width: 4,
                    },
                    &colors,
                    is_active,
                ))
            })
            .collect();
    }

    /// SESSION INFO right panel.
    pub fn render_session_detail(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let colors = self.theme.colors();
        let session = self
            .session_list_state
            .selected()
            .and_then(|i| self.session_list.get(i).cloned());

        let title = session
            .as_ref()
            .map(|s| {
                if s.is_continuation {
                    s.first_created_date
                        .as_ref()
                        .map(|d| format!(" {} [Continue from {}] ", s.id, d))
                        .unwrap_or_else(|| format!(" {} [Continued] ", s.id))
                } else {
                    format!(" {} ", s.id)
                }
            })
            .unwrap_or_else(|| " SESSION INFO ".into());

        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_highlighted {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(s) = session {
            self.render_session_info(frame, inner, &s, &colors);
        }
    }

    fn render_session_info(
        &mut self,
        frame: &mut Frame,
        inner: Rect,
        s: &crate::stats::SessionStat,
        colors: &crate::theme::ThemeColors,
    ) {
        let title = self
            .session_titles
            .get(&s.id)
            .map(|t| t.strip_prefix("New session - ").unwrap_or(t))
            .unwrap_or("Untitled");

        let project_str = if !s.path_root.is_empty() {
            s.path_root.clone()
        } else {
            s.path_cwd.clone()
        };
        let project = project_str.as_ref();

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(inner);

        let muted = Style::default().fg(colors.text_muted);
        let left_w = cols[0].width.saturating_sub(14) as usize;
        let mut left: Vec<Line> = Vec::with_capacity(8);

        left.push(Line::from(vec![
            Span::styled("Title        ", muted),
            Span::styled(
                truncate_with_ellipsis(title, left_w),
                Style::default()
                    .fg(colors.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        left.push(Line::from(vec![
            Span::styled("Project      ", muted),
            Span::styled(
                truncate_with_ellipsis(project, left_w),
                Style::default().fg(colors.accent_blue),
            ),
        ]));

        // Git branch
        let branch = match &self.cached_git_branch {
            Some((root, b)) if root.as_ref() == project => b.clone(),
            _ => {
                let b = crate::session::detect_git_branch(project);
                self.cached_git_branch = Some((project_str.clone(), b.clone()));
                b
            }
        };
        left.push(Line::from(vec![
            Span::styled("Branch       ", muted),
            Span::styled(
                branch
                    .as_deref()
                    .map(|b| truncate_with_ellipsis(b, left_w))
                    .unwrap_or_else(|| "n/a".into()),
                Style::default().fg(if branch.is_some() {
                    colors.info
                } else {
                    colors.text_muted
                }),
            ),
        ]));

        left.push(Line::from(vec![
            Span::styled("Last Active  ", muted),
            Span::styled(
                chrono::DateTime::from_timestamp(s.last_activity / 1000, 0)
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| "n/a".into()),
                Style::default().fg(colors.text_muted),
            ),
        ]));

        left.push(Line::from(vec![
            Span::styled("Duration     ", muted),
            Span::styled(
                format_active_duration(s.active_duration_ms),
                Style::default().fg(colors.accent_cyan),
            ),
        ]));

        // Agents
        if s.agents.is_empty() {
            left.push(Line::from(vec![
                Span::styled("Agents       ", muted),
                Span::styled("n/a", Style::default().fg(colors.text_muted)),
            ]));
        } else {
            let mut agents: Vec<(&str, bool, u64)> = s
                .agents
                .iter()
                .map(|a| (a.name.as_ref(), a.is_main, a.tokens.total()))
                .collect();
            agents.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
            agents.dedup_by(|a, b| a.0 == b.0);
            let names: Vec<&str> = agents.iter().map(|(n, _, _)| *n).collect();
            left.push(Line::from(vec![
                Span::styled("Agents       ", muted),
                Span::styled(
                    format_list_truncated(&names, left_w),
                    Style::default()
                        .fg(colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Models
        let mut models: Vec<&str> = s.models.iter().map(|m| m.as_ref()).collect();
        models.sort_unstable();
        if models.is_empty() {
            left.push(Line::from(vec![
                Span::styled("Models       ", muted),
                Span::styled("n/a", Style::default().fg(colors.text_muted)),
            ]));
        } else {
            left.push(Line::from(vec![
                Span::styled("Models       ", muted),
                Span::styled(
                    truncate_with_ellipsis(&format_list_truncated(&models, left_w), left_w),
                    Style::default()
                        .fg(colors.accent_magenta)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Host
        let device = crate::device::get_device_info();
        let type_color = if device.kind == "server" {
            colors.accent_orange
        } else {
            colors.accent_cyan
        };
        let label = device.display_label();
        let host_w = (cols[0].width as usize).saturating_sub(13 + label.len() + 3 + 1);
        left.push(Line::from(vec![
            Span::styled("Host:        ", muted),
            Span::styled(label, Style::default().fg(type_color)),
            Span::raw(" | "),
            Span::styled(
                truncate_host_name(&device.display_name(), &device.short_name(), host_w),
                Style::default().fg(type_color),
            ),
        ]));

        frame.render_widget(Paragraph::new(left), cols[0]);

        // Right: tokens
        let right = vec![
            Line::from(vec![
                Span::styled("Input         ", Style::default().fg(colors.token_input())),
                Span::styled(
                    format_number_full(s.tokens.input),
                    Style::default().fg(colors.token_input()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Output        ", Style::default().fg(colors.token_output())),
                Span::styled(
                    format_number_full(s.tokens.output),
                    Style::default().fg(colors.token_output()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Thinking      ", Style::default().fg(colors.thinking())),
                Span::styled(
                    format_number_full(s.tokens.reasoning),
                    Style::default().fg(colors.thinking()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cache Read    ", Style::default().fg(colors.cost())),
                Span::styled(
                    format_number_full(s.tokens.cache_read),
                    Style::default().fg(colors.cost()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cache Write   ", Style::default().fg(colors.cost())),
                Span::styled(
                    format_number_full(s.tokens.cache_write),
                    Style::default().fg(colors.cost()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Prompts       ", Style::default().fg(colors.info)),
                Span::styled(
                    format!("{}", s.prompts),
                    Style::default()
                        .fg(colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Responses     ", Style::default().fg(colors.success)),
                Span::styled(
                    format!("{}", s.messages.saturating_sub(s.prompts)),
                    Style::default()
                        .fg(colors.success)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cost          ", Style::default().fg(colors.cost())),
                Span::styled(
                    format!("${:.2}", s.display_cost()),
                    Style::default()
                        .fg(colors.cost())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(right), cols[1]);
    }

    /// SESSIONS right panel
    pub fn render_session_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        is_active: bool,
    ) {
        let inner_width = area.width.saturating_sub(2);
        if self.cached_session_width != inner_width
            || self.cached_session_items.is_empty()
            || self.cached_session_is_active != is_active
        {
            self.rebuild_cached_session_items(inner_width, is_active);
        }

        let colors = self.theme.colors();
        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
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
                            Style::default().fg(colors.text_muted),
                        ))
                        .alignment(Alignment::Center),
                    ),
            )
            .highlight_style(if is_active {
                Style::default()
                    .bg(colors.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .bg(colors.bg_primary)
                    .remove_modifier(Modifier::REVERSED | Modifier::BOLD)
            })
            .highlight_symbol(if is_active { "● " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.session_list_state);
    }

    /// Rebuild cached session list items
    pub fn rebuild_cached_session_items(&mut self, width: u16, is_active: bool) {
        let colors = self.theme.colors();
        let fixed = FixedColors::DEFAULT;
        self.cached_session_width = width;
        self.cached_session_is_active = is_active;

        let sep_color = if is_active {
            colors.border_focus
        } else {
            colors.text_muted
        };

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
                if s.models.len() == 1 {
                    7
                } else {
                    format!("{} models", s.models.len()).len()
                }
            })
            .max()
            .unwrap_or(7);
        let fixed_w = 3 + 8 + 3 + 8 + 3 + (max_cost_len + 1) + 3 + 8 + 3 + max_models_len + 2;
        let title_w = width.saturating_sub(fixed_w.min(u16::MAX as usize) as u16) as usize;

        self.cached_session_items = self
            .session_list
            .iter()
            .map(|s| {
                let title = self
                    .session_titles
                    .get(&s.id)
                    .map(|t| t.strip_prefix("New session - ").unwrap_or(t).to_string())
                    .unwrap_or_else(|| s.id.chars().take(14).collect());

                let model_text = if s.models.len() == 1 {
                    "1 model".into()
                } else {
                    format!("{} models", s.models.len())
                };
                let title_color = if s.is_continuation {
                    colors.text_muted
                } else {
                    colors.text_primary
                };

                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(
                            "{:<1$}",
                            title.chars().take(title_w.max(8)).collect::<String>(),
                            title_w.max(8)
                        ),
                        Style::default().fg(title_color),
                    ),
                    Span::styled(" │ ", Style::default().fg(sep_color)),
                    Span::styled(
                        format!("+{:>7}", format_number(s.diffs.additions)),
                        Style::default().fg(fixed.diff_add),
                    ),
                    Span::styled(" │ ", Style::default().fg(sep_color)),
                    Span::styled(
                        format!("-{:>7}", format_number(s.diffs.deletions)),
                        Style::default().fg(fixed.diff_remove),
                    ),
                    Span::styled(" │ ", Style::default().fg(sep_color)),
                    Span::styled(
                        format!("${:>1$.2}", s.display_cost(), max_cost_len),
                        Style::default().fg(colors.cost()),
                    ),
                    Span::styled(" │ ", Style::default().fg(sep_color)),
                    Span::styled(
                        format!("{:>4} msg", s.messages),
                        Style::default().fg(colors.info),
                    ),
                    Span::styled(" │ ", Style::default().fg(sep_color)),
                    Span::styled(
                        format!("{:>1$}", model_text, max_models_len),
                        Style::default().fg(colors.accent_magenta),
                    ),
                ]))
            })
            .collect();
    }
}

/// Format duration as short string
fn format_duration_short(secs: i64) -> String {
    if secs >= 3600 {
        format!(" · {}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!(" · {}m{}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!(" · {}s", secs)
    } else {
        String::new()
    }
}

/// Format list of names with truncation and overflow count.
fn format_list_truncated(names: &[&str], max_len: usize) -> String {
    let mut display = String::new();
    let mut shown = 0;
    for (i, name) in names.iter().enumerate() {
        let candidate = if i == 0 {
            name.to_string()
        } else {
            format!(", {}", name)
        };
        let overflow = names.len() - i - 1;
        let suffix_len = if overflow > 0 {
            format!(", +{}", overflow).len()
        } else {
            0
        };
        if display.len() + candidate.len() + suffix_len <= max_len || i == 0 {
            display.push_str(&candidate);
            shown += 1;
        } else {
            break;
        }
    }
    let remaining = names.len() - shown;
    if remaining > 0 {
        display.push_str(&format!(", +{}", remaining));
    }
    display
}

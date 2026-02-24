//! DAILY USAGE panel rendering
//!
//! Contains the day list panel (left side) and its associated right panels:
//! - SESSION INFO (detail)
//! - SESSIONS list

use super::helpers::{truncate_host_name, truncate_with_ellipsis, usage_list_row, UsageRowFormat};
use crate::stats::{format_active_duration, format_number, format_number_full};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem},
    Frame,
};

impl super::App {
    /// Render the DAILY USAGE left panel (day list)
    pub fn render_day_list(
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

    /// Rebuild the cached day list items
    pub fn rebuild_day_list_cache(&mut self, width: u16) {
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

    /// Render the SESSION INFO right panel (for Days view)
    pub fn render_session_detail(
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

    /// Render the SESSIONS right panel (for Days view)
    pub fn render_session_list(
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

    /// Rebuild the cached session list items
    pub fn rebuild_cached_session_items(&mut self, width: u16) {
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
}

// Need to import Paragraph for render_session_detail
use ratatui::widgets::Paragraph;

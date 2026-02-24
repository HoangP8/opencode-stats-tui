//! MODEL USAGE panel rendering
//!
//! Contains the model list panel (left side) and its associated right panels:
//! - MODEL INFO
//! - TOOLS USED
//! - MODEL RANKING

use super::helpers::{truncate_with_ellipsis, usage_list_row, UsageRowFormat};
use crate::stats::{format_number, format_number_full};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, Paragraph},
    Frame,
};

impl super::App {
    /// Render the MODEL USAGE left panel (model list)
    pub fn render_model_list(
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

    /// Rebuild the cached model list items
    pub fn rebuild_model_list_cache(&mut self, width: u16) {
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

    /// Render the MODEL DETAIL right panel (for Models view)
    /// Contains MODEL INFO, TOOLS USED, and MODEL RANKING
    pub fn render_model_detail(
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
        let info_focused = is_highlighted && self.right_panel == super::helpers::RightPanel::Detail;
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
        let tools_focused = is_highlighted && self.right_panel == super::helpers::RightPanel::Tools;
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
                // Fixed layout: 14 (name) + bar + 6 (count)
                // Fixed layout: " name " (16) + bar + " XXXXX" (6) = 22
                let bar_max = tools_inner.width.saturating_sub(22) as usize;

                self.model_tool_max_scroll =
                    (tools.len().saturating_sub(tools_inner.height as usize)) as u16;
                self.model_tool_scroll = self.model_tool_scroll.min(self.model_tool_max_scroll);

                // Soft pink color (not bright magenta)
                let bar_color = Color::Rgb(220, 100, 160);
                let empty_color = Color::Rgb(38, 42, 50);

                // Optimized: use single spans for filled and empty portions
                let lines: Vec<Line> = tools
                    .into_iter()
                    .map(|(name, count)| {
                        let width = ((*count as f64 / total as f64) * bar_max as f64) as usize;

                        let filled_str: String = " ".repeat(width);
                        let empty_str: String = " ".repeat(bar_max.saturating_sub(width));

                        Line::from(vec![
                            Span::styled(
                                format!(" {:<14} ", truncate_with_ellipsis(name, 14)),
                                Style::default().fg(Color::White),
                            ),
                            Span::styled(filled_str, Style::default().bg(bar_color)),
                            Span::styled(empty_str, Style::default().bg(empty_color)),
                            Span::styled(
                                format!(" {:>5}", count),
                                Style::default()
                                    .fg(Color::Rgb(220, 100, 160))
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
                let placeholder = "░".repeat(tools_inner.width.saturating_sub(2) as usize);
                let lines: Vec<Line> = (0..tools_inner.height)
                    .map(|_| {
                        Line::styled(
                            placeholder.clone(),
                            Style::default().fg(Color::Rgb(30, 30, 40)),
                        )
                    })
                    .collect();
                frame.render_widget(
                    Paragraph::new(lines).alignment(Alignment::Center),
                    tools_inner,
                );
            }
        }

        // --- 3. MODEL RANKING ---
        let ranking_focused =
            is_highlighted && self.right_panel == super::helpers::RightPanel::List;
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

                // Build continuous bar with background colors
                let bar_color = if is_selected {
                    Color::Rgb(0, 200, 200) // Cyan
                } else {
                    Color::Rgb(80, 80, 100) // Dim gray
                };
                let empty_color = Color::Rgb(38, 42, 50);

                // Optimized: use single spans for filled and empty portions
                let filled_str: String = " ".repeat(bar_width.min(bar_max_width));
                let empty_str: String = " ".repeat(bar_max_width.saturating_sub(bar_width));

                Line::from(vec![
                    Span::styled(filled_str, Style::default().bg(bar_color)),
                    Span::styled(empty_str, Style::default().bg(empty_color)),
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
}

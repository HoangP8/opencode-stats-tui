//! Model usage panel rendering.

use super::helpers::{truncate_with_ellipsis, usage_list_row, UsageRowFormat};
use crate::cost::estimate_cost;
use crate::stats::{format_number, format_number_full};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

impl super::App {
    /// MODEL USAGE left panel.
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

        let colors = self.theme.colors();
        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
        };

        let list = List::new(self.cached_model_items.clone())
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
            })
            .highlight_symbol(if is_active { "▶ " } else { "  " })
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.model_list_state);
    }

    /// Rebuild cached model list items.
    pub fn rebuild_model_list_cache(&mut self, width: u16) {
        let colors = self.theme.colors();
        self.cached_model_width = width;
        let cost_width = self.max_cost_width();
        let fixed = 3 + 7 + 4 + 7 + 4 + 3 + (cost_width + 1) + 3 + 9;
        let name_width = width.saturating_sub((fixed + 2).min(u16::MAX as usize) as u16) as usize;

        self.cached_model_items = self
            .model_usage
            .iter()
            .map(|m| {
                ListItem::new(usage_list_row(
                    m.name.to_string(),
                    m.tokens.input,
                    m.tokens.output,
                    m.cost,
                    m.sessions.len(),
                    &UsageRowFormat {
                        name_width: name_width.max(8),
                        cost_width,
                        sess_width: 4,
                    },
                    &colors,
                ))
            })
            .collect();
    }

    /// MODEL DETAIL right panel.
    pub fn render_model_detail(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let colors = self.theme.colors();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(0)])
            .split(area);

        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);

        self.cached_rects.detail = Some(chunks[0]);
        self.cached_rects.tools = Some(bottom[0]);
        self.cached_rects.list = Some(bottom[1]);

        let selected_data = self.selected_model_index.and_then(|i| {
            let m = self.model_usage.get(i)?;
            Some((
                m.name.to_string(),
                m.sessions.len(),
                m.messages,
                m.cost,
                m.tokens,
                m.agents.clone(),
                m.tools.clone(),
            ))
        });

        // MODEL INFO
        let info_focused = is_highlighted && self.right_panel == super::helpers::RightPanel::Detail;
        let title = selected_data
            .as_ref()
            .map(|d| format!(" {} ", d.0))
            .unwrap_or_else(|| " MODEL INFO ".into());
        let info_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if info_focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(if info_focused {
                            colors.border_focus
                        } else {
                            colors.border_default
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = info_block.inner(chunks[0]);
        frame.render_widget(info_block, chunks[0]);

        if let Some((name, sessions, messages, cost, tokens, agents, _)) = &selected_data {
            self.render_model_info(
                frame, inner, name, *sessions, *messages, *cost, tokens, agents, &colors,
            );
        }

        // TOOLS USED
        let tools_focused = is_highlighted && self.right_panel == super::helpers::RightPanel::Tools;
        let tools = selected_data.as_ref().map(|d| &d.6);
        self.render_tools_panel(
            frame,
            bottom[0],
            border_style,
            tools,
            &colors,
            tools_focused,
        );

        // MODEL RANKING
        let rank_focused = is_highlighted && self.right_panel == super::helpers::RightPanel::List;
        self.render_ranking_panel(
            frame,
            bottom[1],
            border_style,
            self.selected_model_index,
            &colors,
            rank_focused,
        );
    }

    fn render_model_info(
        &self,
        frame: &mut Frame,
        inner: Rect,
        model_name: &str,
        sessions: usize,
        messages: u64,
        cost: f64,
        tokens: &crate::stats::Tokens,
        agents: &FxHashMap<Box<str>, u64>,
        colors: &crate::theme::ThemeColors,
    ) {
        let mut agent_vec: Vec<_> = agents.iter().collect();
        agent_vec.sort_unstable_by(|a, b| b.1.cmp(a.1));

        let (show_agents, show_tokens) = match inner.width {
            w if w < 45 => (false, false),
            w if w < 80 => (false, true),
            _ => (true, true),
        };

        let constraints = match (show_agents, show_tokens) {
            (true, true) => vec![
                Constraint::Percentage(25),
                Constraint::Percentage(37),
                Constraint::Percentage(38),
            ],
            (false, true) => vec![Constraint::Percentage(45), Constraint::Percentage(55)],
            _ => vec![Constraint::Percentage(100)],
        };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(inner);
        let muted = Style::default().fg(colors.text_muted);
        let col_w = cols.get(1).map(|c| c.width as usize).unwrap_or(0);

        let est = estimate_cost(model_name, tokens);
        let savings = est.map(|e| e - cost);

        let left = vec![
            Line::from(vec![
                Span::styled("Sessions  ", muted),
                Span::styled(
                    format!("{}", sessions),
                    Style::default()
                        .fg(colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Responses ", muted),
                Span::styled(
                    format!("{}", messages),
                    Style::default()
                        .fg(colors.success)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cost      ", muted),
                Span::styled(
                    format!("${:.2}", cost),
                    Style::default()
                        .fg(colors.cost())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Est. Cost ", muted),
                Span::styled(
                    est.map_or("$0.00".into(), |c| format!("${:.2}", c)),
                    Style::default()
                        .fg(est
                            .filter(|&c| c > 0.0)
                            .map_or(colors.text_muted, |_| colors.accent_orange))
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Savings   ", muted),
                Span::styled(
                    savings.map_or("$0.00".into(), |s| format!("${:.2}", s)),
                    Style::default()
                        .fg(savings
                            .filter(|&s| s > 0.0)
                            .map_or(colors.text_muted, |_| colors.success))
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(left), cols[0]);

        if show_agents {
            let agent_lines = if agent_vec.is_empty() {
                vec![Line::from(vec![
                    Span::styled("Agents: ", muted),
                    Span::styled("n/a", Style::default().fg(colors.text_muted)),
                ])]
            } else {
                let mut lines: Vec<Line> = agent_vec
                    .iter()
                    .take(5)
                    .enumerate()
                    .map(|(i, (a, c))| {
                        let prefix = if i == 0 { "Agents: " } else { "        " };
                        let text = format!("{} ({} msg)", a.as_ref(), c);
                        Line::from(vec![
                            Span::styled(prefix, muted),
                            Span::styled(
                                truncate_with_ellipsis(
                                    &text,
                                    col_w.saturating_sub(prefix.len() + 1),
                                ),
                                Style::default().fg(colors.accent_magenta),
                            ),
                        ])
                    })
                    .collect();
                if agent_vec.len() > 5 {
                    lines[4] = Line::from(vec![
                        Span::styled("        ", muted),
                        Span::styled("...", Style::default().fg(colors.accent_magenta)),
                    ]);
                }
                lines
            };
            frame.render_widget(Paragraph::new(agent_lines), cols[1]);
        }

        if show_tokens {
            let token_lines = vec![
                Line::from(vec![
                    Span::styled("Input         ", Style::default().fg(colors.token_input())),
                    Span::styled(
                        format_number_full(tokens.input),
                        Style::default().fg(colors.token_input()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Output        ", Style::default().fg(colors.token_output())),
                    Span::styled(
                        format_number_full(tokens.output),
                        Style::default().fg(colors.token_output()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Thinking      ", Style::default().fg(colors.thinking())),
                    Span::styled(
                        format_number_full(tokens.reasoning),
                        Style::default().fg(colors.thinking()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cache Read    ", Style::default().fg(colors.cost())),
                    Span::styled(
                        format_number_full(tokens.cache_read),
                        Style::default().fg(colors.cost()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Cache Write   ", Style::default().fg(colors.cost())),
                    Span::styled(
                        format_number_full(tokens.cache_write),
                        Style::default().fg(colors.cost()),
                    ),
                ]),
            ];
            frame.render_widget(
                Paragraph::new(token_lines),
                cols[if show_agents { 2 } else { 1 }],
            );
        }
    }

    fn render_tools_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        tools: Option<&FxHashMap<Box<str>, u64>>,
        colors: &crate::theme::ThemeColors,
        focused: bool,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    " TOOLS USED ",
                    Style::default()
                        .fg(if focused {
                            colors.border_focus
                        } else {
                            colors.border_default
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(tools) = tools else { return };

        if tools.is_empty() {
            let placeholder = "░".repeat(inner.width.saturating_sub(2) as usize);
            let lines: Vec<Line> = (0..inner.height)
                .map(|_| Line::styled(placeholder.clone(), Style::default().fg(colors.bg_primary)))
                .collect();
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
            return;
        }

        let mut items: Vec<_> = tools.iter().collect();
        items.sort_unstable_by(|a, b| b.1.cmp(a.1));
        let total: u64 = items.iter().map(|(_, c)| **c).sum();
        let bar_max = inner.width.saturating_sub(22) as usize;

        self.model_tool_max_scroll = items.len().saturating_sub(inner.height as usize) as u16;
        self.model_tool_scroll = self.model_tool_scroll.min(self.model_tool_max_scroll);

        let lines: Vec<Line> = items
            .into_iter()
            .map(|(name, count)| {
                let w = ((*count as f64 / total as f64) * bar_max as f64) as usize;
                Line::from(vec![
                    Span::styled(
                        format!(" {:<14} ", truncate_with_ellipsis(name, 14)),
                        Style::default().fg(colors.text_primary),
                    ),
                    Span::styled(" ".repeat(w), Style::default().bg(colors.accent_pink)),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(w)),
                        Style::default().bg(colors.bg_tertiary),
                    ),
                    Span::styled(
                        format!(" {:>5}", count),
                        Style::default()
                            .fg(colors.accent_pink)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(
            Paragraph::new(lines).scroll((self.model_tool_scroll, 0)),
            inner,
        );
    }

    fn render_ranking_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        selected_idx: Option<usize>,
        colors: &crate::theme::ThemeColors,
        focused: bool,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    " MODEL RANKING ",
                    Style::default()
                        .fg(if focused {
                            colors.border_focus
                        } else {
                            colors.border_default
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut ranked: Vec<_> = self.model_usage.iter().enumerate().collect();
        ranked.sort_unstable_by(|a, b| b.1.tokens.total().cmp(&a.1.tokens.total()));

        self.ranking_max_scroll = ranked.len().saturating_sub(inner.height as usize);
        self.ranking_scroll = self.ranking_scroll.min(self.ranking_max_scroll);

        let grand: u64 = self.model_usage.iter().map(|m| m.tokens.total()).sum();
        let max_tok_len = self
            .model_usage
            .iter()
            .map(|m| format_number(m.tokens.total()).len())
            .max()
            .unwrap_or(1);
        let bar_avail = inner.width.saturating_sub(2);

        let lines: Vec<Line> = ranked
            .iter()
            .map(|(idx, m)| {
                let sel = selected_idx == Some(*idx);
                let pct = if grand > 0 {
                    (m.tokens.total() as f64 / grand as f64) * 100.0
                } else {
                    0.0
                };
                let suffix = format!(
                    " {:>5.1}% ({:>w$})",
                    pct,
                    format_number(m.tokens.total()),
                    w = max_tok_len
                );
                let bar_max = bar_avail.saturating_sub(suffix.chars().count() as u16) as usize;
                let bar_w = if grand > 0 {
                    ((m.tokens.total() as f64 / grand as f64) * bar_max as f64) as usize
                } else {
                    0
                };

                Line::from(vec![
                    Span::styled(
                        " ".repeat(bar_w.min(bar_max)),
                        Style::default().bg(if sel { colors.info } else { colors.bg_tertiary }),
                    ),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(bar_w)),
                        Style::default().bg(colors.bg_tertiary),
                    ),
                    Span::styled(
                        suffix,
                        Style::default()
                            .fg(if sel {
                                colors.accent_yellow
                            } else {
                                colors.text_muted
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        if let Some(idx) = selected_idx {
            if let Some(pos) = ranked.iter().position(|(i, _)| *i == idx) {
                let h = inner.height as usize;
                if h > 0 && (pos >= self.ranking_scroll + h || pos < self.ranking_scroll) {
                    self.ranking_scroll =
                        pos.saturating_sub(h / 2).min(lines.len().saturating_sub(h));
                }
            }
        }

        let visible: Vec<Line> = lines
            .into_iter()
            .skip(self.ranking_scroll)
            .take(inner.height as usize)
            .collect();
        frame.render_widget(Paragraph::new(visible), inner);
    }
}

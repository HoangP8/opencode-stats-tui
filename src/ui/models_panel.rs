//! Model usage panel rendering.

use super::helpers::{truncate_with_ellipsis, usage_list_row, UsageRowFormat};
use crate::cost::{estimate_cost, lookup_pricing};
use crate::stats::{format_number, format_number_full};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
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
        if self.cached_model_items.is_empty()
            || self.cached_model_width != inner_width
            || self.cached_model_is_active != is_active
        {
            self.rebuild_model_list_cache(inner_width, is_active);
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
    pub fn rebuild_model_list_cache(&mut self, width: u16, is_active: bool) {
        let colors = self.theme.colors();
        self.cached_model_width = width;
        self.cached_model_is_active = is_active;
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
                    is_active,
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
            .constraints([
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Min(5),
            ])
            .split(area);

        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2]);

        self.cached_rects.detail = Some(chunks[0]);
        self.cached_rects.activity = Some(chunks[1]);
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
                frame,
                inner,
                name,
                *sessions,
                *messages,
                *cost,
                tokens,
                agents,
                &colors,
                info_focused,
            );
        }

        let timeline_focused =
            is_highlighted && self.right_panel == super::helpers::RightPanel::Activity;
        self.render_model_timeline(frame, chunks[1], border_style, &colors, timeline_focused);

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
        focused: bool,
    ) {
        if inner.height < 3 {
            return;
        }
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(inner);

        let mut agent_vec: Vec<_> = agents.iter().collect();
        agent_vec.sort_unstable_by(|a, b| b.1.cmp(a.1));

        let (show_agents, show_tokens) = match sections[0].width {
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
            .split(sections[0]);
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

        let pricing_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(sections[1]);

        let sep_color = if focused {
            colors.border_focus
        } else {
            colors.text_muted
        };
        let label = " OpenRouter Price ";
        let sep_w = pricing_rows[0].width as usize;
        let label_len = label.chars().count();
        let pad = 10usize;
        let dash_total = sep_w.saturating_sub(label_len + pad * 2);
        let dash_left = dash_total / 2;
        let dash_right = dash_total.saturating_sub(dash_left);
        let sep_style = Style::default().fg(sep_color);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled("╌".repeat(dash_left), sep_style),
                Span::styled(label, sep_style.add_modifier(Modifier::BOLD)),
                Span::styled("╌".repeat(dash_right), sep_style),
                Span::raw(" ".repeat(pad)),
            ])),
            pricing_rows[0],
        );

        let pricing_line =
            self.model_price_line(model_name, pricing_rows[1].width as usize, colors);
        frame.render_widget(
            Paragraph::new(Line::from(pricing_line)).alignment(Alignment::Center),
            pricing_rows[1],
        );
    }

    fn model_price_line(
        &self,
        model_name: &str,
        width: usize,
        colors: &crate::theme::ThemeColors,
    ) -> Vec<Span<'static>> {
        let muted = Style::default().fg(colors.text_muted);
        let Some(p) = lookup_pricing(model_name) else {
            return vec![Span::styled("n/a", Style::default().fg(colors.text_muted))];
        };

        let mut spans = vec![
            Span::styled(
                format!("In ${:.2}/M", p.prompt * 1_000_000.0),
                Style::default()
                    .fg(colors.token_input())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" • ", muted),
            Span::styled(
                format!("Out ${:.2}/M", p.completion * 1_000_000.0),
                Style::default()
                    .fg(colors.token_output())
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        let extras = [
            (
                "Cache R",
                p.input_cache_read,
                Style::default()
                    .fg(colors.cost())
                    .add_modifier(Modifier::BOLD),
            ),
            (
                "Cache W",
                p.input_cache_write,
                Style::default()
                    .fg(colors.cost())
                    .add_modifier(Modifier::BOLD),
            ),
            (
                "Think",
                p.reasoning,
                Style::default()
                    .fg(colors.thinking())
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        let mut used = spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum::<usize>();
        for (name, rate, style) in extras {
            let part = format!("{} ${:.2}/M", name, rate * 1_000_000.0);
            let need = 3 + part.chars().count();
            if used + need > width.saturating_sub(1) {
                break;
            }
            spans.push(Span::styled(" • ", muted));
            spans.push(Span::styled(part, style));
            used += need;
        }
        spans
    }

    fn render_model_timeline(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        colors: &crate::theme::ThemeColors,
        focused: bool,
    ) {
        let approx_inner_w = area.width.saturating_sub(2);
        let approx_chart_w = approx_inner_w.saturating_sub(36 + 1).max(4);
        let approx_days = (approx_chart_w as usize).max(1);
        let title = format!(" ACTIVITY (LAST {} DAYS) ", approx_days);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(if focused {
                            colors.border_focus
                        } else {
                            colors.border_default
                        })
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if focused {
                        " Click a bar to inspect usage "
                    } else {
                        " "
                    },
                    Style::default().fg(colors.text_muted),
                ))
                .alignment(Alignment::Center),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(idx) = self.selected_model_index else {
            self.model_timeline_layout = None;
            return;
        };
        let Some(model) = self.model_usage.get(idx) else {
            self.model_timeline_layout = None;
            return;
        };

        if inner.width < 41 || inner.height < 7 {
            self.model_timeline_layout = None;
            return;
        }

        let mut points: Vec<(chrono::NaiveDate, u64)> = model
            .daily_tokens
            .iter()
            .filter_map(|(d, v)| {
                chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .ok()
                    .map(|nd| (nd, *v))
            })
            .collect();
        if points.is_empty() {
            self.model_timeline_layout = None;
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    "No daily model data available.",
                    Style::default().fg(colors.text_muted),
                )]))
                .alignment(Alignment::Center),
                inner,
            );
            return;
        }
        points.sort_unstable_by_key(|(d, _)| *d);

        let stats_col_w = 36u16;
        let sep_w = 1u16;
        let chart_w = inner.width.saturating_sub(stats_col_w + sep_w).max(4);
        let chart_h = inner.height;

        let stats_area = Rect::new(inner.x, inner.y, stats_col_w, chart_h);
        let chart_area = Rect::new(inner.x + stats_col_w + sep_w, inner.y, chart_w, chart_h);

        // Vertical separator between columns
        let sep_x = inner.x + stats_col_w;
        let sep_color = if focused {
            colors.border_focus
        } else {
            colors.text_muted
        };
        let sep_lines: Vec<Line> = (0..chart_h)
            .map(|_| Line::from(Span::styled("│", Style::default().fg(sep_color))))
            .collect();
        frame.render_widget(
            Paragraph::new(sep_lines),
            Rect::new(sep_x, inner.y, 1, chart_h),
        );

        // Left column stats
        let total_tokens = model.tokens.total();
        let num_sessions = model.sessions.len().max(1) as u64;
        let total_cost = model.cost;
        let active_days = points.len();

        // Last used
        let last_used = points.last().map(|(d, _)| *d);
        let last_used_str = last_used
            .map(|d| d.format("%b %d, %Y").to_string())
            .unwrap_or_else(|| "—".to_string());

        // Active range
        let first_day = points.first().map(|(d, _)| *d);
        let range_str = match (first_day, last_used) {
            (Some(start), Some(end)) => {
                let days = (end - start).num_days() + 1;
                format!(
                    "{} → {} ({}d)",
                    start.format("%b %d"),
                    end.format("%b %d"),
                    days
                )
            }
            _ => "—".to_string(),
        };

        // Active days
        let active_days_str = format!("{} days", active_days);

        // Peak day
        let (peak_date, peak_tokens_val) = points
            .iter()
            .max_by_key(|(_, t)| *t)
            .map(|(d, t)| (*d, *t))
            .unwrap_or((chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(), 0));
        let peak_str = format!(
            "{} ({})",
            peak_date.format("%b %d"),
            format_compact(peak_tokens_val)
        );

        // Avg tokens per session
        let avg_tokens_per_session = total_tokens / num_sessions;
        let avg_token_str = format!("{} tok/sess", format_compact(avg_tokens_per_session));

        // Avg cost per session
        let avg_cost_per_session = total_cost / num_sessions as f64;
        let avg_cost_str = format!("{}/sess", format!("${:.2}", avg_cost_per_session));

        // Selected day info
        let sel_day_str = self
            .model_timeline_selected_day
            .as_ref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .map(|nd| nd.format("%b %d").to_string())
            .unwrap_or_else(|| "—".to_string());
        let sel_day_info = format!(
            "{} {} ({:.1}%)",
            sel_day_str,
            format_compact(self.model_timeline_selected_tokens),
            self.model_timeline_selected_pct
        );

        // Render stats
        let stats_lines = vec![
            Line::from(vec![
                Span::styled("Last Used   ", Style::default().fg(colors.text_muted)),
                Span::styled(&last_used_str, Style::default().fg(colors.text_primary)),
            ]),
            Line::from(vec![
                Span::styled("Active      ", Style::default().fg(colors.text_muted)),
                Span::styled(&range_str, Style::default().fg(colors.text_primary)),
            ]),
            Line::from(vec![
                Span::styled("Active Days ", Style::default().fg(colors.text_muted)),
                Span::styled(&active_days_str, Style::default().fg(colors.text_primary)),
            ]),
            Line::from(vec![
                Span::styled("Peak        ", Style::default().fg(colors.text_muted)),
                Span::styled(&peak_str, Style::default().fg(colors.success)),
            ]),
            Line::from(vec![
                Span::styled("Avg Token   ", Style::default().fg(colors.text_muted)),
                Span::styled(&avg_token_str, Style::default().fg(colors.text_primary)),
            ]),
            Line::from(vec![
                Span::styled("Avg Cost    ", Style::default().fg(colors.text_muted)),
                Span::styled(&avg_cost_str, Style::default().fg(colors.cost())),
            ]),
            Line::from(vec![
                Span::styled("Selected    ", Style::default().fg(colors.accent_cyan)),
                Span::styled(
                    &sel_day_info,
                    Style::default()
                        .fg(colors.accent_cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(stats_lines), stats_area);

        // Right column: bar chart
        let mut global_end = self
            .per_day
            .keys()
            .filter_map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .max()
            .unwrap_or_else(|| chrono::Local::now().date_naive());

        let bars = (chart_w as usize).max(1);
        let bucket_days = 1i64;
        let span_days = (bars as i64) * bucket_days;
        let mut window_start = global_end - chrono::Duration::days(span_days.saturating_sub(1));

        // Fall back to model's own window if no activity in global window
        if !points
            .iter()
            .any(|(d, _)| *d >= window_start && *d <= global_end)
        {
            let model_end = points.last().map(|(d, _)| *d).unwrap_or(global_end);
            global_end = model_end;
            window_start = global_end - chrono::Duration::days(span_days.saturating_sub(1));
        }

        self.model_timeline_layout = Some(super::helpers::ModelTimelineLayout {
            inner: chart_area,
            chart_y: chart_area.y,
            chart_h,
            bars,
            bar_w: 1,
            start_date: window_start,
            bucket_days,
        });

        // Aggregate tokens per bucket
        let mut model_sums = vec![0u64; bars];
        for (d, t) in &points {
            if *d < window_start || *d > global_end {
                continue;
            }
            let rel = (*d - window_start).num_days().max(0);
            let b = ((rel / bucket_days) as usize).min(bars - 1);
            model_sums[b] += *t;
        }
        let max_model = model_sums.iter().copied().max().unwrap_or(1);

        // Selected bar
        let selected_bar = self
            .model_timeline_selected_day
            .as_deref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .map(|d| {
                let rel = (d - window_start).num_days().max(0);
                ((rel / bucket_days) as usize).min(bars - 1)
            })
            .unwrap_or_else(|| {
                model_sums
                    .iter()
                    .rposition(|v| *v > 0)
                    .unwrap_or(bars.saturating_sub(1))
            });

        // Update selected day info
        let sel_day = window_start + chrono::Duration::days(selected_bar as i64);
        let sel_tokens = model_sums[selected_bar];
        let sel_pct = if total_tokens > 0 {
            (sel_tokens as f64 / total_tokens as f64) * 100.0
        } else {
            0.0
        };
        self.model_timeline_selected_day = Some(sel_day.format("%Y-%m-%d").to_string());
        self.model_timeline_selected_tokens = sel_tokens;
        self.model_timeline_selected_pct = sel_pct;

        // Flash effect
        let flash = self.model_timeline_flash_time.map(|t| {
            (1.0 - (t.elapsed().as_millis() as f64 * std::f64::consts::TAU / 600.0).cos()) * 0.2
        });

        // Bar color (consistent, no gradient)
        let bar_color = colors.accent_cyan;
        let empty_color = colors.bg_tertiary;

        // Solid bars with round() for best accuracy at 7 discrete levels
        let mut lines: Vec<Line> = Vec::with_capacity(chart_h as usize);

        for row in 0..chart_h {
            let mut spans: Vec<Span> = Vec::with_capacity(bars);
            let from_bottom = chart_h - 1 - row;

            for i in 0..bars {
                let val = model_sums[i];
                let bar_height = if val == 0 {
                    0
                } else {
                    ((val as f64 / max_model as f64) * chart_h as f64)
                        .round()
                        .max(1.0) as u16
                };
                let filled = from_bottom < bar_height;
                let sel = i == selected_bar;

                let c = if filled {
                    if sel {
                        if let (Some(f), Color::Rgb(r, g, b)) = (flash, bar_color) {
                            Color::Rgb(
                                (r as f64 + (255.0 - r as f64) * f) as u8,
                                (g as f64 + (255.0 - g as f64) * f) as u8,
                                (b as f64 + (255.0 - b as f64) * f) as u8,
                            )
                        } else {
                            bar_color
                        }
                    } else {
                        bar_color
                    }
                } else {
                    empty_color
                };

                spans.push(Span::styled(" ", Style::default().bg(c)));
            }
            lines.push(Line::from(spans));
        }

        frame.render_widget(Paragraph::new(lines), chart_area);
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
            )
            .title_bottom(
                Line::from(Span::styled(
                    if focused { " ↑↓: scroll " } else { " " },
                    Style::default().fg(colors.text_muted),
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
            )
            .title_bottom(
                Line::from(Span::styled(
                    if focused { " ↑↓: scroll " } else { " " },
                    Style::default().fg(colors.text_muted),
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

fn format_compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{:.1}B", n as f64 / 1_000_000_000.0),
    }
}

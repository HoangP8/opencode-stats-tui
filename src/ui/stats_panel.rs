//! Stats panel rendering.

use super::helpers::{
    month_abbr, stat_widget, truncate_with_ellipsis, ActivityView, HeatmapLayout,
    WeeklyHeatmapLayout,
};
use crate::stats::format_number;

use chrono::Datelike;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

impl super::App {
    /// GENERAL USAGE left panel.
    pub fn render_stats_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let colors = self.theme.colors();
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
                    " GENERAL USAGE ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(24),
                Constraint::Length(1),
                Constraint::Percentage(18),
                Constraint::Percentage(18),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        // Separators - use border_focus when highlighted
        let sep_style = if is_highlighted {
            Style::default().fg(colors.border_focus)
        } else {
            Style::default().fg(colors.text_muted)
        };
        for &i in &[1, 4] {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled("│", sep_style)),
                    Line::from(Span::styled("│", sep_style)),
                    Line::from(Span::styled("│", sep_style)),
                    Line::from(Span::styled("│", sep_style)),
                ]),
                cols[i],
            );
        }

        let total_responses = self.totals.messages.saturating_sub(self.totals.prompts);

        // Col 1: Sessions / Cost
        let c1 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[0]);
        frame.render_widget(
            stat_widget(
                "Sessions",
                format!("{}", self.totals.sessions.len()),
                colors.session,
                &colors,
            ),
            c1[0],
        );
        frame.render_widget(
            stat_widget(
                "Cost",
                format!("${:.2}", self.totals.display_cost()),
                colors.cost(),
                &colors,
            ),
            c1[1],
        );

        // Col 2: Input / Output
        let c2 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[2]);
        frame.render_widget(
            stat_widget(
                "Input",
                format_number(self.totals.tokens.input),
                colors.token_input(),
                &colors,
            ),
            c2[0],
        );
        frame.render_widget(
            stat_widget(
                "Output",
                format_number(self.totals.tokens.output),
                colors.token_output(),
                &colors,
            ),
            c2[1],
        );

        // Col 3: Thinking / Cache
        let c3 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[3]);
        frame.render_widget(
            stat_widget(
                "Thinking",
                format_number(self.totals.tokens.reasoning),
                colors.thinking(),
                &colors,
            ),
            c3[0],
        );
        frame.render_widget(
            stat_widget(
                "Cache",
                format_number(self.totals.tokens.cache_read + self.totals.tokens.cache_write),
                colors.cache_read,
                &colors,
            ),
            c3[1],
        );

        // Col 4: Lines / Messages
        let c4 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[5]);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Line Changes",
                    Style::default().fg(colors.text_primary),
                )),
                Line::from(vec![
                    Span::styled(
                        format!("+{}", format_number(self.totals.diffs.additions)),
                        Style::default()
                            .fg(colors.add_line)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" / ", Style::default().fg(colors.text_muted)),
                    Span::styled(
                        format!("-{}", format_number(self.totals.diffs.deletions)),
                        Style::default()
                            .fg(colors.remove_line)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ])
            .alignment(Alignment::Center),
            c4[0],
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "User / Agent Messages",
                    Style::default().fg(colors.text_primary),
                )),
                Line::from(vec![
                    Span::styled(
                        format!("{}", self.totals.prompts),
                        Style::default()
                            .fg(colors.user)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" / ", Style::default().fg(colors.text_muted)),
                    Span::styled(
                        format!("{}", total_responses),
                        Style::default()
                            .fg(colors.agent_general)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ])
            .alignment(Alignment::Center),
            c4[1],
        );
    }

    /// OVERVIEW right panel
    pub fn render_overview_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        // Calculate stats (5s cache)
        let stats = self.overview_stats_cache.get(
            &self.per_day,
            &self.model_usage,
            self.totals.display_cost(),
        );

        let colors = self.theme.colors();
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
                    " OVERVIEW ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let _muted = Style::default().fg(colors.text_muted);
        let secondary = Style::default().fg(colors.text_secondary);
        let sep_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.text_muted
        };
        let sep_style = Style::default().fg(sep_color);

        const SEP_W: u16 = 1;

        // Calculate actual content widths needed
        let col0_content_width = 15 + stats.peak_day.len().max(10);
        let col1_content_width = 15 + stats.avg_sessions.len().max(12);
        let col2_min_width = 18;

        // Total width needed for all 3 columns
        let total_3col_needed =
            col0_content_width + col1_content_width + col2_min_width + 2 * SEP_W as usize;

        // Hide Languages column if content doesn't fit
        let show_languages = (inner.width as usize) >= total_3col_needed;
        let num_separators = if show_languages { 2 } else { 1 };
        let available_width = inner.width.saturating_sub(num_separators * SEP_W);

        let cols = if show_languages {
            // 3 columns: 37% : 35% : 28%
            let col0_w = ((available_width as f32 * 0.37) as u16).max(col0_content_width as u16);
            let col1_w = ((available_width as f32 * 0.35) as u16).max(col1_content_width as u16);
            let col2_w = available_width
                .saturating_sub(col0_w + col1_w)
                .max(col2_min_width as u16);
            vec![
                Rect::new(inner.x, inner.y, col0_w, inner.height),
                Rect::new(inner.x + col0_w + SEP_W, inner.y, col1_w, inner.height),
                Rect::new(
                    inner.x + col0_w + SEP_W + col1_w + SEP_W,
                    inner.y,
                    col2_w,
                    inner.height,
                ),
            ]
        } else {
            // 2 columns
            let col0_w = ((available_width as f32 * 0.52) as u16)
                .max(col0_content_width as u16)
                .min(available_width - col1_content_width as u16);
            let col1_w = available_width.saturating_sub(col0_w);
            vec![
                Rect::new(inner.x, inner.y, col0_w, inner.height),
                Rect::new(inner.x + col0_w + SEP_W, inner.y, col1_w, inner.height),
            ]
        };

        // ========== Column 1: Core Stats ==========
        // All labels aligned to "Total Savings" length (13 chars) + 2 spaces indent
        let col1_lines = vec![
            Line::from(vec![
                Span::styled("Peak Day       ", secondary),
                Span::styled(
                    &stats.peak_day,
                    Style::default()
                        .fg(colors.day_stats)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Start Day      ", secondary),
                Span::styled(&stats.start_day, Style::default().fg(colors.day_stats)),
            ]),
            Line::from(vec![
                Span::styled("Active Days    ", secondary),
                Span::styled(&stats.active_days, Style::default().fg(colors.day_stats)),
            ]),
            Line::from(vec![
                Span::styled("Longest Sess   ", secondary),
                Span::styled(&stats.longest_session, Style::default().fg(colors.session)),
            ]),
            Line::from(vec![
                Span::styled("Total Time     ", secondary),
                Span::styled(
                    &stats.total_active_time,
                    Style::default()
                        .fg(colors.total_time)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Total Savings  ", secondary),
                Span::styled(&stats.total_savings, Style::default().fg(colors.savings)),
            ]),
        ];
        frame.render_widget(Paragraph::new(col1_lines), cols[0]);

        // ========== Column 2: Averages & Patterns ==========
        let col2_lines = vec![
            Line::from(vec![
                Span::styled("Avg Sessions  ", secondary),
                Span::styled(&stats.avg_sessions, Style::default().fg(colors.session)),
            ]),
            Line::from(vec![
                Span::styled("Avg Cost      ", secondary),
                Span::styled(&stats.avg_cost, Style::default().fg(colors.cost())),
            ]),
            Line::from(vec![
                Span::styled("Avg Tokens    ", secondary),
                Span::styled(&stats.avg_tokens, Style::default().fg(colors.avg_tokens)),
            ]),
            Line::from(vec![
                Span::styled("Chronotype    ", secondary),
                Span::styled(
                    &stats.chronotype,
                    Style::default()
                        .fg(colors.chronotype)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Fav Day       ", secondary),
                Span::styled(&stats.favorite_day, Style::default().fg(colors.fav_day)),
            ]),
            Line::from(vec![
                Span::styled("Total Models  ", secondary),
                Span::styled(&stats.total_models, Style::default().fg(colors.model)),
            ]),
        ];
        frame.render_widget(Paragraph::new(col2_lines), cols[1]);

        // Column separator
        let sep_char = "│";
        let sep_lines: Vec<Line> = (0..inner.height)
            .map(|_| Line::from(Span::styled(sep_char, sep_style)))
            .collect();

        // Separator between column 0 and 1
        let sep0_x = cols[0].x + cols[0].width;
        frame.render_widget(
            Paragraph::new(sep_lines.clone()),
            Rect::new(sep0_x, inner.y, 1, inner.height),
        );

        // ========== Column 3: Languages ==========
        if show_languages {
            // No indent for title, 2 char indent for languages
            let mut col3_lines: Vec<Line> = vec![Line::from(Span::styled(
                "Languages",
                secondary.add_modifier(Modifier::BOLD),
            ))];

            if stats.top_languages.is_empty() {
                col3_lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled("No data", secondary),
                ]));
            } else {
                for (lang, pct) in &stats.top_languages {
                    col3_lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(
                            format!("{:<12} ", lang),
                            Style::default().fg(colors.language),
                        ),
                        Span::styled(format!("{:>5.1}%", pct), secondary),
                    ]));
                }
                if stats.has_more_langs {
                    col3_lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled("...", secondary),
                    ]));
                }
            }
            frame.render_widget(Paragraph::new(col3_lines), cols[2]);

            // Separator between column 1 and 2
            let sep1_x = cols[1].x + cols[1].width;
            frame.render_widget(
                Paragraph::new(sep_lines),
                Rect::new(sep1_x, inner.y, 1, inner.height),
            );
        }
    }

    /// Activity heatmap: last 365 days
    pub fn render_activity_heatmap(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_focused: bool,
    ) {
        // Dispatch to weekly view if selected
        if self.activity_view == ActivityView::Weekly {
            self.overview_heatmap_layout = None;
            return self.render_weekly_heatmap(frame, area, border_style, is_focused);
        }
        self.weekly_heatmap_layout = None;

        let colors = self.theme.colors();
        let title_color = if is_focused {
            colors.border_focus
        } else {
            colors.border_default
        };

        let is_view_active = self.is_active
            && self.focus == super::helpers::Focus::Right
            && self.left_panel == super::helpers::LeftPanel::Stats
            && self.right_panel == super::helpers::RightPanel::Activity;

        let bottom_text = if is_focused {
            if is_view_active {
                " ←→ change view │ click to select "
            } else {
                " click to select "
            }
        } else {
            " "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    " ACTIVITY · YEARLY ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if is_focused { bottom_text } else { " " },
                    Style::default().fg(colors.text_muted),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 16 || inner.height < 6 {
            self.overview_heatmap_layout = None;
            return;
        }

        let today = self
            .per_day
            .keys()
            .filter_map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .max()
            .unwrap_or_else(|| chrono::Local::now().date_naive());

        let start_365 = today - chrono::Duration::days(364);
        let grid_start =
            start_365 - chrono::Duration::days(start_365.weekday().num_days_from_monday() as i64);
        let total_weeks = ((today - grid_start).num_days().max(0) as usize + 1).div_ceil(7);

        let label_w = 6u16;
        let avail_w = inner.width.saturating_sub(label_w + 1);
        if avail_w < 2 {
            self.overview_heatmap_layout = None;
            return;
        }

        let week_w = 2u16;
        let max_weeks = (avail_w / week_w) as usize;
        if max_weeks == 0 {
            self.overview_heatmap_layout = None;
            return;
        }

        let weeks = total_weeks.min(max_weeks).max(1);
        let render_start =
            grid_start + chrono::Duration::days((total_weeks.saturating_sub(weeks) * 7) as i64);

        let mut grid: Vec<[Option<u64>; 7]> = vec![[None; 7]; weeks];
        let mut max_tokens: u64 = 1;

        for (w, col) in grid.iter_mut().enumerate() {
            for (d, cell) in col.iter_mut().enumerate() {
                let date = render_start + chrono::Duration::days((w * 7 + d) as i64);
                if date > today {
                    continue;
                }
                let tokens = self
                    .per_day
                    .get(&date.format("%Y-%m-%d").to_string())
                    .map(|ds| ds.tokens.total())
                    .unwrap_or(0);
                *cell = Some(tokens);
                max_tokens = max_tokens.max(tokens);
            }
        }

        self.overview_heatmap_layout = Some(HeatmapLayout {
            inner,
            label_w,
            weeks,
            grid_start: render_start,
            week_w,
            extra_cols: 0,
            grid_pad: 0,
        });

        // Selected cell
        let (sel_w, sel_d) = self
            .overview_heatmap_selected_day
            .as_deref()
            .and_then(|k| chrono::NaiveDate::parse_from_str(k, "%Y-%m-%d").ok())
            .and_then(|d| {
                let n = (d - render_start).num_days();
                if n >= 0 {
                    Some((Some((n as usize) / 7), Some((n as usize) % 7)))
                } else {
                    None
                }
            })
            .unwrap_or((None, None));

        // Month labels
        let grid_w = week_w as usize * weeks;
        let mut month_row = vec![' '; grid_w];
        let mut ranges: Vec<(u32, u16, u16)> = Vec::new();
        let (mut x, mut cur_m, mut start) = (0u16, None::<u32>, 0u16);

        for w in 0..weeks {
            let m = (render_start + chrono::Duration::days((w * 7) as i64)).month();
            if let Some(cm) = cur_m {
                if cm != m {
                    ranges.push((cm, start, x));
                    start = x;
                }
            } else {
                start = x;
            }
            cur_m = Some(m);
            x += week_w;
        }
        if let Some(m) = cur_m {
            ranges.push((m, start, x));
        }

        let mut last_end = -2i32;
        for (m, x0, x1) in ranges {
            let name = month_abbr(m);
            let span = x1.saturating_sub(x0) as usize;
            if span < name.len() {
                continue;
            }
            let center = (x0 as usize + x1 as usize) / 2;
            let s = center.saturating_sub(name.len() / 2) as i32;
            let e = s + name.len() as i32 - 1;
            if s <= last_end + 1 || s < 0 || e >= grid_w as i32 {
                continue;
            }
            for (i, c) in name.chars().enumerate() {
                month_row[s as usize + i] = c;
            }
            last_end = e;
        }

        let day_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let mut lines: Vec<Line> = Vec::with_capacity(11);

        if inner.height > 8 {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<1$}", "", label_w as usize), Style::default()),
                Span::styled(
                    month_row.iter().collect::<String>(),
                    Style::default().fg(colors.text_secondary),
                ),
            ]));
        }

        let flash = self.overview_heatmap_flash_time.map(|t| {
            (1.0 - (t.elapsed().as_millis() as f64 * std::f64::consts::TAU / 600.0).cos()) * 0.2
        });
        let base_g = match colors.general_heatmap {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (100, 200, 100),
        };
        let bg_b = match colors.bg_empty {
            Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
            _ => (60.0, 60.0, 60.0),
        };

        for d in 0..7 {
            let mut spans: Vec<Span> = vec![Span::styled(
                format!(" {:<1$}", day_labels[d], (label_w - 1) as usize),
                Style::default().fg(colors.text_secondary),
            )];
            for (w, week) in grid.iter().enumerate().take(weeks) {
                let sel = sel_w == Some(w) && sel_d == Some(d);
                let bg = match week[d] {
                    None => colors.bg_empty,
                    Some(0) => colors.bg_empty,
                    Some(t) => {
                        let i = match t as f64 / max_tokens as f64 {
                            r if r <= 0.15 => 0.25,
                            r if r <= 0.35 => 0.45,
                            r if r <= 0.55 => 0.65,
                            r if r <= 0.75 => 0.82,
                            _ => 1.0,
                        };
                        Color::Rgb(
                            (bg_b.0 + (base_g.0 as f64 - bg_b.0) * i) as u8,
                            (bg_b.1 + (base_g.1 as f64 - bg_b.1) * i) as u8,
                            (bg_b.2 + (base_g.2 as f64 - bg_b.2) * i) as u8,
                        )
                    }
                };
                let style = if sel {
                    if let (Some(f), Color::Rgb(r, g, b)) = (flash, bg) {
                        Style::default().bg(Color::Rgb(
                            (r as f64 + (255.0 - r as f64) * f) as u8,
                            (g as f64 + (255.0 - g as f64) * f) as u8,
                            (b as f64 + (255.0 - b as f64) * f) as u8,
                        ))
                    } else {
                        Style::default().bg(bg)
                    }
                } else {
                    Style::default().bg(bg)
                };
                spans.push(Span::styled("  ", style));
            }
            lines.push(Line::from(spans));
        }

        if inner.height > 9 {
            lines.push(Line::from(""));
        }

        // Legend
        let legend_colors: [Color; 5] = [0.25, 0.45, 0.65, 0.82, 1.0].map(|i| {
            Color::Rgb(
                (bg_b.0 + (base_g.0 as f64 - bg_b.0) * i) as u8,
                (bg_b.1 + (base_g.1 as f64 - bg_b.1) * i) as u8,
                (bg_b.2 + (base_g.2 as f64 - bg_b.2) * i) as u8,
            )
        });

        let mut legend = vec![
            Span::styled(format!("{:<1$}", "", label_w as usize), Style::default()),
            Span::styled("Less ", Style::default().fg(colors.text_secondary)),
            Span::styled("  ", Style::default().bg(colors.bg_empty)),
        ];
        legend.extend(
            legend_colors
                .iter()
                .map(|c| Span::styled("  ", Style::default().bg(*c))),
        );
        legend.push(Span::styled(
            " More ",
            Style::default().fg(colors.text_secondary),
        ));

        // Selected day info
        if let Some(day) = &self.overview_heatmap_selected_day {
            let display = self
                .cached_day_strings
                .get(day.as_str())
                .cloned()
                .unwrap_or_else(|| {
                    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                        .map(|d| format!("{} {:02}, {}", month_abbr(d.month()), d.day(), d.year()))
                        .unwrap_or_else(|_| day.clone())
                });
            let dim = Style::default().fg(colors.text_muted);
            legend.extend([
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!(" {} ", display),
                    Style::default()
                        .fg(colors.text_primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("tok:", dim),
                Span::styled(
                    format_number(self.overview_heatmap_selected_tokens),
                    Style::default().fg(colors.avg_tokens),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("sess:", dim),
                Span::styled(
                    format!("{}", self.overview_heatmap_selected_sessions),
                    Style::default().fg(colors.session),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("cost:", dim),
                Span::styled(
                    format!("${:.2}", self.overview_heatmap_selected_cost),
                    Style::default().fg(colors.cost()),
                ),
            ]);
        }
        lines.push(Line::from(legend));
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// TOP PROJECTS right panel.
    pub fn render_projects_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let colors = self.theme.colors();
        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(
                Line::from(Span::styled(
                    " TOP PROJECTS ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if is_highlighted {
                        " ↑↓: scroll "
                    } else {
                        " "
                    },
                    Style::default().fg(colors.text_muted),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.overview_projects.is_empty() {
            let placeholder = "░".repeat(inner.width.saturating_sub(2) as usize);
            let lines: Vec<Line> = (0..inner.height)
                .map(|_| Line::styled(placeholder.clone(), Style::default().fg(colors.bg_empty)))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_project_max_scroll = self.overview_projects.len().saturating_sub(visible);
        self.overview_project_scroll = self
            .overview_project_scroll
            .min(self.overview_project_max_scroll);

        let max_count = self
            .overview_projects
            .iter()
            .map(|(_, c)| *c)
            .max()
            .unwrap_or(0);
        let name_w = 17;
        let bar_max = inner.width.saturating_sub(28) as usize;

        let lines: Vec<Line> = self
            .overview_projects
            .iter()
            .skip(self.overview_project_scroll)
            .take(visible)
            .map(|(name, count)| {
                let bar = if max_count > 0 {
                    ((*count as f64 / max_count as f64) * bar_max as f64) as usize
                } else {
                    0
                };
                Line::from(vec![
                    Span::styled(
                        format!(
                            "{:<1$} ",
                            truncate_with_ellipsis(name, name_w - 1),
                            name_w - 1
                        ),
                        Style::default().fg(colors.text_primary),
                    ),
                    Span::styled(" ".repeat(bar), Style::default().bg(colors.top_projects)),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(bar)),
                        Style::default().bg(colors.bg_empty),
                    ),
                    Span::styled(
                        format!(" {:>5} sess", count),
                        Style::default()
                            .fg(colors.top_projects)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// TOOL USAGE right panel.
    pub fn render_overview_tools_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let colors = self.theme.colors();
        let title_color = if is_highlighted {
            colors.border_focus
        } else {
            colors.border_default
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(
                Line::from(Span::styled(
                    " TOOL USAGE ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if is_highlighted {
                        " ↑↓: scroll "
                    } else {
                        " "
                    },
                    Style::default().fg(colors.text_muted),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.tool_usage.is_empty() {
            let placeholder = "░".repeat(inner.width.saturating_sub(2) as usize);
            let lines: Vec<Line> = (0..inner.height)
                .map(|_| Line::styled(placeholder.clone(), Style::default().fg(colors.bg_empty)))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_tool_max_scroll = self.tool_usage.len().saturating_sub(visible);
        self.overview_tool_scroll = self.overview_tool_scroll.min(self.overview_tool_max_scroll);

        let max_count = self.tool_usage.iter().map(|t| t.count).max().unwrap_or(0);
        let name_w = 14;
        let bar_max = inner.width.saturating_sub(22) as usize;

        let lines: Vec<Line> = self
            .tool_usage
            .iter()
            .skip(self.overview_tool_scroll)
            .take(visible)
            .map(|tool| {
                let bar = if max_count > 0 {
                    ((tool.count as f64 / max_count as f64) * bar_max as f64) as usize
                } else {
                    0
                };
                Line::from(vec![
                    Span::styled(
                        format!(
                            " {:<1$} ",
                            truncate_with_ellipsis(&tool.name, name_w - 1),
                            name_w - 1
                        ),
                        Style::default().fg(colors.text_primary),
                    ),
                    Span::styled(" ".repeat(bar), Style::default().bg(colors.tools_used)),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(bar)),
                        Style::default().bg(colors.bg_empty),
                    ),
                    Span::styled(
                        format!(" {:>5}", tool.count),
                        Style::default()
                            .fg(colors.tools_used)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Weekly activity heatmap:
    pub fn render_weekly_heatmap(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_focused: bool,
    ) {
        let colors = self.theme.colors();
        let title_color = if is_focused {
            colors.border_focus
        } else {
            colors.border_default
        };

        let is_view_active = self.is_active
            && self.focus == super::helpers::Focus::Right
            && self.left_panel == super::helpers::LeftPanel::Stats
            && self.right_panel == super::helpers::RightPanel::Activity;

        let bottom_text = if is_focused {
            if is_view_active {
                " ←→ change view │ click to select "
            } else {
                " click to select "
            }
        } else {
            " "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_focused {
                border_style
            } else {
                Style::default().fg(colors.border_default)
            })
            .title(
                Line::from(Span::styled(
                    " ACTIVITY · WEEKLY ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(Span::styled(
                    if is_focused { bottom_text } else { " " },
                    Style::default().fg(colors.text_muted),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 20 || inner.height < 9 {
            self.weekly_heatmap_layout = None;
            return;
        }

        // Determine time period granularity dynamically based on available width
        let label_w = 2u16;
        let avail_width = inner.width.saturating_sub(label_w + 1);

        // Slot sizing: prefer 1-hour slots, fallback to 2-hour if width insufficient
        let min_cell_w = 2u16;
        let hours_per_period = if avail_width >= 24 * min_cell_w {
            1
        } else {
            2
        };

        let num_periods = 24 / hours_per_period;

        // Fill full width
        let cell_w = (avail_width / num_periods as u16).max(2);
        let used_w = cell_w * num_periods as u16;
        let extra_cols = avail_width.saturating_sub(used_w);

        let period_extra = |period: usize| -> u16 {
            let prev = period * extra_cols as usize / num_periods;
            let next = (period + 1) * extra_cols as usize / num_periods;
            next.saturating_sub(prev) as u16
        };

        // Aggregate data by periods
        let mut period_tokens: Vec<Vec<u64>> = vec![vec![0u64; num_periods]; 7];
        let mut period_sessions: Vec<Vec<u32>> = vec![vec![0u32; num_periods]; 7];
        let mut period_cost: Vec<Vec<f64>> = vec![vec![0.0f64; num_periods]; 7];
        let mut max_tokens: u64 = 1;

        for weekday in 0..7 {
            for period in 0..num_periods {
                let h_start = period * hours_per_period;
                let h_end = ((period + 1) * hours_per_period).min(24);
                let mut tokens = 0u64;
                let mut sessions = 0u32;
                let mut cost = 0.0f64;

                for h in h_start..h_end {
                    tokens += self.weekly_heatmap_tokens[weekday][h];
                    sessions += self.weekly_heatmap_sessions[weekday][h];
                    cost += self.weekly_heatmap_cost[weekday][h];
                }

                period_tokens[weekday][period] = tokens;
                period_sessions[weekday][period] = sessions;
                period_cost[weekday][period] = cost;
                max_tokens = max_tokens.max(tokens);
            }
        }

        // Store layout for mouse interaction
        self.weekly_heatmap_layout = Some(WeeklyHeatmapLayout {
            inner,
            label_w,
            num_periods,
            hours_per_period,
            cell_w,
            extra_cols,
        });

        // Get selected cell
        let (sel_weekday, sel_period) = if let (Some(wd), Some(hour)) = (
            self.weekly_heatmap_selected_weekday,
            self.weekly_heatmap_selected_hour,
        ) {
            let period = hour / hours_per_period;
            (Some(wd), Some(period))
        } else {
            (None, None)
        };

        // Flash effect for selection
        let flash = self.weekly_heatmap_flash_time.map(|t| {
            (1.0 - (t.elapsed().as_millis() as f64 * std::f64::consts::TAU / 600.0).cos()) * 0.2
        });

        // Compute sparse time slot labels row
        let label_interval_hours = if cell_w <= 2 { 6 } else { 3 };
        let label_every_periods = (label_interval_hours / hours_per_period).max(1);
        let grid_w = (cell_w as usize * num_periods) + extra_cols as usize;
        let mut time_label_row = vec![' '; grid_w];

        let mut period_x = vec![0usize; num_periods + 1];
        for period in 0..num_periods {
            period_x[period + 1] =
                period_x[period] + cell_w as usize + period_extra(period) as usize;
        }

        for period in (0..num_periods).step_by(label_every_periods) {
            let h_start = period * hours_per_period;
            let label = format!("{:02}", h_start);
            let pos = period_x[period];
            for (i, c) in label.chars().enumerate() {
                if pos + i < time_label_row.len() {
                    time_label_row[pos + i] = c;
                }
            }
        }

        let mut lines: Vec<Line> = Vec::with_capacity(10);

        // Render time label row first
        let has_time_labels = inner.height >= 9;
        if has_time_labels {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("     {:<1$}", "", (label_w - 1) as usize),
                    Style::default(),
                ),
                Span::styled(
                    time_label_row.iter().collect::<String>(),
                    Style::default().fg(colors.text_secondary),
                ),
            ]));
        }

        // Heatmap colors
        let base_g = match colors.general_heatmap {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (100, 200, 100),
        };
        let bg_b = match colors.bg_empty {
            Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
            _ => (60.0, 60.0, 60.0),
        };

        // Day rows
        for row in 0..7 {
            let day_label = self.weekly_heatmap_dates[row]
                .map(|d| d.format("%a").to_string())
                .unwrap_or_else(|| "---".to_string());
            let mut spans: Vec<Span> = vec![
                Span::styled(
                    format!(" {:<1$}", day_label, (label_w - 1) as usize),
                    Style::default().fg(colors.text_secondary),
                ),
                Span::styled("  ", Style::default()),
            ];

            for period in 0..num_periods {
                let sel = sel_weekday == Some(row) && sel_period == Some(period);
                let tokens = period_tokens[row][period];
                let w = cell_w + period_extra(period);

                let bg = if tokens == 0 {
                    colors.bg_empty
                } else {
                    let intensity = match tokens as f64 / max_tokens as f64 {
                        r if r <= 0.15 => 0.25,
                        r if r <= 0.35 => 0.45,
                        r if r <= 0.55 => 0.65,
                        r if r <= 0.75 => 0.82,
                        _ => 1.0,
                    };
                    Color::Rgb(
                        (bg_b.0 + (base_g.0 as f64 - bg_b.0) * intensity) as u8,
                        (bg_b.1 + (base_g.1 as f64 - bg_b.1) * intensity) as u8,
                        (bg_b.2 + (base_g.2 as f64 - bg_b.2) * intensity) as u8,
                    )
                };

                let style = if sel {
                    if let (Some(f), Color::Rgb(r, g, b)) = (flash, bg) {
                        Style::default().bg(Color::Rgb(
                            (r as f64 + (255.0 - r as f64) * f) as u8,
                            (g as f64 + (255.0 - g as f64) * f) as u8,
                            (b as f64 + (255.0 - b as f64) * f) as u8,
                        ))
                    } else {
                        Style::default().bg(bg)
                    }
                } else {
                    Style::default().bg(bg)
                };

                // Render cell with proper width (stretches to fill available space)
                spans.push(Span::styled(" ".repeat(w as usize), style));
            }

            lines.push(Line::from(spans));
        }

        // Spacer
        if inner.height > 10 {
            lines.push(Line::from(""));
        }

        // Legend
        let legend_colors: [Color; 5] = [0.25, 0.45, 0.65, 0.82, 1.0].map(|i| {
            Color::Rgb(
                (bg_b.0 + (base_g.0 as f64 - bg_b.0) * i) as u8,
                (bg_b.1 + (base_g.1 as f64 - bg_b.1) * i) as u8,
                (bg_b.2 + (base_g.2 as f64 - bg_b.2) * i) as u8,
            )
        });

        let mut legend = vec![
            Span::styled(
                format!("     {:<1$}", "", (label_w - 1) as usize),
                Style::default(),
            ),
            Span::styled("Less ", Style::default().fg(colors.text_secondary)),
            Span::styled("  ", Style::default().bg(colors.bg_empty)),
        ];
        legend.extend(
            legend_colors
                .iter()
                .map(|c| Span::styled("  ", Style::default().bg(*c))),
        );
        legend.push(Span::styled(
            " More ",
            Style::default().fg(colors.text_secondary),
        ));

        // Selected period info
        if let (Some(row), Some(h_start)) = (
            self.weekly_heatmap_selected_weekday,
            self.weekly_heatmap_selected_hour,
        ) {
            let h_end = (h_start + hours_per_period).min(24);
            let time_label = format!("{:02}:00–{:02}:00", h_start, h_end);
            let date_label = self
                .weekly_heatmap_dates
                .get(row)
                .and_then(|d| *d)
                .map(|d| d.format("%a (%b %-d)").to_string())
                .unwrap_or_else(|| "---".to_string());
            let dim = Style::default().fg(colors.text_muted);

            legend.extend([
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!(" {} · {}", date_label, time_label),
                    Style::default()
                        .fg(colors.text_primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("tok:", dim),
                Span::styled(
                    format_number(self.weekly_heatmap_selected_tokens),
                    Style::default().fg(colors.avg_tokens),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("sess:", dim),
                Span::styled(
                    format!("{}", self.weekly_heatmap_selected_sessions),
                    Style::default().fg(colors.session),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("cost:", dim),
                Span::styled(
                    format!("${:.2}", self.weekly_heatmap_selected_cost),
                    Style::default().fg(colors.cost()),
                ),
            ]);
        }
        lines.push(Line::from(legend));
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

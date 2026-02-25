//! Stats panel rendering.

use super::helpers::{
    lang_from_ext, month_abbr, stat_widget, truncate_with_ellipsis, weekday_abbr, HeatmapLayout,
};
use crate::stats::{format_active_duration, format_number};
use crate::theme::FixedColors;
use chrono::Datelike;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

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

        // Separators
        let sep_style = Style::default().fg(colors.text_muted);
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
                colors.info,
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
                colors.cost(),
                &colors,
            ),
            c3[1],
        );

        // Col 4: Lines / Messages
        let c4 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[5]);
        let fixed = FixedColors::DEFAULT;

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Line Changes",
                    Style::default().fg(colors.text_secondary),
                )),
                Line::from(vec![
                    Span::styled(
                        format!("+{}", format_number(self.totals.diffs.additions)),
                        Style::default()
                            .fg(fixed.diff_add)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" / ", Style::default().fg(colors.text_muted)),
                    Span::styled(
                        format!("-{}", format_number(self.totals.diffs.deletions)),
                        Style::default()
                            .fg(fixed.diff_remove)
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
                    Style::default().fg(colors.text_secondary),
                )),
                Line::from(vec![
                    Span::styled(
                        format!("{}", self.totals.prompts),
                        Style::default()
                            .fg(colors.info)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" / ", Style::default().fg(colors.text_muted)),
                    Span::styled(
                        format!("{}", total_responses),
                        Style::default()
                            .fg(colors.success)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ])
            .alignment(Alignment::Center),
            c4[1],
        );
    }

    /// OVERVIEW right panel.
    pub fn render_overview_panel(
        &self,
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

        let total_sessions = self.totals.sessions.len();
        let total_days = self.day_list.len();
        let start_day = self.day_list.last().cloned().unwrap_or_else(|| "—".into());
        let days_since_start = self
            .day_list
            .last()
            .and_then(|first| {
                chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d")
                    .ok()
                    .map(|d| (chrono::Local::now().date_naive() - d).num_days().max(1) as usize)
            })
            .unwrap_or_else(|| total_days.max(1));

        let avg_sess_per_day = if total_days > 0 {
            total_sessions as f64 / total_days as f64
        } else {
            0.0
        };
        let avg_cost_per_sess = if total_sessions > 0 {
            self.totals.display_cost() / total_sessions as f64
        } else {
            0.0
        };

        let (peak_day, peak_count) = self
            .per_day
            .iter()
            .map(|(d, s)| (d.clone(), s.sessions.len()))
            .max_by_key(|(_, c)| *c)
            .unwrap_or_else(|| ("—".into(), 0));
        let peak_display = self
            .cached_day_strings
            .get(&peak_day)
            .cloned()
            .unwrap_or(peak_day);

        let longest_ms: i64 = self
            .per_day
            .values()
            .flat_map(|d| d.sessions.values())
            .map(|s| s.active_duration_ms)
            .max()
            .unwrap_or(0);

        let total_active_ms: i64 = self
            .per_day
            .values()
            .flat_map(|d| d.sessions.values())
            .map(|s| s.active_duration_ms)
            .sum();

        // Find favorite language by diff lines
        let fav_lang = {
            let mut counts: FxHashMap<&str, u64> = FxHashMap::default();
            for day in self.per_day.values() {
                for sess in day.sessions.values() {
                    for fd in &sess.file_diffs {
                        if let Some(ext) = fd.path.rsplit('.').next() {
                            if let Some(lang) = lang_from_ext(ext) {
                                *counts.entry(lang).or_default() +=
                                    (fd.additions + fd.deletions).max(1);
                            }
                        }
                    }
                }
            }
            counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(l, _)| l.to_string())
                .unwrap_or_else(|| "—".into())
        };

        let start_display = chrono::NaiveDate::parse_from_str(&start_day, "%Y-%m-%d")
            .map(|d| format!("{} {:02}, {}", month_abbr(d.month()), d.day(), d.year()))
            .unwrap_or(start_day);

        let muted = Style::default().fg(colors.text_muted);
        let w = 18usize;

        if inner.width < 50 {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled("Peak: ", muted),
                        Span::styled(peak_display, Style::default().fg(colors.cost())),
                    ]),
                    Line::from(vec![
                        Span::styled("Long: ", muted),
                        Span::styled(
                            format_active_duration(longest_ms),
                            Style::default().fg(colors.accent_cyan),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("Avg:  ", muted),
                        Span::styled(
                            format!("{:.1}", avg_sess_per_day),
                            Style::default().fg(colors.info),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("Fav:  ", muted),
                        Span::styled(fav_lang, Style::default().fg(colors.accent_magenta)),
                    ]),
                ]),
                inner,
            );
        } else {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Peak Day", w = w), muted),
                        Span::styled(
                            format!("{} ({}s)", peak_display, peak_count),
                            Style::default()
                                .fg(colors.cost())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Longest Session", w = w), muted),
                        Span::styled(
                            format_active_duration(longest_ms),
                            Style::default().fg(colors.accent_cyan),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Avg Sessions/Day", w = w), muted),
                        Span::styled(
                            format!("{:.1}", avg_sess_per_day),
                            Style::default()
                                .fg(colors.info)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Total Active", w = w), muted),
                        Span::styled(
                            format_active_duration(total_active_ms),
                            Style::default()
                                .fg(colors.success)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                ]),
                cols[0],
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Start Day", w = w), muted),
                        Span::styled(start_display, Style::default().fg(colors.text_primary)),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Active Days", w = w), muted),
                        Span::styled(
                            format!("{} / {}", total_days, days_since_start),
                            Style::default().fg(colors.info),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Avg Cost/Session", w = w), muted),
                        Span::styled(
                            format!("${:.2}", avg_cost_per_sess),
                            Style::default().fg(colors.cost()),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("  {:<w$}", "Fav Language", w = w), muted),
                        Span::styled(
                            fav_lang,
                            Style::default()
                                .fg(colors.accent_magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                ]),
                cols[1],
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
        let colors = self.theme.colors();
        let title_color = if is_focused {
            colors.border_focus
        } else {
            colors.border_default
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
                    " ACTIVITY ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            )
            .title_bottom(
                Line::from(if is_focused {
                    Span::styled(
                        " Click a day to see breakdown ",
                        Style::default().fg(colors.text_muted),
                    )
                } else {
                    Span::styled(" ", Style::default())
                })
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
                    Style::default().fg(colors.text_muted),
                ),
            ]));
        }

        let flash = self.overview_heatmap_flash_time.map(|t| {
            (1.0 - (t.elapsed().as_millis() as f64 * std::f64::consts::TAU / 600.0).cos()) * 0.2
        });
        let base_g = match colors.accent_green {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (100, 200, 100),
        };
        let bg_b = match colors.bg_tertiary {
            Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
            _ => (60.0, 60.0, 60.0),
        };

        for d in 0..7 {
            let mut spans: Vec<Span> = vec![Span::styled(
                format!(" {:<1$}", day_labels[d], (label_w - 1) as usize),
                Style::default().fg(colors.text_muted),
            )];
            for (w, week) in grid.iter().enumerate().take(weeks) {
                let sel = sel_w == Some(w) && sel_d == Some(d);
                let bg = match week[d] {
                    None => Color::Rgb(38, 41, 56),
                    Some(0) => colors.bg_tertiary,
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
            Span::styled("Less ", Style::default().fg(colors.text_muted)),
            Span::styled("  ", Style::default().bg(colors.bg_tertiary)),
        ];
        legend.extend(
            legend_colors
                .iter()
                .map(|c| Span::styled("  ", Style::default().bg(*c))),
        );
        legend.push(Span::styled(
            " More ",
            Style::default().fg(colors.text_muted),
        ));

        // Selected day info
        if let Some(day) = &self.overview_heatmap_selected_day {
            let display = self
                .cached_day_strings
                .get(day.as_str())
                .cloned()
                .unwrap_or_else(|| {
                    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                        .map(|d| {
                            format!(
                                "{} {:02}, {} {}",
                                month_abbr(d.month()),
                                d.day(),
                                d.year(),
                                weekday_abbr(d.weekday())
                            )
                        })
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
                    Style::default().fg(colors.success),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("sess:", dim),
                Span::styled(
                    format!("{}", self.overview_heatmap_selected_sessions),
                    Style::default().fg(colors.info),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("cost:", dim),
                Span::styled(
                    format!("${:.2}", self.overview_heatmap_selected_cost),
                    Style::default().fg(colors.cost()),
                ),
                Span::styled(" ╱ ", dim),
                Span::styled("active:", dim),
                Span::styled(
                    format_active_duration(self.overview_heatmap_selected_active_ms),
                    Style::default().fg(colors.accent_cyan),
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
                .map(|_| Line::styled(placeholder.clone(), Style::default().fg(colors.bg_tertiary)))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_project_max_scroll = self.overview_projects.len().saturating_sub(visible);
        self.overview_project_scroll = self
            .overview_project_scroll
            .min(self.overview_project_max_scroll);

        let total: u64 = self.overview_projects.iter().map(|(_, c)| *c as u64).sum();
        let name_w = 14;
        let bar_max = inner.width.saturating_sub(26) as usize;

        let lines: Vec<Line> = self
            .overview_projects
            .iter()
            .enumerate()
            .skip(self.overview_project_scroll)
            .take(visible)
            .map(|(i, (name, count))| {
                let pct = if total > 0 {
                    *count as f64 / total as f64
                } else {
                    0.0
                };
                let bar = (pct * bar_max as f64) as usize;
                Line::from(vec![
                    Span::styled(
                        format!(" {:>2}. ", i + 1),
                        Style::default().fg(colors.text_muted),
                    ),
                    Span::styled(
                        format!("{:<1$} ", truncate_with_ellipsis(name, name_w), name_w),
                        Style::default().fg(colors.text_primary),
                    ),
                    Span::styled(" ".repeat(bar), Style::default().bg(colors.info)),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(bar)),
                        Style::default().bg(colors.bg_tertiary),
                    ),
                    Span::styled(
                        format!(" {:>5}", count),
                        Style::default()
                            .fg(colors.info)
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
                .map(|_| Line::styled(placeholder.clone(), Style::default().fg(colors.bg_tertiary)))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_tool_max_scroll = self.tool_usage.len().saturating_sub(visible);
        self.overview_tool_scroll = self.overview_tool_scroll.min(self.overview_tool_max_scroll);

        let total: u64 = self.tool_usage.iter().map(|t| t.count).sum();
        let name_w = 14;
        let bar_max = inner.width.saturating_sub(22) as usize;

        let lines: Vec<Line> = self
            .tool_usage
            .iter()
            .skip(self.overview_tool_scroll)
            .take(visible)
            .map(|tool| {
                let pct = if total > 0 {
                    tool.count as f64 / total as f64
                } else {
                    0.0
                };
                let bar = (pct * bar_max as f64) as usize;
                Line::from(vec![
                    Span::styled(
                        format!(
                            " {:<1$} ",
                            truncate_with_ellipsis(&tool.name, name_w),
                            name_w
                        ),
                        Style::default().fg(colors.text_primary),
                    ),
                    Span::styled(" ".repeat(bar), Style::default().bg(colors.accent_pink)),
                    Span::styled(
                        " ".repeat(bar_max.saturating_sub(bar)),
                        Style::default().bg(colors.bg_tertiary),
                    ),
                    Span::styled(
                        format!(" {:>5}", tool.count),
                        Style::default()
                            .fg(colors.accent_pink)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

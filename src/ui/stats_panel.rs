//! GENERAL USAGE panel rendering
//!
//! Contains the stats panel (left side) and its associated right panels:
//! - OVERVIEW panel
//! - ACTIVITY heatmap
//! - TOP PROJECTS
//! - TOOL USAGE

use super::helpers::{stat_widget, truncate_with_ellipsis, HeatmapLayout};
use crate::stats::{format_active_duration, format_number};
use chrono::Datelike;
use fxhash::FxHashMap;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

impl super::App {
    /// Render the GENERAL USAGE left panel (stats summary)
    pub fn render_stats_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
        _is_active: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
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

        let sep_style = Style::default().fg(Color::Rgb(180, 180, 180));
        for &i in &[1, 4] {
            let sep_area = cols[i];
            let sep = Paragraph::new(vec![
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
                Line::from(Span::styled("│", sep_style)),
            ]);
            frame.render_widget(sep, sep_area);
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
                Color::Cyan,
            ),
            c1[0],
        );
        frame.render_widget(
            stat_widget(
                "Cost",
                format!("${:.2}", self.totals.display_cost()),
                Color::Yellow,
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
                Color::Blue,
            ),
            c2[0],
        );
        frame.render_widget(
            stat_widget(
                "Output",
                format_number(self.totals.tokens.output),
                Color::Magenta,
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
                Color::Rgb(255, 165, 0),
            ),
            c3[0],
        );
        frame.render_widget(
            stat_widget(
                "Cache",
                format_number(self.totals.tokens.cache_read + self.totals.tokens.cache_write),
                Color::Yellow,
            ),
            c3[1],
        );

        // Col 4: Lines / User · Agent
        let c4 = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(cols[5]);

        let lines_widget = Paragraph::new(vec![
            Line::from(Span::styled(
                "Line Changes",
                Style::default().fg(Color::Rgb(180, 180, 180)),
            )),
            Line::from(vec![
                Span::styled(
                    format!("+{}", format_number(self.totals.diffs.additions)),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" / ", Style::default().fg(Color::Rgb(100, 100, 120))),
                Span::styled(
                    format!("-{}", format_number(self.totals.diffs.deletions)),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(lines_widget, c4[0]);

        let msg_widget = Paragraph::new(vec![
            Line::from(Span::styled(
                "User / Agent Messages",
                Style::default().fg(Color::Rgb(180, 180, 180)),
            )),
            Line::from(vec![
                Span::styled(
                    format!("{}", self.totals.prompts),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" / ", Style::default().fg(Color::Rgb(100, 100, 120))),
                Span::styled(
                    format!("{}", total_responses),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(msg_widget, c4[1]);
    }

    /// Render the OVERVIEW right panel (for Stats view)
    pub fn render_overview_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
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
                    " OVERVIEW ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Compute stats
        let total_sessions = self.totals.sessions.len();
        let total_days = self.day_list.len();
        let start_day = self.day_list.last().cloned().unwrap_or_else(|| "—".into());
        let active_days = total_days;

        let days_since_start = if let Some(first) = self.day_list.last() {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d") {
                let today = chrono::Local::now().date_naive();
                (today - d).num_days().max(1) as usize
            } else {
                total_days.max(1)
            }
        } else {
            1
        };

        let avg_sess_per_day = if active_days > 0 {
            total_sessions as f64 / active_days as f64
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

        let fav_lang = {
            let mut ext_counts: FxHashMap<&str, u64> = FxHashMap::default();
            for day_stat in self.per_day.values() {
                for session in day_stat.sessions.values() {
                    for fd in &session.file_diffs {
                        let ext = fd.path.rsplit('.').next().unwrap_or("");
                        let lang = match ext {
                            "rs" => "Rust",
                            "py" => "Python",
                            "js" => "JavaScript",
                            "ts" | "tsx" => "TypeScript",
                            "go" => "Go",
                            "java" => "Java",
                            "c" | "h" => "C",
                            "cpp" | "cc" | "cxx" | "hpp" => "C++",
                            "rb" => "Ruby",
                            "swift" => "Swift",
                            "kt" => "Kotlin",
                            "lua" => "Lua",
                            "sh" | "bash" | "zsh" => "Shell",
                            "css" | "scss" | "sass" => "CSS",
                            "html" | "htm" => "HTML",
                            "json" => "JSON",
                            "yaml" | "yml" => "YAML",
                            "toml" => "TOML",
                            "md" | "mdx" => "Markdown",
                            "sql" => "SQL",
                            "svelte" => "Svelte",
                            "vue" => "Vue",
                            "dart" => "Dart",
                            "zig" => "Zig",
                            "ex" | "exs" => "Elixir",
                            _ => "",
                        };
                        if !lang.is_empty() {
                            *ext_counts.entry(lang).or_insert(0) +=
                                (fd.additions + fd.deletions).max(1);
                        }
                    }
                }
            }
            ext_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(l, _)| l.to_string())
                .unwrap_or_else(|| "—".into())
        };

        let start_display = if let Ok(d) = chrono::NaiveDate::parse_from_str(&start_day, "%Y-%m-%d")
        {
            let month = match d.month() {
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
            };
            format!("{} {:02}, {}", month, d.day(), d.year())
        } else {
            start_day
        };

        let label_style = Style::default().fg(Color::Rgb(140, 140, 160));
        let val_col = 18usize;

        if inner.width < 50 {
            // 1-column layout for narrow screens
            let all_lines = vec![
                Line::from(vec![
                    Span::styled("Peak: ", label_style),
                    Span::styled(peak_display, Style::default().fg(Color::Yellow)),
                ]),
                Line::from(vec![
                    Span::styled("Long: ", label_style),
                    Span::styled(
                        format_active_duration(longest_ms),
                        Style::default().fg(Color::Rgb(100, 200, 255)),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Avg:  ", label_style),
                    Span::styled(
                        format!("{:.1}", avg_sess_per_day),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Fav:  ", label_style),
                    Span::styled(fav_lang, Style::default().fg(Color::Magenta)),
                ]),
            ];
            frame.render_widget(Paragraph::new(all_lines), inner);
        } else {
            // 2-column layout (standard)
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner);

            let left_lines = vec![
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Peak Day", w = val_col), label_style),
                    Span::styled(
                        format!("{} ({}s)", peak_display, peak_count),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Longest Session", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format_active_duration(longest_ms),
                        Style::default().fg(Color::Rgb(100, 200, 255)),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Avg Sessions/Day", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format!("{:.1}", avg_sess_per_day),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Total Active", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format_active_duration(total_active_ms),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            let right_lines = vec![
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Start Day", w = val_col), label_style),
                    Span::styled(start_display, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled(format!("  {:<w$}", "Active Days", w = val_col), label_style),
                    Span::styled(
                        format!("{} / {}", active_days, days_since_start),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Avg Cost/Session", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        format!("${:.2}", avg_cost_per_sess),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<w$}", "Fav Language", w = val_col),
                        label_style,
                    ),
                    Span::styled(
                        fav_lang,
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            frame.render_widget(Paragraph::new(left_lines), cols[0]);
            frame.render_widget(Paragraph::new(right_lines), cols[1]);
        }
    }

    /// Activity heatmap: last 365 days, Mon-Sun rows, adaptive to terminal width.
    pub fn render_activity_heatmap(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_focused: bool,
    ) {
        let title_color = if is_focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if is_focused {
                border_style
            } else {
                Style::default().fg(Color::DarkGray)
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
                        Style::default().fg(Color::DarkGray),
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

        // Use max date from actual data instead of system date
        let today = self
            .per_day
            .keys()
            .filter_map(|day_str| chrono::NaiveDate::parse_from_str(day_str, "%Y-%m-%d").ok())
            .max()
            .unwrap_or_else(|| chrono::Local::now().date_naive());

        let start_365 = today - chrono::Duration::days(364);
        let start_offset = start_365.weekday().num_days_from_monday() as i64;
        let grid_start = start_365 - chrono::Duration::days(start_offset);

        let total_days_365 = (today - grid_start).num_days().max(0) as usize + 1;
        let total_weeks_365 = total_days_365.div_ceil(7);

        let label_w = 6u16;
        let avail_w = inner.width.saturating_sub(label_w + 1);
        if avail_w < 2 {
            self.overview_heatmap_layout = None;
            return;
        }

        // Fixed column width of 2 chars; show as many weeks as fit, latest first.
        let week_w: u16 = 2;
        let max_weeks_fit = (avail_w / week_w) as usize;
        if max_weeks_fit == 0 {
            self.overview_heatmap_layout = None;
            return;
        }

        let weeks = total_weeks_365.min(max_weeks_fit).max(1);
        let start_week = total_weeks_365.saturating_sub(weeks);
        let render_start = grid_start + chrono::Duration::days((start_week * 7) as i64);

        let extra_cols: u16 = 0;
        // Left-align the grid (no padding between labels and grid)
        let grid_pad: usize = 0;

        let mut grid: Vec<[Option<u64>; 7]> = vec![[None; 7]; weeks];
        let mut max_tokens: u64 = 1;

        for (w, col) in grid.iter_mut().enumerate() {
            for (d, cell) in col.iter_mut().enumerate() {
                let date = render_start + chrono::Duration::days((w * 7 + d) as i64);
                if date > today {
                    continue;
                }
                // Fill all days up to today (including pre-range days in the
                // first partial week) so the first column is never half-empty.
                let key = date.format("%Y-%m-%d").to_string();
                let tokens = self
                    .per_day
                    .get(&key)
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
            extra_cols,
            grid_pad: grid_pad as u16,
        });

        let week_width_at = |idx: usize| week_w + if (idx as u16) < extra_cols { 1 } else { 0 };

        let selected_key = self.overview_heatmap_selected_day.as_deref();

        // Pre-compute selected cell coordinates for full-square border
        let (sel_w, sel_d): (Option<usize>, Option<usize>) = if let Some(sel_key) = selected_key {
            if let Ok(sel_date) = chrono::NaiveDate::parse_from_str(sel_key, "%Y-%m-%d") {
                let days_from_start = (sel_date - render_start).num_days();
                if days_from_start >= 0 {
                    let d = days_from_start as usize;
                    (Some(d / 7), Some(d % 7))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Month labels centered over each visible month range.
        let grid_w = (week_w as usize) * weeks;
        let mut month_row: Vec<char> = vec![' '; grid_w];
        let mut month_ranges: Vec<(u32, u16, u16)> = Vec::new(); // month, x_start, x_end
        let mut x_cursor: u16 = 0;
        let mut cur_month: Option<u32> = None;
        let mut range_start: u16 = 0;
        for w in 0..weeks {
            let d0 = render_start + chrono::Duration::days((w * 7) as i64);
            let m = d0.month();
            if cur_month.is_none() {
                cur_month = Some(m);
                range_start = x_cursor;
            } else if cur_month != Some(m) {
                month_ranges.push((cur_month.unwrap_or(m), range_start, x_cursor));
                cur_month = Some(m);
                range_start = x_cursor;
            }
            x_cursor = x_cursor.saturating_add(week_width_at(w));
        }
        if let Some(m) = cur_month {
            month_ranges.push((m, range_start, x_cursor));
        }

        let mut last_label_end: i32 = -2;
        for (m, x0, x1) in month_ranges {
            let name = match m {
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
            };
            let span_w = x1.saturating_sub(x0) as usize;
            if span_w < name.len() {
                continue;
            }
            let center = (x0 as usize + x1 as usize) / 2;
            let start = center.saturating_sub(name.len() / 2) as i32;
            let end = start + name.len() as i32 - 1;
            if start <= last_label_end + 1 {
                continue;
            }
            if start < 0 || end >= month_row.len() as i32 {
                continue;
            }
            for (i, ch) in name.chars().enumerate() {
                month_row[(start as usize) + i] = ch;
            }
            last_label_end = end;
        }

        let day_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

        let mut lines: Vec<Line> = Vec::with_capacity(11);
        if inner.height > 8 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<w$}", "", w = label_w as usize + grid_pad),
                    Style::default(),
                ),
                Span::styled(
                    month_row.iter().collect::<String>(),
                    Style::default().fg(Color::Rgb(140, 140, 160)),
                ),
            ]));
        }

        // Precompute pulse blend factor once (avoids per-cell trig)
        let flash_blend = self.overview_heatmap_flash_time.map(|t| {
            let ms = t.elapsed().as_millis() as f64;
            // Smooth sine pulse: 600ms period, peaks at 40% white blend
            (1.0 - (ms * std::f64::consts::TAU / 600.0).cos()) * 0.2
        });

        // 7 day rows (show all labels)
        for d in 0..7usize {
            let mut spans: Vec<Span> = Vec::with_capacity(weeks + 2);
            let label = format!(" {:<w$}", day_labels[d], w = (label_w - 1) as usize);
            spans.push(Span::styled(
                label,
                Style::default().fg(Color::Rgb(100, 100, 120)),
            ));
            if grid_pad > 0 {
                spans.push(Span::styled(" ".repeat(grid_pad), Style::default()));
            }

            for (w, week) in grid.iter().enumerate().take(weeks) {
                let col_w = week_width_at(w) as usize;

                let is_selected_cell = sel_w == Some(w) && sel_d == Some(d);

                let bg = match week[d] {
                    None => None,
                    Some(0) => Some(Color::Rgb(38, 42, 50)),
                    Some(day_tokens) => {
                        let ratio = day_tokens as f64 / max_tokens as f64;
                        Some(if ratio <= 0.20 {
                            Color::Rgb(24, 66, 44)
                        } else if ratio <= 0.40 {
                            Color::Rgb(28, 102, 58)
                        } else if ratio <= 0.60 {
                            Color::Rgb(42, 138, 74)
                        } else if ratio <= 0.80 {
                            Color::Rgb(64, 181, 96)
                        } else if ratio <= 0.95 {
                            Color::Rgb(94, 230, 126)
                        } else {
                            Color::Rgb(118, 255, 149)
                        })
                    }
                };

                match bg {
                    None => {
                        spans.push(Span::styled(" ".repeat(col_w), Style::default()));
                    }
                    Some(bg) => {
                        let style = if is_selected_cell {
                            if let (Some(blend), Color::Rgb(r, g, b)) = (flash_blend, bg) {
                                // Smooth sine pulse: blend toward white
                                Style::default().bg(Color::Rgb(
                                    (r as f64 + (255.0 - r as f64) * blend) as u8,
                                    (g as f64 + (255.0 - g as f64) * blend) as u8,
                                    (b as f64 + (255.0 - b as f64) * blend) as u8,
                                ))
                            } else {
                                Style::default().bg(bg)
                            }
                        } else {
                            Style::default().bg(bg)
                        };
                        spans.push(Span::styled(" ".repeat(col_w), style));
                    }
                }
            }
            lines.push(Line::from(spans));
        }

        if inner.height > 9 {
            lines.push(Line::from(""));
        }
        let mut legend = vec![
            Span::styled(
                format!("{:<w$}", "", w = label_w as usize),
                Style::default(),
            ),
            Span::styled("Less ", Style::default().fg(Color::Rgb(100, 100, 120))),
            Span::styled("  ", Style::default().bg(Color::Rgb(38, 42, 50))),
            Span::styled("  ", Style::default().bg(Color::Rgb(24, 66, 44))),
            Span::styled("  ", Style::default().bg(Color::Rgb(28, 102, 58))),
            Span::styled("  ", Style::default().bg(Color::Rgb(42, 138, 74))),
            Span::styled("  ", Style::default().bg(Color::Rgb(64, 181, 96))),
            Span::styled("  ", Style::default().bg(Color::Rgb(94, 230, 126))),
            Span::styled(" More ", Style::default().fg(Color::Rgb(100, 100, 120))),
        ];
        if let Some(day) = &self.overview_heatmap_selected_day {
            let dim = Style::default().fg(Color::Rgb(100, 100, 120));
            // Format date as "Feb 22, 2026 Sun"
            let display_day = self
                .cached_day_strings
                .get(day.as_str())
                .cloned()
                .unwrap_or_else(|| {
                    if let Ok(d) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
                        let month = match d.month() {
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
                        };
                        let wday = match d.weekday() {
                            chrono::Weekday::Mon => "Mon",
                            chrono::Weekday::Tue => "Tue",
                            chrono::Weekday::Wed => "Wed",
                            chrono::Weekday::Thu => "Thu",
                            chrono::Weekday::Fri => "Fri",
                            chrono::Weekday::Sat => "Sat",
                            chrono::Weekday::Sun => "Sun",
                        };
                        format!("{} {:02}, {} {}", month, d.day(), d.year(), wday)
                    } else {
                        day.clone()
                    }
                });
            legend.push(Span::styled("  ", Style::default()));
            legend.push(Span::styled(
                format!(" {} ", display_day),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            legend.push(Span::styled(" ╱ ", dim));
            legend.push(Span::styled("tok:", dim));
            legend.push(Span::styled(
                format_number(self.overview_heatmap_selected_tokens),
                Style::default().fg(Color::Green),
            ));
            legend.push(Span::styled(" ╱ ", dim));
            legend.push(Span::styled("sess:", dim));
            legend.push(Span::styled(
                format!("{}", self.overview_heatmap_selected_sessions),
                Style::default().fg(Color::Cyan),
            ));
            legend.push(Span::styled(" ╱ ", dim));
            legend.push(Span::styled("cost:", dim));
            legend.push(Span::styled(
                format!("${:.2}", self.overview_heatmap_selected_cost),
                Style::default().fg(Color::Yellow),
            ));
            legend.push(Span::styled(" ╱ ", dim));
            legend.push(Span::styled("active:", dim));
            legend.push(Span::styled(
                format_active_duration(self.overview_heatmap_selected_active_ms),
                Style::default().fg(Color::Rgb(100, 200, 255)),
            ));
        }
        lines.push(Line::from(legend));

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Render the TOP PROJECTS right panel (for Stats view)
    pub fn render_projects_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
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
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.overview_projects.is_empty() {
            let placeholder = "░".repeat(inner.width.saturating_sub(2) as usize);
            let lines: Vec<Line> = (0..inner.height)
                .map(|_| {
                    Line::styled(
                        placeholder.clone(),
                        Style::default().fg(Color::Rgb(30, 30, 40)),
                    )
                })
                .collect();
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_project_max_scroll = self.overview_projects.len().saturating_sub(visible);
        self.overview_project_scroll = self
            .overview_project_scroll
            .min(self.overview_project_max_scroll);

        let total_count: u64 = self.overview_projects.iter().map(|(_, c)| *c as u64).sum();
        // Fixed layout: " X. " (5) + "name " (15) + bar + " XXXXX" (6) = 26
        let name_width = 14;
        let bar_max = inner.width.saturating_sub(26) as usize;

        // Solid bar colors - Cyan for TOP PROJECTS
        let bar_color = Color::Cyan;
        let empty_color = Color::Rgb(38, 42, 50);

        let lines: Vec<Line> = self
            .overview_projects
            .iter()
            .enumerate()
            .skip(self.overview_project_scroll)
            .take(visible)
            .map(|(i, (name, count))| {
                let pct = if total_count > 0 {
                    *count as f64 / total_count as f64
                } else {
                    0.0
                };
                let bar_len = (pct * bar_max as f64) as usize;

                // Optimized: use single spans for filled and empty portions
                let filled_str: String = " ".repeat(bar_len);
                let empty_str: String = " ".repeat(bar_max.saturating_sub(bar_len));

                Line::from(vec![
                    Span::styled(
                        format!(" {:>2}. ", i + 1),
                        Style::default().fg(Color::Rgb(100, 100, 120)),
                    ),
                    Span::styled(
                        format!(
                            "{:<width$} ",
                            truncate_with_ellipsis(name, name_width),
                            width = name_width
                        ),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(filled_str, Style::default().bg(bar_color)),
                    Span::styled(empty_str, Style::default().bg(empty_color)),
                    Span::styled(
                        format!(" {:>5}", count),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Render the TOOL USAGE right panel (for Stats view)
    pub fn render_overview_tools_panel(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        border_style: Style,
        is_highlighted: bool,
    ) {
        let title_color = if is_highlighted {
            Color::Cyan
        } else {
            Color::DarkGray
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
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.tool_usage.is_empty() {
            let placeholder = "░".repeat(inner.width.saturating_sub(2) as usize);
            let lines: Vec<Line> = (0..inner.height)
                .map(|_| {
                    Line::styled(
                        placeholder.clone(),
                        Style::default().fg(Color::Rgb(30, 30, 40)),
                    )
                })
                .collect();
            frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
            return;
        }

        let visible = inner.height as usize;
        self.overview_tool_max_scroll = self.tool_usage.len().saturating_sub(visible);
        self.overview_tool_scroll = self.overview_tool_scroll.min(self.overview_tool_max_scroll);

        let total_count: u64 = self.tool_usage.iter().map(|t| t.count).sum();
        // Fixed layout: " name " (16) + bar + " XXXXX" (6) = 22
        let name_w = 14;
        let bar_max = inner.width.saturating_sub(22) as usize;

        // Soft pink color (not bright magenta)
        let bar_color = Color::Rgb(220, 100, 160);
        let empty_color = Color::Rgb(38, 42, 50);

        let lines: Vec<Line> = self
            .tool_usage
            .iter()
            .skip(self.overview_tool_scroll)
            .take(visible)
            .map(|tool| {
                let pct = if total_count > 0 {
                    tool.count as f64 / total_count as f64
                } else {
                    0.0
                };
                let bar_len = (pct * bar_max as f64) as usize;

                // Optimized: use single spans for filled and empty portions
                let filled_str: String = " ".repeat(bar_len);
                let empty_str: String = " ".repeat(bar_max.saturating_sub(bar_len));

                Line::from(vec![
                    Span::styled(
                        format!(
                            " {:<width$} ",
                            truncate_with_ellipsis(&tool.name, name_w),
                            width = name_w
                        ),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(filled_str, Style::default().bg(bar_color)),
                    Span::styled(empty_str, Style::default().bg(empty_color)),
                    Span::styled(
                        format!(" {:>5}", tool.count),
                        Style::default()
                            .fg(Color::Rgb(220, 100, 160))
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

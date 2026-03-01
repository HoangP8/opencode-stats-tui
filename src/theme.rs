//! Theme system

use ratatui::style::Color;

const fn hex(color: &str) -> Color {
    let bytes = color.as_bytes();
    let r = hex_byte(bytes[1], bytes[2]);
    let g = hex_byte(bytes[3], bytes[4]);
    let b = hex_byte(bytes[5], bytes[6]);
    Color::Rgb(r, g, b)
}

const fn hex_byte(hi: u8, lo: u8) -> u8 {
    hex_nibble(hi) << 4 | hex_nibble(lo)
}

const fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub bg_primary: Color,
    pub bg_highlight: Color,
    pub bg_empty: Color,

    pub border_default: Color,
    pub border_focus: Color,

    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,

    pub general_heatmap: Color,
    pub model_heatmap: Color,

    pub input: Color,
    pub output: Color,

    pub cost: Color,
    pub thinking: Color,
    pub cache_read: Color,
    pub cache_write: Color,

    pub add_line: Color,
    pub remove_line: Color,

    pub user: Color,
    pub agent_general: Color,
    pub main_agent: Color,
    pub sub_agent: Color,
    pub model: Color,

    pub host: Color,
    pub branch: Color,

    pub tools_used: Color,
    pub language: Color,

    pub session: Color,
    pub day_stats: Color,
    pub total_time: Color,
    pub avg_tokens: Color,
    pub chronotype: Color,
    pub fav_day: Color,

    pub project: Color,
    pub top_projects: Color,

    pub pos_savings: Color,
    pub neg_savings: Color,
    pub savings: Color,

    pub cost_estimated: Color,
}

impl ThemeColors {
    pub const DEFAULT: Self = Self {
        bg_primary: hex("#161826"),
        bg_highlight: hex("#2d3142"),
        bg_empty: hex("#1e2130"),

        border_default: hex("#4a4e63"),
        border_focus: hex("#d48ad8"),

        text_primary: hex("#e2e5f5"),
        text_secondary: hex("#9aa3c2"),
        text_muted: hex("#6b728a"),

        general_heatmap: hex("#39d353"),
        model_heatmap: hex("#3ebfce"),

        input: hex("#7089ec"),
        output: hex("#b24dcb"),

        cost: hex("#f0a45e"),
        thinking: hex("#38bdf8"),
        cache_read: hex("#e8d59c"),
        cache_write: hex("#c9a86e"),

        add_line: hex("#4ade80"),
        remove_line: hex("#f87171"),

        user: hex("#347cc4"),
        agent_general: hex("#a3e635"),
        main_agent: hex("#54c574"),
        sub_agent: hex("#fbbf24"),
        model: hex("#b894e0"),

        host: hex("#d4a373"),
        branch: hex("#818cf8"),

        tools_used: hex("#c46294"),
        language: hex("#a78bfa"),

        session: hex("#29c7ca"),
        day_stats: hex("#fbbf24"),
        total_time: hex("#7dd3c0"),
        avg_tokens: hex("#df778c"),
        chronotype: hex("#a855f7"),
        fav_day: hex("#f97316"),

        project: hex("#60a5fa"),
        top_projects: hex("#60a5fa"),

        pos_savings: hex("#4ade80"),
        neg_savings: hex("#fb7185"),
        savings: hex("#4ade80"),

        cost_estimated: hex("#fb923c"),
    };

    #[inline]
    pub fn subagent_color(&self, index: usize) -> Color {
        const COLORS: [Color; 6] = [
            hex("#22d3d6"),
            hex("#c084fc"),
            hex("#4ade80"),
            hex("#fbbf24"),
            hex("#60a5fa"),
            hex("#f472b6"),
        ];
        COLORS[index % 6]
    }

    #[inline]
    pub const fn token_input(&self) -> Color {
        self.input
    }

    #[inline]
    pub const fn token_output(&self) -> Color {
        self.output
    }

    #[inline]
    pub const fn thinking(&self) -> Color {
        self.thinking
    }

    #[inline]
    pub const fn cost(&self) -> Color {
        self.cost
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Theme;

impl Theme {
    #[inline]
    pub const fn colors(&self) -> ThemeColors {
        ThemeColors::DEFAULT
    }
}

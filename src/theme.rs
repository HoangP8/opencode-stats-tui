//! Theme system

use ratatui::style::Color;

/// Fixed semantic colors for diff highlighting
#[derive(Debug, Clone, Copy)]
pub struct FixedColors {
    pub diff_add: Color,
    pub diff_remove: Color,
}

impl FixedColors {
    pub const DEFAULT: Self = Self {
        diff_add: Color::Rgb(120, 200, 140),
        diff_remove: Color::Rgb(230, 130, 130),
    };
}

/// Complete color palette for TUI rendering
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    // Backgrounds
    pub bg_primary: Color,
    pub bg_tertiary: Color,
    pub bg_highlight: Color,

    // Borders
    pub border_default: Color,
    pub border_focus: Color,
    pub border_muted: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,

    // Status
    pub success: Color,
    pub error: Color,
    pub info: Color,

    // Accents
    pub accent_blue: Color,
    pub accent_cyan: Color,
    pub accent_green: Color,
    pub accent_magenta: Color,
    pub accent_orange: Color,
    pub accent_pink: Color,
    pub accent_yellow: Color,
}

impl ThemeColors {
    /// Default theme
    pub const DEFAULT: Self = Self {
        // Backgrounds
        bg_primary: Color::Rgb(22, 24, 38),
        bg_tertiary: Color::Rgb(32, 35, 52),
        bg_highlight: Color::Rgb(50, 54, 72),

        // Borders
        border_default: Color::Rgb(130, 135, 160),
        border_focus: Color::Rgb(120, 220, 170),
        border_muted: Color::Rgb(90, 95, 115),

        // Text
        text_primary: Color::Rgb(230, 233, 248),
        text_secondary: Color::Rgb(185, 190, 210),
        text_muted: Color::Rgb(140, 145, 168),

        // Status
        success: Color::Rgb(110, 220, 120),
        error: Color::Rgb(250, 120, 130),
        info: Color::Rgb(110, 200, 245),

        // Accents
        accent_blue: Color::Rgb(120, 170, 250),
        accent_cyan: Color::Rgb(100, 215, 235),
        accent_green: Color::Rgb(110, 210, 120),
        accent_magenta: Color::Rgb(210, 150, 235),
        accent_orange: Color::Rgb(245, 175, 100),
        accent_pink: Color::Rgb(240, 145, 180),
        accent_yellow: Color::Rgb(235, 195, 100),
    };

    /// Token input color
    #[inline]
    pub const fn token_input(&self) -> Color {
        Color::Rgb(120, 170, 250)
    }

    /// Token output color
    #[inline]
    pub const fn token_output(&self) -> Color {
        Color::Rgb(210, 150, 235)
    }

    /// Cost display color
    #[inline]
    pub const fn cost(&self) -> Color {
        Color::Rgb(235, 195, 100)
    }

    /// Thinking/reasoning color
    #[inline]
    pub const fn thinking(&self) -> Color {
        Color::Rgb(100, 215, 235)
    }

    /// Subagent color rotation by index
    #[inline]
    pub fn subagent_color(&self, index: usize) -> Color {
        const COLORS: [Color; 6] = [
            Color::Rgb(100, 210, 225),
            Color::Rgb(200, 150, 225),
            Color::Rgb(110, 200, 120),
            Color::Rgb(225, 190, 100),
            Color::Rgb(120, 165, 240),
            Color::Rgb(235, 140, 175),
        ];
        COLORS[index % 6]
    }
}

/// Theme container providing access to color palette
#[derive(Debug, Clone, Copy, Default)]
pub struct Theme;

impl Theme {
    #[inline]
    pub const fn colors(&self) -> ThemeColors {
        ThemeColors::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_background() {
        let colors = ThemeColors::DEFAULT;
        assert_eq!(colors.bg_primary, Color::Rgb(22, 24, 38));
    }
}

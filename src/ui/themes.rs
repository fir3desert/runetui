//! Visual themes for the whole application.
//!
//! A theme is a plain palette plus a handful of semantic style helpers.  The
//! Markdown renderer, the explorer and the status bar all derive their colours
//! from this single source of truth, and the syntax highlighter builds a
//! matching [`syntect`] theme out of the same colours.

use ratatui::style::{Color, Modifier, Style};

/// Available themes.  The order is also the cycle order for the `T` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    CatppuccinMocha,
    TokyoNight,
    Nord,
    Dracula,
    Gruvbox,
    DefaultDark,
    Black,
    White,
    Snow,
}

impl ThemeName {
    pub const ALL: [ThemeName; 9] = [
        ThemeName::CatppuccinMocha,
        ThemeName::TokyoNight,
        ThemeName::Nord,
        ThemeName::Dracula,
        ThemeName::Gruvbox,
        ThemeName::DefaultDark,
        ThemeName::Black,
        ThemeName::White,
        ThemeName::Snow,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            ThemeName::CatppuccinMocha => "catppuccin-mocha",
            ThemeName::TokyoNight => "tokyo-night",
            ThemeName::Nord => "nord",
            ThemeName::Dracula => "dracula",
            ThemeName::Gruvbox => "gruvbox",
            ThemeName::DefaultDark => "default-dark",
            ThemeName::Black => "black",
            ThemeName::White => "white",
            ThemeName::Snow => "snow",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            ThemeName::CatppuccinMocha => "Catppuccin Mocha",
            ThemeName::TokyoNight => "Tokyo Night",
            ThemeName::Nord => "Nord",
            ThemeName::Dracula => "Dracula",
            ThemeName::Gruvbox => "Gruvbox",
            ThemeName::DefaultDark => "Default Dark",
            ThemeName::Black => "Black",
            ThemeName::White => "White",
            ThemeName::Snow => "Snow",
        }
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        let normalized = normalized.replace(' ', "-");
        Self::ALL.into_iter().find(|theme| {
            theme.slug() == normalized
                || theme.label().to_ascii_lowercase().replace(' ', "-") == normalized
        })
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|item| *item == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn theme(self) -> Theme {
        match self {
            ThemeName::CatppuccinMocha => catppuccin_mocha(),
            ThemeName::TokyoNight => tokyo_night(),
            ThemeName::Nord => nord(),
            ThemeName::Dracula => dracula(),
            ThemeName::Gruvbox => gruvbox(),
            ThemeName::DefaultDark => default_dark(),
            ThemeName::Black => black(),
            ThemeName::White => white(),
            ThemeName::Snow => snow(),
        }
    }
}

/// A full colour palette.  Fields are kept semantic rather than indexed so the
/// renderer never hard-codes a raw colour.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: ThemeName,
    pub bg: Color,
    pub bg_alt: Color,
    pub bg_sel: Color,
    pub fg: Color,
    pub fg_dim: Color,
    pub fg_muted: Color,
    pub accent: Color,
    pub accent2: Color,
    pub green: Color,
    pub yellow: Color,
    pub orange: Color,
    pub red: Color,
    pub purple: Color,
    pub cyan: Color,
    pub border: Color,

    // Syntax roles used to build the syntect theme.
    pub syn_comment: Color,
    pub syn_string: Color,
    pub syn_number: Color,
    pub syn_keyword: Color,
    pub syn_function: Color,
    pub syn_type: Color,
    pub syn_variable: Color,
    pub syn_operator: Color,
    pub syn_punct: Color,
    pub syn_tag: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Theme {
    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn dim(&self) -> Style {
        Style::default().fg(self.fg_dim)
    }

    pub fn muted(&self) -> Style {
        Style::default().fg(self.fg_muted)
    }

    pub fn border(&self) -> Style {
        Style::default().fg(self.border)
    }

    pub fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
            2 => Style::default()
                .fg(self.accent2)
                .add_modifier(Modifier::BOLD),
            3 => Style::default()
                .fg(self.yellow)
                .add_modifier(Modifier::BOLD),
            4 => Style::default().fg(self.fg).add_modifier(Modifier::BOLD),
            5 => Style::default()
                .fg(self.fg_dim)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(self.fg_muted)
                .add_modifier(Modifier::BOLD | Modifier::ITALIC),
        }
    }

    /// Background used for code blocks and inline code.
    pub fn code_bg(&self) -> Color {
        self.bg_alt
    }

    pub fn code_block_style(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg_alt)
    }

    pub fn inline_code_style(&self) -> Style {
        Style::default()
            .fg(self.orange)
            .bg(self.bg_sel)
            .add_modifier(Modifier::BOLD)
    }

    pub fn quote_style(&self) -> Style {
        Style::default().fg(self.fg_dim)
    }

    pub fn list_marker_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn cursor_line_bg(&self) -> Color {
        self.bg_sel
    }

    pub fn search_match(&self) -> Style {
        Style::default()
            .fg(self.bg)
            .bg(self.yellow)
            .add_modifier(Modifier::BOLD)
    }

    pub fn search_current(&self) -> Style {
        Style::default()
            .fg(self.bg)
            .bg(self.orange)
            .add_modifier(Modifier::BOLD)
    }
}

pub fn catppuccin_mocha() -> Theme {
    Theme {
        name: ThemeName::CatppuccinMocha,
        bg: rgb(0x1e1e2e),
        bg_alt: rgb(0x181825),
        bg_sel: rgb(0x313244),
        fg: rgb(0xcdd6f4),
        fg_dim: rgb(0xa6adc8),
        fg_muted: rgb(0x6c7086),
        accent: rgb(0xcba6f7),
        accent2: rgb(0x89b4fa),
        green: rgb(0xa6e3a1),
        yellow: rgb(0xf9e2af),
        orange: rgb(0xfab387),
        red: rgb(0xf38ba8),
        purple: rgb(0xcba6f7),
        cyan: rgb(0x94e2d5),
        border: rgb(0x45475a),
        syn_comment: rgb(0x6c7086),
        syn_string: rgb(0xa6e3a1),
        syn_number: rgb(0xfab387),
        syn_keyword: rgb(0xcba6f7),
        syn_function: rgb(0x89b4fa),
        syn_type: rgb(0xf9e2af),
        syn_variable: rgb(0xf2cdcd),
        syn_operator: rgb(0x89dceb),
        syn_punct: rgb(0x9399b2),
        syn_tag: rgb(0xf38ba8),
    }
}

pub fn tokyo_night() -> Theme {
    Theme {
        name: ThemeName::TokyoNight,
        bg: rgb(0x1a1b26),
        bg_alt: rgb(0x16161e),
        bg_sel: rgb(0x292e42),
        fg: rgb(0xc0caf5),
        fg_dim: rgb(0x9aa5ce),
        fg_muted: rgb(0x565f89),
        accent: rgb(0xbb9af7),
        accent2: rgb(0x7aa2f7),
        green: rgb(0x9ece6a),
        yellow: rgb(0xe0af68),
        orange: rgb(0xff9e64),
        red: rgb(0xf7768e),
        purple: rgb(0xbb9af7),
        cyan: rgb(0x7dcfff),
        border: rgb(0x3b4261),
        syn_comment: rgb(0x565f89),
        syn_string: rgb(0x9ece6a),
        syn_number: rgb(0xff9e64),
        syn_keyword: rgb(0xbb9af7),
        syn_function: rgb(0x7aa2f7),
        syn_type: rgb(0x2ac3de),
        syn_variable: rgb(0xc0caf5),
        syn_operator: rgb(0x89ddff),
        syn_punct: rgb(0xa9b1d6),
        syn_tag: rgb(0xf7768e),
    }
}

pub fn nord() -> Theme {
    Theme {
        name: ThemeName::Nord,
        bg: rgb(0x2e3440),
        bg_alt: rgb(0x272c36),
        bg_sel: rgb(0x3b4252),
        fg: rgb(0xd8dee9),
        fg_dim: rgb(0xaeb8c8),
        fg_muted: rgb(0x616e88),
        accent: rgb(0x88c0d0),
        accent2: rgb(0x81a1c1),
        green: rgb(0xa3be8c),
        yellow: rgb(0xebcb8b),
        orange: rgb(0xd08770),
        red: rgb(0xbf616a),
        purple: rgb(0xb48ead),
        cyan: rgb(0x8fbcbb),
        border: rgb(0x434c5e),
        syn_comment: rgb(0x616e88),
        syn_string: rgb(0xa3be8c),
        syn_number: rgb(0xb48ead),
        syn_keyword: rgb(0x81a1c1),
        syn_function: rgb(0x88c0d0),
        syn_type: rgb(0x8fbcbb),
        syn_variable: rgb(0xd8dee9),
        syn_operator: rgb(0x81a1c1),
        syn_punct: rgb(0xd8dee9),
        syn_tag: rgb(0xbf616a),
    }
}

pub fn dracula() -> Theme {
    Theme {
        name: ThemeName::Dracula,
        bg: rgb(0x282a36),
        bg_alt: rgb(0x21222c),
        bg_sel: rgb(0x44475a),
        fg: rgb(0xf8f8f2),
        fg_dim: rgb(0xc5c8d6),
        fg_muted: rgb(0x6272a4),
        accent: rgb(0xbd93f9),
        accent2: rgb(0x8be9fd),
        green: rgb(0x50fa7b),
        yellow: rgb(0xf1fa8c),
        orange: rgb(0xffb86c),
        red: rgb(0xff5555),
        purple: rgb(0xbd93f9),
        cyan: rgb(0x8be9fd),
        border: rgb(0x44475a),
        syn_comment: rgb(0x6272a4),
        syn_string: rgb(0xf1fa8c),
        syn_number: rgb(0xbd93f9),
        syn_keyword: rgb(0xff79c6),
        syn_function: rgb(0x50fa7b),
        syn_type: rgb(0x8be9fd),
        syn_variable: rgb(0xf8f8f2),
        syn_operator: rgb(0xff79c6),
        syn_punct: rgb(0xf8f8f2),
        syn_tag: rgb(0xff79c6),
    }
}

pub fn gruvbox() -> Theme {
    Theme {
        name: ThemeName::Gruvbox,
        bg: rgb(0x282828),
        bg_alt: rgb(0x1d2021),
        bg_sel: rgb(0x3c3836),
        fg: rgb(0xebdbb2),
        fg_dim: rgb(0xd5c4a1),
        fg_muted: rgb(0x928374),
        accent: rgb(0xfabd2f),
        accent2: rgb(0x83a598),
        green: rgb(0xb8bb26),
        yellow: rgb(0xfabd2f),
        orange: rgb(0xfe8019),
        red: rgb(0xfb4934),
        purple: rgb(0xd3869b),
        cyan: rgb(0x8ec07c),
        border: rgb(0x504945),
        syn_comment: rgb(0x928374),
        syn_string: rgb(0xb8bb26),
        syn_number: rgb(0xd3869b),
        syn_keyword: rgb(0xfb4934),
        syn_function: rgb(0x83a598),
        syn_type: rgb(0xfabd2f),
        syn_variable: rgb(0xebdbb2),
        syn_operator: rgb(0xfe8019),
        syn_punct: rgb(0xa89984),
        syn_tag: rgb(0x8ec07c),
    }
}

pub fn default_dark() -> Theme {
    Theme {
        name: ThemeName::DefaultDark,
        bg: rgb(0x16181d),
        bg_alt: rgb(0x101217),
        bg_sel: rgb(0x262a33),
        fg: rgb(0xd6dae4),
        fg_dim: rgb(0xa0a8b7),
        fg_muted: rgb(0x69707e),
        accent: rgb(0x6cb6ff),
        accent2: rgb(0x89ddff),
        green: rgb(0x8bd49c),
        yellow: rgb(0xe5c07b),
        orange: rgb(0xe8a06a),
        red: rgb(0xe06c75),
        purple: rgb(0xc792ea),
        cyan: rgb(0x56b6c2),
        border: rgb(0x2c313a),
        syn_comment: rgb(0x5c6370),
        syn_string: rgb(0x98c379),
        syn_number: rgb(0xd19a66),
        syn_keyword: rgb(0xc678dd),
        syn_function: rgb(0x61afef),
        syn_type: rgb(0xe5c07b),
        syn_variable: rgb(0xe06c75),
        syn_operator: rgb(0x56b6c2),
        syn_punct: rgb(0xabb2bf),
        syn_tag: rgb(0xe06c75),
    }
}

/// True black, in the spirit of btop's darkest theme.
pub fn black() -> Theme {
    Theme {
        name: ThemeName::Black,
        bg: rgb(0x000000),
        bg_alt: rgb(0x0b0b0e),
        bg_sel: rgb(0x1e1e24),
        fg: rgb(0xd4d4d4),
        fg_dim: rgb(0xa8a8a8),
        fg_muted: rgb(0x6e6e6e),
        accent: rgb(0x7aa2f7),
        accent2: rgb(0x7dcfff),
        green: rgb(0x9ece6a),
        yellow: rgb(0xe0af68),
        orange: rgb(0xff9e64),
        red: rgb(0xf7768e),
        purple: rgb(0xbb9af7),
        cyan: rgb(0x7dcfff),
        border: rgb(0x2b2b33),
        syn_comment: rgb(0x6e6e6e),
        syn_string: rgb(0x9ece6a),
        syn_number: rgb(0xff9e64),
        syn_keyword: rgb(0xbb9af7),
        syn_function: rgb(0x7aa2f7),
        syn_type: rgb(0x2ac3de),
        syn_variable: rgb(0xd4d4d4),
        syn_operator: rgb(0x89ddff),
        syn_punct: rgb(0x9a9a9a),
        syn_tag: rgb(0xf7768e),
    }
}

/// Bright white theme for well-lit rooms and light terminals.
pub fn white() -> Theme {
    Theme {
        name: ThemeName::White,
        bg: rgb(0xffffff),
        bg_alt: rgb(0xf2f2f2),
        bg_sel: rgb(0xdcdcdc),
        fg: rgb(0x1f2328),
        fg_dim: rgb(0x3d444d),
        fg_muted: rgb(0x6e7781),
        accent: rgb(0x0550ae),
        accent2: rgb(0x0969da),
        green: rgb(0x116329),
        yellow: rgb(0x9a6700),
        orange: rgb(0xbc4c00),
        red: rgb(0xcf222e),
        purple: rgb(0x8250df),
        cyan: rgb(0x1b7c83),
        border: rgb(0xd0d7de),
        syn_comment: rgb(0x6e7781),
        syn_string: rgb(0x0a3069),
        syn_number: rgb(0x953800),
        syn_keyword: rgb(0xcf222e),
        syn_function: rgb(0x8250df),
        syn_type: rgb(0x0550ae),
        syn_variable: rgb(0x24292f),
        syn_operator: rgb(0x0550ae),
        syn_punct: rgb(0x57606a),
        syn_tag: rgb(0x116329),
    }
}

/// Snow: a soft greyish-white background with cool, dark text.
pub fn snow() -> Theme {
    Theme {
        name: ThemeName::Snow,
        bg: rgb(0xe6e9ef),
        bg_alt: rgb(0xd7dce5),
        bg_sel: rgb(0xc3cad6),
        fg: rgb(0x2b303b),
        fg_dim: rgb(0x4a5263),
        fg_muted: rgb(0x707a8c),
        accent: rgb(0x4f6f9f),
        accent2: rgb(0x2f6f8f),
        green: rgb(0x477a4b),
        yellow: rgb(0x9a7420),
        orange: rgb(0xb4692d),
        red: rgb(0xa83a3a),
        purple: rgb(0x75588f),
        cyan: rgb(0x2f7f86),
        border: rgb(0xb3bac6),
        syn_comment: rgb(0x707a8c),
        syn_string: rgb(0x356b4a),
        syn_number: rgb(0x9a5a20),
        syn_keyword: rgb(0x8a3f7a),
        syn_function: rgb(0x2f5f9f),
        syn_type: rgb(0x2f6f6f),
        syn_variable: rgb(0x2b303b),
        syn_operator: rgb(0x2f6f8f),
        syn_punct: rgb(0x586174),
        syn_tag: rgb(0xa83a3a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_roundtrip() {
        for name in ThemeName::ALL {
            assert_eq!(ThemeName::from_slug(name.slug()), Some(name));
            assert_eq!(name.theme().name, name);
        }
        assert_eq!(
            ThemeName::from_slug("Catppuccin Mocha"),
            Some(ThemeName::CatppuccinMocha)
        );
        assert_eq!(
            ThemeName::from_slug("TOKYO_NIGHT"),
            Some(ThemeName::TokyoNight)
        );
        assert_eq!(ThemeName::from_slug("nope"), None);
    }

    #[test]
    fn theme_cycle_visits_every_theme() {
        let mut name = ThemeName::ALL[0];
        for _ in 0..ThemeName::ALL.len() {
            name = name.next();
        }
        assert_eq!(name, ThemeName::ALL[0]);
    }
}

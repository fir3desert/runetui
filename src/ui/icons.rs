//! File and folder icons for the project explorer.
//!
//! Three styles are available so the explorer looks right in every terminal:
//!
//! * `nerd`    – Nerd Font glyphs (needs a patched font such as
//!   “JetBrainsMono Nerd Font”); the default.
//! * `unicode` – letter badges (`R` for Rust, `M` for Markdown, a red `D` for
//!   `.d` files...) and simple symbols that any font can draw.
//! * `ascii`   – letters and punctuation only.

use std::path::Path;

use ratatui::style::Color;

use crate::ui::themes::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconStyle {
    Nerd,
    Unicode,
    Ascii,
}

impl IconStyle {
    pub const ALL: [IconStyle; 3] = [IconStyle::Nerd, IconStyle::Unicode, IconStyle::Ascii];

    pub fn slug(self) -> &'static str {
        match self {
            IconStyle::Nerd => "nerd",
            IconStyle::Unicode => "unicode",
            IconStyle::Ascii => "ascii",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|style| style.slug().eq_ignore_ascii_case(slug.trim()))
    }

    /// Next style in the cycle `nerd → unicode → ascii → nerd`.
    pub fn next(self) -> Self {
        match self {
            IconStyle::Nerd => IconStyle::Unicode,
            IconStyle::Unicode => IconStyle::Ascii,
            IconStyle::Ascii => IconStyle::Nerd,
        }
    }

    /// Marker drawn before a folder name: collapsed or expanded.
    pub fn chevron(self, expanded: bool) -> &'static str {
        match (self, expanded) {
            (IconStyle::Ascii, true) => "v ",
            (IconStyle::Ascii, false) => "> ",
            (_, true) => "▾ ",
            (_, false) => "▸ ",
        }
    }

    /// Vertical guide drawn for every ancestor level.
    pub fn guide(self) -> &'static str {
        match self {
            IconStyle::Ascii => "| ",
            _ => "│ ",
        }
    }
}

/// Icon and colour for a folder.
pub fn folder_icon(
    name: &str,
    expanded: bool,
    style: IconStyle,
    theme: &Theme,
) -> (&'static str, Color) {
    let lower = name.to_ascii_lowercase();

    match style {
        IconStyle::Ascii => (if expanded { "[-]" } else { "[+]" }, theme.yellow),
        IconStyle::Unicode => (
            if expanded { "▣" } else { "▢" },
            folder_color(&lower, theme),
        ),
        IconStyle::Nerd => {
            // (closed glyph, open glyph, colour)
            let (closed, open, color) = match lower.as_str() {
                ".git" => ("\u{e5fb}", "\u{e5fb}", theme.orange),
                ".github" => ("\u{e5fd}", "\u{e5fd}", theme.fg_dim),
                "node_modules" => ("\u{e5fa}", "\u{e5fa}", theme.green),
                ".config" | "config" | "configs" | "conf" => ("\u{e5fc}", "\u{e5fc}", theme.cyan),
                "src" | "source" | "lib" | "crates" | "app" => {
                    ("\u{f07b}", "\u{f07c}", theme.accent)
                }
                "test" | "tests" | "spec" | "specs" | "__tests__" => {
                    ("\u{f07b}", "\u{f07c}", theme.green)
                }
                "doc" | "docs" | "documentation" | "notes" | "wiki" => {
                    ("\u{f07b}", "\u{f07c}", theme.accent2)
                }
                "target" | "build" | "dist" | "out" | "bin" | "release" | "debug" => {
                    ("\u{f07b}", "\u{f07c}", theme.fg_muted)
                }
                "assets" | "images" | "img" | "static" | "public" | "media" => {
                    ("\u{f07b}", "\u{f07c}", theme.purple)
                }
                _ => ("\u{f07b}", "\u{f07c}", theme.yellow),
            };
            (if expanded { open } else { closed }, color)
        }
    }
}

/// Folder colour by well-known name, shared by the unicode style.
fn folder_color(lower: &str, theme: &Theme) -> Color {
    match lower {
        ".git" => theme.orange,
        ".github" => theme.fg_dim,
        "node_modules" => theme.green,
        ".config" | "config" | "configs" | "conf" => theme.cyan,
        "src" | "source" | "lib" | "crates" | "app" => theme.accent,
        "test" | "tests" | "spec" | "specs" | "__tests__" => theme.green,
        "doc" | "docs" | "documentation" | "notes" | "wiki" => theme.accent2,
        "target" | "build" | "dist" | "out" | "bin" | "release" | "debug" => theme.fg_muted,
        "assets" | "images" | "img" | "static" | "public" | "media" => theme.purple,
        _ => theme.yellow,
    }
}

/// Icon and colour for a file, chosen by exact name first and extension second.
pub fn file_icon(name: &str, style: IconStyle, theme: &Theme) -> (&'static str, Color) {
    match style {
        IconStyle::Nerd => nerd_file_icon(name, theme),
        IconStyle::Unicode => unicode_file_icon(name, theme),
        IconStyle::Ascii => ascii_file_icon(name, theme),
    }
}

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn nerd_file_icon(name: &str, theme: &Theme) -> (&'static str, Color) {
    let lower = name.to_ascii_lowercase();

    // Well-known file names.
    match lower.as_str() {
        "cargo.toml" => return ("\u{e7a8}", theme.orange),
        "cargo.lock" => return ("\u{f023}", theme.fg_muted),
        "readme" | "readme.md" | "readme.txt" => return ("\u{f05a}", theme.accent2),
        "license" | "license.md" | "license.txt" | "licence" | "copying" => {
            return ("\u{f0a3}", theme.yellow);
        }
        "dockerfile"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | "compose.yml"
        | "compose.yaml" => return ("\u{f308}", theme.accent2),
        "makefile" | "gnumakefile" | "justfile" => return ("\u{e779}", theme.orange),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => {
            return ("\u{e702}", theme.orange);
        }
        "package.json" | "package-lock.json" => return ("\u{e71e}", theme.green),
        ".env" | ".envrc" => return ("\u{f023}", theme.yellow),
        _ => {}
    }
    if lower.starts_with(".env") {
        return ("\u{f023}", theme.yellow);
    }

    match extension_of(name).as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => ("\u{e73e}", theme.accent2),
        "rs" => ("\u{e7a8}", theme.orange),
        "js" | "mjs" | "cjs" => ("\u{e74e}", theme.yellow),
        "ts" => ("\u{e628}", theme.accent2),
        "tsx" | "jsx" => ("\u{e7ba}", theme.cyan),
        "json" | "jsonc" | "json5" => ("\u{e60b}", theme.yellow),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" => ("\u{e615}", theme.fg_dim),
        "html" | "htm" => ("\u{e736}", theme.orange),
        "xml" => ("\u{f1c9}", theme.orange),
        "css" | "less" => ("\u{e749}", theme.accent2),
        "scss" | "sass" => ("\u{e603}", theme.purple),
        "sh" | "bash" | "zsh" | "fish" | "nu" => ("\u{e795}", theme.green),
        "py" | "pyw" => ("\u{e73c}", theme.yellow),
        "go" => ("\u{e627}", theme.cyan),
        "c" | "h" => ("\u{e61e}", theme.accent2),
        "cc" | "cpp" | "cxx" | "hpp" => ("\u{e61d}", theme.accent2),
        "java" => ("\u{e738}", theme.red),
        "kt" | "kts" => ("\u{e634}", theme.purple),
        "scala" => ("\u{e737}", theme.red),
        "lua" => ("\u{e620}", theme.accent2),
        "vim" => ("\u{e62b}", theme.green),
        "sql" => ("\u{e706}", theme.cyan),
        "txt" | "text" | "rst" | "adoc" | "org" => ("\u{f15c}", theme.fg_dim),
        "log" => ("\u{f18d}", theme.fg_muted),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => {
            ("\u{f1c5}", theme.purple)
        }
        "pdf" => ("\u{f1c1}", theme.red),
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" => ("\u{f1c6}", theme.yellow),
        "lock" => ("\u{f023}", theme.fg_muted),
        _ => ("\u{f15b}", theme.fg_muted),
    }
}

fn unicode_file_icon(name: &str, theme: &Theme) -> (&'static str, Color) {
    let lower = name.to_ascii_lowercase();
    // Well-known file names.
    match lower.as_str() {
        "cargo.toml" => return ("C", theme.orange),
        "cargo.lock" => return ("▪", theme.fg_muted),
        "readme" | "readme.md" | "readme.txt" => return ("i", theme.accent2),
        "license" | "license.md" | "license.txt" | "licence" | "copying" => {
            return ("§", theme.yellow);
        }
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" | "compose.yml"
        | "compose.yaml" => return ("D", theme.accent2),
        "makefile" | "gnumakefile" | "justfile" => return ("M", theme.orange),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => {
            return ("G", theme.orange);
        }
        "package.json" | "package-lock.json" => return ("N", theme.green),
        _ => {}
    }
    if lower.starts_with(".env") {
        return ("E", theme.yellow);
    }

    match extension_of(name).as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => ("M", theme.accent2),
        "rs" => ("R", theme.orange),
        // Cargo dependency files (`target/debug/deps/*.d`).
        "d" => ("D", theme.red),
        "js" | "mjs" | "cjs" => ("J", theme.yellow),
        "ts" => ("T", theme.accent2),
        "tsx" | "jsx" => ("X", theme.cyan),
        "json" | "jsonc" | "json5" => ("{", theme.yellow),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" => ("⚙", theme.green),
        "html" | "htm" | "xml" | "svg" => ("<", theme.orange),
        "css" | "scss" | "sass" | "less" => ("#", theme.purple),
        "sh" | "bash" | "zsh" | "fish" | "nu" => ("$", theme.green),
        "py" | "pyw" => ("P", theme.yellow),
        "go" => ("G", theme.cyan),
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => ("C", theme.accent2),
        "java" => ("J", theme.red),
        "kt" | "kts" => ("K", theme.purple),
        "scala" => ("S", theme.red),
        "lua" => ("L", theme.accent2),
        "vim" => ("V", theme.green),
        "sql" => ("Q", theme.cyan),
        "txt" | "text" | "log" | "rst" | "adoc" | "org" => ("≡", theme.fg_dim),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => ("▣", theme.purple),
        "pdf" => ("P", theme.red),
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" => ("Z", theme.yellow),
        "lock" => ("▪", theme.fg_muted),
        _ => {
            if name.starts_with('.') {
                ("•", theme.fg_muted)
            } else {
                ("·", theme.fg_muted)
            }
        }
    }
}

fn ascii_file_icon(name: &str, theme: &Theme) -> (&'static str, Color) {
    match extension_of(name).as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => ("#", theme.accent2),
        "rs" | "go" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp"
        | "java" | "kt" | "lua" | "sh" | "bash" | "zsh" => ("*", theme.orange),
        "toml" | "yaml" | "yml" | "json" | "ini" | "conf" | "cfg" => ("%", theme.green),
        _ => ("-", theme.fg_muted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::ThemeName;

    #[test]
    fn style_slugs_roundtrip_and_cycle() {
        for style in IconStyle::ALL {
            assert_eq!(IconStyle::from_slug(style.slug()), Some(style));
        }
        assert_eq!(IconStyle::from_slug("NERD"), Some(IconStyle::Nerd));
        assert_eq!(IconStyle::from_slug("emoji"), None);
        assert_eq!(IconStyle::Nerd.next().next().next(), IconStyle::Nerd);
    }

    #[test]
    fn nerd_folders_change_glyph_when_opened() {
        let theme = ThemeName::CatppuccinMocha.theme();
        let (closed, _) = folder_icon("docs", false, IconStyle::Nerd, &theme);
        let (open, _) = folder_icon("docs", true, IconStyle::Nerd, &theme);
        assert_ne!(closed, open);
    }

    #[test]
    fn special_names_beat_extensions() {
        let theme = ThemeName::CatppuccinMocha.theme();
        let (lock, _) = file_icon("Cargo.lock", IconStyle::Nerd, &theme);
        let (other, _) = file_icon("data.lock", IconStyle::Nerd, &theme);
        assert_eq!(lock, other);
        let (cargo, _) = file_icon("Cargo.toml", IconStyle::Nerd, &theme);
        let (plain, _) = file_icon("settings.toml", IconStyle::Nerd, &theme);
        assert_ne!(cargo, plain);
    }

    #[test]
    fn unicode_files_get_letter_badges_by_type() {
        let theme = ThemeName::CatppuccinMocha.theme();
        assert_eq!(file_icon("main.rs", IconStyle::Unicode, &theme), ("R", theme.orange));
        assert_eq!(file_icon("notes.md", IconStyle::Unicode, &theme), ("M", theme.accent2));
        // Like the red `D` badge of a Cargo dependency file.
        assert_eq!(
            file_icon("aho_corasick-f671788b879fe6cb.d", IconStyle::Unicode, &theme),
            ("D", theme.red)
        );
        assert_ne!(
            file_icon("Cargo.toml", IconStyle::Unicode, &theme).0,
            file_icon("other.toml", IconStyle::Unicode, &theme).0
        );
        // Every badge is a single column wide so the tree stays aligned.
        for name in ["a.rs", "a.md", "a.json", "a.toml", "a.sh", "a.py", "a.html", "LICENSE", ".env"] {
            let (icon, _) = file_icon(name, IconStyle::Unicode, &theme);
            assert_eq!(icon.chars().count(), 1, "{name}");
        }
    }

    #[test]
    fn unicode_folders_are_coloured_by_name() {
        let theme = ThemeName::CatppuccinMocha.theme();
        let (_, src) = folder_icon("src", false, IconStyle::Unicode, &theme);
        let (_, tests) = folder_icon("tests", false, IconStyle::Unicode, &theme);
        let (_, plain) = folder_icon("misc", false, IconStyle::Unicode, &theme);
        assert_eq!(src, theme.accent);
        assert_eq!(tests, theme.green);
        assert_eq!(plain, theme.yellow);
    }

    #[test]
    fn every_style_returns_an_icon_for_unknown_files() {
        let theme = ThemeName::Nord.theme();
        for style in IconStyle::ALL {
            let (icon, _) = file_icon("mystery.zzz", style, &theme);
            assert!(!icon.is_empty());
        }
    }
}

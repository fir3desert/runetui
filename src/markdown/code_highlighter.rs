//! Real token-based syntax highlighting built on `syntect`.
//!
//! Instead of shipping a fixed `.tmTheme`, a syntect theme is generated from
//! the active `runetui` palette.  As a result every theme produces coherent
//! code colours and the highlighting never looks pasted on.

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    FontStyle, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::markdown::RSpan;
use crate::ui::themes::{Theme, ThemeName};

pub struct CodeHighlighter {
    syntaxes: SyntaxSet,
    cached_theme: Option<(ThemeName, SynTheme)>,
}

#[derive(Debug, Clone)]
pub struct HighlightedCode {
    /// Nice label for the block header.  Empty for plain text.
    pub label: String,
    /// True when the language was inferred rather than declared.
    pub detected: bool,
    pub lines: Vec<Vec<RSpan>>,
}

impl CodeHighlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_nonewlines(),
            cached_theme: None,
        }
    }

    /// Highlight `code` using an explicit language tag (may be empty).  The
    /// returned spans only carry foreground colour and font modifiers; the
    /// renderer decides the background.
    pub fn highlight(&mut self, code: &str, lang: &str, theme: &Theme) -> HighlightedCode {
        self.ensure_theme(theme);

        let (syntax, label, detected) = self.resolve(lang, code);

        let Some((_, syn_theme)) = &self.cached_theme else {
            return plain_code(code, label, detected);
        };

        let mut highlighter = HighlightLines::new(syntax, syn_theme);
        let mut lines = Vec::new();

        // `lines()` drops a trailing newline; the renderer deals with empty
        // blocks separately.
        for source_line in code.lines() {
            match highlighter.highlight_line(source_line, &self.syntaxes) {
                Ok(tokens) => {
                    let spans = tokens
                        .into_iter()
                        .filter(|(_, text)| !text.is_empty())
                        .map(|(style, text)| {
                            RSpan::new(text.replace('\t', "    "), convert_style(&style))
                        })
                        .collect::<Vec<_>>();
                    lines.push(spans);
                }
                Err(_) => lines.push(vec![RSpan::new(
                    source_line.replace('\t', "    "),
                    Style::default().fg(theme.fg),
                )]),
            }
        }

        if code.is_empty() {
            lines.push(Vec::new());
        }

        HighlightedCode {
            label,
            detected,
            lines,
        }
    }

    fn ensure_theme(&mut self, theme: &Theme) {
        let needs_rebuild = self
            .cached_theme
            .as_ref()
            .map(|(name, _)| *name != theme.name)
            .unwrap_or(true);
        if needs_rebuild {
            self.cached_theme = Some((theme.name, build_syntect_theme(theme)));
        }
    }

    fn resolve(&self, lang: &str, code: &str) -> (&SyntaxReference, String, bool) {
        let token = normalize_token(lang);

        if !token.is_empty() {
            if let Some(syntax) = self
                .syntaxes
                .find_syntax_by_token(&token)
                .or_else(|| self.syntaxes.find_syntax_by_extension(&token))
                .or_else(|| self.syntaxes.find_syntax_by_name(lang.trim()))
            {
                return (syntax, display_label(&token, syntax), false);
            }
            // Unknown tag: still try the first line, then fall back to plain.
            if let Some(syntax) = self
                .syntaxes
                .find_syntax_by_first_line(first_line(code).unwrap_or(""))
            {
                return (syntax, display_label(&token, syntax), false);
            }
            return (self.syntaxes.find_syntax_plain_text(), token, false);
        }

        if let Some(guessed) = sniff_language(code) {
            if let Some(syntax) = self
                .syntaxes
                .find_syntax_by_token(guessed)
                .or_else(|| self.syntaxes.find_syntax_by_extension(guessed))
            {
                return (syntax, guessed.to_string(), true);
            }
        }

        if let Some(first) = first_line(code) {
            if let Some(syntax) = self.syntaxes.find_syntax_by_first_line(first) {
                return (syntax, syntax.name.to_ascii_lowercase(), true);
            }
        }

        (self.syntaxes.find_syntax_plain_text(), String::new(), false)
    }
}

impl Default for CodeHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

fn plain_code(code: &str, label: String, detected: bool) -> HighlightedCode {
    let mut lines: Vec<Vec<RSpan>> = code
        .lines()
        .map(|line| vec![RSpan::new(line.replace('\t', "    "), Style::default())])
        .collect();
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    HighlightedCode {
        label,
        detected,
        lines,
    }
}

fn first_line(code: &str) -> Option<&str> {
    code.lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

fn normalize_token(lang: &str) -> String {
    lang.trim()
        .trim_start_matches('.')
        .split([' ', '\t', ',', ';', '{'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn display_label(token: &str, syntax: &SyntaxReference) -> String {
    let _ = syntax;
    canonical_label(token)
}

/// Canonical, user-facing language label for a token/extension.
pub fn canonical_label(token: &str) -> String {
    let token = normalize_token(token);
    let canonical = match token.as_str() {
        "sh" | "shell" | "zsh" | "console" | "shell-session" | "bash-session" => "bash",
        "js" | "jsx" | "node" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "yml" => "yaml",
        "rs" => "rust",
        "ps1" | "pwsh" => "powershell",
        "c++" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "htm" => "html",
        "md" | "mdown" | "mkd" => "markdown",
        "docker" | "containerfile" => "dockerfile",
        "jsonc" | "json5" => "json",
        "rb" => "ruby",
        "kt" | "kts" => "kotlin",
        "cs" => "csharp",
        "vb" => "vbnet",
        "objective-c" | "objc" => "objective-c",
        "make" | "mk" => "makefile",
        other => other,
    };
    canonical.to_string()
}

/// Best-effort language sniffing for fenced blocks without an info string.
/// Only high-confidence patterns are accepted; otherwise the block is rendered
/// as plain text, exactly as requested.
pub fn sniff_language(code: &str) -> Option<&'static str> {
    let trimmed = code.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    let first = trimmed.lines().next().unwrap_or("").trim();

    if let Some(shebang) = first.strip_prefix("#!") {
        let lower = shebang.to_ascii_lowercase();
        if lower.contains("python") {
            return Some("python");
        }
        if lower.contains("bash") || lower.contains("zsh") || lower.contains("/sh") {
            return Some("bash");
        }
        if lower.contains("node") || lower.contains("deno") {
            return Some("javascript");
        }
        if lower.contains("ruby") {
            return Some("ruby");
        }
        if lower.contains("perl") {
            return Some("perl");
        }
        if lower.contains("pwsh") || lower.contains("powershell") {
            return Some("powershell");
        }
    }

    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("<?php") {
        return Some("php");
    }
    if lower.starts_with("<?xml") {
        return Some("xml");
    }
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return Some("html");
    }
    if lower.starts_with("package main") && lower.contains("func ") {
        return Some("go");
    }
    if lower.starts_with("#include") {
        if lower.contains("<iostream>") || lower.contains("std::") {
            return Some("cpp");
        }
        return Some("c");
    }
    if lower.contains("public class ") && lower.contains("static void main") {
        return Some("java");
    }
    if lower.starts_with("[package]")
        || lower.starts_with("[workspace]")
        || (lower.contains(" = ")
            && lower.contains('[')
            && lower.contains(']')
            && !lower.contains('{'))
    {
        return Some("toml");
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && trimmed.contains("\":")
        && trimmed.contains('}')
    {
        return Some("json");
    }
    if lower.contains("fn main(")
        || (lower.contains("fn ") && lower.contains("let ") && lower.contains("->"))
    {
        return Some("rust");
    }
    if lower.contains("def ") && lower.contains(":\n") && !lower.contains("<?") {
        return Some("python");
    }
    if lower.contains("select ")
        && (lower.contains(" from ") || lower.contains("\nfrom "))
        && lower.contains("where")
    {
        return Some("sql");
    }
    if (lower.starts_with("from ") || lower.contains("\nfrom "))
        && (lower.contains("\nrun ") || lower.contains("\ncmd ") || lower.contains("\ncopy "))
    {
        return Some("dockerfile");
    }
    if lower.contains("$env:") || lower.contains("write-host") || lower.contains("get-childitem") {
        return Some("powershell");
    }
    if lower.contains("function ")
        && lower.contains('{')
        && lower.contains('}')
        && lower.contains(';')
    {
        return Some("javascript");
    }
    if lower.contains("{#") && lower.contains("#}") {
        return Some("handlebars");
    }
    if trimmed.starts_with("# ")
        && (lower.contains("\n## ") || lower.contains("\n- ") || lower.contains("\n> "))
    {
        return Some("markdown");
    }
    if first.starts_with('>') && lower.contains(':') {
        return Some("yaml");
    }
    let shell_markers = [
        "#!/bin/bash",
        "#!/bin/sh",
        "echo ",
        "chmod ",
        "mkdir ",
        "sudo ",
        "apt ",
    ];
    if lower.contains("; do") && lower.contains("done") {
        return Some("bash");
    }
    if shell_markers
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count()
        >= 1
        && (lower.contains("$") || lower.contains("export "))
    {
        return Some("bash");
    }

    None
}

fn convert_style(style: &syntect::highlighting::Style) -> Style {
    let mut converted = Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));
    if style.font_style.contains(FontStyle::BOLD) {
        converted = converted.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        converted = converted.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        converted = converted.add_modifier(Modifier::UNDERLINED);
    }
    converted
}

fn syntect_color(color: Color) -> syntect::highlighting::Color {
    match color {
        Color::Rgb(r, g, b) => syntect::highlighting::Color { r, g, b, a: 0xff },
        Color::Black => syntect::highlighting::Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0xff,
        },
        Color::White => syntect::highlighting::Color {
            r: 0xff,
            g: 0xff,
            b: 0xff,
            a: 0xff,
        },
        _ => syntect::highlighting::Color {
            r: 0xcd,
            g: 0xd6,
            b: 0xf4,
            a: 0xff,
        },
    }
}

fn build_syntect_theme(theme: &Theme) -> SynTheme {
    let mut scopes: Vec<ThemeItem> = Vec::new();

    let mut push = |selector: &str, foreground: Color, font_style: Option<FontStyle>| {
        if let Ok(scope) = selector.parse::<ScopeSelectors>() {
            scopes.push(ThemeItem {
                scope,
                style: StyleModifier {
                    foreground: Some(syntect_color(foreground)),
                    background: None,
                    font_style,
                },
            });
        }
    };

    // Base text roles first; more specific selectors later.  Syntect scores
    // selectors by specificity, and order breaks ties.
    push("source", theme.fg, None);
    push("text", theme.fg, None);
    push("punctuation", theme.syn_punct, None);
    push("punctuation.definition.comment", theme.syn_comment, None);
    push("comment", theme.syn_comment, Some(FontStyle::ITALIC));
    push("string", theme.syn_string, None);
    push("string.regexp", theme.syn_string, None);
    push("constant.character.escape", theme.syn_number, None);
    push("constant.numeric", theme.syn_number, None);
    push("constant.language", theme.syn_number, None);
    push("constant.character", theme.syn_number, None);
    push("constant.other", theme.syn_number, None);
    push("keyword.operator", theme.syn_operator, None);
    push("keyword.control", theme.syn_keyword, Some(FontStyle::BOLD));
    push("keyword", theme.syn_keyword, Some(FontStyle::BOLD));
    push("storage", theme.syn_keyword, Some(FontStyle::BOLD));
    push("entity.name.function", theme.syn_function, None);
    push("support.function", theme.syn_function, None);
    push("entity.name.type", theme.syn_type, None);
    push("entity.name.class", theme.syn_type, None);
    push("support.type", theme.syn_type, None);
    push("support.class", theme.syn_type, None);
    push("entity.other.inherited-class", theme.syn_type, None);
    push("entity.name.tag", theme.syn_tag, None);
    push("entity.other.attribute-name", theme.syn_type, None);
    push("variable.parameter", theme.syn_variable, None);
    push("variable.other", theme.syn_variable, None);
    push("variable", theme.syn_variable, None);
    push("meta.preprocessor", theme.syn_keyword, None);
    push("meta.annotation", theme.syn_function, None);
    push("invalid", theme.red, None);
    push("markup.heading", theme.accent, Some(FontStyle::BOLD));
    push("markup.bold", theme.fg, Some(FontStyle::BOLD));
    push("markup.italic", theme.fg, Some(FontStyle::ITALIC));
    push("markup.raw", theme.syn_string, None);
    push(
        "markup.underline.link",
        theme.accent2,
        Some(FontStyle::UNDERLINE),
    );
    push("markup.inserted", theme.green, None);
    push("markup.deleted", theme.red, None);

    SynTheme {
        name: Some(theme.name.label().to_string()),
        author: Some("runetui".to_string()),
        settings: ThemeSettings {
            foreground: Some(syntect_color(theme.fg)),
            background: Some(syntect_color(theme.code_bg())),
            caret: Some(syntect_color(theme.accent)),
            selection: Some(syntect_color(theme.bg_sel)),
            ..ThemeSettings::default()
        },
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::catppuccin_mocha;

    #[test]
    fn highlights_rust_tokens() {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let result = highlighter.highlight("fn main() {\n    let x = 1;\n}\n", "rust", &theme);
        assert_eq!(result.label, "rust");
        assert!(!result.detected);
        assert_eq!(result.lines.len(), 3);
        let whole: String = result
            .lines
            .iter()
            .flat_map(|line| line.iter().map(|span| span.text.as_str()))
            .collect();
        assert!(whole.contains("fn main"));
        // Tokens must be split, not one monolithic span.
        assert!(result.lines[0].len() > 1);
    }

    #[test]
    fn auto_detects_common_languages() {
        assert_eq!(
            sniff_language("#!/usr/bin/env bash\nfor i in {1..3}; do echo $i; done"),
            Some("bash")
        );
        assert_eq!(
            sniff_language("def main():\n    print('hi')\n"),
            Some("python")
        );
        assert_eq!(
            sniff_language("fn main() {\n    let x = 1;\n}\n"),
            Some("rust")
        );
        assert_eq!(sniff_language("{\n  \"a\": 1\n}\n"), Some("json"));
        assert_eq!(sniff_language("just some prose"), None);
    }

    #[test]
    fn resolves_aliases() {
        let mut highlighter = CodeHighlighter::new();
        let theme = catppuccin_mocha();
        assert_eq!(highlighter.highlight("echo hi", "sh", &theme).label, "bash");
        assert_eq!(highlighter.highlight("x = 1", "py", &theme).label, "python");
        assert_eq!(
            highlighter.highlight("{\"a\":1}", "json", &theme).label,
            "json"
        );
    }

    #[test]
    fn unknown_language_falls_back_to_plain() {
        let mut highlighter = CodeHighlighter::new();
        let theme = catppuccin_mocha();
        let result = highlighter.highlight("some text", "notalanguage", &theme);
        assert_eq!(result.label, "notalanguage");
        assert_eq!(result.lines.len(), 1);
    }
}

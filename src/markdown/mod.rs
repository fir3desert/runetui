//! Markdown parsing, rendering and syntax highlighting.
//!
//! The pipeline is:
//!
//! 1. [`parser`] converts the source into an owned block/inline AST.
//! 2. [`renderer`] lays the AST out into styled terminal lines for a given
//!    width and theme.
//! 3. [`code_highlighter`] provides real token-based highlighting for fenced
//!    code blocks (and standalone source files).
//!
//! Rendering works entirely in memory.  The source files are never modified.

pub mod code_highlighter;
pub mod parser;
pub mod renderer;
pub mod toc;

use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

pub use parser::{LinkKind, parse};

/// A complete, width-specific rendering of a document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderedDoc {
    pub lines: Vec<RLine>,
    pub headings: Vec<Heading>,
    pub links: Vec<DocLink>,
}

impl RenderedDoc {
    /// Link (if any) located on `line`.  The first link wins; that matches the
    /// "press Enter on the current line" interaction.
    #[cfg(test)]
    pub fn link_at(&self, line: usize) -> Option<&DocLink> {
        self.links
            .iter()
            .filter(|link| link.line == line)
            .min_by_key(|link| link.column)
    }
}

/// One rendered display line.  All wrapping has already been applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RLine {
    pub spans: Vec<RSpan>,
}

impl RLine {
    pub fn new(spans: Vec<RSpan>) -> Self {
        Self { spans }
    }

    #[cfg(test)]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![RSpan::plain(text)],
        }
    }

    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|span| span.text.trim().is_empty())
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            out.push_str(&span.text);
        }
        out
    }

    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.text.as_str()))
            .sum()
    }
}

/// A styled piece of text.  `link` indexes [`RenderedDoc::links`] and lets the
/// renderer map wrapped link text back to the logical link.
#[derive(Debug, Clone, PartialEq)]
pub struct RSpan {
    pub text: String,
    pub style: Style,
    pub link: Option<usize>,
}

impl RSpan {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, Style::default())
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(text, style)
    }

    pub fn with_link(mut self, link: Option<usize>) -> Self {
        self.link = link;
        self
    }

    pub fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

impl From<&str> for RSpan {
    fn from(value: &str) -> Self {
        RSpan::plain(value)
    }
}

impl From<String> for RSpan {
    fn from(value: String) -> Self {
        RSpan::plain(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub slug: String,
    pub line: usize,
}

/// A navigable link discovered while rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLink {
    pub line: usize,
    /// Column (character index) of the link start, used for deterministic
    /// selection when a line contains several links.
    pub column: usize,
    pub label: String,
    pub dest: String,
    pub kind: LinkKind,
    pub image: bool,
}

/// Converts a heading into a stable anchor, e.g. `## Hello, World!` ->
/// `hello-world`.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in text.chars() {
        if character.is_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            for lowercase in character.to_lowercase() {
                slug.push(lowercase);
            }
        } else if character == '-' || character == '_' || character.is_whitespace() {
            pending_dash = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_handles_punctuation_and_case() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  Installation  "), "installation");
        assert_eq!(slugify("API v2.0 — Guide"), "api-v20-guide");
        assert_eq!(slugify("¿Qué es esto?"), "qué-es-esto");
    }

    #[test]
    fn line_width_respects_unicode() {
        let line = RLine {
            spans: vec![RSpan::plain("hola "), RSpan::plain("日本")],
        };
        assert_eq!(line.width(), 9);
        assert_eq!(line.text(), "hola 日本");
    }
}

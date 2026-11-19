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

pub use parser::{LineSpan, LinkKind, SourceLink, markdown_options, parse_spanned, scan_links};
#[cfg(test)]
pub use parser::parse;

/// A complete, width-specific rendering of a document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderedDoc {
    pub lines: Vec<RLine>,
    pub headings: Vec<Heading>,
    pub links: Vec<DocLink>,
    /// Fenced/indented code blocks, so Enter or Space on one can copy it.
    pub code_blocks: Vec<CodeRegion>,
    /// Which source lines each top-level block of the document came from, in
    /// row order.  Empty when the renderer was not given the source.
    pub blocks: Vec<BlockMap>,
}

/// Where a top-level block of the rendered document came from in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMap {
    /// Rendered rows of the block (end exclusive), blank separators included.
    pub start: usize,
    pub end: usize,
    /// The rows that actually hold content (end exclusive).
    pub content_start: usize,
    pub content_end: usize,
    /// Source lines, 0-based and both ends included.
    pub src_start: usize,
    pub src_end: usize,
    /// Content row `content_start + i` is exactly source line `src_start + i`
    /// (checked against the text, so wrapped or re-flowed blocks are not).
    pub exact: bool,
}

/// A stretch of source lines found for some rendered rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    /// 0-based, both ends included.
    pub start: usize,
    pub end: usize,
    /// False when the rows could only be mapped to whole blocks, so the range
    /// may hold more than was selected.
    pub exact: bool,
}

/// A code block's place in the rendered document and its raw source text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeRegion {
    /// First rendered row of the block (its top border, when it has a frame).
    pub start: usize,
    /// One past the last rendered row of the block.
    pub end: usize,
    /// The code exactly as written, without the frame or the highlighting.
    pub code: String,
}

impl RenderedDoc {
    /// The block that owns rendered row `row`.
    fn block_of_row(&self, row: usize) -> Option<&BlockMap> {
        let index = self.blocks.partition_point(|block| block.start <= row);
        let block = self.blocks.get(index.checked_sub(1)?)?;
        (row < block.end).then_some(block)
    }

    /// Source line shown at rendered row `row` (the first line of the block
    /// when the block cannot be mapped row by row).
    pub fn source_line_at(&self, row: usize) -> Option<usize> {
        let block = self.block_of_row(row)?;
        Some(block_line(block, row))
    }

    /// First rendered row that shows source line `line`.  A line between two
    /// blocks (blank) leads to the next block.
    pub fn row_of_source_line(&self, line: usize) -> Option<usize> {
        let block = self
            .blocks
            .iter()
            .find(|block| line <= block.src_end)
            .or(self.blocks.last())?;
        if line < block.src_start {
            return Some(block.content_start);
        }
        Some(if block.exact {
            (block.content_start + (line - block.src_start)).min(block.content_end - 1)
        } else {
            block.content_start
        })
    }

    /// Within the block that holds source line `line`, the first row whose
    /// text contains `needle`.  Used to land on a link inside a block that
    /// can only be mapped as a whole.
    pub fn row_of_text_in_block(&self, line: usize, needle: &str) -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        let block = self
            .blocks
            .iter()
            .find(|block| block.src_start <= line && line <= block.src_end)?;
        (block.content_start..block.content_end)
            .find(|row| self.lines.get(*row).is_some_and(|line| line.text().contains(needle)))
    }

    /// Source lines behind the rendered rows `first..=last`.
    pub fn source_range(&self, first: usize, last: usize) -> Option<SourceRange> {
        let (first, last) = if first <= last { (first, last) } else { (last, first) };
        let top = self.block_of_row(first)?;
        let bottom = self.block_of_row(last).or_else(|| self.blocks.last())?;
        if std::ptr::eq(top, bottom) {
            if top.exact {
                let start = block_line(top, first);
                let end = block_line(top, last);
                return Some(SourceRange { start, end, exact: true });
            }
            return Some(SourceRange { start: top.src_start, end: top.src_end, exact: false });
        }
        let start = if top.exact { block_line(top, first) } else { top.src_start };
        let end = if bottom.exact { block_line(bottom, last) } else { bottom.src_end };
        Some(SourceRange {
            start,
            end: end.max(start),
            exact: top.exact && bottom.exact,
        })
    }

    /// The code block that covers rendered row `line`, if any.
    pub fn code_block_at(&self, line: usize) -> Option<&CodeRegion> {
        self.code_blocks
            .iter()
            .find(|region| line >= region.start && line < region.end)
    }

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

/// Source line of `row` inside `block`.
fn block_line(block: &BlockMap, row: usize) -> usize {
    if !block.exact {
        return block.src_start;
    }
    let row = row.clamp(block.content_start, block.content_end.saturating_sub(1));
    block.src_start + (row - block.content_start)
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

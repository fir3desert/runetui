//! Markdown parsing.
//!
//! `pulldown-cmark` emits a flat event stream; this module turns it into a
//! small owned AST that is independent of the rendered width.  That separation
//! lets the viewer re-flow a document on resize without re-reading or
//! re-parsing it.

use std::cell::Cell;
use std::rc::Rc;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    Code {
        lang: String,
        code: String,
    },
    List(ListBlock),
    Table(TableBlock),
    Quote {
        kind: QuoteKind,
        blocks: Vec<Block>,
    },
    Rule,
    Html(String),
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
    /// Container used for definition lists and other block containers.
    Group(Vec<Block>),
    /// Front matter is intentionally hidden: it is metadata, not prose.
    Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListBlock {
    pub ordered: bool,
    pub start: u64,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableBlock {
    pub alignments: Vec<TableAlign>,
    pub header: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteKind {
    Plain,
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl QuoteKind {
    pub fn label(self) -> Option<&'static str> {
        match self {
            QuoteKind::Plain => None,
            QuoteKind::Note => Some("NOTE"),
            QuoteKind::Tip => Some("TIP"),
            QuoteKind::Important => Some("IMPORTANT"),
            QuoteKind::Warning => Some("WARNING"),
            QuoteKind::Caution => Some("CAUTION"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    /// Raw inline HTML (kept visible, but dimmed by the renderer).
    Html(String),
    Math(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Superscript(Vec<Inline>),
    Subscript(Vec<Inline>),
    Link {
        dest: String,
        title: String,
        kind: LinkKind,
        children: Vec<Inline>,
    },
    Image {
        dest: String,
        alt: Vec<Inline>,
    },
    SoftBreak,
    HardBreak,
    FootnoteRef(String),
    Task(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Internal,
    Anchor,
    External,
    Wiki,
}

impl LinkKind {
    pub(crate) fn classify(dest: &str) -> Self {
        let lower = dest.trim().to_ascii_lowercase();
        if lower.starts_with('#') {
            LinkKind::Anchor
        } else if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
            || lower.starts_with("ftp://")
        {
            LinkKind::External
        } else {
            LinkKind::Internal
        }
    }
}

/// The Markdown dialect every part of runetui reads (the viewer, the link
/// index): CommonMark plus GFM, wikilinks, math, footnotes and front matter.
pub fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_MATH
            | Options::ENABLE_GFM
            | Options::ENABLE_WIKILINKS
            | Options::ENABLE_HEADING_ATTRIBUTES
            | Options::ENABLE_DEFINITION_LIST
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
            | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS,
    );
    options
}

/// A link found in a Markdown source, with the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLink {
    /// 0-based source line.
    pub line: usize,
    /// Destination as written (`notes/a.md#intro`, or the target of a
    /// `[[wikilink|alias]]`).
    pub dest: String,
    pub kind: LinkKind,
    /// The text the link shows (what the viewer draws for it).
    pub label: String,
}

/// Every link of a Markdown source (not images, and nothing inside code),
/// classified exactly as the viewer classifies them.
pub fn scan_links(text: &str) -> Vec<SourceLink> {
    let index = LineIndex::new(text);
    let mut links = Vec::new();
    let mut open: Option<SourceLink> = None;
    for (event, range) in Parser::new_ext(text, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let kind = if matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. }) {
                    LinkKind::Wiki
                } else {
                    LinkKind::classify(&dest_url)
                };
                open = Some(SourceLink {
                    line: index.line_of(range.start),
                    dest: dest_url.to_string(),
                    kind,
                    label: String::new(),
                });
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(link) = &mut open {
                    link.label.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(link) = &mut open {
                    link.label.push(' ');
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = open.take() {
                    links.push(link);
                }
            }
            _ => {}
        }
    }
    links
}

/// Source lines of one top-level block: 0-based, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

/// A parsed document: the blocks and, for each top-level block, the lines of
/// the source it came from.  `spans[i]` belongs to `blocks[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub blocks: Vec<Block>,
    pub spans: Vec<LineSpan>,
}

/// Start offset of every line of `text`, to turn byte offsets into lines.
struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        Self { text, starts }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset).saturating_sub(1)
    }

    /// Lines covered by the byte range `start..end` (end exclusive).  Blank
    /// lines at the end of the range (a list owns the gap after it) do not count.
    fn span(&self, start: usize, end: usize) -> LineSpan {
        let bytes = self.text.as_bytes();
        let mut end = end.min(bytes.len());
        while end > start && matches!(bytes[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
            end -= 1;
        }
        let first = self.line_of(start);
        let last = self.line_of(end.saturating_sub(1).max(start)).max(first);
        LineSpan { start: first, end: last }
    }
}

/// Parse a Markdown document into blocks.  Never panics on malformed input:
/// `pulldown-cmark` is robust and anything unknown is skipped or kept as text.
#[cfg(test)]
pub fn parse(text: &str) -> Vec<Block> {
    parse_spanned(text).blocks
}

/// Like [`parse`], but also reports where in the source every top-level block
/// came from (used to map rendered rows back to source lines).
pub fn parse_spanned(text: &str) -> Parsed {
    let options = markdown_options();

    // The offset iterator reports where every event came from; the cell keeps
    // the range of the event handed out last so the builder can read it.
    let last = Rc::new(Cell::new((0usize, 0usize)));
    let tracker = Rc::clone(&last);
    let events = Parser::new_ext(text, options)
        .into_offset_iter()
        .map(move |(event, range)| {
            tracker.set((range.start, range.end));
            event
        });
    let mut builder = Builder {
        events: events.peekable(),
        last,
        ranges: Vec::new(),
    };
    let blocks = builder.blocks(None);
    let index = LineIndex::new(text);
    let spans = builder
        .ranges
        .iter()
        .map(|(start, end)| index.span(*start, *end))
        .collect();
    Parsed { blocks, spans }
}

struct Builder<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    events: std::iter::Peekable<I>,
    /// Byte range of the event returned by the last call to `next`.
    last: Rc<Cell<(usize, usize)>>,
    /// Byte range of every top-level block, in order.
    ranges: Vec<(usize, usize)>,
}

impl<'a, I> Builder<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    fn blocks(&mut self, until: Option<TagEnd>) -> Vec<Block> {
        let mut blocks = Vec::new();
        loop {
            let Some(event) = self.events.next() else {
                break;
            };
            // For a block's `Start` event this is the whole block.
            let range = self.last.get();
            let top_level = until.is_none();
            match event {
                Event::Start(tag) => {
                    if let Some(block) = self.block_from_tag(tag) {
                        blocks.push(block);
                        if top_level {
                            self.ranges.push(range);
                        }
                    }
                }
                Event::End(end) => {
                    if let Some(until) = until {
                        if at_end(until, end) {
                            break;
                        }
                    }
                }
                Event::Rule => {
                    blocks.push(Block::Rule);
                    if top_level {
                        self.ranges.push(range);
                    }
                }
                Event::Text(text) => {
                    // Text outside a block-level container (malformed input).
                    if !text.trim().is_empty() {
                        blocks.push(Block::Paragraph(vec![Inline::Text(text.to_string())]));
                        if top_level {
                            self.ranges.push(range);
                        }
                    }
                }
                Event::Html(html) => {
                    if !html.trim().is_empty() {
                        blocks.push(Block::Html(html.to_string()));
                        if top_level {
                            self.ranges.push(range);
                        }
                    }
                }
                _ => {}
            }
        }
        blocks
    }

    fn block_from_tag(&mut self, tag: Tag<'a>) -> Option<Block> {
        match tag {
            Tag::Paragraph => Some(Block::Paragraph(self.inlines(TagEnd::Paragraph))),
            Tag::Heading { level, .. } => Some(Block::Heading {
                level: heading_level(level),
                inlines: self.inlines(TagEnd::Heading(level)),
            }),
            Tag::CodeBlock(kind) => Some(self.code_block(kind)),
            Tag::BlockQuote(kind) => Some(Block::Quote {
                kind: quote_kind(kind),
                blocks: self.blocks(Some(TagEnd::BlockQuote(kind))),
            }),
            Tag::List(start) => Some(self.list(start)),
            Tag::Table(alignments) => Some(self.table(alignments)),
            Tag::HtmlBlock => Some(Block::Html(self.raw_until(TagEnd::HtmlBlock))),
            Tag::FootnoteDefinition(label) => Some(Block::Footnote {
                label: label.to_string(),
                blocks: self.blocks(Some(TagEnd::FootnoteDefinition)),
            }),
            Tag::DefinitionList => Some(Block::Group(self.blocks(Some(TagEnd::DefinitionList)))),
            Tag::DefinitionListTitle => Some(Block::Paragraph(vec![Inline::Strong(
                self.inlines(TagEnd::DefinitionListTitle),
            )])),
            Tag::DefinitionListDefinition => Some(Block::Group(
                self.blocks(Some(TagEnd::DefinitionListDefinition)),
            )),
            Tag::MetadataBlock(kind) => {
                let _ = self.raw_until(TagEnd::MetadataBlock(kind));
                Some(Block::Metadata)
            }
            // Containers that should not appear at block level.  Consume their
            // contents so the event stream stays balanced, and flatten them.
            other => {
                let end = other.to_end();
                if matches!(
                    end,
                    TagEnd::Emphasis
                        | TagEnd::Strong
                        | TagEnd::Strikethrough
                        | TagEnd::Superscript
                        | TagEnd::Subscript
                        | TagEnd::Link
                        | TagEnd::Image
                ) {
                    let inlines = self.inlines(end);
                    Some(Block::Paragraph(inlines))
                } else {
                    let blocks = self.blocks(Some(end));
                    if blocks.is_empty() {
                        None
                    } else {
                        Some(Block::Group(blocks))
                    }
                }
            }
        }
    }

    fn code_block(&mut self, kind: CodeBlockKind<'a>) -> Block {
        let lang = match kind {
            CodeBlockKind::Indented => String::new(),
            CodeBlockKind::Fenced(info) => info
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string(),
        };
        let mut code = String::new();
        loop {
            match self.events.next() {
                Some(Event::Text(text)) | Some(Event::Html(text)) => code.push_str(&text),
                Some(Event::End(TagEnd::CodeBlock)) | None => break,
                Some(Event::Start(_)) => {}
                Some(_) => {}
            }
        }
        Block::Code { lang, code }
    }

    fn list(&mut self, start: Option<u64>) -> Block {
        let ordered = start.is_some();
        let start = start.unwrap_or(1);
        let mut items = Vec::new();

        loop {
            match self.events.next() {
                Some(Event::Start(Tag::Item)) => {
                    // pulldown-cmark emits the task marker before the item's
                    // first paragraph in most cases.
                    let peeked = match self.events.peek() {
                        Some(Event::TaskListMarker(checked)) => Some(*checked),
                        _ => None,
                    };
                    if peeked.is_some() {
                        self.events.next();
                    }
                    let mut blocks = self.blocks(Some(TagEnd::Item));
                    let checked = peeked.or_else(|| extract_task_marker(&mut blocks));
                    items.push(ListItem { checked, blocks });
                }
                Some(Event::End(TagEnd::List(_))) | None => break,
                Some(Event::Start(tag)) => {
                    // Be forgiving about unexpected containers inside a list.
                    if let Some(block) = self.block_from_tag(tag) {
                        if let Some(item) = items.last_mut() {
                            item.blocks.push(block);
                        }
                    }
                }
                _ => {}
            }
        }

        Block::List(ListBlock {
            ordered,
            start,
            items,
        })
    }

    fn table(&mut self, alignments: Vec<Alignment>) -> Block {
        let alignments = alignments
            .into_iter()
            .map(|alignment| match alignment {
                Alignment::None => TableAlign::None,
                Alignment::Left => TableAlign::Left,
                Alignment::Center => TableAlign::Center,
                Alignment::Right => TableAlign::Right,
            })
            .collect();

        let mut header = Vec::new();
        let mut rows = Vec::new();

        loop {
            match self.events.next() {
                Some(Event::Start(Tag::TableHead)) => loop {
                    match self.events.next() {
                        Some(Event::Start(Tag::TableCell)) => {
                            header.push(self.inlines(TagEnd::TableCell));
                        }
                        Some(Event::End(TagEnd::TableHead)) | None => break,
                        _ => {}
                    }
                },
                Some(Event::Start(Tag::TableRow)) => {
                    let mut row = Vec::new();
                    let mut ended_table = false;
                    loop {
                        match self.events.next() {
                            Some(Event::Start(Tag::TableCell)) => {
                                row.push(self.inlines(TagEnd::TableCell));
                            }
                            Some(Event::End(TagEnd::TableRow)) => break,
                            Some(Event::End(TagEnd::Table)) => {
                                ended_table = true;
                                break;
                            }
                            None => {
                                ended_table = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if !row.is_empty() {
                        rows.push(row);
                    }
                    if ended_table {
                        break;
                    }
                }
                Some(Event::Start(Tag::TableCell)) => {
                    // Header cells without an explicit TableHead (defensive).
                    header.push(self.inlines(TagEnd::TableCell));
                }
                Some(Event::End(TagEnd::Table)) | None => break,
                _ => {}
            }
        }

        Block::Table(TableBlock {
            alignments,
            header,
            rows,
        })
    }

    fn inlines(&mut self, until: TagEnd) -> Vec<Inline> {
        let mut out = Vec::new();
        loop {
            let Some(event) = self.events.next() else {
                break;
            };
            match event {
                Event::Text(text) => push_text(&mut out, &text),
                Event::Code(code) => out.push(Inline::Code(code.to_string())),
                Event::InlineMath(math) => out.push(Inline::Math(format!("${math}$"))),
                Event::DisplayMath(math) => out.push(Inline::Math(math.to_string())),
                Event::Html(html) => push_text(&mut out, &html),
                Event::InlineHtml(html) => out.push(Inline::Html(html.to_string())),
                Event::FootnoteReference(label) => {
                    out.push(Inline::FootnoteRef(label.to_string()));
                }
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::TaskListMarker(checked) => out.push(Inline::Task(checked)),
                Event::Rule => out.push(Inline::Text("─".to_string())),
                Event::Start(tag) => match tag {
                    Tag::Emphasis => {
                        let children = self.inlines(TagEnd::Emphasis);
                        out.push(Inline::Emph(children));
                    }
                    Tag::Strong => {
                        let children = self.inlines(TagEnd::Strong);
                        out.push(Inline::Strong(children));
                    }
                    Tag::Strikethrough => {
                        let children = self.inlines(TagEnd::Strikethrough);
                        out.push(Inline::Strike(children));
                    }
                    Tag::Superscript => {
                        let children = self.inlines(TagEnd::Superscript);
                        out.push(Inline::Superscript(children));
                    }
                    Tag::Subscript => {
                        let children = self.inlines(TagEnd::Subscript);
                        out.push(Inline::Subscript(children));
                    }
                    Tag::Link {
                        link_type,
                        dest_url,
                        title,
                        ..
                    } => {
                        let children = self.inlines(TagEnd::Link);
                        out.push(Inline::Link {
                            kind: if matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. })
                            {
                                LinkKind::Wiki
                            } else {
                                LinkKind::classify(&dest_url)
                            },
                            dest: dest_url.to_string(),
                            title: title.to_string(),
                            children,
                        });
                    }
                    Tag::Image { dest_url, .. } => {
                        let children = self.inlines(TagEnd::Image);
                        out.push(Inline::Image {
                            dest: dest_url.to_string(),
                            alt: children,
                        });
                    }
                    other => {
                        // Unknown inline container: flatten its contents.
                        let end = other.to_end();
                        let children = self.inlines(end);
                        out.extend(children);
                    }
                },
                Event::End(end) => {
                    if at_end(until, end) {
                        break;
                    }
                }
            }
        }
        trim_inline_edges(&mut out);
        out
    }

    fn raw_until(&mut self, until: TagEnd) -> String {
        let mut raw = String::new();
        loop {
            match self.events.next() {
                Some(Event::Text(text)) | Some(Event::Html(text)) => raw.push_str(&text),
                Some(Event::End(end)) if at_end(until, end) => break,
                Some(Event::End(_)) => {}
                None => break,
                Some(Event::Start(_)) => {}
                Some(_) => {}
            }
        }
        raw.trim_end_matches('\n').to_string()
    }
}

fn at_end(until: TagEnd, end: TagEnd) -> bool {
    match (until, end) {
        (TagEnd::Heading(_), TagEnd::Heading(_)) => true,
        (TagEnd::BlockQuote(_), TagEnd::BlockQuote(_)) => true,
        (TagEnd::List(_), TagEnd::List(_)) => true,
        (TagEnd::MetadataBlock(_), TagEnd::MetadataBlock(_)) => true,
        (a, b) => a == b,
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    (level as u8).clamp(1, 6)
}

fn quote_kind(kind: Option<BlockQuoteKind>) -> QuoteKind {
    match kind {
        None => QuoteKind::Plain,
        Some(BlockQuoteKind::Note) => QuoteKind::Note,
        Some(BlockQuoteKind::Tip) => QuoteKind::Tip,
        Some(BlockQuoteKind::Important) => QuoteKind::Important,
        Some(BlockQuoteKind::Warning) => QuoteKind::Warning,
        Some(BlockQuoteKind::Caution) => QuoteKind::Caution,
    }
}

fn push_text(out: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(Inline::Text(existing)) => existing.push_str(text),
        _ => out.push(Inline::Text(text.to_string())),
    }
}

fn extract_task_marker(blocks: &mut Vec<Block>) -> Option<bool> {
    let Some(Block::Paragraph(inlines)) = blocks.first_mut() else {
        return None;
    };
    let position = inlines
        .iter()
        .position(|inline| matches!(inline, Inline::Task(_)))?;
    let Inline::Task(checked) = inlines.remove(position) else {
        return None;
    };
    trim_inline_edges(inlines);
    if inlines.is_empty() {
        blocks.remove(0);
    }
    Some(checked)
}

fn trim_inline_edges(inlines: &mut Vec<Inline>) {
    while let Some(Inline::Text(text)) = inlines.first_mut() {
        let trimmed = text.trim_start().to_string();
        if trimmed.is_empty() {
            inlines.remove(0);
        } else {
            *text = trimmed;
            break;
        }
    }
    while let Some(Inline::Text(text)) = inlines.last_mut() {
        let trimmed = text.trim_end().to_string();
        if trimmed.is_empty() {
            inlines.pop();
        } else {
            *text = trimmed;
            break;
        }
    }
}

/// Flatten inline content to plain text.  Used for TOC entries, link labels,
/// table measurement and search.
pub fn inline_plain(inlines: &[Inline]) -> String {
    let mut text = String::new();
    fn walk(inlines: &[Inline], out: &mut String) {
        for inline in inlines {
            match inline {
                Inline::Text(value) | Inline::Code(value) | Inline::Math(value) => {
                    out.push_str(value)
                }
                Inline::Html(_) => {}
                Inline::Emph(children)
                | Inline::Strong(children)
                | Inline::Strike(children)
                | Inline::Superscript(children)
                | Inline::Subscript(children)
                | Inline::Link { children, .. } => walk(children, out),
                Inline::Image { alt, .. } => walk(alt, out),
                Inline::SoftBreak | Inline::HardBreak => out.push(' '),
                Inline::FootnoteRef(label) => {
                    out.push_str("[^");
                    out.push_str(label);
                    out.push(']');
                }
                Inline::Task(checked) => out.push_str(if *checked { "[x] " } else { "[ ] " }),
            }
        }
    }
    walk(inlines, &mut text);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_and_paragraphs() {
        let blocks = parse("# Title\n\nHello **world** and `code`.\n");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::Heading { level, inlines } => {
                assert_eq!(*level, 1);
                assert_eq!(inline_plain(inlines), "Title");
            }
            other => panic!("expected heading, got {other:?}"),
        }
        match &blocks[1] {
            Block::Paragraph(inlines) => assert_eq!(inline_plain(inlines), "Hello world and code."),
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_lists_with_tasks() {
        let blocks = parse("- [x] done\n- [ ] todo\n  - nested\n");
        let Block::List(list) = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[0].checked, Some(true));
        assert_eq!(list.items[1].checked, Some(false));
        assert!(
            list.items[1]
                .blocks
                .iter()
                .any(|b| matches!(b, Block::List(_)))
        );
    }

    #[test]
    fn parses_tables() {
        let blocks = parse("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        let Block::Table(table) = &blocks[0] else {
            panic!("expected table");
        };
        assert_eq!(table.header.len(), 2);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.alignments[0], TableAlign::Left);
        assert_eq!(table.alignments[1], TableAlign::Right);
    }

    #[test]
    fn parses_code_and_language() {
        let blocks = parse("```bash\necho hi\n```\n");
        let Block::Code { lang, code } = &blocks[0] else {
            panic!("expected code block");
        };
        assert_eq!(lang, "bash");
        assert_eq!(code, "echo hi\n");
    }

    #[test]
    fn parses_wikilinks() {
        let blocks = parse("See [[Other Page|the other page]] now.\n");
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link {
                kind: LinkKind::Wiki,
                ..
            }
        )));
        assert_eq!(inline_plain(inlines), "See the other page now.");
    }

    #[test]
    fn hides_front_matter() {
        let blocks = parse("---\ntitle: Test\n---\n\n# Body\n");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], Block::Metadata));
    }

    #[test]
    fn parses_blockquotes_and_rules() {
        let blocks = parse("> [!NOTE]\n> quoted\n\n---\n");
        let Block::Quote { kind, .. } = &blocks[0] else {
            panic!("expected quote");
        };
        assert_eq!(*kind, QuoteKind::Note);
        assert!(matches!(blocks[1], Block::Rule));
    }

    // ------------------------------------------------------------ scan_links

    #[test]
    fn scan_links_reports_line_destination_kind_and_label() {
        let source = "# T\n\nA [plain](a.md#x) and [web](https://e.com).\n\n- [[wiki|shown]] and [[bare]]\n\n[jump](#top)\n";
        let links = scan_links(source);
        let summary: Vec<_> = links
            .iter()
            .map(|l| (l.line, l.dest.as_str(), l.kind, l.label.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (2, "a.md#x", LinkKind::Internal, "plain"),
                (2, "https://e.com", LinkKind::External, "web"),
                (4, "wiki", LinkKind::Wiki, "shown"),
                (4, "bare", LinkKind::Wiki, "bare"),
                (6, "#top", LinkKind::Anchor, "jump"),
            ]
        );
    }

    #[test]
    fn scan_links_skips_images_and_code_and_handles_reference_links() {
        let source = "![img](a.png)\n\n`[c](x.md)`\n\n```\n[d](y.md)\n```\n\n[ref][r] and `code` in [**bold** `mono`](z.md)\n\n[r]: target.md\n";
        let links = scan_links(source);
        let summary: Vec<_> = links.iter().map(|l| (l.line, l.dest.as_str(), l.label.as_str())).collect();
        assert_eq!(summary, vec![(8, "target.md", "ref"), (8, "z.md", "bold mono")]);
        assert!(scan_links("").is_empty());
        assert!(scan_links("no links here\n").is_empty());
    }

    #[test]
    fn a_link_that_wraps_over_lines_starts_on_its_first_line() {
        let links = scan_links("text [one\ntwo](a.md) end\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].line, 0);
        assert_eq!(links[0].label, "one two");
    }
}

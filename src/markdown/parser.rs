//! Markdown parsing.
//!
//! `pulldown-cmark` emits a flat event stream; this module turns it into a
//! small owned AST that is independent of the rendered width.  That separation
//! lets the viewer re-flow a document on resize without re-reading or
//! re-parsing it.

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

/// Parse a Markdown document into blocks.  Never panics on malformed input:
/// `pulldown-cmark` is robust and anything unknown is skipped or kept as text.
pub fn parse(text: &str) -> Vec<Block> {
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

    let mut builder = Builder {
        events: Parser::new_ext(text, options).peekable(),
    };
    builder.blocks(None)
}

struct Builder<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    events: std::iter::Peekable<I>,
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
            match event {
                Event::Start(tag) => {
                    if let Some(block) = self.block_from_tag(tag) {
                        blocks.push(block);
                    }
                }
                Event::End(end) => {
                    if let Some(until) = until {
                        if at_end(until, end) {
                            break;
                        }
                    }
                }
                Event::Rule => blocks.push(Block::Rule),
                Event::Text(text) => {
                    // Text outside a block-level container (malformed input).
                    if !text.trim().is_empty() {
                        blocks.push(Block::Paragraph(vec![Inline::Text(text.to_string())]));
                    }
                }
                Event::Html(html) => {
                    if !html.trim().is_empty() {
                        blocks.push(Block::Html(html.to_string()));
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
}

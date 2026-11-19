//! Width-aware Markdown layout.
//!
//! This renderer turns the parsed AST into styled lines for Ratatui.  It is the
//! only place that knows about indentation, wrapping, table column sizing and
//! code block frames.  Everything is computed in memory for a given width and
//! theme; nothing touches the filesystem.

use ratatui::style::{Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::code_highlighter::CodeHighlighter;
use super::parser::{Block, Inline, LinkKind, ListBlock, TableAlign, TableBlock, inline_plain};
use super::parser::LineSpan;
use super::{BlockMap, CodeRegion, DocLink, Heading, RLine, RSpan, RenderedDoc, slugify};
use crate::ui::themes::Theme;

/// Render a Markdown document.
#[cfg(test)]
pub fn render(
    blocks: &[Block],
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    render_with_source(blocks, &[], "", width, theme, highlighter)
}

/// Render a Markdown document and remember which source lines each top-level
/// block came from (`spans[i]` belongs to `blocks[i]`; `source` is the text
/// they were parsed from).  Without spans the result has no source map.
pub fn render_with_source(
    blocks: &[Block],
    spans: &[LineSpan],
    source: &str,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    let mut renderer = Renderer {
        theme,
        width: width.max(16),
        highlighter,
        out: RenderedDoc::default(),
    };
    let ctx = Ctx::new(theme);
    let source_lines: Vec<&str> = if spans.is_empty() { Vec::new() } else { source.lines().collect() };
    for (index, block) in blocks.iter().enumerate() {
        let start = renderer.out.lines.len();
        match block {
            Block::Metadata => {}
            Block::Group(children) => renderer.blocks(children, ctx),
            other => renderer.block(other, ctx, None),
        }
        let end = renderer.out.lines.len();
        if let (Some(span), true) = (spans.get(index), end > start) {
            if let Some(map) = block_map(&renderer.out.lines, start, end, *span, &source_lines) {
                renderer.out.blocks.push(map);
            }
        }
    }
    renderer.finish()
}

/// Build the source map entry of the block rendered into rows `start..end`.
fn block_map(
    lines: &[RLine],
    start: usize,
    end: usize,
    span: LineSpan,
    source: &[&str],
) -> Option<BlockMap> {
    let content_start = (start..end).find(|row| !lines[*row].is_blank())?;
    let mut content_end = (start..end).rfind(|row| !lines[*row].is_blank())? + 1;
    let wanted = span.end.saturating_sub(span.start) + 1;
    // One row per source line, and the same letters and digits on each: the
    // block was not wrapped, folded or decorated into something else.
    let same = |rows_end: usize| {
        rows_end - content_start == wanted
            && (0..wanted).all(|offset| {
                source.get(span.start + offset).is_some_and(|line| {
                    lines[content_start + offset]
                        .text()
                        .chars()
                        .filter(|character| character.is_alphanumeric())
                        .eq(line.chars().filter(|character| character.is_alphanumeric()))
                })
            })
    };
    let mut exact = same(content_end);
    if !exact {
        // A heading is followed by a rule: decoration at the end is not content.
        let texty = (content_start..content_end)
            .rfind(|row| lines[*row].text().chars().any(char::is_alphanumeric))
            .map(|row| row + 1);
        if let Some(trimmed) = texty.filter(|trimmed| *trimmed < content_end) {
            if same(trimmed) {
                content_end = trimmed;
                exact = true;
            }
        }
    }
    Some(BlockMap {
        start,
        end,
        content_start,
        content_end,
        src_start: span.start,
        src_end: span.end,
        exact,
    })
}

#[derive(Debug, Clone, Copy)]
struct Ctx {
    indent: usize,
    quote_depth: usize,
    base: Style,
    compact: bool,
}

impl Ctx {
    fn new(theme: Theme) -> Self {
        Self {
            indent: 0,
            quote_depth: 0,
            base: Style::default().fg(theme.fg),
            compact: false,
        }
    }

    fn content_width(&self, total: usize) -> usize {
        total
            .saturating_sub(self.indent)
            .saturating_sub(self.quote_depth * 2)
            .max(4)
    }

    fn nested(&self, indent_delta: usize, compact: bool) -> Self {
        Self {
            indent: self.indent + indent_delta,
            compact,
            ..*self
        }
    }
}

struct Renderer<'a> {
    theme: Theme,
    width: usize,
    highlighter: &'a mut CodeHighlighter,
    out: RenderedDoc,
}

impl Renderer<'_> {
    fn finish(mut self) -> RenderedDoc {
        while self.out.lines.last().is_some_and(RLine::is_blank) {
            self.out.lines.pop();
        }
        self.out.links.retain(|link| link.line != usize::MAX);
        // Trailing blank rows were dropped: keep the map inside the document.
        let total = self.out.lines.len();
        self.out.blocks.retain_mut(|block| {
            block.end = block.end.min(total);
            block.content_end = block.content_end.min(total);
            block.start < total && block.content_start < block.content_end
        });
        self.out
    }

    fn blocks(&mut self, blocks: &[Block], ctx: Ctx) {
        for block in blocks {
            match block {
                Block::Metadata => {}
                Block::Group(children) => self.blocks(children, ctx),
                other => self.block(other, ctx, None),
            }
        }
    }

    fn block(&mut self, block: &Block, ctx: Ctx, marker: Option<Vec<RSpan>>) {
        match block {
            Block::Heading { level, inlines } => self.heading(*level, inlines, ctx),
            Block::Paragraph(inlines) => self.paragraph(inlines, ctx, marker),
            Block::Code { lang, code } => self.code_block(lang, code, ctx),
            Block::List(list) => self.list(list, ctx),
            Block::Table(table) => self.table(table, ctx),
            Block::Quote { kind, blocks } => self.quote(*kind, blocks, ctx),
            Block::Rule => self.rule(ctx),
            Block::Html(html) => self.html(html, ctx),
            Block::Footnote { label, blocks } => self.footnote(label, blocks, ctx),
            Block::Group(children) => self.blocks(children, ctx),
            Block::Metadata => {}
        }
    }

    // ------------------------------------------------------------------ text

    fn paragraph(&mut self, inlines: &[Inline], ctx: Ctx, marker: Option<Vec<RSpan>>) {
        let marker = marker.unwrap_or_default();
        let marker_width = spans_width(&marker);
        let content_ctx = Ctx {
            indent: ctx.indent + marker_width,
            ..ctx
        };
        let width = content_ctx.content_width(self.width);
        let logical_lines = self.inline_spans(inlines, ctx.base);

        let mut first_visual = true;
        for logical in logical_lines {
            for wrapped in wrap_spans(&logical, width) {
                let mut spans = Vec::new();
                if first_visual {
                    spans.extend(marker.iter().cloned());
                } else {
                    spans.push(spaces(marker_width, ctx.base));
                }
                spans.extend(wrapped);
                self.emit(spans, ctx);
                first_visual = false;
            }
        }

        if !ctx.compact {
            self.ensure_blank();
        }
    }

    fn heading(&mut self, level: u8, inlines: &[Inline], ctx: Ctx) {
        self.ensure_blank();
        let style = self.theme.heading(level);
        let width = ctx.content_width(self.width);
        let text = inline_plain(inlines);
        let slug = slugify(&text);
        let logical_lines = self.inline_spans(inlines, style);
        let heading_line = self.out.lines.len();

        for logical in logical_lines {
            for wrapped in wrap_spans(&logical, width) {
                self.emit(wrapped, ctx);
            }
        }

        if self.out.lines.len() == heading_line {
            self.emit(vec![RSpan::styled(text.clone(), style)], ctx);
        }

        self.out.headings.push(Heading {
            level,
            text,
            slug,
            line: heading_line,
        });

        if level == 1 {
            let rule_width = ctx.content_width(self.width).max(1);
            self.emit(
                vec![RSpan::styled(
                    "─".repeat(rule_width),
                    Style::default().fg(self.theme.border),
                )],
                ctx,
            );
        }
        if level <= 2 {
            self.ensure_blank();
        }
    }

    fn inline_spans(&mut self, inlines: &[Inline], base: Style) -> Vec<Vec<RSpan>> {
        let mut lines = vec![Vec::new()];
        self.walk_inlines(inlines, base, None, &mut lines);
        for line in &mut lines {
            trim_spans_end(line);
        }
        while lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
        }
        lines
    }

    fn walk_inlines(
        &mut self,
        inlines: &[Inline],
        style: Style,
        link: Option<usize>,
        lines: &mut Vec<Vec<RSpan>>,
    ) {
        for inline in inlines {
            match inline {
                Inline::Text(text) => {
                    let text = text.replace('\t', "    ");
                    if !text.is_empty() {
                        push_span(lines, RSpan::new(text, style).with_link(link));
                    } else if text.is_empty() {
                        // Empty text nodes are ignored.
                    }
                }
                Inline::Code(code) => {
                    let mut code_style = self.theme.inline_code_style();
                    if style.add_modifier.contains(Modifier::BOLD) {
                        code_style = code_style.add_modifier(Modifier::BOLD);
                    }
                    if link.is_some() {
                        code_style = code_style.add_modifier(Modifier::UNDERLINED);
                    }
                    let text = format!(" {} ", code.replace('\t', "    "));
                    push_span(lines, RSpan::new(text, code_style).with_link(link));
                }
                Inline::Math(math) => {
                    let mut math_style = Style::default()
                        .fg(self.theme.cyan)
                        .bg(self.theme.bg_sel)
                        .add_modifier(Modifier::ITALIC);
                    if link.is_some() {
                        math_style = math_style.add_modifier(Modifier::UNDERLINED);
                    }
                    push_span(
                        lines,
                        RSpan::new(format!(" {math} "), math_style).with_link(link),
                    );
                }
                // Inline HTML is hidden; its text content is emitted as normal
                // text nodes by the parser.
                Inline::Html(_) => {}
                Inline::Emph(children) => {
                    self.walk_inlines(children, style.add_modifier(Modifier::ITALIC), link, lines);
                }
                Inline::Strong(children) => {
                    self.walk_inlines(children, style.add_modifier(Modifier::BOLD), link, lines);
                }
                Inline::Strike(children) => {
                    self.walk_inlines(
                        children,
                        style.add_modifier(Modifier::CROSSED_OUT),
                        link,
                        lines,
                    );
                }
                Inline::Superscript(children) | Inline::Subscript(children) => {
                    // Terminal cells cannot raise/lower glyphs; keep the text
                    // visible and slightly de-emphasised instead.
                    self.walk_inlines(children, style.add_modifier(Modifier::ITALIC), link, lines);
                }
                Inline::Link {
                    dest,
                    kind,
                    children,
                    ..
                } => {
                    let label = inline_plain(children);
                    let id = self.out.links.len();
                    self.out.links.push(DocLink {
                        line: usize::MAX,
                        column: 0,
                        label: if label.is_empty() {
                            dest.clone()
                        } else {
                            label
                        },
                        dest: dest.clone(),
                        kind: *kind,
                        image: false,
                    });
                    let link_style = style
                        .fg(self.theme.accent2)
                        .add_modifier(Modifier::UNDERLINED);
                    self.walk_inlines(children, link_style, Some(id), lines);
                }
                Inline::Image { dest, alt } => {
                    let alt_text = inline_plain(alt);
                    let display = if alt_text.is_empty() {
                        dest.rsplit('/').next().unwrap_or(dest).to_string()
                    } else {
                        alt_text
                    };
                    let id = self.out.links.len();
                    self.out.links.push(DocLink {
                        line: usize::MAX,
                        column: 0,
                        label: display.clone(),
                        dest: dest.clone(),
                        kind: LinkKind::classify(dest),
                        image: true,
                    });
                    push_span(
                        lines,
                        RSpan::new(
                            format!("▨ {display}"),
                            style
                                .fg(self.theme.fg_muted)
                                .add_modifier(Modifier::ITALIC | Modifier::UNDERLINED),
                        )
                        .with_link(Some(id)),
                    );
                }
                Inline::SoftBreak => push_span(lines, RSpan::new(" ", style).with_link(link)),
                Inline::HardBreak => lines.push(Vec::new()),
                Inline::FootnoteRef(label) => push_span(
                    lines,
                    RSpan::new(
                        format!("[^{label}]"),
                        Style::default().fg(self.theme.accent2),
                    )
                    .with_link(link),
                ),
                Inline::Task(checked) => {
                    let (text, check_style) = if *checked {
                        (
                            "[x] ",
                            Style::default()
                                .fg(self.theme.green)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        ("[ ] ", Style::default().fg(self.theme.fg_muted))
                    };
                    push_span(lines, RSpan::new(text, check_style).with_link(link));
                }
            }
        }
    }

    // ------------------------------------------------------------------ lists

    fn list(&mut self, list: &ListBlock, ctx: Ctx) {
        self.ensure_blank();

        for (index, item) in list.items.iter().enumerate() {
            let (marker_text, marker_style) = if let Some(checked) = item.checked {
                if checked {
                    (
                        "[x] ".to_string(),
                        Style::default()
                            .fg(self.theme.green)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("[ ] ".to_string(), Style::default().fg(self.theme.fg_muted))
                }
            } else if list.ordered {
                (
                    format!("{}. ", list.start + index as u64),
                    Style::default().fg(self.theme.accent2),
                )
            } else {
                ("• ".to_string(), self.theme.list_marker_style())
            };
            let marker = vec![RSpan::styled(marker_text, marker_style)];
            let marker_width = spans_width(&marker);
            let item_ctx = ctx.nested(0, true);

            if item.blocks.is_empty() {
                self.emit(marker, item_ctx);
                continue;
            }

            let mut first = true;
            for block in &item.blocks {
                let is_paragraph = matches!(block, Block::Paragraph(_));
                if first {
                    if is_paragraph {
                        self.block(block, item_ctx, Some(marker.clone()));
                    } else {
                        // Marker on its own line, then the block (nested list,
                        // code block, quote…) indented below it.
                        self.emit(marker.clone(), item_ctx);
                        self.block(block, item_ctx.nested(marker_width, true), None);
                    }
                    first = false;
                } else if is_paragraph {
                    self.block(block, item_ctx.nested(marker_width, true), None);
                } else {
                    self.block(block, item_ctx.nested(marker_width, true), None);
                }
            }
        }

        if !ctx.compact {
            self.ensure_blank();
        }
    }

    // ------------------------------------------------------------------ code

    fn code_block(&mut self, lang: &str, code: &str, ctx: Ctx) {
        self.ensure_blank();
        let region_start = self.out.lines.len();
        let highlighted = self.highlighter.highlight(code, lang, &self.theme);
        let width = ctx.content_width(self.width);

        if width < 14 {
            for line in &highlighted.lines {
                for wrapped in wrap_hard(line, width) {
                    self.emit(wrapped, ctx);
                }
            }
        } else {
            self.code_frame(
                &highlighted.label,
                &highlighted.lines,
                highlighted.detected,
                ctx,
                width,
            );
        }

        let region_end = self.out.lines.len();
        if region_end > region_start {
            self.out.code_blocks.push(CodeRegion {
                start: region_start,
                end: region_end,
                code: code.trim_end_matches('\n').to_string(),
            });
        }

        if !ctx.compact {
            self.ensure_blank();
        }
    }

    fn code_frame(
        &mut self,
        label: &str,
        lines: &[Vec<RSpan>],
        _detected: bool,
        ctx: Ctx,
        width: usize,
    ) {
        let border_style = Style::default()
            .fg(self.theme.border)
            .bg(self.theme.code_bg());
        let label_style = Style::default()
            .fg(self.theme.accent)
            .bg(self.theme.code_bg())
            .add_modifier(Modifier::BOLD);
        let inside = width.saturating_sub(4);

        let label_text = if label.is_empty() {
            String::new()
        } else {
            format!(" {label} ")
        };
        let label_width = UnicodeWidthStr::width(label_text.as_str());

        let mut top = Vec::new();
        if label_width + 3 < width {
            let fill = width.saturating_sub(2 + label_width);
            top.push(RSpan::styled(
                format!("╭{}", "─".repeat(fill)),
                border_style,
            ));
            top.push(RSpan::styled(label_text, label_style));
            top.push(RSpan::styled("╮", border_style));
        } else {
            top.push(RSpan::styled(
                format!("╭{}╮", "─".repeat(width.saturating_sub(2))),
                border_style,
            ));
        }
        self.emit(top, ctx);

        for line in lines {
            for wrapped in wrap_hard(line, inside) {
                let mut spans = Vec::with_capacity(wrapped.len() + 4);
                spans.push(RSpan::styled("│", border_style));
                spans.push(RSpan::styled(" ", self.theme.code_block_style()));
                let chunk_width = spans_width(&wrapped);
                for span in wrapped {
                    let mut span = span;
                    if span.style.bg.is_none() {
                        span.style.bg = Some(self.theme.code_bg());
                    }
                    spans.push(span);
                }
                let padding = inside.saturating_sub(chunk_width);
                spans.push(RSpan::styled(
                    " ".repeat(padding),
                    self.theme.code_block_style(),
                ));
                spans.push(RSpan::styled(" ", self.theme.code_block_style()));
                spans.push(RSpan::styled("│", border_style));
                self.emit(spans, ctx);
            }
        }

        self.emit(
            vec![RSpan::styled(
                format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
                border_style,
            )],
            ctx,
        );
    }

    // ------------------------------------------------------------------ table

    fn table(&mut self, table: &TableBlock, ctx: Ctx) {
        self.ensure_blank();
        let width = ctx.content_width(self.width);

        let header = table
            .header
            .iter()
            .map(|cell| self.cell_spans(cell))
            .collect::<Vec<_>>();
        let rows = table
            .rows
            .iter()
            .map(|row| row.iter().map(|cell| self.cell_spans(cell)).collect())
            .collect::<Vec<Vec<Vec<RSpan>>>>();

        let columns = table
            .alignments
            .len()
            .max(header.len())
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));

        if columns == 0 {
            return;
        }

        if width < columns * 5 + 1 {
            self.table_fallback(&header, &rows, columns, ctx);
            if !ctx.compact {
                self.ensure_blank();
            }
            return;
        }

        let alignments = (0..columns)
            .map(|index| {
                table
                    .alignments
                    .get(index)
                    .copied()
                    .unwrap_or(TableAlign::None)
            })
            .collect::<Vec<_>>();

        let mut widths = vec![1usize; columns];
        for (column, slot) in widths.iter_mut().enumerate() {
            let mut natural = header
                .get(column)
                .map(|cell| spans_width(cell))
                .unwrap_or(0);
            for row in &rows {
                if let Some(cell) = row.get(column) {
                    natural = natural.max(spans_width(cell));
                }
            }
            *slot = natural.max(1);
        }

        let borders = columns + 1;
        let available = width.saturating_sub(borders);

        // Shrink the widest columns until the table fits.
        let mut outer: usize = widths.iter().map(|value| value + 2).sum();
        while outer > available {
            let Some((index, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, value)| **value > 1)
                .max_by_key(|(_, value)| **value)
            else {
                break;
            };
            let reduce = (outer - available).min(widths[index] - 1);
            widths[index] -= reduce;
            outer -= reduce;
        }

        if outer > available {
            self.table_fallback(&header, &rows, columns, ctx);
            if !ctx.compact {
                self.ensure_blank();
            }
            return;
        }

        // Top border.
        let mut top = String::from("┌");
        for (index, column_width) in widths.iter().enumerate() {
            top.push_str(&"─".repeat(column_width + 2));
            top.push(if index + 1 == columns { '┐' } else { '┬' });
        }
        self.emit(vec![RSpan::styled(top, self.theme.border())], ctx);

        // Header rows.
        let header_lines = self.layout_table_row(&header, &widths, &alignments, true);
        for line in header_lines {
            self.emit(line, ctx);
        }

        let mut separator = String::from("├");
        for (index, column_width) in widths.iter().enumerate() {
            separator.push_str(&"─".repeat(column_width + 2));
            separator.push(if index + 1 == columns { '┤' } else { '┼' });
        }
        self.emit(vec![RSpan::styled(separator, self.theme.border())], ctx);

        // Body rows.
        for row in &rows {
            let mut laid_out = self.layout_table_row(row, &widths, &alignments, false);
            if laid_out.is_empty() {
                laid_out.push(vec![RSpan::styled("│", self.theme.border())]);
            }
            for line in laid_out {
                self.emit(line, ctx);
            }
        }

        let mut bottom = String::from("└");
        for (index, column_width) in widths.iter().enumerate() {
            bottom.push_str(&"─".repeat(column_width + 2));
            bottom.push(if index + 1 == columns { '┘' } else { '┴' });
        }
        self.emit(vec![RSpan::styled(bottom, self.theme.border())], ctx);

        if !ctx.compact {
            self.ensure_blank();
        }
    }

    fn cell_spans(&mut self, cell: &[Inline]) -> Vec<RSpan> {
        let logical = self.inline_spans(cell, self.theme.base());
        let mut out = Vec::new();
        for (index, line) in logical.into_iter().enumerate() {
            if index > 0 {
                out.push(RSpan::new(" ", self.theme.base()));
            }
            out.extend(line);
        }
        trim_spans_end(&mut out);
        out
    }

    fn layout_table_row(
        &mut self,
        cells: &[Vec<RSpan>],
        widths: &[usize],
        alignments: &[TableAlign],
        header: bool,
    ) -> Vec<Vec<RSpan>> {
        let columns = widths.len();
        let mut wrapped_cells: Vec<Vec<Vec<RSpan>>> = Vec::with_capacity(columns);
        for (index, column_width) in widths.iter().enumerate() {
            let content = cells.get(index).cloned().unwrap_or_default();
            let mut lines = wrap_spans(&content, *column_width);
            if lines.is_empty() {
                lines.push(Vec::new());
            }
            wrapped_cells.push(lines);
        }

        let height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1).max(1);

        let mut output = Vec::with_capacity(height);
        for visual_row in 0..height {
            let mut spans: Vec<RSpan> = Vec::new();
            spans.push(RSpan::styled("│", Style::default().fg(self.theme.border)));
            for column in 0..columns {
                let column_width = widths[column];
                let align = alignments.get(column).copied().unwrap_or(TableAlign::None);
                let mut cell = wrapped_cells[column]
                    .get(visual_row)
                    .cloned()
                    .unwrap_or_default();
                let cell_width = spans_width(&cell);

                let (left, right) = match align {
                    TableAlign::Right => (column_width.saturating_sub(cell_width), 0),
                    TableAlign::Center => {
                        let pad = column_width.saturating_sub(cell_width);
                        (pad / 2, pad - pad / 2)
                    }
                    TableAlign::Left | TableAlign::None => {
                        (0, column_width.saturating_sub(cell_width))
                    }
                };

                let padding_style = if header {
                    Style::default().bg(self.theme.bg_alt)
                } else {
                    Style::default()
                };

                spans.push(RSpan::styled(" ", padding_style));
                if left > 0 {
                    spans.push(RSpan::styled(" ".repeat(left), padding_style));
                }
                for span in &mut cell {
                    if header {
                        if span.style.bg.is_none() {
                            span.style.bg = Some(self.theme.bg_alt);
                        }
                        if span.style.fg.is_none() {
                            span.style.fg = Some(self.theme.accent);
                        }
                        span.style = span.style.add_modifier(Modifier::BOLD);
                    }
                }
                spans.extend(cell);
                if right > 0 {
                    spans.push(RSpan::styled(" ".repeat(right), padding_style));
                }
                spans.push(RSpan::styled(" ", padding_style));
                spans.push(RSpan::styled("│", Style::default().fg(self.theme.border)));
            }
            output.push(spans);
        }

        output
    }

    fn table_fallback(
        &mut self,
        header: &[Vec<RSpan>],
        rows: &[Vec<Vec<RSpan>>],
        columns: usize,
        ctx: Ctx,
    ) {
        let width = ctx.content_width(self.width).max(4);
        let border_style = Style::default().fg(self.theme.border);

        let mut joined = Vec::new();
        for column in 0..columns {
            if column > 0 {
                joined.push(RSpan::styled(" │ ", border_style));
            }
            joined.extend(header.get(column).cloned().unwrap_or_default());
        }
        for span in &mut joined {
            span.style = span.style.add_modifier(Modifier::BOLD);
            if span.style.fg.is_none() {
                span.style.fg = Some(self.theme.accent);
            }
        }
        for line in wrap_spans(&joined, width) {
            self.emit(line, ctx);
        }
        self.emit(vec![RSpan::styled("─".repeat(width), border_style)], ctx);

        for row in rows {
            let mut joined = Vec::new();
            for column in 0..columns {
                if column > 0 {
                    joined.push(RSpan::styled(" │ ", border_style));
                }
                joined.extend(row.get(column).cloned().unwrap_or_default());
            }
            for line in wrap_spans(&joined, width) {
                self.emit(line, ctx);
            }
        }
    }

    // ------------------------------------------------------- quotes & rules

    fn quote(&mut self, kind: super::parser::QuoteKind, blocks: &[Block], ctx: Ctx) {
        self.ensure_blank();
        let child = Ctx {
            quote_depth: ctx.quote_depth + 1,
            base: self.theme.quote_style(),
            compact: false,
            ..ctx
        };

        if let Some(label) = kind.label() {
            let color = match kind {
                super::parser::QuoteKind::Note => self.theme.accent2,
                super::parser::QuoteKind::Tip => self.theme.green,
                super::parser::QuoteKind::Important => self.theme.purple,
                super::parser::QuoteKind::Warning => self.theme.yellow,
                super::parser::QuoteKind::Caution => self.theme.red,
                super::parser::QuoteKind::Plain => self.theme.accent,
            };
            self.emit(
                vec![RSpan::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )],
                child,
            );
        }

        self.blocks(blocks, child);
        self.ensure_blank();
    }

    fn rule(&mut self, ctx: Ctx) {
        self.ensure_blank();
        let width = ctx.content_width(self.width).max(1);
        self.emit(
            vec![RSpan::styled(
                "─".repeat(width),
                Style::default().fg(self.theme.border),
            )],
            ctx,
        );
        if !ctx.compact {
            self.ensure_blank();
        }
    }

    fn html(&mut self, html: &str, ctx: Ctx) {
        self.ensure_blank();
        let width = ctx.content_width(self.width);
        let style = Style::default().fg(self.theme.fg_muted);
        for source_line in html.lines() {
            let spans = vec![RSpan::styled(source_line.replace('\t', "    "), style)];
            for wrapped in wrap_hard(&spans, width) {
                self.emit(wrapped, ctx);
            }
        }
        self.ensure_blank();
    }

    fn footnote(&mut self, label: &str, blocks: &[Block], ctx: Ctx) {
        self.ensure_blank();
        self.emit(
            vec![RSpan::styled(
                format!("[^{label}]"),
                Style::default()
                    .fg(self.theme.accent2)
                    .add_modifier(Modifier::BOLD),
            )],
            ctx,
        );
        let child = ctx.nested(2, true);
        self.blocks(blocks, child);
        self.ensure_blank();
    }

    // ------------------------------------------------------------- plumbing

    fn emit(&mut self, spans: Vec<RSpan>, ctx: Ctx) {
        let mut line = Vec::new();
        for _ in 0..ctx.quote_depth {
            line.push(RSpan::styled("▎ ", Style::default().fg(self.theme.border)));
        }
        if ctx.indent > 0 {
            let style = if ctx.quote_depth > 0 {
                Style::default().fg(self.theme.border)
            } else {
                Style::default()
            };
            line.push(RSpan::styled(" ".repeat(ctx.indent), style));
        }
        line.extend(spans);
        self.push_line(RLine::new(line));
    }

    fn push_line(&mut self, line: RLine) {
        let index = self.out.lines.len();
        let mut column = 0;
        for span in &line.spans {
            if let Some(id) = span.link {
                if let Some(link) = self.out.links.get_mut(id) {
                    if link.line == usize::MAX {
                        link.line = index;
                        link.column = column;
                    }
                }
            }
            column += span.width();
        }
        self.out.lines.push(line);
    }

    fn ensure_blank(&mut self) {
        if let Some(last) = self.out.lines.last() {
            if !last.is_blank() {
                self.out.lines.push(RLine::default());
            }
        }
    }
}

// ---------------------------------------------------------------- wrapping

/// Word-wrap styled spans to `width` display columns.  Whitespace at line
/// boundaries is collapsed, which is the correct behaviour for prose.
pub fn wrap_spans(spans: &[RSpan], width: usize) -> Vec<Vec<RSpan>> {
    let width = width.max(1);
    let cells = to_cells(spans);
    if cells.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines: Vec<Vec<Cell>> = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    let mut current_width = 0usize;
    let mut index = 0usize;

    while index < cells.len() {
        while index < cells.len() && cells[index].ch.is_whitespace() {
            index += 1;
        }
        if index >= cells.len() {
            break;
        }

        let word_start = index;
        while index < cells.len() && !cells[index].ch.is_whitespace() {
            index += 1;
        }
        let word = &cells[word_start..index];
        let word_width: usize = word.iter().map(|cell| cell.width).sum();

        if !current.is_empty() && current_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }

        if current.is_empty() && word_width > width {
            // Hard-break an over-long word (URLs, long identifiers…).
            let mut consumed = 0;
            while consumed < word.len() {
                let mut take = 0;
                let mut taken_width = 0;
                while consumed + take < word.len()
                    && taken_width + word[consumed + take].width <= width
                {
                    taken_width += word[consumed + take].width;
                    take += 1;
                }
                if take == 0 {
                    take = 1;
                }
                lines.push(word[consumed..consumed + take].to_vec());
                consumed += take;
            }
            current_width = 0;
            continue;
        }

        if !current.is_empty() {
            if let Some(space) = word.first() {
                current.push(Cell {
                    ch: ' ',
                    style: space.style,
                    link: space.link,
                    width: 1,
                });
                current_width += 1;
            }
        }
        current.extend_from_slice(word);
        current_width += word_width;
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines.into_iter().map(cells_to_spans).collect()
}

/// Hard-wrap styled spans at exactly `width` columns, preserving spaces.
/// Used for code, where indentation is significant.
pub fn wrap_hard(spans: &[RSpan], width: usize) -> Vec<Vec<RSpan>> {
    let width = width.max(1);
    let cells = to_cells(spans);
    if cells.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0usize;

    for cell in cells {
        if current_width + cell.width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current_width += cell.width;
        current.push(cell);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines.into_iter().map(cells_to_spans).collect()
}

pub fn spans_width(spans: &[RSpan]) -> usize {
    spans.iter().map(RSpan::width).sum()
}

fn spaces(count: usize, style: Style) -> RSpan {
    RSpan::styled(" ".repeat(count), style)
}

fn push_span(lines: &mut [Vec<RSpan>], span: RSpan) {
    if let Some(line) = lines.last_mut() {
        line.push(span);
    }
}

fn trim_spans_end(spans: &mut Vec<RSpan>) {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end().to_string();
        if trimmed.is_empty() {
            spans.pop();
        } else {
            last.text = trimmed;
            break;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
    link: Option<usize>,
    width: usize,
}

fn to_cells(spans: &[RSpan]) -> Vec<Cell> {
    let mut cells = Vec::new();
    for span in spans {
        for ch in span.text.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            cells.push(Cell {
                ch,
                style: span.style,
                link: span.link,
                width,
            });
        }
    }
    cells
}

fn cells_to_spans(cells: Vec<Cell>) -> Vec<RSpan> {
    let mut spans: Vec<RSpan> = Vec::new();
    for cell in cells {
        match spans.last_mut() {
            Some(span) if span.style == cell.style && span.link == cell.link => {
                span.text.push(cell.ch);
            }
            _ => spans.push(RSpan {
                text: cell.ch.to_string(),
                style: cell.style,
                link: cell.link,
            }),
        }
    }
    // Trim trailing spaces that came from source whitespace.
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end().to_string();
        if trimmed.is_empty() {
            spans.pop();
        } else {
            last.text = trimmed;
            break;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;
    use crate::ui::themes::catppuccin_mocha;

    fn render_str(source: &str, width: usize) -> RenderedDoc {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let blocks = parse(source);
        render(&blocks, width, theme, &mut highlighter)
    }

    fn texts(doc: &RenderedDoc) -> Vec<String> {
        doc.lines.iter().map(RLine::text).collect()
    }

    #[test]
    fn renders_headings_and_toc() {
        let doc = render_str("# Title\n\nSome text.\n\n## Section Two\n", 60);
        assert_eq!(doc.headings.len(), 2);
        assert_eq!(doc.headings[0].slug, "title");
        assert_eq!(doc.headings[1].text, "Section Two");
        assert!(doc.lines[doc.headings[0].line].text().contains("Title"));
        assert!(texts(&doc).iter().any(|line| line.starts_with('─')));
    }

    #[test]
    fn wraps_long_paragraphs_without_exceeding_width() {
        let source = "word ".repeat(80);
        let doc = render_str(&source, 30);
        for line in &doc.lines {
            assert!(line.width() <= 30, "line too wide: {:?}", line.text());
        }
        assert!(doc.lines.len() > 3);
    }

    #[test]
    fn renders_lists_and_tasks() {
        let doc = render_str("- [x] done\n- [ ] later\n  - nested\n", 50);
        let all = texts(&doc).join("\n");
        assert!(all.contains("[x] done"));
        assert!(all.contains("[ ] later"));
        assert!(all.contains("nested"));
    }

    #[test]
    fn renders_tables_with_borders_and_width_limit() {
        let doc = render_str(
            "| Name | Value |\n|------|------:|\n| alpha | 42 |\n| a longer cell | 7 |\n",
            42,
        );
        let all = texts(&doc).join("\n");
        assert!(all.contains('┌'));
        assert!(all.contains("alpha"));
        assert!(all.contains("a longer cell"));
        for line in &doc.lines {
            assert!(line.width() <= 42, "table line too wide: {}", line.width());
        }
    }

    #[test]
    fn renders_code_block_with_language_label() {
        let doc = render_str(
            "```bash\nfor i in {1..10}; do\n  echo \"Hello $i\"\ndone\n```\n",
            60,
        );
        let all = texts(&doc).join("\n");
        assert!(all.contains("bash"));
        assert!(all.contains('╭'));
        assert!(all.contains("echo"));
        // Syntax highlighting must produce more than one style.
        let styles = doc
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.style.fg))
            .collect::<std::collections::HashSet<_>>();
        assert!(styles.len() > 2);
    }

    #[test]
    fn renders_quotes_with_bar() {
        let doc = render_str("> quoted text\n> more\n", 40);
        assert!(texts(&doc).iter().any(|line| line.contains('▎')));
    }

    #[test]
    fn records_links_and_their_lines() {
        let doc = render_str(
            "See [docs](docs/readme.md) and [[Other Page]] and [#anchor](#intro).\n",
            80,
        );
        assert_eq!(doc.links.len(), 3);
        for link in &doc.links {
            assert!(link.line < doc.lines.len());
            assert!(doc.lines[link.line].text().contains(&link.label) || link.image);
        }
        assert!(doc.link_at(doc.links[0].line).is_some());
    }

    #[test]
    fn long_code_lines_are_hard_wrapped() {
        let doc = render_str(
            &format!("```rust\n{}\n```\n", "let value = 1; ".repeat(20)),
            40,
        );
        for line in &doc.lines {
            assert!(line.width() <= 40, "code line too wide: {}", line.width());
        }
    }

    #[test]
    fn unicode_text_does_not_break_width() {
        let doc = render_str("日本語のテキストを表示します。これは長い行です。\n", 20);
        for line in &doc.lines {
            assert!(line.width() <= 20, "line too wide: {}", line.width());
        }
    }

    #[test]
    fn wrap_hard_preserves_indentation() {
        let spans = vec![RSpan::plain("    indented code that is long")];
        let lines = wrap_hard(&spans, 12);
        assert!(lines[0][0].text.starts_with("    "));
        assert!(lines.iter().all(|line| spans_width(line) <= 12));
    }

    // ------------------------------------------------------------ source map

    fn render_mapped(source: &str, width: usize) -> RenderedDoc {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let parsed = crate::markdown::parse_spanned(source);
        render_with_source(&parsed.blocks, &parsed.spans, source, width, theme, &mut highlighter)
    }

    fn row_with(doc: &RenderedDoc, needle: &str) -> usize {
        doc.lines
            .iter()
            .position(|line| line.text().contains(needle))
            .unwrap_or_else(|| panic!("no row contains {needle:?}"))
    }

    #[test]
    fn every_top_level_block_knows_its_source_lines() {
        let source = "# Title\n\nfirst line\nsecond line\n\n- a\n- b\n\n```rust\nlet x = 1;\n```\n";
        let doc = render_mapped(source, 60);
        let spans: Vec<(usize, usize)> = doc.blocks.iter().map(|b| (b.src_start, b.src_end)).collect();
        assert_eq!(spans, vec![(0, 0), (2, 3), (5, 6), (8, 10)]);
        // The map is contiguous and inside the document.
        assert!(doc.blocks.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert!(doc.blocks.iter().all(|b| b.end <= doc.lines.len()));
    }

    #[test]
    fn unwrapped_blocks_map_row_by_row_and_code_blocks_include_their_fences() {
        let source = "# Title\n\nfirst\n\n- apple\n- banana\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n";
        let doc = render_mapped(source, 60);
        assert_eq!(doc.source_line_at(row_with(&doc, "Title")), Some(0));
        assert_eq!(doc.source_line_at(row_with(&doc, "first")), Some(2));
        assert_eq!(doc.source_line_at(row_with(&doc, "apple")), Some(4));
        assert_eq!(doc.source_line_at(row_with(&doc, "banana")), Some(5));
        assert_eq!(doc.source_line_at(row_with(&doc, "let x")), Some(8));
        assert_eq!(doc.source_line_at(row_with(&doc, "let y")), Some(9));
        assert!(doc.blocks.iter().all(|block| block.exact), "{:?}", doc.blocks);
    }

    #[test]
    fn a_paragraph_over_several_source_lines_is_one_block() {
        // Soft line breaks flow into one row, so the rows are not the lines.
        let doc = render_mapped("first line\nsecond line\n", 60);
        assert_eq!(doc.blocks.len(), 1);
        assert!(!doc.blocks[0].exact);
        assert_eq!(doc.source_line_at(0), Some(0));
        let range = doc.source_range(0, 0).unwrap();
        assert_eq!((range.start, range.end, range.exact), (0, 1, false));
    }

    #[test]
    fn rows_and_source_lines_map_back_and_forth() {
        let source = "# Title\n\npara\n\n- one\n- two\n\nlast\n";
        let doc = render_mapped(source, 60);
        for line in [0usize, 2, 4, 5, 7] {
            let row = doc.row_of_source_line(line).expect("a row");
            assert_eq!(doc.source_line_at(row), Some(line), "line {line}");
        }
        // A blank source line leads to the next block; past the end, to the last.
        assert_eq!(doc.row_of_source_line(1), doc.row_of_source_line(2));
        assert_eq!(doc.row_of_source_line(999), doc.row_of_source_line(7));
    }

    #[test]
    fn wrapped_paragraphs_fall_back_to_the_whole_block() {
        let source = "# T\n\nthis paragraph is long enough that it has to be wrapped over several rows\n";
        let doc = render_mapped(source, 24);
        let paragraph = doc.blocks.iter().find(|b| b.src_start == 2).unwrap();
        assert!(!paragraph.exact);
        assert!(paragraph.content_end - paragraph.content_start > 1);
        for row in paragraph.content_start..paragraph.content_end {
            assert_eq!(doc.source_line_at(row), Some(2));
        }
        let range = doc.source_range(paragraph.content_start + 1, paragraph.content_start + 1).unwrap();
        assert_eq!((range.start, range.end, range.exact), (2, 2, false));
    }

    #[test]
    fn links_and_tables_are_not_claimed_to_be_exact() {
        let source = "see [the guide](notes/guide.md) now\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let doc = render_mapped(source, 60);
        assert_eq!(doc.blocks.len(), 2);
        assert!(!doc.blocks[0].exact, "the url is not shown, so the text differs");
        assert!(!doc.blocks[1].exact, "a table is drawn with its own borders");
        assert_eq!(doc.source_range(row_with(&doc, "guide"), row_with(&doc, "guide")).unwrap().start, 0);
    }

    #[test]
    fn source_ranges_cover_exact_selections_and_whole_blocks_across_blocks() {
        let source = "alpha\n\n- item one\n- item two\n- item three\n\ntail\n";
        let doc = render_mapped(source, 60);
        // Inside one exact block: precisely the selected lines.
        let range = doc.source_range(row_with(&doc, "item one"), row_with(&doc, "item two")).unwrap();
        assert_eq!((range.start, range.end, range.exact), (2, 3, true));
        // Across blocks, ending inside the list: exact as well.
        let range = doc.source_range(row_with(&doc, "alpha"), row_with(&doc, "item two")).unwrap();
        assert_eq!((range.start, range.end, range.exact), (0, 3, true));
        // Backwards selections are the same selection.
        let backwards = doc.source_range(row_with(&doc, "item two"), row_with(&doc, "alpha")).unwrap();
        assert_eq!(backwards, range);
        let range = doc.source_range(row_with(&doc, "item three"), row_with(&doc, "tail")).unwrap();
        assert_eq!((range.start, range.end, range.exact), (4, 6, true));
        // Rows past the end of the document map to nothing.
        assert!(doc.source_range(10_000, 10_001).is_none());
    }

    #[test]
    fn front_matter_hides_its_rows_but_keeps_the_numbers_right() {
        let source = "---\ntitle: x\n---\n\n# Real heading\n\ntext\n";
        let doc = render_mapped(source, 60);
        assert_eq!(doc.source_line_at(row_with(&doc, "Real heading")), Some(4));
        assert_eq!(doc.source_line_at(row_with(&doc, "text")), Some(6));
    }

    #[test]
    fn a_document_without_spans_has_no_source_map() {
        let doc = render_str("# T\n\ntext\n", 60);
        assert!(doc.blocks.is_empty());
        assert_eq!(doc.source_line_at(0), None);
        assert_eq!(doc.row_of_source_line(0), None);
        assert_eq!(doc.source_range(0, 1), None);
    }
}

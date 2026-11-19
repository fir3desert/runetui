//! Plain-text and message views.

use ratatui::style::Style;

use crate::markdown::renderer::wrap_spans;
use crate::markdown::{BlockMap, RLine, RSpan, RenderedDoc};

/// Longer documents get no source map (it would cost more than it is worth).
const MAP_LIMIT: usize = 300_000;
use crate::ui::themes::Theme;

/// Render a plain `.txt`-style document with soft wrapping.
pub fn render_plain(lines: &[String], width: usize, theme: Theme) -> RenderedDoc {
    let base = Style::default().fg(theme.fg);
    let width = width.max(8);
    let mut doc = RenderedDoc::default();

    if lines.is_empty() {
        return render_message(
            "Empty file",
            "There is nothing to preview in this file.",
            width,
            theme,
        );
    }

    for (index, source_line) in lines.iter().enumerate() {
        let start = doc.lines.len();
        let spans = vec![RSpan::styled(source_line.replace('\t', "    "), base)];
        for wrapped in wrap_spans(&spans, width) {
            doc.lines.push(RLine::new(wrapped));
        }
        let end = doc.lines.len();
        if end > start && lines.len() <= MAP_LIMIT {
            doc.blocks.push(BlockMap {
                start,
                end,
                content_start: start,
                content_end: end,
                src_start: index,
                src_end: index,
                // A line that wrapped is several rows of one source line.
                exact: end - start == 1,
            });
        }
    }

    if doc.lines.is_empty() {
        doc.lines.push(RLine::default());
    }
    doc
}

/// A centred-ish informational panel used for binary files, oversized files,
/// load errors and empty files.
pub fn render_message(title: &str, detail: &str, width: usize, theme: Theme) -> RenderedDoc {
    let width = width.max(8);
    let mut doc = RenderedDoc::default();
    doc.lines.push(RLine::default());
    doc.lines.push(RLine::default());

    let icon_style = Style::default()
        .fg(theme.accent)
        .add_modifier(ratatui::style::Modifier::BOLD);
    let title_style = Style::default()
        .fg(theme.fg)
        .add_modifier(ratatui::style::Modifier::BOLD);
    doc.lines.push(RLine {
        spans: vec![
            RSpan::styled("   ▨  ", icon_style),
            RSpan::styled(title.to_string(), title_style),
        ],
    });

    if !detail.is_empty() {
        doc.lines.push(RLine::default());
        for paragraph in detail.split("\n\n") {
            let spans = vec![RSpan::styled(
                paragraph.replace('\n', " "),
                Style::default().fg(theme.fg_dim),
            )];
            for wrapped in wrap_spans(&spans, width.saturating_sub(8).max(8)) {
                let mut line = vec![RSpan::styled("      ", Style::default())];
                line.extend(wrapped);
                doc.lines.push(RLine::new(line));
            }
            doc.lines.push(RLine::default());
        }
    }

    doc.lines.push(RLine::default());
    doc.lines.push(RLine {
        spans: vec![RSpan::styled(
            format!(
                "      {}",
                "Preview · press e to edit this file in your editor"
            ),
            Style::default().fg(theme.fg_muted),
        )],
    });

    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::catppuccin_mocha;

    #[test]
    fn plain_lines_respect_width() {
        let source = vec!["word ".repeat(40)];
        let doc = render_plain(&source, 24, catppuccin_mocha());
        assert!(doc.lines.iter().all(|line| line.width() <= 24));
    }

    #[test]
    fn messages_have_a_title() {
        let doc = render_message("Binary file", "No preview.", 60, catppuccin_mocha());
        let text = doc
            .lines
            .iter()
            .map(RLine::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Binary file"));
        assert!(text.contains("press e to edit"));
    }
}

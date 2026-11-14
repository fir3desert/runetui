//! Standalone source-file view with line numbers and real syntax highlighting.

use ratatui::style::{Modifier, Style};

use crate::markdown::code_highlighter::CodeHighlighter;
use crate::markdown::renderer::{spans_width, wrap_hard};
use crate::markdown::{RLine, RSpan, RenderedDoc};
use crate::ui::themes::Theme;

/// Column layout of the code view; it depends on the total line count so the
/// gutter keeps the same width while a long file is revealed chunk by chunk.
#[derive(Debug, Clone, Copy)]
pub struct CodeLayout {
    pub number_width: usize,
    pub gutter: usize,
    pub content_width: usize,
    pub width: usize,
}

pub fn code_layout(total_lines: usize, width: usize) -> CodeLayout {
    let width = width.max(12);
    let number_width = total_lines.max(1).to_string().len().max(2);
    let gutter = number_width + 3; // " 12 │ "
    CodeLayout {
        number_width,
        gutter,
        content_width: width.saturating_sub(gutter).max(8),
        width,
    }
}

/// Render already highlighted source lines.  `first_index` is the zero-based
/// number of the first line in `highlighted`.
pub fn render_code_range(
    highlighted: &[Vec<RSpan>],
    first_index: usize,
    layout: CodeLayout,
    theme: Theme,
) -> Vec<RLine> {
    let number_style = Style::default().fg(theme.fg_muted);
    let border_style = Style::default().fg(theme.border);
    let CodeLayout {
        number_width,
        gutter,
        content_width,
        ..
    } = layout;
    let mut out = Vec::new();
    for (offset, highlighted_line) in highlighted.iter().enumerate() {
        let index = first_index + offset;
        let wrapped = wrap_hard(highlighted_line, content_width);
        for (part, chunk) in wrapped.into_iter().enumerate() {
            let mut spans = Vec::new();
            if part == 0 {
                spans.push(RSpan::styled(
                    format!("{:>number_width$} ", index + 1),
                    number_style,
                ));
                spans.push(RSpan::styled("│ ", border_style));
            } else {
                spans.push(RSpan::styled(" ".repeat(gutter), Style::default()));
            }
            let chunk_width = spans_width(&chunk);
            spans.extend(chunk);
            if chunk_width < content_width {
                spans.push(RSpan::styled(
                    " ".repeat(content_width - chunk_width),
                    Style::default(),
                ));
            }
            out.push(RLine::new(spans));
        }
    }
    out
}

/// Make the code view feel like a buffer: a subtle rule at the end.
pub fn closing_lines(layout: CodeLayout, theme: Theme) -> Vec<RLine> {
    vec![
        RLine::default(),
        RLine {
            spans: vec![RSpan::styled(
                "─".repeat(layout.width.saturating_sub(2)),
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            )],
        },
    ]
}

#[allow(dead_code)]
pub fn render_code(
    lines: &[String],
    lang: &str,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    let mut doc = RenderedDoc::default();
    if lines.is_empty() {
        doc.lines.push(RLine::default());
        return doc;
    }
    let layout = code_layout(lines.len(), width);
    let sample = lines
        .iter()
        .take(SAMPLE_LINES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let mut session = highlighter.start_session(lang, &sample, &theme);
    let highlighted = highlighter.highlight_chunk(&mut session, lines, &theme);
    doc.lines = render_code_range(&highlighted, 0, layout, theme);
    doc.lines.extend(closing_lines(layout, theme));
    doc
}

/// Lines used to detect the language of a file without an obvious extension.
pub const SAMPLE_LINES: usize = 20;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::catppuccin_mocha;

    #[test]
    fn code_view_has_line_numbers_and_respects_width() {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let lines = vec![
            "fn main() {".to_string(),
            "    println!(\"hello\");".to_string(),
            "}".to_string(),
        ];
        let doc = render_code(&lines, "rust", 50, theme, &mut highlighter);
        assert!(doc.lines.len() >= 3);
        assert!(doc.lines[0].text().contains('1'));
        assert!(doc.lines[0].text().contains('│'));
        assert!(doc.lines[1].text().contains("println"));
        for line in &doc.lines {
            assert!(line.width() <= 50, "line too wide: {}", line.width());
        }
        let colours = doc
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().filter_map(|span| span.style.fg))
            .collect::<std::collections::HashSet<_>>();
        assert!(colours.len() > 2, "expected syntax colours");
    }

    #[test]
    fn long_lines_wrap_with_indentation() {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let lines =
            vec!["let some_identifier = another_identifier + yet_another_identifier;".to_string()];
        let doc = render_code(&lines, "rust", 32, theme, &mut highlighter);
        assert!(doc.lines.iter().all(|line| line.width() <= 32));
        assert!(doc.lines.len() > 1);
    }
}

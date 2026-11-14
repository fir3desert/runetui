//! Standalone source-file view with line numbers and real syntax highlighting.

use ratatui::style::{Modifier, Style};

use crate::markdown::code_highlighter::CodeHighlighter;
use crate::markdown::renderer::{spans_width, wrap_hard};
use crate::markdown::{RLine, RSpan, RenderedDoc};
use crate::ui::themes::Theme;

pub fn render_code(
    lines: &[String],
    lang: &str,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    let width = width.max(12);
    let code = lines.join("\n");
    let highlighted = highlighter.highlight(&code, lang, &theme);

    let count = highlighted.lines.len().max(lines.len()).max(1);
    let number_width = count.to_string().len().max(2);
    let gutter = number_width + 3; // " 12 │ "
    let content_width = width.saturating_sub(gutter).max(8);

    let number_style = Style::default().fg(theme.fg_muted);
    let border_style = Style::default().fg(theme.border);
    let mut doc = RenderedDoc::default();

    if highlighted.lines.is_empty() {
        doc.lines.push(RLine::default());
        return doc;
    }

    for (index, highlighted_line) in highlighted.lines.iter().enumerate() {
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
            doc.lines.push(RLine::new(spans));
        }
    }

    // Make the code view feel like a buffer: a subtle rule at the end.
    doc.lines.push(RLine::default());
    doc.lines.push(RLine {
        spans: vec![RSpan::styled(
            "─".repeat(width.saturating_sub(2)),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        )],
    });

    doc
}

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

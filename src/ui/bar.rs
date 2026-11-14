//! Single-row bar composition used by the header and status bar.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

fn span_width(span: &Span<'_>) -> usize {
    UnicodeWidthStr::width(span.content.as_ref())
}

fn width_of(spans: &[Span<'_>]) -> usize {
    spans.iter().map(span_width).sum()
}

/// Join left- and right-aligned spans, dropping right-hand items from the
/// front when space runs out.  Text is never truncated mid-span; a line wider
/// than the terminal is simply clipped by Ratatui.
pub fn build(
    mut left: Vec<Span<'static>>,
    mut right: Vec<Span<'static>>,
    width: u16,
) -> Line<'static> {
    let width = width as usize;
    let mut left_width = width_of(&left);
    let mut right_width = width_of(&right);

    while !right.is_empty() && left_width + right_width + 1 > width {
        let removed = right.remove(0);
        right_width = right_width.saturating_sub(span_width(&removed));
    }

    // If even one right-hand item cannot fit, drop the whole group.
    if right_width > 0 && left_width + right_width + 1 > width {
        right.clear();
        right_width = 0;
    }

    let gap = width.saturating_sub(left_width + right_width);
    if gap > 0 {
        left.push(Span::raw(" ".repeat(gap)));
        left_width += gap;
    }
    let _ = left_width;
    left.extend(right);
    Line::from(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn aligns_both_sides() {
        let line = build(vec![Span::raw("left")], vec![Span::raw("right")], 20);
        assert_eq!(text(&line), "left           right");
    }

    #[test]
    fn drops_right_items_when_narrow() {
        let line = build(
            vec![Span::raw("abcdefghij")],
            vec![Span::raw("one"), Span::raw("two")],
            12,
        );
        assert_eq!(text(&line).trim_end(), "abcdefghij");
    }

    #[test]
    fn keeps_at_least_some_right_content() {
        let line = build(
            vec![Span::raw("abc")],
            vec![Span::raw("one"), Span::raw("two")],
            8,
        );
        assert_eq!(text(&line), "abc  two");
    }
}

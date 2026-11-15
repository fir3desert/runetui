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

/// A composed bar and where each right-hand item ended up.
pub struct Built {
    pub line: Line<'static>,
    /// For every item of `right`: `(offset from the bar's left edge, width)`,
    /// or `None` when it was dropped.
    pub right_at: Vec<Option<(u16, u16)>>,
}

/// Like [`build`], but the right-hand items are shown in the order given and
/// are dropped in `drop_order` (indexes into `right`) when space runs out.
/// `tail` is always kept after them unless nothing fits at all.
pub fn build_ordered(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    tail: Span<'static>,
    width: u16,
    drop_order: &[usize],
) -> Built {
    let width = width as usize;
    let left_width = width_of(&left);
    let tail_width = span_width(&tail);
    let mut keep = vec![true; right.len()];
    let group_width = |keep: &[bool]| -> usize {
        right
            .iter()
            .zip(keep)
            .filter(|(_, kept)| **kept)
            .map(|(span, _)| span_width(span))
            .sum::<usize>()
            + tail_width
    };

    for &index in drop_order {
        if left_width + group_width(&keep) + 1 <= width {
            break;
        }
        if let Some(kept) = keep.get_mut(index) {
            *kept = false;
        }
    }
    let mut use_tail = true;
    if left_width + group_width(&keep) + 1 > width {
        // Not even the tail fits next to the left side: show only the left.
        keep.iter_mut().for_each(|kept| *kept = false);
        use_tail = false;
    }

    let right_width = if use_tail { group_width(&keep) } else { 0 };
    let gap = width.saturating_sub(left_width + right_width);
    let mut cursor = left_width + gap;
    let mut right_at = vec![None; right.len()];
    if gap > 0 {
        left.push(Span::raw(" ".repeat(gap)));
    }
    for (index, span) in right.into_iter().enumerate() {
        if !keep[index] {
            continue;
        }
        let span_w = span_width(&span);
        right_at[index] = Some((cursor as u16, span_w as u16));
        cursor += span_w;
        left.push(span);
    }
    if use_tail {
        left.push(tail);
    }
    Built {
        line: Line::from(left),
        right_at,
    }
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

    #[test]
    fn ordered_build_drops_in_the_given_order_and_reports_positions() {
        let right = || {
            vec![
                Span::raw("kind"),
                Span::raw(" "),
                Span::raw("0%"),
                Span::raw(" ★ 5"),
            ]
        };
        let order = [3, 0, 1, 2];
        // Plenty of room: everything shows, stars last.
        let built = build_ordered(vec![Span::raw("ab")], right(), Span::raw(" "), 20, &order);
        assert_eq!(text(&built.line), format!("ab{}kind 0% ★ 5 ", " ".repeat(6)));
        let star = built.right_at[3].unwrap();
        assert_eq!(star.1, 4);
        assert_eq!(star.0 + star.1 + 1, 20, "the tail margin follows the stars");

        // Tight: the stars go first, then the kind.
        let built = build_ordered(vec![Span::raw("ab")], right(), Span::raw(" "), 12, &order);
        assert!(built.right_at[3].is_none());
        assert!(built.right_at[0].is_some());
        let built = build_ordered(vec![Span::raw("ab")], right(), Span::raw(" "), 7, &order);
        assert!(built.right_at[3].is_none() && built.right_at[0].is_none());
        assert!(built.right_at[2].is_some());
        // Hopeless: only the left side.
        let built = build_ordered(vec![Span::raw("abcdef")], right(), Span::raw(" "), 6, &order);
        assert!(built.right_at.iter().all(Option::is_none));
        assert_eq!(text(&built.line).trim_end(), "abcdef");
    }
}

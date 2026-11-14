//! Mouse text selection limited to one pane.
//!
//! The terminal's own selection would grab the explorer, the borders and the
//! document at once.  With mouse capture on, runetui tracks the drag itself
//! and clamps it to the pane where it started, so only that pane's text is
//! highlighted and copied.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The pane the selection is confined to.
    pub area: Rect,
    /// Where the button went down, in screen coordinates `(column, row)`.
    pub anchor: (u16, u16),
    /// Where the pointer is now.
    pub head: (u16, u16),
    /// The button was released: the text is ready to be copied.
    pub done: bool,
}

impl Selection {
    pub fn new(area: Rect, x: u16, y: u16) -> Self {
        let point = clamp(area, x, y);
        Self {
            area,
            anchor: point,
            head: point,
            done: false,
        }
    }

    pub fn drag_to(&mut self, x: u16, y: u16) {
        self.head = clamp(self.area, x, y);
    }

    /// A click without a drag selects nothing.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in reading order.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let a = (self.anchor.1, self.anchor.0);
        let b = (self.head.1, self.head.0);
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        ((start.1, start.0), (end.1, end.0))
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        if self.is_empty() || !rect_contains(self.area, x, y) {
            return false;
        }
        let ((start_x, start_y), (end_x, end_y)) = self.ordered();
        if y < start_y || y > end_y {
            return false;
        }
        let first = if y == start_y { start_x } else { self.area.x };
        let last = if y == end_y {
            end_x
        } else {
            self.area.right().saturating_sub(1)
        };
        x >= first && x <= last
    }

    /// The selected text, one line per row, trailing blanks removed.
    pub fn extract(&self, buffer: &Buffer) -> String {
        if self.is_empty() {
            return String::new();
        }
        let ((_, start_y), (_, end_y)) = self.ordered();
        let mut lines = Vec::new();
        for y in start_y..=end_y {
            let mut line = String::new();
            for x in self.area.x..self.area.right() {
                if !self.contains(x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Paint the selection with explicit colours.
    pub fn highlight(&self, buffer: &mut Buffer, background: Color, foreground: Color) {
        if self.is_empty() {
            return;
        }
        let ((_, start_y), (_, end_y)) = self.ordered();
        for y in start_y..=end_y {
            for x in self.area.x..self.area.right() {
                if !self.contains(x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_bg(background);
                    cell.set_fg(foreground);
                }
            }
        }
    }
}

fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

fn clamp(area: Rect, x: u16, y: u16) -> (u16, u16) {
    if area.width == 0 || area.height == 0 {
        return (area.x, area.y);
    }
    (
        x.clamp(area.x, area.right() - 1),
        y.clamp(area.y, area.bottom() - 1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with(lines: &[&str]) -> Buffer {
        let width = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0) as u16;
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, lines.len() as u16));
        for (row, line) in lines.iter().enumerate() {
            buffer.set_string(0, row as u16, line, ratatui::style::Style::default());
        }
        buffer
    }

    #[test]
    fn selection_stays_inside_its_pane() {
        // Left pane (0..10) is the explorer, the right pane starts at column 10.
        let pane = Rect::new(10, 0, 10, 3);
        let mut selection = Selection::new(pane, 12, 0);
        selection.drag_to(2, 2); // dragged far into the explorer
        assert_eq!(selection.head, (10, 2));
        assert!(!selection.contains(5, 1));
        assert!(selection.contains(10, 2));
    }

    #[test]
    fn extracts_reading_order_text_without_the_neighbouring_pane() {
        let buffer = buffer_with(&[
            "EXPLORERalpha beta",
            "EXPLORERgamma     ",
            "EXPLORERdelta eps ",
        ]);
        let pane = Rect::new(8, 0, 10, 3);
        let mut selection = Selection::new(pane, 14, 0); // from "beta"
        selection.drag_to(10, 2); // to "del"
        let text = selection.extract(&buffer);
        assert_eq!(text, "beta\ngamma\ndel");
        assert!(!text.contains("EXPLORER"));
    }

    #[test]
    fn dragging_backwards_selects_the_same_text() {
        let buffer = buffer_with(&["hello world"]);
        let pane = Rect::new(0, 0, 11, 1);
        let mut forward = Selection::new(pane, 0, 0);
        forward.drag_to(4, 0);
        let mut backward = Selection::new(pane, 4, 0);
        backward.drag_to(0, 0);
        assert_eq!(forward.extract(&buffer), "hello");
        assert_eq!(backward.extract(&buffer), "hello");
    }

    #[test]
    fn a_click_selects_nothing_and_highlight_paints_only_the_selection() {
        let mut buffer = buffer_with(&["abcdef"]);
        let pane = Rect::new(0, 0, 6, 1);
        let click = Selection::new(pane, 2, 0);
        assert!(click.is_empty());
        assert_eq!(click.extract(&buffer), "");

        let mut selection = Selection::new(pane, 1, 0);
        selection.drag_to(3, 0);
        selection.highlight(&mut buffer, Color::Red, Color::White);
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        assert_eq!(buffer[(2, 0)].bg, Color::Red);
        assert_eq!(buffer[(4, 0)].bg, Color::Reset);
    }
}

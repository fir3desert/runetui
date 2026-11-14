//! Responsive screen layout.
//!
//! The screen is stacked vertically as header, body and status bar.  The body
//! is split into explorer, a one-column divider and the document column.  The
//! document column holds the viewer and, below it, an optional terminal panel
//! (a one-row title bar plus the shell).  On very narrow terminals the
//! explorer is hidden so the document keeps a readable width, and short
//! terminals drop the terminal panel.

use ratatui::layout::Rect;

pub const HEADER_HEIGHT: u16 = 1;
pub const STATUS_HEIGHT: u16 = 1;
pub const DIVIDER_WIDTH: u16 = 1;
/// Below this terminal width the explorer is suppressed entirely.
pub const NARROW_BREAKPOINT: u16 = 52;
pub const MIN_VIEWER_WIDTH: u16 = 24;
pub const MIN_EXPLORER_WIDTH: u16 = 18;
pub const TERMINAL_BAR_HEIGHT: u16 = 1;
/// The viewer never gets fewer rows than this when the terminal is open.
pub const MIN_VIEWER_HEIGHT: u16 = 4;
/// Shell rows, not counting the title bar.
pub const MIN_TERMINAL_ROWS: u16 = 3;
pub const MIN_TERMINAL_PCT: u16 = 10;
pub const MAX_TERMINAL_PCT: u16 = 70;
pub const DEFAULT_TERMINAL_PCT: u16 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub header: Rect,
    pub explorer: Option<Rect>,
    pub divider: Option<Rect>,
    pub viewer: Rect,
    /// Title row above the terminal.
    pub terminal_bar: Option<Rect>,
    /// Terminal area, including its one-column focus gutter on the left.
    pub terminal: Option<Rect>,
    pub status: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Self::compute(Rect::new(0, 0, 100, 30), true, 28, None)
    }
}

impl Layout {
    pub fn compute(
        area: Rect,
        show_explorer: bool,
        width_pct: u16,
        terminal_pct: Option<u16>,
    ) -> Self {
        let header_height = if area.height >= 3 { HEADER_HEIGHT } else { 0 };
        let status_height = if area.height >= 2 { STATUS_HEIGHT } else { 0 };

        let header = Rect::new(area.x, area.y, area.width, header_height);
        let status = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(status_height),
            area.width,
            status_height,
        );

        let body_y = area.y + header_height;
        let body_height = area.height.saturating_sub(header_height + status_height);

        let wants_explorer = show_explorer
            && body_height >= 2
            && area.width >= NARROW_BREAKPOINT
            && area.width > MIN_VIEWER_WIDTH + MIN_EXPLORER_WIDTH + DIVIDER_WIDTH;

        let (explorer, divider, viewer) = if wants_explorer {
            let requested = (u32::from(area.width) * u32::from(width_pct) / 100) as u16;
            let maximum = area.width.saturating_sub(MIN_VIEWER_WIDTH + DIVIDER_WIDTH);
            let width = requested.clamp(MIN_EXPLORER_WIDTH, maximum.max(MIN_EXPLORER_WIDTH));
            let explorer = Rect::new(area.x, body_y, width, body_height);
            let divider = Rect::new(
                area.x + width,
                body_y,
                DIVIDER_WIDTH.min(area.width.saturating_sub(width)),
                body_height,
            );
            let viewer_x = area.x + width + DIVIDER_WIDTH;
            let viewer_width = area.width.saturating_sub(width + DIVIDER_WIDTH);
            (
                Some(explorer),
                Some(divider),
                Rect::new(viewer_x, body_y, viewer_width, body_height),
            )
        } else {
            (
                None,
                None,
                Rect::new(area.x, body_y, area.width, body_height),
            )
        };

        // The terminal panel takes the bottom of the document column.
        let (viewer, terminal_bar, terminal) = match terminal_pct {
            Some(pct)
                if viewer.width >= 10
                    && viewer.height
                        >= MIN_VIEWER_HEIGHT + TERMINAL_BAR_HEIGHT + MIN_TERMINAL_ROWS =>
            {
                let requested = (u32::from(viewer.height) * u32::from(pct) / 100) as u16;
                let minimum = TERMINAL_BAR_HEIGHT + MIN_TERMINAL_ROWS;
                let maximum = viewer.height - MIN_VIEWER_HEIGHT;
                let panel = requested.clamp(minimum, maximum.max(minimum));
                let top_height = viewer.height - panel;
                let bar = Rect::new(viewer.x, viewer.y + top_height, viewer.width, TERMINAL_BAR_HEIGHT);
                let shell = Rect::new(
                    viewer.x,
                    bar.y + TERMINAL_BAR_HEIGHT,
                    viewer.width,
                    panel - TERMINAL_BAR_HEIGHT,
                );
                (
                    Rect::new(viewer.x, viewer.y, viewer.width, top_height),
                    Some(bar),
                    Some(shell),
                )
            }
            _ => (viewer, None, None),
        };

        Self {
            header,
            explorer,
            divider,
            viewer,
            terminal_bar,
            terminal,
            status,
        }
    }

    /// The part of the terminal area that holds the shell (the area minus
    /// the one-column focus gutter).
    pub fn terminal_inner(&self) -> Option<Rect> {
        self.terminal.map(|area| {
            let gutter = 1.min(area.width.saturating_sub(1));
            Rect::new(area.x + gutter, area.y, area.width - gutter, area.height)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_wide_layout() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 40), true, 30, None);
        assert_eq!(layout.header.height, 1);
        assert_eq!(layout.status.height, 1);
        let explorer = layout.explorer.unwrap();
        assert_eq!(explorer.width, 36);
        assert_eq!(explorer.height, 38);
        assert_eq!(layout.divider.unwrap().width, 1);
        assert_eq!(layout.viewer.width, 120 - explorer.width - DIVIDER_WIDTH);
        assert_eq!(layout.viewer.height, 38);
    }

    #[test]
    fn hides_explorer_on_narrow_terminals() {
        let layout = Layout::compute(Rect::new(0, 0, 45, 20), true, 30, None);
        assert!(layout.explorer.is_none());
        assert!(layout.divider.is_none());
        assert_eq!(layout.viewer.width, 45);
    }

    #[test]
    fn hidden_explorer_gives_full_width() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 40), false, 30, None);
        assert!(layout.explorer.is_none());
        assert_eq!(layout.viewer.width, 120);
    }

    #[test]
    fn explorer_never_squeezes_the_document() {
        let layout = Layout::compute(Rect::new(0, 0, 60, 20), true, 60, None);
        let explorer = layout.explorer.unwrap();
        assert!(layout.viewer.width >= MIN_VIEWER_WIDTH);
        assert!(explorer.width >= MIN_EXPLORER_WIDTH);
    }

    #[test]
    fn terminal_panel_sits_under_the_viewer_next_to_the_explorer() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 40), true, 30, Some(30));
        let explorer = layout.explorer.unwrap();
        let bar = layout.terminal_bar.unwrap();
        let shell = layout.terminal.unwrap();
        // The explorer keeps the whole height; the panel only spans the document column.
        assert_eq!(explorer.height, 38);
        assert_eq!(shell.x, layout.viewer.x);
        assert_eq!(shell.width, layout.viewer.width);
        assert_eq!(bar.y, layout.viewer.y + layout.viewer.height);
        assert_eq!(shell.y, bar.y + 1);
        // Viewer + bar + shell fill the body exactly.
        assert_eq!(layout.viewer.height + 1 + shell.height, 38);
        assert_eq!(shell.y + shell.height, layout.status.y);
        // 30% of 38 rows, bar included.
        assert_eq!(1 + shell.height, 11);
    }

    #[test]
    fn terminal_spans_the_full_width_without_the_explorer() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 40), false, 30, Some(30));
        assert_eq!(layout.terminal.unwrap().width, 120);
        assert_eq!(layout.terminal.unwrap().x, 0);
    }

    #[test]
    fn terminal_height_follows_the_percentage_within_limits() {
        let small = Layout::compute(Rect::new(0, 0, 120, 40), true, 30, Some(MIN_TERMINAL_PCT));
        let large = Layout::compute(Rect::new(0, 0, 120, 40), true, 30, Some(MAX_TERMINAL_PCT));
        assert!(small.terminal.unwrap().height < large.terminal.unwrap().height);
        // Even at the maximum the viewer keeps a few rows.
        assert!(large.viewer.height >= MIN_VIEWER_HEIGHT);
    }

    #[test]
    fn short_screens_drop_the_terminal() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 9), true, 30, Some(30));
        assert!(layout.terminal.is_none());
        assert!(layout.terminal_bar.is_none());
        assert_eq!(layout.viewer.height, 7);
    }

    #[test]
    fn terminal_inner_leaves_room_for_the_focus_gutter() {
        let layout = Layout::compute(Rect::new(0, 0, 120, 40), true, 30, Some(30));
        let area = layout.terminal.unwrap();
        let inner = layout.terminal_inner().unwrap();
        assert_eq!(inner.x, area.x + 1);
        assert_eq!(inner.width, area.width - 1);
        assert_eq!(inner.height, area.height);
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for width in 0..8 {
            for height in 0..8 {
                for terminal in [None, Some(30), Some(70)] {
                    let layout =
                        Layout::compute(Rect::new(0, 0, width, height), true, 28, terminal);
                    assert!(layout.viewer.x + layout.viewer.width <= width);
                    assert!(layout.header.y + layout.header.height <= height);
                    if let Some(shell) = layout.terminal {
                        assert!(shell.y + shell.height <= height);
                        assert!(shell.x + shell.width <= width);
                    }
                }
            }
        }
    }
}

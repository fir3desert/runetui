//! Top header: identity and document position.
//!
//! The project name and path are shown once, in the explorer and the status
//! bar, so the header stays short: the app badge, the heading under the
//! cursor, the kind of document, how far down it you are and, at the far right, the
//! GitHub star count (a click on it opens the repository).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::app::{App, Overlay};
use crate::ui::bar;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();

    let mut left = vec![Span::styled(
        " runetui ",
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];

    if let Some(heading) = app.viewer.heading_nearest_cursor() {
        left.push(Span::styled("  › ", theme.muted()));
        left.push(Span::styled(heading.text.clone(), theme.dim()));
    }

    let total = app.viewer.total_lines();
    let position = if app.viewer.has_document() && total > 0 {
        format!("{}%", app.viewer.progress())
    } else {
        "—".to_string()
    };

    // Kind, progress, then the star count at the far right.  The stars are the
    // first thing dropped when the bar runs out of room, then the kind.  The
    // Esc menu draws its own copy in the same corner, so the header leaves it
    // out then.
    let mut right = vec![
        Span::styled(
            format!(" {} ", app.viewer.kind_label),
            Style::default()
                .fg(theme.accent2)
                .bg(theme.bg_sel)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(position, theme.muted()),
    ];
    let stars_label = if matches!(app.overlay, Overlay::Menu { .. }) {
        None
    } else {
        crate::stars::label(app.stars.state())
    };
    if let Some(label) = &stars_label {
        right.push(Span::styled(
            format!("  {label}"),
            Style::default()
                .fg(crate::ui::menu::STAR_COLOR)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let built = bar::build_ordered(left, right, Span::raw(" "), area.width, &[3, 0, 1, 2]);
    // The label itself (not its leading blanks) is what a click hits.
    if stars_label.is_some() {
        if let Some(Some((offset, width))) = built.right_at.get(3) {
            app.star_rect
                .set(Some(Rect::new(area.x + offset + 2, area.y, width - 2, 1)));
        }
    }
    let line = built.line;
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg_alt).fg(theme.fg_dim)),
        area,
    );
}

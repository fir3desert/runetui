//! Top header: identity and document position.
//!
//! The project name and path are shown once, in the explorer and the status
//! bar, so the header stays short: the app badge, the heading under the
//! cursor, the GitHub star count, the kind of document and how far down it
//! you are.

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

    let mut right = Vec::new();
    // The star count sits left of the document kind.  It is the first item
    // dropped when the bar runs out of room, and the Esc menu draws its own
    // copy in the same corner, so the header leaves it out then.
    if !matches!(app.overlay, Overlay::Menu { .. }) {
        if let Some(label) = crate::stars::label(app.stars.state()) {
            right.push(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(crate::ui::menu::STAR_COLOR)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    right.extend([
        Span::styled(
            format!(" {} ", app.viewer.kind_label),
            Style::default()
                .fg(theme.accent2)
                .bg(theme.bg_sel)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(format!("{position} "), theme.muted()),
    ]);

    let line = bar::build(left, right, area.width);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg_alt).fg(theme.fg_dim)),
        area,
    );
}

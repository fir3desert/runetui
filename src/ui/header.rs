//! Top header: identity and document position.
//!
//! The project name and path are shown once, in the explorer and the status
//! bar, so the header stays short: the app badge, the heading under the
//! cursor, the kind of document, how far down it you are and, at the far right, the
//! GitHub star count (a click on it opens the repository).  The system clock
//! sits in the middle of the bar when there is room for it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
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
    // Esc menu leaves this whole corner as it is (it is not dimmed).
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
    let stars_label = crate::stars::label(app.stars.state());
    if let Some(label) = &stars_label {
        right.push(Span::styled(
            format!("  {label}"),
            Style::default()
                .fg(crate::ui::menu::STAR_COLOR)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let left_width: u16 = left
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()) as u16)
        .sum();
    let built = bar::build_ordered(left, right, Span::raw(" "), area.width, &[3, 0, 1, 2]);
    // Where the right-hand group starts (the bar's width when it was dropped).
    let right_start = built
        .right_at
        .iter()
        .flatten()
        .map(|(offset, _)| *offset)
        .min();
    if let Some(start) = right_start {
        app.header_keep
            .set(Some(Rect::new(area.x + start, area.y, area.width - start, 1)));
    }
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

    // The system time, centred on the bar; it is left out when it would touch
    // either side.
    let clock = format!(" {} ", chrono::Local::now().format("%H:%M:%S"));
    let clock_width = UnicodeWidthStr::width(clock.as_str()) as u16;
    let x = area.x + area.width.saturating_sub(clock_width) / 2;
    let free_from = area.x + left_width + 1;
    let free_to = area.x + right_start.unwrap_or(area.width);
    if clock_width <= area.width && x >= free_from && x + clock_width < free_to {
        frame.render_widget(
            Paragraph::new(Span::styled(clock, theme.muted()))
                .style(Style::default().bg(theme.bg_alt)),
            Rect::new(x, area.y, clock_width, 1),
        );
    }
}

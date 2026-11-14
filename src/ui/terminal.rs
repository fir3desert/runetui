//! Terminal panel at the bottom: a title bar, a focus gutter and the shell.
//!
//! The gutter is a one-column strip on the left.  It is a coloured `▍` bar
//! (like the selection bar of the explorer) while the terminal has the
//! keyboard and a faint `│` otherwise, so it is always clear where typing goes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
use crate::ui::themes::Theme;

pub const GUTTER_FOCUSED: &str = "▍";
pub const GUTTER_IDLE: &str = "│";
pub const GUTTER_BLANK: &str = " ";

pub fn render(frame: &mut Frame, app: &mut App) {
    let (Some(bar), Some(area), Some(inner)) = (
        app.layout.terminal_bar,
        app.layout.terminal,
        app.layout.terminal_inner(),
    ) else {
        return;
    };
    let theme = app.theme();
    let focused = app.focus == Focus::Terminal;

    render_bar(frame, bar, app, &theme, focused);
    // The explorer divider already draws a line right next to the gutter.
    let idle = if app.layout.divider.is_some() { GUTTER_BLANK } else { GUTTER_IDLE };
    render_gutter(frame, area, &theme, focused, idle);

    if let Some(term) = app.term.as_mut() {
        term.resize(inner.height, inner.width);
    }
    match &app.term {
        Some(term) => term.render(frame, inner, &theme, focused && !app.overlay.is_open()),
        None => {
            let message = if app.terminal_failed() {
                "The terminal could not be started (see the status bar)."
            } else {
                "Starting terminal…"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(message, theme.muted()))).style(theme.base()),
                inner,
            );
        }
    }
}

fn render_gutter(frame: &mut Frame, area: Rect, theme: &Theme, focused: bool, idle: &'static str) {
    let (glyph, color) = if focused {
        (GUTTER_FOCUSED, theme.accent)
    } else {
        (idle, theme.border)
    };
    for y in area.y..area.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((area.x, y)) {
            cell.set_symbol(glyph);
            cell.set_fg(color);
            cell.set_bg(theme.bg);
        }
    }
}

fn render_bar(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line_color = if focused { theme.accent } else { theme.border };

    let label_style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_muted)
    };
    let mut spans = vec![
        Span::styled("─", Style::default().fg(line_color)),
        Span::styled(" TERMINAL ", label_style),
    ];
    if let Some(term) = &app.term {
        let shell = std::path::Path::new(&term.program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| term.program.clone());
        spans.push(Span::styled(format!(" {shell} "), theme.dim()));
    }

    let hint = format!(
        " {} then ↑ ↓ resize · {} hide ",
        app.keymap.prefix_label(),
        app.keymap
            .primary(crate::keymap::Action::ToggleTerminal)
            .map(|chord| chord.label())
            .unwrap_or_else(|| format!("{} t", app.keymap.prefix_label())),
    );
    let used: usize = spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
    let hint_width = UnicodeWidthStr::width(hint.as_str());
    let width = area.width as usize;
    let show_hint = used + hint_width + 2 <= width;
    let fill = width.saturating_sub(used + if show_hint { hint_width + 1 } else { 0 });
    spans.push(Span::styled("─".repeat(fill), Style::default().fg(line_color)));
    if show_hint {
        spans.push(Span::styled(hint, theme.muted()));
        spans.push(Span::styled("─", Style::default().fg(line_color)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.base()),
        area,
    );
}

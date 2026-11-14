//! Document viewer rendering, including cursor-line and search overlays.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus};
use crate::markdown::RLine;
use crate::ui::themes::Theme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();
    let height = area.height as usize;
    let top = app.viewer.top;
    let end = app.viewer.doc.lines.len().min(top.saturating_add(height));
    let cursor_enabled = app.focus == Focus::Viewer;
    let width = area.width as usize;

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
    for index in top..end {
        let ranges = app.viewer.matches_on_line(index);
        let is_cursor = cursor_enabled && index == app.viewer.cursor;
        lines.push(build_line(
            &app.viewer.doc.lines[index],
            &theme,
            &ranges,
            is_cursor,
            width,
        ));
    }

    frame.render_widget(Paragraph::new(lines).style(theme.base()), area);
}

fn patched_fg(mut style: Style, theme: &Theme) -> Style {
    if style.fg.is_none() {
        style.fg = Some(theme.fg);
    }
    style
}

fn build_line(
    line: &RLine,
    theme: &Theme,
    ranges: &[(usize, usize, bool)],
    is_cursor: bool,
    width: usize,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut char_position = 0usize;

    for span in &line.spans {
        let characters: Vec<char> = span.text.chars().collect();
        let span_start = char_position;
        let span_end = char_position + characters.len();
        let base_style = patched_fg(span.style, theme);
        let mut consumed = 0usize;

        for (start, end, current) in ranges {
            if *end <= span_start || *start >= span_end {
                continue;
            }
            let local_start = (*start).max(span_start) - span_start;
            let local_end = (*end).min(span_end) - span_start;
            if local_start > consumed {
                push_span(&mut spans, &characters[consumed..local_start], base_style);
            }
            let highlight = if *current {
                theme.search_current()
            } else {
                theme.search_match()
            };
            push_span(
                &mut spans,
                &characters[local_start..local_end],
                base_style.patch(highlight),
            );
            consumed = local_end.max(consumed);
        }

        if consumed < characters.len() {
            push_span(&mut spans, &characters[consumed..], base_style);
        }
        char_position = span_end;
    }

    if is_cursor {
        let used = line.width().min(width);
        if used < width {
            spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(theme.cursor_line_bg()),
            ));
        }
    }

    let mut rendered = Line::from(spans);
    if is_cursor {
        rendered = rendered.style(Style::default().bg(theme.cursor_line_bg()));
    }
    rendered
}

fn push_span(target: &mut Vec<Span<'static>>, characters: &[char], style: Style) {
    if characters.is_empty() {
        return;
    }
    let text: String = characters.iter().collect();
    target.push(Span::styled(text, style));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::RSpan;
    use crate::ui::themes::catppuccin_mocha;

    #[test]
    fn search_ranges_split_spans_safely() {
        let line = RLine::new(vec![
            RSpan::plain("alpha "),
            RSpan::styled("beta", Style::default().fg(Color::Red)),
        ]);
        let theme = catppuccin_mocha();
        let rendered = build_line(&line, &theme, &[(6, 10, true)], false, 40);
        let text: String = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "alpha beta");
        assert!(
            rendered
                .spans
                .iter()
                .any(|span| span.style.bg == Some(theme.orange))
        );
    }

    #[test]
    fn cursor_line_is_padded_to_full_width() {
        let line = RLine::plain("hello");
        let theme = catppuccin_mocha();
        let rendered = build_line(&line, &theme, &[], true, 20);
        let text: String = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text.chars().count(), 20);
        assert_eq!(rendered.style.bg, Some(theme.cursor_line_bg()));
    }

    use ratatui::style::Color;
}

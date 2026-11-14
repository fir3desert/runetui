//! Project explorer rendering.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus};
use crate::filesystem::tree::FileNode;
use crate::ui::icons::{self, IconStyle};
use crate::ui::themes::Theme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();
    let visible = app.explorer.visible();
    let offset = app.explorer.offset();
    let selected = app.explorer.selected_position();
    let focused = app.focus == Focus::Explorer;
    let active_file = app
        .viewer
        .loaded
        .as_ref()
        .map(|loaded| loaded.path.as_path());
    let style = app.config.icon_style();
    let height = area.height as usize;

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
    for position in offset..visible.len().min(offset + height) {
        let Some(node) = app.explorer.tree.node(visible[position]) else {
            continue;
        };
        let is_selected = position == selected;
        lines.push(node_line(
            node,
            &theme,
            style,
            is_selected,
            focused,
            active_file,
        ));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

fn node_line(
    node: &FileNode,
    theme: &Theme,
    style: IconStyle,
    selected: bool,
    focused: bool,
    active_file: Option<&Path>,
) -> Line<'static> {
    let is_active = active_file.is_some_and(|path| path == node.path);
    let is_root = node.depth == 0;

    let mut spans = Vec::with_capacity(8);

    // Selection bar.
    if selected {
        spans.push(Span::styled(
            "▍",
            Style::default().fg(if focused { theme.accent } else { theme.border }),
        ));
    } else {
        spans.push(Span::raw(" "));
    }

    // Indent guides: one faint vertical line under every ancestor folder.
    for level in 0..node.depth {
        if level == 0 {
            spans.push(Span::raw("  "));
        } else {
            spans.push(Span::styled(
                style.guide(),
                Style::default().fg(theme.border),
            ));
        }
    }

    if node.is_dir {
        // The project root is always open, so it gets no chevron.
        if is_root {
            spans.push(Span::raw(""));
        } else {
            spans.push(Span::styled(
                style.chevron(node.expanded),
                Style::default()
                    .fg(if node.expanded {
                        theme.accent
                    } else {
                        theme.fg_muted
                    })
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let (icon, color) = icons::folder_icon(&node.name, node.expanded, style, theme);
        spans.push(Span::styled(format!("{icon} "), Style::default().fg(color)));
    } else {
        // Align file icons with folder icons (they have no chevron).
        spans.push(Span::raw("  "));
        let (icon, color) = icons::file_icon(&node.name, style, theme);
        spans.push(Span::styled(format!("{icon} "), Style::default().fg(color)));
    }

    let name_style = if selected {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else if is_active {
        Style::default()
            .fg(theme.accent2)
            .add_modifier(Modifier::BOLD)
    } else if is_root {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if node.is_dir {
        Style::default().fg(theme.fg_dim)
    } else {
        Style::default().fg(theme.fg)
    };
    spans.push(Span::styled(node.name.clone(), name_style));

    if is_active {
        spans.push(Span::styled(" ●", Style::default().fg(theme.green)));
    }

    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(theme.bg_sel));
    }
    line
}

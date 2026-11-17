//! Centered popups: key help, document outline and the theme menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Hit, Overlay};
use crate::ui::help;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    match app.overlay {
        Overlay::None => {}
        Overlay::Help { scroll } => render_help(frame, area, app, scroll),
        Overlay::Outline { selected } => render_outline(frame, area, app, selected),
        Overlay::Links { selected } => render_links(frame, area, app, selected),
        Overlay::FileMenu { selected } => render_file_menu(frame, area, app, selected),
        Overlay::Themes { selected, .. } => {
            crate::ui::menu::render_themes(frame, area, app, selected);
        }
        Overlay::Menu { selected } => crate::ui::menu::render_menu(frame, area, app, selected),
        Overlay::Options { tab, selected } => {
            crate::ui::menu::render_options(frame, area, app, tab, selected);
        }
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn popup_block<'a>(theme: &crate::ui::themes::Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

fn render_help(frame: &mut Frame, area: Rect, app: &App, scroll: usize) {
    if area.width < 24 || area.height < 8 {
        return;
    }
    let theme = app.theme();
    let width = area.width.saturating_sub(6).min(86).max(24);
    let height = area
        .height
        .saturating_sub(4)
        .min(help::rendered_line_count(&app.keymap) as u16 + 2);
    let rect = centered(area, width, height);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "MARKDOWN & CODE VIEWER",
        Style::default()
            .fg(theme.accent2)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    for section in help::sections(&app.keymap) {
        lines.push(Line::from(Span::styled(
            section.title.to_uppercase(),
            Style::default()
                .fg(theme.yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for (key, action) in section.entries.iter() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{key:<24}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(action.clone(), Style::default().fg(theme.fg_dim)),
            ]));
        }
        lines.push(Line::default());
    }

    lines.push(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::styled(" close    ", Style::default().fg(theme.fg_muted)),
        Span::styled("↑ / ↓", Style::default().fg(theme.accent)),
        Span::styled(" scroll    ", Style::default().fg(theme.fg_muted)),
        Span::styled("Home / End", Style::default().fg(theme.accent)),
        Span::styled(" top / bottom", Style::default().fg(theme.fg_muted)),
    ]));

    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Help — key bindings"))
        .style(Style::default().bg(theme.bg).fg(theme.fg))
        .scroll((scroll as u16, 0));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

fn render_outline(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width < 24 || area.height < 6 {
        return;
    }
    let theme = app.theme();
    let outline = app.viewer.outline();
    if outline.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(8).min(76).max(24);
    let height = area.height.saturating_sub(4).min(outline.len() as u16 + 2);
    let rect = centered(area, width, height);
    let inner_height = height.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(inner_height.saturating_sub(1));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_height);
    for (index, item) in outline.iter().enumerate().skip(offset).take(inner_height) {
        let is_selected = index == selected;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + lines.len() as u16, rect.width.saturating_sub(2), 1),
            Hit::Outline(index),
        );
        let indent = "  ".repeat(item.level.saturating_sub(1) as usize);
        let marker = if is_selected { "▍ " } else { "  " };
        let mut line = Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
            ),
            Span::styled(indent, Style::default().fg(theme.border)),
            Span::styled(
                item.title.clone(),
                if is_selected {
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg_dim)
                },
            ),
        ]);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let hint = Line::from(Span::styled(
        " Enter jump · Esc close ",
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Outline").title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

/// Chooser for a line with several links: number, label and target.
fn render_links(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width < 24 || area.height < 6 || app.link_choices.is_empty() {
        return;
    }
    let theme = app.theme();
    let choices = &app.link_choices;
    let width = area.width.saturating_sub(8).min(80).max(24);
    let height = area.height.saturating_sub(4).min(choices.len() as u16 + 2);
    let rect = centered(area, width, height);
    let inner_width = width.saturating_sub(2) as usize;
    let inner_height = height.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(inner_height.saturating_sub(1));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_height);
    for (index, link) in choices.iter().enumerate().skip(offset).take(inner_height) {
        let is_selected = index == selected;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + lines.len() as u16, rect.width.saturating_sub(2), 1),
            Hit::Link(index),
        );
        let number = if index < 9 { format!("{} ", index + 1) } else { "  ".to_string() };
        let label = if link.label.trim().is_empty() { link.dest.clone() } else { link.label.clone() };
        let used = 2 + 2 + label.chars().count() + 3;
        let room = inner_width.saturating_sub(used);
        let dest: String = if link.dest.chars().count() > room {
            let keep = room.saturating_sub(1);
            format!("{}…", link.dest.chars().take(keep).collect::<String>())
        } else {
            link.dest.clone()
        };
        let mut line = Line::from(vec![
            Span::styled(
                if is_selected { "▍ " } else { "  " },
                Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
            ),
            Span::styled(number, Style::default().fg(theme.fg_muted)),
            Span::styled(
                label,
                if is_selected {
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg_dim)
                },
            ),
            Span::styled(" → ", Style::default().fg(theme.border)),
            Span::styled(dest, Style::default().fg(theme.fg_muted)),
        ]);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let hint = Line::from(Span::styled(
        " Enter follow · 1-9 pick · Esc close ",
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Links on this line").title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

/// Actions for the file or folder selected in the explorer (Enter).
fn render_file_menu(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    let Some(menu) = app.file_menu.as_ref() else {
        return;
    };
    if area.width < 24 || area.height < 6 {
        return;
    }
    let theme = app.theme();
    let width = area.width.saturating_sub(8).min(44).max(24);
    let height = area.height.saturating_sub(2).min(menu.actions.len() as u16 + 2);
    let rect = centered(area, width, height);
    let inner_width = width.saturating_sub(2) as usize;
    let inner_height = height.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(inner_height.saturating_sub(1));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_height);
    for (index, action) in menu.actions.iter().enumerate().skip(offset).take(inner_height) {
        let is_selected = index == selected;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + lines.len() as u16, rect.width.saturating_sub(2), 1),
            Hit::FileAction(index),
        );
        let number = if index < 9 { format!("{} ", index + 1) } else { "  ".to_string() };
        let label = action.label(menu.is_dir);
        let used = 2 + 2 + label.chars().count();
        let mut line = Line::from(vec![
            Span::styled(
                if is_selected { "▍ " } else { "  " },
                Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
            ),
            Span::styled(number, Style::default().fg(theme.fg_muted)),
            Span::styled(
                label.to_string(),
                if is_selected {
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg_dim)
                },
            ),
            Span::raw(" ".repeat(inner_width.saturating_sub(used))),
        ]);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let title: String = {
        let kind = if menu.is_dir { "Folder" } else { "File" };
        let name: String = menu.name.chars().take(width.saturating_sub(12) as usize).collect();
        format!("{kind}: {name}")
    };
    let hint = Line::from(Span::styled(
        " Enter run · 1-9 pick · Esc close ",
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, &title).title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

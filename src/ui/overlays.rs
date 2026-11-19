//! Centered popups: key help, document outline, file finder and the theme menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Hit, ListTab, Overlay};
use crate::ui::help;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    match app.overlay {
        Overlay::None => {}
        Overlay::Help { scroll } => render_help(frame, area, app, scroll),
        Overlay::Outline { selected } => render_outline(frame, area, app, selected),
        Overlay::Finder { selected } => render_finder(frame, area, app, selected),
        Overlay::Bookmarks { tab, selected } => render_bookmarks(frame, area, app, tab, selected),
        Overlay::Backlinks { selected } => render_backlinks(frame, area, app, selected),
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
    let rect = if menu.anchored {
        file_menu_rect(area, app, width, height)
    } else {
        centered(area, width, height)
    };
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
        let kind = if menu.submenu {
            "Export"
        } else if menu.is_dir {
            "Folder"
        } else {
            "File"
        };
        let name: String = menu.name.chars().take(width.saturating_sub(12) as usize).collect();
        format!("{kind}: {name}")
    };
    let hint = Line::from(Span::styled(
        if menu.submenu {
            " Enter run · h html · p pdf · Esc back "
        } else {
            " Enter run · 1-9 pick · Esc close "
        },
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, &title).title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

/// Where the file actions menu goes: right under the selected explorer row
/// (above it when there is no room below), or centred when the explorer is
/// hidden.
fn file_menu_rect(area: Rect, app: &App, width: u16, height: u16) -> Rect {
    let Some(explorer) = app.layout.explorer else {
        return centered(area, width, height);
    };
    let row = explorer.y
        + app
            .explorer
            .selected_position()
            .saturating_sub(app.explorer.offset()) as u16;
    let below = row + 1;
    let y = if below + height <= area.bottom() {
        below
    } else {
        row.saturating_sub(height).max(area.y)
    };
    let x = (explorer.x + 2).min(area.right().saturating_sub(width)).max(area.x);
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Result rows the file finder shows at once (the list scrolls beyond them).
const FINDER_ROWS: u16 = 14;

/// Fuzzy file finder: input line, a divider with the counters, then results.
fn render_finder(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width < 30 || area.height < 8 {
        return;
    }
    let theme = app.theme();
    let finder = &app.finder;
    let width = area.width.saturating_sub(6).min(90).max(30);
    let rows = FINDER_ROWS.min(area.height.saturating_sub(6)).max(1);
    let height = rows + 4;
    let rect = centered(area, width, height);
    let inner_width = width.saturating_sub(2) as usize;
    let hits = finder.hits();
    let selected = selected.min(hits.len().saturating_sub(1));
    let offset = selected.saturating_sub(rows as usize - 1);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows as usize + 2);

    // Input.
    let mut input = vec![
        Span::styled(
            " > ",
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            finder.query.clone(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▏", Style::default().fg(theme.accent)),
    ];
    if finder.query.is_empty() {
        input.push(Span::styled(
            " type to search the project's files",
            Style::default().fg(theme.fg_muted),
        ));
    }
    lines.push(Line::from(input));

    // Divider with the counters (and the indexing indicator).
    let status = if finder.is_scanning() {
        format!(" {} indexing… {} files ", app.spinner(), finder.file_count())
    } else {
        format!(
            " {} / {}{} ",
            finder.match_count(),
            finder.file_count(),
            if finder.truncated() { "+" } else { "" }
        )
    };
    let dashes = inner_width.saturating_sub(status.chars().count() + 1);
    lines.push(Line::from(vec![
        Span::styled("─".repeat(dashes), Style::default().fg(theme.border)),
        Span::styled(
            status,
            Style::default().fg(if finder.is_scanning() { theme.yellow } else { theme.fg_muted }),
        ),
        Span::styled("─", Style::default().fg(theme.border)),
    ]));

    // Results.
    if hits.is_empty() {
        let text = if finder.is_scanning() {
            "  indexing…"
        } else if finder.query.is_empty() {
            "  no files in this project"
        } else {
            "  no matches"
        };
        lines.push(Line::from(Span::styled(
            text,
            Style::default().fg(theme.fg_muted),
        )));
    }
    for (index, hit) in hits.iter().enumerate().skip(offset).take(rows as usize) {
        let is_selected = index == selected;
        let row = lines.len() as u16;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + row, rect.width.saturating_sub(2), 1),
            Hit::Finder(index),
        );
        let mut spans = vec![Span::styled(
            if is_selected { "▍ " } else { "  " },
            Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
        )];
        spans.extend(finder_path_spans(
            &hit.path,
            &hit.positions,
            inner_width.saturating_sub(3),
            is_selected,
            &theme,
        ));
        let mut line = Line::from(spans);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let hint = Line::from(Span::styled(
        " ↑/↓ move · Enter open · Esc close ",
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Find file").title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

/// A result path as styled spans: the folders dim, the file name bright and
/// the characters that matched the query highlighted.  A path wider than
/// `room` loses its start (the file name is what matters), marked with `…`.
fn finder_path_spans(
    path: &str,
    positions: &[usize],
    room: usize,
    selected: bool,
    theme: &crate::ui::themes::Theme,
) -> Vec<Span<'static>> {
    let characters: Vec<char> = path.chars().collect();
    let name_start = characters
        .iter()
        .rposition(|character| *character == '/')
        .map_or(0, |slash| slash + 1);

    let (skip, ellipsis) = if characters.len() > room && room > 1 {
        (characters.len() - (room - 1), true)
    } else {
        (0, false)
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    if ellipsis {
        spans.push(Span::styled("…", Style::default().fg(theme.fg_muted)));
    }
    let mut run = String::new();
    let mut run_style = Style::default();
    for (index, character) in characters.iter().enumerate().skip(skip) {
        let matched = positions.contains(&index);
        let mut style = if index >= name_start {
            let base = Style::default().fg(theme.fg);
            if selected { base.add_modifier(Modifier::BOLD) } else { base }
        } else {
            Style::default().fg(theme.fg_muted)
        };
        if matched {
            style = Style::default()
                .fg(theme.yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        if style != run_style && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        run_style = style;
        run.push(*character);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    spans
}

/// Bookmarks and recent files: two tabs, one list.
fn render_bookmarks(frame: &mut Frame, area: Rect, app: &App, tab: ListTab, selected: usize) {
    if area.width < 30 || area.height < 8 {
        return;
    }
    let theme = app.theme();
    let width = area.width.saturating_sub(6).min(90).max(30);
    let rows = FINDER_ROWS.min(area.height.saturating_sub(6)).max(1);
    let rect = centered(area, width, rows + 4);
    let inner_width = width.saturating_sub(2) as usize;
    let count = app.list_len(tab);
    let selected = selected.min(count.saturating_sub(1));
    let offset = selected.saturating_sub(rows as usize - 1);
    let state = &app.project_state;

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Tabs (each one clickable).
    let mut x = rect.x + 1;
    let mut tab_spans = Vec::new();
    for (which, name, number) in [
        (ListTab::Bookmarks, "Bookmarks", state.bookmarks.len()),
        (ListTab::Recent, "Recent", state.recent.len()),
    ] {
        let label = format!(" {name} ({number}) ");
        let label_width = label.chars().count() as u16;
        app.record_hit(Rect::new(x, rect.y + 1, label_width, 1), Hit::BookmarkTab(which));
        let style = if which == tab {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        tab_spans.push(Span::styled(label, style));
        tab_spans.push(Span::raw(" "));
        x += label_width + 1;
    }
    lines.push(Line::from(tab_spans));
    lines.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Style::default().fg(theme.border),
    )));

    if count == 0 {
        let text = match tab {
            ListTab::Bookmarks => "  No bookmarks yet: press m on a line to add one.",
            ListTab::Recent => "  No recent files yet.",
        };
        lines.push(Line::from(Span::styled(text, Style::default().fg(theme.fg_muted))));
    }
    for index in (offset..count).take(rows as usize) {
        let is_selected = index == selected;
        let row = lines.len() as u16;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + row, rect.width.saturating_sub(2), 1),
            Hit::BookmarkRow(index),
        );
        let marker = Span::styled(
            if is_selected { "▍ " } else { "  " },
            Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
        );
        let room = inner_width.saturating_sub(3);
        let mut spans = vec![marker];
        match tab {
            ListTab::Bookmarks => {
                let bookmark = &state.bookmarks[index];
                let place = format!("{}:{}", bookmark.file, bookmark.line);
                let place = truncate_start(&place, room.saturating_sub(4).min(room * 2 / 3).max(8));
                let used = place.chars().count();
                spans.push(Span::styled(
                    place,
                    Style::default().fg(theme.fg).add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ));
                let rest = room.saturating_sub(used + 2);
                if rest > 3 && !bookmark.text.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", truncate_end(&bookmark.text, rest)),
                        Style::default().fg(theme.fg_muted),
                    ));
                }
            }
            ListTab::Recent => {
                let file = &state.recent[index];
                let shown = truncate_start(file, room);
                let name_start = shown.rfind('/').map_or(0, |slash| slash + 1);
                let (folder, name) = shown.split_at(name_start);
                spans.push(Span::styled(folder.to_string(), Style::default().fg(theme.fg_muted)));
                spans.push(Span::styled(
                    name.to_string(),
                    Style::default().fg(theme.fg).add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ));
            }
        }
        let mut line = Line::from(spans);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let hint = match tab {
        ListTab::Bookmarks => " Enter jump · d delete · Tab recent · Esc close ",
        ListTab::Recent => " Enter open · d forget · Tab bookmarks · Esc close ",
    };
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Bookmarks").title_bottom(Line::from(Span::styled(
            hint,
            Style::default().fg(theme.fg_muted),
        ))))
        .style(Style::default().bg(theme.bg).fg(theme.fg));
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

/// `text` cut to `max` characters, keeping the end and marking the cut.
fn truncate_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max < 2 {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - (max - 1)).collect();
    format!("…{tail}")
}

/// `text` cut to `max` characters, keeping the start and marking the cut.
fn truncate_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max < 2 {
        return text.to_string();
    }
    let head: String = text.chars().take(max - 1).collect();
    format!("{head}…")
}

/// Files that link to the open file.
fn render_backlinks(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width < 30 || area.height < 8 {
        return;
    }
    let theme = app.theme();
    let width = area.width.saturating_sub(6).min(90).max(30);
    let rows = FINDER_ROWS.min(area.height.saturating_sub(6)).max(1);
    let rect = centered(area, width, rows + 4);
    let inner_width = width.saturating_sub(2) as usize;
    let links = app.current_backlinks();
    let selected = selected.min(links.len().saturating_sub(1));
    let offset = selected.saturating_sub(rows as usize - 1);
    let target = app
        .viewer
        .loaded
        .as_ref()
        .and_then(|loaded| loaded.path.strip_prefix(&app.root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(" linked from → ", Style::default().fg(theme.fg_muted)),
        Span::styled(
            truncate_start(&target, inner_width.saturating_sub(16)),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ]));

    let scanning = app.backlinks.is_scanning() && !app.backlinks.is_ready();
    let status = if scanning {
        format!(" {} indexing… ", app.backlink_spinner())
    } else {
        let partial = if app.backlinks.is_partial() { ", partial" } else { "" };
        format!(" {} link{}{partial} ", links.len(), if links.len() == 1 { "" } else { "s" })
    };
    let dashes = inner_width.saturating_sub(status.chars().count() + 1);
    lines.push(Line::from(vec![
        Span::styled("─".repeat(dashes), Style::default().fg(theme.border)),
        Span::styled(
            status,
            Style::default().fg(if scanning { theme.yellow } else { theme.fg_muted }),
        ),
        Span::styled("─", Style::default().fg(theme.border)),
    ]));

    if links.is_empty() {
        let text = if scanning {
            "  indexing the project's links…"
        } else {
            "  No other file links to this one."
        };
        lines.push(Line::from(Span::styled(text, Style::default().fg(theme.fg_muted))));
    }
    for (index, link) in links.iter().enumerate().skip(offset).take(rows as usize) {
        let is_selected = index == selected;
        let row = lines.len() as u16;
        app.record_hit(
            Rect::new(rect.x + 1, rect.y + 1 + row, rect.width.saturating_sub(2), 1),
            Hit::Backlink(index),
        );
        let room = inner_width.saturating_sub(3);
        let place = format!("{}:{}", link.source, link.line + 1);
        let place = truncate_start(&place, (room * 2 / 3).max(8));
        let used = place.chars().count();
        let mut spans = vec![
            Span::styled(
                if is_selected { "▍ " } else { "  " },
                Style::default().fg(if is_selected { theme.accent } else { theme.bg }),
            ),
            Span::styled(
                place,
                Style::default().fg(theme.fg).add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ];
        let rest = room.saturating_sub(used + 2);
        if rest > 3 && !link.text.is_empty() {
            spans.push(Span::styled(
                format!("  {}", truncate_end(&link.text, rest)),
                Style::default().fg(theme.fg_muted),
            ));
        }
        let mut line = Line::from(spans);
        if is_selected {
            line = line.style(Style::default().bg(theme.bg_sel));
        }
        lines.push(line);
    }

    let hint = Line::from(Span::styled(
        " ↑/↓ move · Enter open at the link · Esc close ",
        Style::default().fg(theme.fg_muted),
    ));
    let paragraph = Paragraph::new(lines)
        .block(popup_block(&theme, "Backlinks").title_bottom(hint))
        .style(Style::default().bg(theme.bg).fg(theme.fg));
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
    app.record_hit(rect, Hit::Panel);
}

//! Bottom status bar: focus, messages, search prompt and document position.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus};
use crate::keymap::Action;
use crate::ui::bar;
use crate::viewer::format_size;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();

    // The editor owns the document pane while it is open; the terminal owns
    // the keyboard while it has focus.
    let editing = app.editor.is_some() && app.focus == Focus::Viewer;
    let pending = app.pending.map(|(chord, _)| chord);
    let badge_color = if pending.is_some() {
        theme.yellow
    } else if editing {
        theme.green
    } else {
        match app.focus {
            Focus::Explorer => theme.accent,
            Focus::Viewer => theme.accent2,
            Focus::Terminal => theme.purple,
        }
    };
    let badge = if pending.is_some() {
        format!(" {} ", app.keymap.prefix_label().to_uppercase())
    } else if editing {
        " EDIT ".to_string()
    } else {
        format!(" {} ", app.focus.label().to_uppercase())
    };
    let mut left = vec![Span::styled(
        badge,
        Style::default()
            .fg(theme.bg)
            .bg(badge_color)
            .add_modifier(Modifier::BOLD),
    )];

    if app.indexing() {
        left.push(Span::styled(
            format!(" {} indexing…", app.spinner()),
            Style::default().fg(theme.yellow),
        ));
    }

    if let Some(first) = pending {
        // Waiting for the second key of a sequence: list what it can be.
        let options = app
            .keymap
            .follow_ups(first)
            .into_iter()
            .map(|(label, action)| format!("{label} {}", action.short()))
            .collect::<Vec<_>>()
            .join("  ");
        left.push(Span::styled(format!("  {options}"), theme.dim()));
        left.push(Span::styled("  · any other key cancels", theme.muted()));
    } else if let (true, Some(editor)) = (editing, &app.editor) {
        let relative = editor
            .path
            .strip_prefix(&app.root)
            .unwrap_or(&editor.path)
            .display()
            .to_string();
        left.push(Span::styled(
            format!("  {} · {relative}", editor.program),
            theme.dim(),
        ));
        left.push(Span::styled(
            "  :wq save and go back · :q! discard",
            theme.muted(),
        ));
    } else if app.focus == Focus::Terminal && app.term.is_some() {
        let program = app
            .term
            .as_ref()
            .map(|term| term.program.clone())
            .unwrap_or_default();
        left.push(Span::styled(format!("  {program}"), theme.dim()));
        if let Some(status) = app.status_text() {
            left.push(Span::styled(format!("  {status}"), theme.muted()));
        }
    } else if let Some(input) = &app.search_input {
        left.push(Span::styled("  /", Style::default().fg(theme.yellow)));
        left.push(Span::styled(
            input.clone(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        left.push(Span::styled("▏", Style::default().fg(theme.accent)));
    } else if let Some(status) = app.status_text() {
        left.push(Span::styled(format!("  {status}"), theme.dim()));
    } else if let Some(loaded) = &app.viewer.loaded {
        let relative = loaded
            .path
            .strip_prefix(&app.root)
            .unwrap_or(&loaded.path)
            .display()
            .to_string();
        left.push(Span::styled(format!("  {relative}"), theme.dim()));
    } else {
        left.push(Span::styled("  no file open", theme.muted()));
    }

    let mut right = Vec::new();
    if app.watcher_problem().is_some() {
        right.push(Span::styled(
            " no live refresh ",
            Style::default().fg(theme.orange),
        ));
    }
    if app.explorer.show_hidden() {
        right.push(Span::styled(" hidden ", Style::default().fg(theme.yellow)));
    }
    if !app.explorer.respect_gitignore() {
        right.push(Span::styled(
            " no-gitignore ",
            Style::default().fg(theme.orange),
        ));
    }
    if let Some(search) = &app.viewer.search {
        if !search.matches.is_empty() {
            let current = search.current.map(|index| index + 1).unwrap_or(0);
            right.push(Span::styled(
                format!(" {}/{} ", current, search.matches.len()),
                Style::default().fg(theme.yellow),
            ));
        }
    }
    if app.viewer.has_document() {
        if let Some(loaded) = &app.viewer.loaded {
            right.push(Span::styled(
                format!(" {} ", format_size(loaded.bytes)),
                theme.muted(),
            ));
        }
        let total = app.viewer.total_lines().max(1);
        let line = (app.viewer.cursor + 1).min(total);
        right.push(Span::styled(
            format!(" {line}/{total} "),
            Style::default().fg(theme.fg_dim),
        ));
    }
    if app.pane_captured() {
        // Esc, e and ? belong to the program in the pane: only the prefix
        // key is available, so that is the hint worth showing.
        right.push(Span::styled(
            format!(" {} prefix ", app.keymap.prefix_label()),
            theme.muted(),
        ));
    } else {
        right.extend(shortcut_hints(app));
    }

    let line = bar::build(left, right, area.width);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg_alt).fg(theme.fg_dim)),
        area,
    );
}

/// `edit` with a bold `e`, then `Esc` and `?` on their own.
fn shortcut_hints(app: &App) -> Vec<Span<'static>> {
    let theme = app.theme();
    let key = |action: Action| {
        app.keymap
            .primary(action)
            .map(|chord| chord.label())
            .unwrap_or_else(|| app.keymap.describe(action, 1))
    };
    let bold = Style::default().fg(theme.fg).add_modifier(Modifier::BOLD);

    let mut spans = Vec::new();
    let edit = key(Action::Edit);
    spans.push(Span::styled(" ", theme.muted()));
    if edit.chars().count() == 1 && "edit".starts_with(&edit.to_lowercase()) {
        // The key is the first letter: highlight it inside the word.
        spans.push(Span::styled(edit.clone(), bold));
        spans.push(Span::styled("dit ", theme.muted()));
    } else {
        spans.push(Span::styled(edit, bold));
        spans.push(Span::styled(" edit ", theme.muted()));
    }
    spans.push(Span::styled(format!(" {} ", key(Action::Menu)), theme.muted()));
    spans.push(Span::styled(format!(" {} ", key(Action::Help)), theme.muted()));
    spans
}

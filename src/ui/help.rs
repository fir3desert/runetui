//! Key binding reference shown by the `?` overlay.
//!
//! Shortcuts come from the live [`Keymap`], so the help always shows what is
//! actually configured.  Navigation keys that cannot be rebound are static.

use crate::keymap::{Action, Keymap};

pub struct HelpSection {
    pub title: &'static str,
    pub entries: Vec<(String, String)>,
}

fn entry(keymap: &Keymap, action: Action, description: &str) -> (String, String) {
    (keymap.describe(action, 2), description.to_string())
}

fn fixed(keys: &str, description: &str) -> (String, String) {
    (keys.to_string(), description.to_string())
}

pub fn sections(keymap: &Keymap) -> Vec<HelpSection> {
    let prefix = keymap.prefix_label();
    vec![
        HelpSection {
            title: "Panels",
            entries: vec![
                entry(keymap, Action::FocusNext, "Switch focus between explorer, document and terminal"),
                entry(keymap, Action::FocusPrev, "Switch focus backwards"),
                entry(keymap, Action::ToggleExplorer, "Show or hide the explorer"),
                entry(keymap, Action::ToggleTerminal, "Show or hide the terminal"),
                entry(keymap, Action::ExplorerShrink, "Shrink the explorer (left panel)"),
                entry(keymap, Action::ExplorerGrow, "Grow the explorer (left panel)"),
                entry(keymap, Action::TerminalGrow, "Grow the terminal (bottom panel)"),
                entry(keymap, Action::TerminalShrink, "Shrink the terminal (bottom panel)"),
                entry(keymap, Action::ForceClose, "Emergency: kill a hung editor or terminal"),
                fixed(
                    "← / →",
                    "Tree: go to parent / expand or descend · Document: ← to explorer",
                ),
            ],
        },
        HelpSection {
            title: "Prefix (like tmux)",
            entries: vec![
                fixed(
                    &prefix,
                    "Press it, then a key: ← → resize the explorer, ↑ ↓ the terminal",
                ),
                fixed(
                    &format!("{prefix} {prefix}"),
                    "Send the prefix key itself to the terminal or editor",
                ),
                fixed(
                    "Terminal / editor",
                    "While one has focus only the prefix is intercepted",
                ),
            ],
        },
        HelpSection {
            title: "Explorer",
            entries: vec![
                fixed("↑ / ↓", "Move selection (files preview automatically)"),
                fixed("Space", "Open a file, or expand/collapse a folder"),
                fixed("Enter", "Actions: open, VS Code, terminal, default app, edit, copy path"),
                fixed("Home / End", "Jump to the first / last entry"),
                fixed("PageUp / PageDown", "Move a page up / down"),
                entry(keymap, Action::Edit, "Edit the selected file inside runetui (vim)"),
                entry(keymap, Action::ToggleHidden, "Toggle hidden files"),
                entry(keymap, Action::ToggleGitignore, "Toggle .gitignore filtering"),
                entry(keymap, Action::Refresh, "Refresh the project tree"),
                entry(keymap, Action::CycleIcons, "Cycle icon style (nerd / unicode / ascii)"),
            ],
        },
        HelpSection {
            title: "Document",
            entries: vec![
                fixed("↑ / ↓", "Move cursor line up / down"),
                fixed("Home / End", "Jump to the start / end"),
                entry(keymap, Action::HalfPageDown, "Scroll half a page down"),
                entry(keymap, Action::HalfPageUp, "Scroll half a page up"),
                fixed("PageUp / PageDown", "Scroll a page up / down"),
                fixed("Enter", "Follow link, anchor or [[wikilink]]"),
                entry(keymap, Action::FollowLink, "Follow the link on the line (http opens the browser)"),
                entry(keymap, Action::Search, "Search in the document"),
                entry(keymap, Action::SearchNext, "Next search match"),
                entry(keymap, Action::SearchPrev, "Previous search match"),
                entry(keymap, Action::Outline, "Open the document outline"),
                entry(keymap, Action::HistoryBack, "History back"),
                entry(keymap, Action::HistoryForward, "History forward"),
                entry(keymap, Action::Edit, "Edit the current file inside runetui (vim)"),
            ],
        },
        HelpSection {
            title: "Mouse",
            entries: vec![
                fixed("Click", "Focus the panel under the pointer"),
                fixed("Drag", "Select text in one panel only and copy it (OSC 52)"),
                fixed("Wheel", "Scroll the panel under the pointer"),
            ],
        },
        HelpSection {
            title: "Themes",
            entries: vec![
                entry(keymap, Action::Menu, "Open the menu: OPTIONS, HELP, QUIT"),
                fixed("Options → Theme", "Pick a theme with a live preview"),
                fixed("Enter", "Apply the selected theme"),
                entry(keymap, Action::CycleTheme, "Cycle the colour theme"),
            ],
        },
        HelpSection {
            title: "General",
            entries: vec![
                entry(keymap, Action::Export, "Export the open document to HTML or PDF (also: Enter on a file, Export...)"),
                entry(keymap, Action::CopyMarkdown, "Copy the source Markdown behind the text selected with the mouse"),
                entry(keymap, Action::Backlinks, "Files that link to the open file (Enter opens one at the link)"),
                entry(keymap, Action::Bookmark, "Bookmark the current line (again to remove it)"),
                entry(keymap, Action::Bookmarks, "Bookmarks and recent files (Tab switches, Enter opens, d deletes)"),
                entry(keymap, Action::FindFile, "Find a file: fuzzy search of the project (↑/↓ choose, Enter opens, Esc closes)"),
                entry(keymap, Action::Help, "Show or hide this help"),
                entry(keymap, Action::Menu, "Open the menu, or go back / close an overlay or search"),
                fixed("In the editor", "Keys go to vim; :wq saves and returns here"),
                entry(keymap, Action::Quit, "Quit runetui"),
                fixed("config.toml", "[keys] rebinds any shortcut above; `prefix` sets the prefix key"),
            ],
        },
    ]
}

/// Total rendered lines, including section headings and blank separators.
pub fn rendered_line_count(keymap: &Keymap) -> usize {
    let mut count = 2; // title + blank line
    for section in sections(keymap) {
        count += 1 + section.entries.len() + 1;
    }
    count
}

/// The whole help text, for tests.
#[cfg(test)]
pub fn rendered_text(keymap: &Keymap) -> String {
    sections(keymap)
        .into_iter()
        .flat_map(|section| section.entries)
        .map(|(keys, text)| format!("{keys} {text}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_count_covers_every_entry() {
        let keymap = Keymap::default();
        let entries: usize = sections(&keymap)
            .iter()
            .map(|section| section.entries.len())
            .sum();
        assert!(rendered_line_count(&keymap) > entries);
    }

    #[test]
    fn help_shows_the_configured_bindings() {
        let keymap = Keymap::default();
        let all: Vec<(String, String)> = sections(&keymap)
            .into_iter()
            .flat_map(|section| section.entries)
            .collect();
        assert!(all.iter().any(|(keys, text)| keys.contains("Ctrl+R ←") && text.contains("Shrink the explorer")));
        assert!(all.iter().any(|(keys, _)| keys.contains("Ctrl+T")));
    }
}

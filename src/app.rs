//! Application state, key handling and navigation.
//!
//! Everything here is synchronous and in-memory: files are loaded for display
//! only. Editing runs the user's editor inside the document pane (see
//! `crate::editor`), falling back to an external editor request when the
//! embedded one cannot start. The app owns the explorer, the viewer, the active theme
//! and a small amount of UI state (focus, overlays, history, search prompt).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ignore::WalkBuilder;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::config::{self, Config};
use crate::editor::{EmbeddedEditor, PtyPane};
use crate::filesystem::Explorer;
use crate::filesystem::watcher::FileWatcher;
use crate::keymap::{Action, Chord, DEFAULT_PREFIX, Keymap, Resolution};
use crate::stars::Stars;
use crate::markdown::LinkKind;
use crate::markdown::code_highlighter::CodeHighlighter;
use crate::markdown::toc;
use crate::ui::help;
use crate::selection::Selection;
use crate::ui::layout::{Layout, MAX_TERMINAL_PCT, MIN_TERMINAL_PCT};
use crate::ui::themes::{Theme, ThemeName};
use crate::viewer::{LoadedDoc, Viewer, load_document};

const STATUS_TTL: Duration = Duration::from_secs(5);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(180);
const HISTORY_LIMIT: usize = 100;
const WIKI_SCAN_LIMIT: usize = 25_000;
/// How long a pending prefix key waits for the key that completes it.
const PREFIX_TIMEOUT: Duration = Duration::from_secs(3);
/// Lines moved by one mouse-wheel notch.
const WHEEL_LINES: isize = 3;
/// Step of the explorer width (percent) and the terminal height (percent).
pub const EXPLORER_STEP: i16 = 2;
pub const TERMINAL_STEP: i16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Explorer,
    Viewer,
    Terminal,
}

impl Focus {
    pub fn label(self) -> &'static str {
        match self {
            Focus::Explorer => "explorer",
            Focus::Viewer => "document",
            Focus::Terminal => "terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help {
        scroll: usize,
    },
    Outline {
        selected: usize,
    },
    /// btop-style theme chooser. `original` is restored when the menu is
    /// cancelled with Esc.
    Themes {
        selected: usize,
        original: ThemeName,
    },
    /// Main menu opened with Esc (btop / mctui style): OPTIONS, HELP, QUIT.
    Menu {
        selected: usize,
    },
    /// Settings screen: tabs, a list of options and a description panel.
    Options {
        tab: usize,
        selected: usize,
    },
}

/// Entries of the Esc menu, in display order.
pub const MENU_ITEMS: [&str; 3] = ["OPTIONS", "HELP", "QUIT"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionItem {
    Theme,
    Icons,
    ShowExplorer,
    ShowTerminal,
    Mouse,
    Keys,
    Animation,
    GithubStars,
    ShowHidden,
    Gitignore,
    ExplorerWidth,
    EmbeddedEditor,
    EditorCommand,
    TerminalHeight,
    TerminalShell,
}

impl OptionItem {
    /// Items changed by pressing Enter and then `+` / `-`.
    pub fn is_adjustable(self) -> bool {
        matches!(self, OptionItem::ExplorerWidth | OptionItem::TerminalHeight)
    }
}

pub struct OptionTab {
    pub name: &'static str,
    pub items: &'static [OptionItem],
}

pub const OPTION_TABS: [OptionTab; 4] = [
    OptionTab {
        name: "General",
        items: &[
            OptionItem::Theme,
            OptionItem::Icons,
            OptionItem::ShowExplorer,
            OptionItem::ShowTerminal,
            OptionItem::Mouse,
            OptionItem::Keys,
            OptionItem::Animation,
            OptionItem::GithubStars,
        ],
    },
    OptionTab {
        name: "Explorer",
        items: &[
            OptionItem::ShowHidden,
            OptionItem::Gitignore,
            OptionItem::ExplorerWidth,
        ],
    },
    OptionTab {
        name: "Editor",
        items: &[OptionItem::EmbeddedEditor, OptionItem::EditorCommand],
    },
    OptionTab {
        name: "Terminal",
        items: &[OptionItem::TerminalHeight, OptionItem::TerminalShell],
    },
];

impl Overlay {
    pub fn is_open(self) -> bool {
        !matches!(self, Overlay::None)
    }
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub at: Instant,
}

impl StatusMessage {
    pub fn is_expired(&self) -> bool {
        self.at.elapsed() > STATUS_TTL
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    path: PathBuf,
    cursor: usize,
}

pub struct App {
    pub root: PathBuf,
    pub explorer: Explorer,
    pub viewer: Viewer,
    pub theme: ThemeName,
    pub focus: Focus,
    pub show_explorer: bool,
    pub overlay: Overlay,
    pub search_input: Option<String>,
    pub status: Option<StatusMessage>,
    pub layout: Layout,
    pub area: Rect,
    pub should_quit: bool,
    pub config: Config,
    pub watcher: Option<FileWatcher>,
    /// Editor running inside the document pane, if any.
    pub editor: Option<EmbeddedEditor>,
    /// Shell running in the terminal panel, if it has been started.
    pub term: Option<PtyPane>,
    /// Whether the terminal panel is shown.
    pub show_term: bool,
    /// Terminal panel height as a percentage of the body.
    pub term_pct: u16,
    pub keymap: Keymap,
    /// First key of a two-key shortcut that is waiting for the second one.
    pub pending: Option<(Chord, Instant)>,
    /// An option (explorer width, terminal height) is being adjusted with +/-.
    pub adjusting: bool,
    /// Text selection confined to one pane.
    pub selection: Option<Selection>,
    /// Options → Key bindings: the next key press becomes the prefix.
    pub capturing_prefix: bool,
    /// GitHub star count shown in the Esc menu.
    pub stars: Stars,

    selection_copied: bool,
    clipboard: Option<String>,
    term_failed: bool,
    menu_selected: usize,
    /// When the Esc menu was opened from the interface; drives the banner animation.
    menu_opened: Option<Instant>,
    overlay_stack: Vec<Overlay>,
    highlighter: CodeHighlighter,
    history: Vec<HistoryEntry>,
    forward: Vec<HistoryEntry>,
    editor_request: Option<PathBuf>,
    refresh_pending: Option<Instant>,
    reload_pending: bool,
}

impl App {
    pub fn with_config(cli_path: Option<&Path>, mut config: Config) -> Self {
        let root = config::resolve_root(cli_path, &config);
        let initial_file = config::resolve_initial_file(cli_path);
        let theme = config.theme_name();
        let keymap = Keymap::from_config(&config.prefix, &config.keys);
        let term_pct = config
            .terminal_height
            .clamp(MIN_TERMINAL_PCT, MAX_TERMINAL_PCT);
        let mut explorer = Explorer::new(
            &root,
            config.explorer_width,
            config.show_hidden,
            config.respect_gitignore,
        );

        let mut app = Self {
            root: root.clone(),
            explorer: {
                explorer.set_width_pct(config.explorer_width);
                explorer
            },
            viewer: Viewer::default(),
            theme,
            focus: Focus::Explorer,
            show_explorer: config.show_explorer,
            overlay: Overlay::None,
            search_input: None,
            status: None,
            layout: Layout::default(),
            area: Rect::new(0, 0, 100, 30),
            should_quit: false,
            config: config.clone(),
            watcher: FileWatcher::new(&root).ok(),
            editor: None,
            term: None,
            show_term: config.show_terminal,
            term_pct,
            keymap,
            pending: None,
            adjusting: false,
            selection: None,
            capturing_prefix: false,
            stars: Stars::start(config.show_stars),
            selection_copied: false,
            clipboard: None,
            term_failed: false,
            menu_selected: 0,
            menu_opened: None,
            overlay_stack: Vec::new(),
            highlighter: CodeHighlighter::new(),
            history: Vec::new(),
            forward: Vec::new(),
            editor_request: None,
            refresh_pending: None,
            reload_pending: false,
        };

        config.last_directory = Some(root.clone());
        app.config = config;

        if let Some(file) = initial_file {
            app.open_file(file, None, true);
        } else {
            let readme = root.join("README.md");
            if readme.is_file() {
                app.open_file(readme, None, true);
            } else {
                app.show_welcome();
            }
        }
        // Start in the explorer so the project can be browsed immediately.
        app.focus = if app.layout.explorer.is_some() {
            Focus::Explorer
        } else {
            Focus::Viewer
        };
        if let Some(warning) = app.keymap.warnings.first().cloned() {
            app.set_status(warning);
        }
        app
    }

    pub fn theme(&self) -> Theme {
        self.theme.theme()
    }

    /// Width used for rendering the document.
    pub fn content_width(&self) -> usize {
        let width = self.layout.viewer.width as usize;
        if width < 16 { 80 } else { width }
    }

    /// Recalculate pane sizes for the current terminal area and reflow the
    /// document when its width or theme changed.
    pub fn set_terminal_area(&mut self, area: Rect) {
        self.area = area;
        let terminal_pct = self.show_term.then_some(self.term_pct);
        self.layout = Layout::compute(
            area,
            self.show_explorer,
            self.explorer.width_pct(),
            terminal_pct,
        );
        if self.layout.terminal.is_none() && self.focus == Focus::Terminal {
            self.focus = Focus::Viewer;
        }

        self.viewer
            .set_viewport_height(self.layout.viewer.height as usize);
        if let Some(explorer) = self.layout.explorer {
            self.explorer.set_viewport_height(explorer.height as usize);
        } else {
            self.explorer.set_viewport_height(18);
            if self.focus == Focus::Explorer {
                self.focus = Focus::Viewer;
            }
        }

        let width = self.layout.viewer.width as usize;
        if width >= 16
            && (self.viewer.rendered_width != width
                || self.viewer.rendered_theme != Some(self.theme))
        {
            let theme = self.theme();
            self.viewer.rerender(width, theme, &mut self.highlighter);
        }
    }

    /// Highlight the next chunk of a long source file when the reader is close
    /// to the end of what has been highlighted so far.
    pub fn fill_viewer(&mut self) {
        if self.viewer.needs_more() {
            let theme = self.theme();
            self.viewer.load_more(theme, &mut self.highlighter);
        }
    }

    /// Highlight the whole document (search, jump to the end).
    fn load_all_viewer(&mut self) {
        if self.viewer.is_partial() {
            let theme = self.theme();
            self.viewer.load_all(theme, &mut self.highlighter);
        }
    }

    /// Move the viewer cursor to a rendered row, loading what is needed to get there.
    fn jump_viewer_to(&mut self, line: usize) {
        let theme = self.theme();
        self.viewer.load_through(line, theme, &mut self.highlighter);
        self.viewer.jump_to_line(line);
    }

    pub fn show_welcome(&mut self) {
        let name = self.explorer.root_name();
        let path = self.root.display().to_string();
        let width = self.content_width();
        let theme = self.theme();
        self.viewer
            .show_welcome(&name, &path, width, theme, &mut self.highlighter);
    }

    // ------------------------------------------------------------- documents

    /// Load a file into the viewer without changing which pane has focus.
    fn load_into_viewer(
        &mut self,
        path: PathBuf,
        anchor: Option<String>,
        record_history: bool,
    ) -> bool {
        if path.is_dir() {
            self.focus_folder(&path);
            return false;
        }
        if !path.exists() {
            self.set_status(format!("Not found: {}", path.display()));
            return false;
        }

        if record_history {
            self.push_history();
            self.forward.clear();
        }

        let loaded = match load_document(&path) {
            Ok(loaded) => loaded,
            Err(error) => LoadedDoc::error(path.clone(), error.to_string()),
        };
        let width = self.content_width();
        let theme = self.theme();
        self.viewer.show(
            Arc::new(loaded),
            width,
            theme,
            &mut self.highlighter,
            anchor.as_deref(),
        );
        self.explorer.select_path(&path);
        true
    }

    fn open_file(&mut self, path: PathBuf, anchor: Option<String>, record_history: bool) {
        if self.load_into_viewer(path, anchor, record_history) {
            self.focus = Focus::Viewer;
        }
    }

    /// Preview the explorer's current selection while focus stays in the tree.
    fn preview_selection(&mut self) {
        let Some(path) = self.explorer.selected_path() else {
            return;
        };
        if !path.is_file() {
            return;
        }
        if self
            .viewer
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.path == path)
        {
            return;
        }
        self.load_into_viewer(path, None, false);
    }

    fn focus_folder(&mut self, path: &Path) {
        if self.explorer.select_path(path) {
            self.show_explorer();
            self.focus = Focus::Explorer;
            self.set_status(format!("Folder: {}", path.display()));
        }
    }

    /// Reload the current document from disk, preserving the cursor position.
    pub fn reload_current(&mut self) {
        let Some(loaded) = self.viewer.loaded.clone() else {
            self.show_welcome();
            return;
        };
        let path = loaded.path.clone();
        if !path.is_file() {
            return;
        }
        let cursor = self.viewer.cursor;
        let query = self
            .viewer
            .search
            .as_ref()
            .map(|search| search.query.clone())
            .filter(|query| !query.is_empty());
        let width = self.content_width();
        let theme = self.theme();

        match load_document(&path) {
            Ok(reloaded) => {
                self.viewer.show(
                    Arc::new(reloaded),
                    width,
                    theme,
                    &mut self.highlighter,
                    None,
                );
                self.jump_viewer_to(cursor);
                if let Some(query) = query {
                    self.load_all_viewer();
                    self.viewer.apply_search(&query);
                }
                self.set_status("Document reloaded");
            }
            Err(error) => {
                self.viewer.show(
                    Arc::new(LoadedDoc::error(path, error.to_string())),
                    width,
                    theme,
                    &mut self.highlighter,
                    None,
                );
                self.set_status("The document could not be reloaded");
            }
        }
    }

    // ---------------------------------------------------------------- status

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            text: text.into(),
            at: Instant::now(),
        });
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|message| !message.is_expired())
            .map(|message| message.text.as_str())
    }

    // -------------------------------------------------------------- periodic

    /// Called on every loop iteration.  Drains filesystem events, debounces
    /// refreshes, and expires transient messages.
    pub fn tick(&mut self) {
        self.fill_viewer();
        self.stars.poll();
        self.poll_editor();
        self.poll_term();
        if self
            .pending
            .is_some_and(|(_, since)| since.elapsed() >= PREFIX_TIMEOUT)
        {
            self.pending = None;
        }
        if let Some(watcher) = &self.watcher {
            if let Some(event) = watcher.drain() {
                if let Some(current) = &self.viewer.loaded {
                    if event.touches(&current.path) {
                        self.reload_pending = true;
                    }
                }
                self.refresh_pending = Some(Instant::now());
            }
        }

        if self
            .refresh_pending
            .is_some_and(|since| since.elapsed() >= WATCH_DEBOUNCE)
        {
            self.refresh_pending = None;
            self.explorer.refresh();
            if std::mem::take(&mut self.reload_pending) {
                self.reload_current();
            }
        }

        if self.status.as_ref().is_some_and(StatusMessage::is_expired) {
            self.status = None;
        }
    }

    // ---------------------------------------------------------------- themes

    /// Apply a theme immediately and persist the choice in the configuration.
    fn apply_theme(&mut self, name: ThemeName) {
        self.theme = name;
        self.config.set_theme(name);
        let width = self.content_width();
        let theme = self.theme();
        self.viewer.rerender(width, theme, &mut self.highlighter);
    }

    pub fn cycle_theme(&mut self) {
        self.apply_theme(self.theme.next());
        self.set_status(format!("Theme: {}", self.theme.label()));
    }

    /// Open the btop-style theme chooser with the current theme selected.
    /// Open the Esc menu as the root of a fresh overlay stack.
    fn open_menu(&mut self) {
        self.overlay_stack.clear();
        self.menu_opened = Some(Instant::now());
        self.overlay = Overlay::Menu {
            selected: self.menu_selected,
        };
    }

    /// Time since the Esc menu opened while its banner animation is still
    /// running.  `None` once it has finished, or when the menu was reached by
    /// going back from another screen: then the banner is static.
    pub fn menu_animation(&self) -> Option<Duration> {
        if !matches!(self.overlay, Overlay::Menu { .. }) || !self.config.banner_animation {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(crate::ui::menu::ANIMATION_MS))
    }

    /// Test helper: jump to the end of the menu banner animation.
    #[cfg(test)]
    pub fn finish_menu_animation(&mut self) {
        self.menu_opened = None;
    }

    /// Open `next` on top of the current overlay so Esc returns to it.
    fn push_overlay(&mut self, next: Overlay) {
        // Coming back to the menu later shows the banner without animation.
        self.menu_opened = None;
        if self.overlay.is_open() {
            self.overlay_stack.push(self.overlay);
        }
        self.overlay = next;
    }

    /// Leave the current overlay, going back to the one that opened it.
    fn close_overlay(&mut self) {
        self.adjusting = false;
        self.capturing_prefix = false;
        self.overlay = self.overlay_stack.pop().unwrap_or(Overlay::None);
    }

    fn open_theme_menu(&mut self) {
        let selected = ThemeName::ALL
            .iter()
            .position(|name| *name == self.theme)
            .unwrap_or(0);
        self.push_overlay(Overlay::Themes {
            selected,
            original: self.theme,
        });
    }

    pub fn toggle_explorer(&mut self) {
        self.show_explorer = !self.show_explorer;
        self.config.show_explorer = self.show_explorer;
        if !self.show_explorer {
            self.focus = Focus::Viewer;
        } else {
            self.focus = Focus::Explorer;
        }
        self.set_terminal_area(self.area);
        self.set_status(if self.show_explorer {
            "Explorer shown"
        } else {
            "Explorer hidden"
        });
    }

    fn show_explorer(&mut self) {
        if !self.show_explorer {
            self.show_explorer = true;
            self.config.show_explorer = true;
            self.set_terminal_area(self.area);
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.explorer.toggle_hidden();
        self.config.show_hidden = self.explorer.show_hidden();
        self.set_status(if self.explorer.show_hidden() {
            "Hidden files shown"
        } else {
            "Hidden files hidden"
        });
    }

    pub fn toggle_gitignore(&mut self) {
        self.explorer.toggle_gitignore();
        self.config.respect_gitignore = self.explorer.respect_gitignore();
        self.set_status(if self.explorer.respect_gitignore() {
            ".gitignore rules enabled"
        } else {
            ".gitignore rules disabled"
        });
    }

    pub fn cycle_icons(&mut self) {
        let next = self.config.icon_style().next();
        self.config.set_icon_style(next);
        self.set_status(format!("Icons: {}", next.slug()));
    }

    fn resize_explorer(&mut self, delta: i16) {
        self.explorer.resize(delta);
        self.config.explorer_width = self.explorer.width_pct();
        self.set_terminal_area(self.area);
        self.set_status(format!("Explorer width: {}%", self.explorer.width_pct()));
    }

    fn resize_terminal(&mut self, delta: i16) {
        let next = (self.term_pct as i16 + delta)
            .clamp(MIN_TERMINAL_PCT as i16, MAX_TERMINAL_PCT as i16) as u16;
        self.term_pct = next;
        self.config.terminal_height = next;
        self.set_terminal_area(self.area);
        self.set_status(if self.show_term {
            format!("Terminal height: {next}%")
        } else {
            format!("Terminal height: {next}% (terminal hidden)")
        });
    }

    /// Show or hide the terminal panel.  The shell keeps running while the
    /// panel is hidden.  `take_focus` moves the keyboard to the shell when it
    /// is shown.
    pub fn toggle_terminal(&mut self, take_focus: bool) {
        self.show_term = !self.show_term;
        self.config.show_terminal = self.show_term;
        self.term_failed = false;
        self.selection = None;
        self.set_terminal_area(self.area);
        if self.show_term {
            if self.layout.terminal.is_none() {
                self.set_status("Not enough room for the terminal");
                return;
            }
            if take_focus {
                self.focus = Focus::Terminal;
            }
            self.set_status("Terminal shown");
        } else {
            if self.focus == Focus::Terminal {
                self.focus = Focus::Viewer;
            }
            self.set_status("Terminal hidden");
        }
    }

    /// Start the shell the first time the panel is visible.  Called from the
    /// event loop rather than `tick` so headless tests never spawn a shell.
    pub fn ensure_terminal(&mut self) {
        if !self.show_term || self.term.is_some() || self.term_failed {
            return;
        }
        let Some(inner) = self.layout.terminal_inner() else {
            return;
        };
        match PtyPane::spawn_shell(&self.root, inner.height, inner.width) {
            Ok(pane) => self.term = Some(pane),
            Err(error) => {
                self.term_failed = true;
                self.set_status(format!("Terminal unavailable: {error}"));
            }
        }
    }

    /// Emergency exit: kill the focused editor or terminal without waiting
    /// for it to cooperate.  Works while the pane owns the keyboard because
    /// the prefix is always intercepted, and also when a pane is hidden or
    /// unfocused: the editor goes first, then the shell.
    pub fn force_close(&mut self) {
        let terminal_first = self.focus == Focus::Terminal && self.term.is_some();
        if !terminal_first {
            if let Some(mut editor) = self.editor.take() {
                editor.kill();
                let path = editor.path.clone();
                drop(editor);
                self.after_editor(&path);
                if self.focus == Focus::Terminal && self.term.is_none() {
                    self.focus = Focus::Viewer;
                }
                self.set_terminal_area(self.area);
                self.set_status("Editor killed (unsaved changes are lost)");
                return;
            }
        }
        if let Some(mut term) = self.term.take() {
            term.kill();
            drop(term);
            self.show_term = false;
            self.config.show_terminal = false;
            if self.focus == Focus::Terminal {
                self.focus = Focus::Viewer;
            }
            self.set_terminal_area(self.area);
            self.set_status("Terminal killed");
            return;
        }
        self.set_status("Nothing to kill: no editor or terminal is running");
    }

    /// Close the panel when the shell exits (`exit`, Ctrl+D...).
    fn poll_term(&mut self) {
        let Some(term) = &mut self.term else {
            return;
        };
        if term.exit_status().is_none() {
            return;
        }
        self.term = None;
        self.show_term = false;
        if self.focus == Focus::Terminal {
            self.focus = Focus::Viewer;
        }
        self.set_terminal_area(self.area);
        self.set_status("Terminal closed (the shell exited)");
    }

    pub fn terminal_failed(&self) -> bool {
        self.term_failed
    }

    // --------------------------------------------------------------- history

    fn current_entry(&self) -> Option<HistoryEntry> {
        self.viewer.loaded.as_ref().map(|loaded| HistoryEntry {
            path: loaded.path.clone(),
            cursor: self.viewer.cursor,
        })
    }

    fn push_history(&mut self) {
        if let Some(entry) = self.current_entry() {
            self.history.push(entry);
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
    }

    fn history_back(&mut self) {
        let Some(entry) = self.history.pop() else {
            self.set_status("No previous document");
            return;
        };
        if let Some(current) = self.current_entry() {
            self.forward.push(current);
            if self.forward.len() > HISTORY_LIMIT {
                self.forward.remove(0);
            }
        }
        self.restore_history(entry);
    }

    fn history_forward(&mut self) {
        let Some(entry) = self.forward.pop() else {
            self.set_status("No next document");
            return;
        };
        if let Some(current) = self.current_entry() {
            self.history.push(current);
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
        self.restore_history(entry);
    }

    fn restore_history(&mut self, entry: HistoryEntry) {
        self.open_file(entry.path, None, false);
        self.jump_viewer_to(entry.cursor);
        self.focus = Focus::Viewer;
    }

    // ----------------------------------------------------------------- links

    fn follow_link(&mut self) {
        let Some(link) = self.viewer.doc.link_at(self.viewer.cursor).cloned() else {
            self.set_status("No link on this line");
            return;
        };
        if link.image {
            self.set_status(format!("Image: {} (images are not rendered)", link.dest));
            return;
        }
        match link.kind {
            LinkKind::External => {
                self.set_status(format!("External link: {} (not opened)", link.dest));
            }
            LinkKind::Anchor => self.navigate_anchor(&link.dest),
            LinkKind::Wiki => self.follow_wiki_link(&link.dest),
            LinkKind::Internal => self.follow_internal_link(&link.dest),
        }
    }

    fn navigate_anchor(&mut self, anchor: &str) {
        match toc::resolve_anchor(&self.viewer.doc.headings, anchor) {
            Some(line) => {
                self.viewer.jump_to_line(line);
                self.focus = Focus::Viewer;
            }
            None => self.set_status(format!("Anchor not found: {anchor}")),
        }
    }

    fn follow_internal_link(&mut self, dest: &str) {
        let (path_part, fragment) = toc::split_fragment(dest);
        let path_part = toc::decode_fragment(path_part);
        let path_part = path_part
            .split('?')
            .next()
            .unwrap_or(&path_part)
            .trim()
            .to_string();

        if path_part.is_empty() {
            if let Some(fragment) = fragment {
                self.navigate_anchor(fragment);
            }
            return;
        }
        if path_part.contains("://") {
            self.set_status(format!("External link: {dest} (not opened)"));
            return;
        }

        let base = self
            .viewer
            .loaded
            .as_ref()
            .and_then(|loaded| loaded.path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());
        let mut candidate = if let Some(stripped) = path_part.strip_prefix('/') {
            self.root.join(stripped)
        } else {
            base.join(&path_part)
        };
        if !candidate.exists() && candidate.extension().is_none() {
            candidate.set_extension("md");
        }

        if candidate.is_dir() {
            self.focus_folder(&candidate);
        } else if candidate.is_file() {
            self.open_file(candidate, fragment.map(str::to_string), true);
        } else {
            self.set_status(format!("Cannot find linked file: {path_part}"));
        }
    }

    fn follow_wiki_link(&mut self, dest: &str) {
        let (path_part, fragment) = toc::split_fragment(dest);
        let target = toc::decode_fragment(path_part);
        let target = target
            .split('|')
            .next()
            .unwrap_or(&target)
            .trim()
            .to_string();

        if target.is_empty() {
            if let Some(fragment) = fragment {
                self.navigate_anchor(fragment);
            }
            return;
        }

        match self.find_wiki_file(&target) {
            Some(path) => self.open_file(path, fragment.map(str::to_string), true),
            None => self.set_status(format!("Wikilink target not found: {target}")),
        }
    }

    fn find_wiki_file(&self, target: &str) -> Option<PathBuf> {
        let base = self
            .viewer
            .loaded
            .as_ref()
            .and_then(|loaded| loaded.path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());
        let with_extension = Path::new(target).extension().is_none();
        let variants = if with_extension {
            vec![target.to_string(), format!("{target}.md")]
        } else {
            vec![target.to_string()]
        };

        for variant in &variants {
            let direct = base.join(variant);
            if direct.is_file() {
                return Some(direct);
            }
            let rooted = self.root.join(variant);
            if rooted.is_file() {
                return Some(rooted);
            }
        }

        let target_lower = target.replace('\\', "/").to_ascii_lowercase();
        let wanted: Vec<String> = if with_extension {
            vec![target_lower.clone(), format!("{target_lower}.md")]
        } else {
            vec![target_lower.clone()]
        };

        let mut builder = WalkBuilder::new(&self.root);
        builder
            .hidden(!self.explorer.show_hidden())
            .max_depth(Some(16))
            .require_git(false)
            .follow_links(false);

        for (seen, result) in builder.build().enumerate() {
            if seen > WIKI_SCAN_LIMIT {
                break;
            }
            let Ok(entry) = result else {
                continue;
            };
            if !entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            let matched = wanted
                .iter()
                .any(|want| relative == *want || relative.ends_with(&format!("/{want}")));
            if matched {
                return Some(entry.into_path());
            }
        }
        None
    }

    // ------------------------------------------------------------- key input

    /// True while the terminal panel or the embedded editor owns the keyboard:
    /// everything except the prefix key goes to the program running there.
    pub fn pane_captured(&self) -> bool {
        if self.focus == Focus::Terminal {
            self.term.is_some()
        } else {
            self.editor.is_some()
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        // Typing ends a text selection.
        self.selection = None;

        if self.capturing_prefix {
            self.handle_prefix_capture(key);
            return;
        }

        if self.search_input.is_some() {
            self.handle_search_key(key);
            return;
        }
        if self.overlay.is_open() {
            self.handle_overlay_key(key);
            return;
        }

        let captured = self.pane_captured();
        let pending = self.pending.map(|(chord, _)| chord);
        match self.keymap.resolve(pending, &key, captured) {
            Resolution::Action(action) => {
                self.pending = None;
                self.run_action(action);
                return;
            }
            Resolution::Pending(chord) => {
                self.pending = Some((chord, Instant::now()));
                return;
            }
            Resolution::Literal(chord) => {
                self.pending = None;
                if captured {
                    self.send_to_pane(chord.to_event());
                }
                return;
            }
            Resolution::Cancel => {
                self.pending = None;
                return;
            }
            Resolution::Pass => {}
        }

        if captured {
            self.send_to_pane(key);
            return;
        }
        self.handle_navigation_key(key);
    }

    /// Forward a key to the focused terminal or editor.
    fn send_to_pane(&mut self, key: KeyEvent) {
        if self.focus == Focus::Terminal {
            if let Some(term) = &mut self.term {
                term.send_key(key);
                return;
            }
        }
        if let Some(editor) = &mut self.editor {
            editor.send_key(key);
        }
    }

    /// Paste text into the focused terminal or editor.
    pub fn paste(&mut self, text: &str) {
        if self.focus == Focus::Terminal {
            if let Some(term) = &mut self.term {
                term.send_paste(text);
                return;
            }
        }
        if let Some(editor) = &mut self.editor {
            editor.send_paste(text);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit(),
            Action::Menu => {
                if self.viewer.search.is_some() {
                    self.viewer.clear_search();
                    self.set_status("Search cleared");
                } else {
                    self.open_menu();
                }
            }
            Action::Help => self.overlay = Overlay::Help { scroll: 0 },
            Action::Outline => {
                let outline = self.viewer.outline();
                if outline.is_empty() {
                    self.set_status("This document has no headings");
                } else {
                    let current = self.viewer.cursor;
                    let selected = outline
                        .iter()
                        .rposition(|item| item.line <= current)
                        .unwrap_or(0);
                    self.overlay = Overlay::Outline { selected };
                }
            }
            Action::Search => self.start_search(),
            Action::SearchNext => {
                if !self.viewer.search_next() {
                    self.set_status("No active search");
                }
            }
            Action::SearchPrev => {
                if !self.viewer.search_previous() {
                    self.set_status("No active search");
                }
            }
            Action::FocusNext => self.cycle_focus(true),
            Action::FocusPrev => self.cycle_focus(false),
            Action::Edit => self.edit_selected(),
            Action::ToggleExplorer => self.toggle_explorer(),
            Action::ToggleTerminal => self.toggle_terminal(true),
            Action::CycleTheme => self.cycle_theme(),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::ToggleGitignore => self.toggle_gitignore(),
            Action::CycleIcons => self.cycle_icons(),
            Action::Refresh => match self.focus {
                Focus::Explorer => {
                    self.explorer.refresh();
                    self.preview_selection();
                    self.set_status("Project tree refreshed");
                }
                Focus::Viewer => {
                    self.explorer.refresh();
                    self.reload_current();
                }
                Focus::Terminal => {}
            },
            Action::HistoryBack => self.history_back(),
            Action::HistoryForward => self.history_forward(),
            Action::HalfPageDown => self.half_page(true),
            Action::HalfPageUp => self.half_page(false),
            Action::PageDown => self.page(true),
            Action::ExplorerShrink => self.resize_explorer(-EXPLORER_STEP),
            Action::ExplorerGrow => self.resize_explorer(EXPLORER_STEP),
            Action::TerminalGrow => self.resize_terminal(TERMINAL_STEP),
            Action::TerminalShrink => self.resize_terminal(-TERMINAL_STEP),
            Action::ForceClose => self.force_close(),
        }
    }

    /// Panels that can take the keyboard, in Tab order.
    fn focus_order(&self) -> Vec<Focus> {
        let mut order = Vec::with_capacity(3);
        if self.focusable(Focus::Explorer) {
            order.push(Focus::Explorer);
        }
        order.push(Focus::Viewer);
        if self.focusable(Focus::Terminal) {
            order.push(Focus::Terminal);
        }
        order
    }

    fn focusable(&self, pane: Focus) -> bool {
        match pane {
            // While the editor is open the explorer would only confuse it.
            Focus::Explorer => self.layout.explorer.is_some() && self.editor.is_none(),
            Focus::Viewer => true,
            Focus::Terminal => self.layout.terminal.is_some(),
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order = self.focus_order();
        let count = order.len();
        let index = order.iter().position(|pane| *pane == self.focus).unwrap_or(0);
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        self.focus = order[next];
    }

    /// Navigation keys that are not rebindable shortcuts.
    fn handle_navigation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.activate(),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::PageDown => self.page(true),
            KeyCode::PageUp => self.page(false),
            KeyCode::Home => self.go_top(),
            KeyCode::End => self.go_bottom(),
            // Space opens a file or folds/unfolds the selected folder.
            KeyCode::Char(' ') if self.focus == Focus::Explorer => {
                self.activate();
                self.preview_selection();
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------- mouse

    fn pane_at(&self, x: u16, y: u16) -> Option<(Focus, Rect)> {
        let inside = |rect: Rect| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom();
        if let Some(rect) = self.layout.explorer.filter(|rect| inside(*rect)) {
            return Some((Focus::Explorer, rect));
        }
        if inside(self.layout.viewer) {
            return Some((Focus::Viewer, self.layout.viewer));
        }
        if let Some(rect) = self.layout.terminal_inner().filter(|rect| inside(*rect)) {
            return Some((Focus::Terminal, rect));
        }
        None
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) {
        if !self.config.mouse || self.overlay.is_open() || self.search_input.is_some() {
            return;
        }
        let (x, y) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(x, y),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    if !selection.done {
                        selection.drag_to(x, y);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let empty = self.selection.as_ref().map(Selection::is_empty);
                match empty {
                    Some(true) => self.selection = None,
                    Some(false) => {
                        if let Some(selection) = self.selection.as_mut() {
                            selection.done = true;
                        }
                    }
                    None => {}
                }
            }
            MouseEventKind::ScrollUp => self.mouse_wheel(x, y, -WHEEL_LINES),
            MouseEventKind::ScrollDown => self.mouse_wheel(x, y, WHEEL_LINES),
            _ => {}
        }
    }

    fn mouse_down(&mut self, x: u16, y: u16) {
        self.selection = None;
        self.selection_copied = false;
        let Some((pane, rect)) = self.pane_at(x, y) else {
            return;
        };
        if self.focusable(pane) {
            self.focus = pane;
        }
        match pane {
            // The explorer is for picking files: a click selects the row and
            // its text is never part of a selection.
            Focus::Explorer => {
                let position = self.explorer.offset() + (y - rect.y) as usize;
                if position < self.explorer.visible().len() {
                    self.explorer.select_position(position);
                    self.preview_selection();
                }
            }
            Focus::Viewer | Focus::Terminal => {
                self.selection = Some(Selection::new(rect, x, y));
            }
        }
    }

    /// `lines` is positive when scrolling down.
    fn mouse_wheel(&mut self, x: u16, y: u16, lines: isize) {
        let Some((pane, _)) = self.pane_at(x, y) else {
            return;
        };
        self.selection = None;
        match pane {
            Focus::Explorer => {
                self.explorer.move_selection(lines);
                self.preview_selection();
            }
            Focus::Viewer => {
                if let Some(editor) = &mut self.editor {
                    let code = if lines < 0 { KeyCode::Up } else { KeyCode::Down };
                    for _ in 0..lines.unsigned_abs() {
                        editor.send_key(KeyEvent::new(code, KeyModifiers::NONE));
                    }
                } else {
                    self.viewer.move_cursor(lines);
                }
            }
            Focus::Terminal => {
                if let Some(term) = &mut self.term {
                    // Wheel up goes back through the scrollback.
                    term.scroll_view(-lines);
                }
            }
        }
    }

    /// Called after every draw: when a selection was just completed, read its
    /// text from the screen buffer and queue it for the clipboard.
    pub fn capture_selection(&mut self, buffer: &Buffer) {
        if self.selection_copied {
            return;
        }
        let Some(selection) = &self.selection else {
            return;
        };
        if !selection.done {
            return;
        }
        self.selection_copied = true;
        let text = selection.extract(buffer);
        if !text.is_empty() {
            self.clipboard = Some(text);
        }
    }

    /// Text waiting to be sent to the system clipboard, if any.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.search_input = None;
                self.set_status("Search cancelled");
            }
            KeyCode::Enter => {
                let query = self.search_input.take().unwrap_or_default();
                if query.trim().is_empty() {
                    self.viewer.clear_search();
                    self.set_status("Search cleared");
                    return;
                }
                // A search covers the whole file, not just what is highlighted.
                self.load_all_viewer();
                let count = self.viewer.apply_search(query.trim());
                if count == 0 {
                    self.set_status(format!("No matches for “{}”", query.trim()));
                } else {
                    self.set_status(format!(
                        "{count} match{} for “{}”",
                        if count == 1 { "" } else { "es" },
                        query.trim()
                    ));
                }
            }
            KeyCode::Backspace => {
                if let Some(input) = &mut self.search_input {
                    input.pop();
                }
            }
            KeyCode::Char('u') if ctrl => {
                if let Some(input) = &mut self.search_input {
                    input.clear();
                }
            }
            KeyCode::Char(character) if !ctrl => {
                if let Some(input) = &mut self.search_input {
                    input.push(character);
                }
            }
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.overlay {
            Overlay::Help { scroll } => {
                let maximum = help::rendered_line_count(&self.keymap).saturating_sub(3);
                let next = match (key.code, ctrl) {
                    (KeyCode::Esc, _) | (KeyCode::Char('?'), _) | (KeyCode::Char('q'), false) => {
                        self.close_overlay();
                        return;
                    }
                    (KeyCode::Down, _) => scroll.saturating_add(1),
                    (KeyCode::Up, _) => scroll.saturating_sub(1),
                    (KeyCode::Char('d'), true) | (KeyCode::PageDown, _) => {
                        scroll.saturating_add(10)
                    }
                    (KeyCode::Char('u'), true) | (KeyCode::PageUp, _) => scroll.saturating_sub(10),
                    (KeyCode::Home, _) => 0,
                    (KeyCode::End, _) => maximum,
                    _ => scroll,
                };
                self.overlay = Overlay::Help {
                    scroll: next.min(maximum),
                };
            }
            Overlay::Outline { selected } => {
                let outline = self.viewer.outline();
                let count = outline.len();
                let next = match (key.code, ctrl) {
                    (KeyCode::Esc, _)
                    | (KeyCode::Char('o'), false)
                    | (KeyCode::Char('q'), false) => {
                        self.overlay = Overlay::None;
                        return;
                    }
                    (KeyCode::Down, _) => selected.saturating_add(1).min(count.saturating_sub(1)),
                    (KeyCode::Up, _) => selected.saturating_sub(1),
                    (KeyCode::Home, _) => 0,
                    (KeyCode::End, _) => count.saturating_sub(1),
                    (KeyCode::Enter, _) => {
                        if let Some(item) = outline.get(selected) {
                            self.viewer.jump_to_line(item.line);
                        }
                        self.overlay = Overlay::None;
                        return;
                    }
                    _ => selected,
                };
                self.overlay = Overlay::Outline {
                    selected: next.min(count.saturating_sub(1)),
                };
            }
            Overlay::Themes { selected, original } => {
                let count = ThemeName::ALL.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.apply_theme(original);
                        self.close_overlay();
                        self.set_status(format!("Theme: {}", original.label()));
                        return;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        self.close_overlay();
                        self.set_status(format!("Theme: {}", self.theme.label()));
                        return;
                    }
                    _ => {}
                }
                let next = match key.code {
                    KeyCode::Up => selected.saturating_sub(1),
                    KeyCode::Down => selected.saturating_add(1).min(count.saturating_sub(1)),
                    KeyCode::Home => 0,
                    KeyCode::End => count.saturating_sub(1),
                    _ => selected,
                };
                if next != selected {
                    let name = ThemeName::ALL[next];
                    self.apply_theme(name);
                    self.set_status(format!("Theme: {}", name.label()));
                }
                self.overlay = Overlay::Themes {
                    selected: next,
                    original,
                };
            }
            Overlay::Menu { selected } => self.handle_menu_key(key, selected),
            Overlay::Options { tab, selected } => self.handle_options_key(key, tab, selected),
            Overlay::None => {}
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent, selected: usize) {
        let count = MENU_ITEMS.len();
        let next = match key.code {
            KeyCode::Esc => {
                self.close_overlay();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => (selected + count - 1) % count,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => (selected + 1) % count,
            KeyCode::BackTab => (selected + count - 1) % count,
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.menu_selected = selected;
                match selected {
                    0 => self.push_overlay(Overlay::Options {
                        tab: 0,
                        selected: 0,
                    }),
                    1 => self.push_overlay(Overlay::Help { scroll: 0 }),
                    _ => self.quit(),
                }
                return;
            }
            _ => selected,
        };
        self.menu_selected = next;
        self.overlay = Overlay::Menu { selected: next };
    }

    fn handle_options_key(&mut self, key: KeyEvent, tab: usize, selected: usize) {
        if self.adjusting {
            // Enter was pressed on an adjustable option: + and - change it.
            let item = OPTION_TABS[tab].items[selected];
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.adjusting = false;
                    self.set_status("Adjustment finished");
                }
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right | KeyCode::Up => {
                    self.adjust_option(item, 1);
                }
                KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Left | KeyCode::Down => {
                    self.adjust_option(item, -1);
                }
                _ => {}
            }
            return;
        }
        let tabs = OPTION_TABS.len();
        let items = OPTION_TABS[tab].items.len();
        let (mut tab, mut selected) = (tab, selected);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_overlay();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => selected = (selected + items - 1) % items,
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % items,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                tab = (tab + tabs - 1) % tabs;
                selected = 0;
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                tab = (tab + 1) % tabs;
                selected = 0;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let item = OPTION_TABS[tab].items[selected];
                // The overlay is updated first: the theme picker pushes it on
                // the stack so Esc comes back to this very screen.
                self.overlay = Overlay::Options { tab, selected };
                self.activate_option(item);
                return;
            }
            _ => {}
        }
        self.overlay = Overlay::Options { tab, selected };
    }

    /// While Options → Key bindings waits for a key: the pressed key becomes
    /// the prefix, Backspace restores the default and Esc cancels.
    fn handle_prefix_capture(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.capturing_prefix = false;
                self.set_status("Prefix unchanged");
            }
            KeyCode::Backspace => {
                self.capturing_prefix = false;
                self.apply_prefix(DEFAULT_PREFIX.to_string());
            }
            _ => {
                let chord = Chord::from_event(&key);
                match self.keymap.check_prefix(chord) {
                    Ok(()) => {
                        self.capturing_prefix = false;
                        self.apply_prefix(chord.spec());
                    }
                    // Stay in capture mode so another key can be tried.
                    Err(reason) => self.set_status(format!("{reason} · try another key, Esc cancels")),
                }
            }
        }
    }

    /// Make `spec` (e.g. `ctrl+a`) the prefix and save it in `config.toml`.
    fn apply_prefix(&mut self, spec: String) {
        self.config.prefix = spec;
        self.keymap = Keymap::from_config(&self.config.prefix, &self.config.keys);
        self.pending = None;
        // Tests must not write the real configuration file.
        #[cfg(not(test))]
        let _ = self.config.save();
        self.set_status(format!("Prefix is now {}", self.keymap.prefix_label()));
    }

    fn activate_option(&mut self, item: OptionItem) {
        match item {
            OptionItem::Theme => self.open_theme_menu(),
            OptionItem::Icons => self.cycle_icons(),
            OptionItem::ShowExplorer => self.toggle_explorer(),
            OptionItem::ShowHidden => self.toggle_hidden(),
            OptionItem::Gitignore => self.toggle_gitignore(),
            OptionItem::ExplorerWidth | OptionItem::TerminalHeight => {
                self.adjusting = true;
                self.set_status("Adjusting: + / − change the size, Enter or Esc to finish");
            }
            OptionItem::ShowTerminal => self.toggle_terminal(false),
            OptionItem::Mouse => {
                self.config.mouse = !self.config.mouse;
                self.selection = None;
                self.set_status(if self.config.mouse {
                    "Mouse on: selection stays inside one panel"
                } else {
                    "Mouse off: the terminal's own selection is back"
                });
            }
            OptionItem::Keys => {
                self.capturing_prefix = true;
                self.set_status(format!(
                    "Press the new prefix key (now {}) · Backspace restores {} · Esc cancels",
                    self.keymap.prefix_label(),
                    Chord::parse(DEFAULT_PREFIX).map_or_else(|_| DEFAULT_PREFIX.to_string(), |chord| chord.label())
                ));
            }
            OptionItem::Animation => {
                self.config.banner_animation = !self.config.banner_animation;
                self.set_status(if self.config.banner_animation {
                    "Banner animation on"
                } else {
                    "Banner animation off: the menu opens with a static banner"
                });
            }
            OptionItem::GithubStars => {
                self.config.show_stars = !self.config.show_stars;
                self.stars.set_enabled(self.config.show_stars);
                self.set_status(if self.config.show_stars {
                    "GitHub stars on: the count is fetched once an hour with curl"
                } else {
                    "GitHub stars off: runetui makes no network requests"
                });
            }
            OptionItem::TerminalShell => {
                self.set_status("Set $SHELL to change the shell");
            }
            OptionItem::EmbeddedEditor => {
                self.config.embedded_editor = !self.config.embedded_editor;
                self.set_status(if self.config.embedded_editor {
                    "Editing happens inside runetui"
                } else {
                    "Editing opens the external editor"
                });
            }
            OptionItem::EditorCommand => {
                self.set_status("Set $VISUAL or $EDITOR to change the editor");
            }
        }
    }

    /// Nudge an adjustable option up (`+1`) or down (`-1`).
    fn adjust_option(&mut self, item: OptionItem, direction: i16) {
        match item {
            OptionItem::ExplorerWidth => self.resize_explorer(direction * EXPLORER_STEP),
            OptionItem::TerminalHeight => self.resize_terminal(direction * TERMINAL_STEP),
            _ => {}
        }
    }

    // --------------------------------------------------------------- actions

    fn start_search(&mut self) {
        if self.viewer.loaded.is_none() && !self.viewer.has_document() {
            self.set_status("Nothing to search yet");
            return;
        }
        self.search_input = Some(String::new());
        self.focus = Focus::Viewer;
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Explorer => {
                if let Some(path) = self.explorer.activate_selected() {
                    // Enter the file: load it if it was not already previewed,
                    // then move focus to the document pane.
                    if !self
                        .viewer
                        .loaded
                        .as_ref()
                        .is_some_and(|loaded| loaded.path == path)
                    {
                        self.load_into_viewer(path, None, true);
                    }
                    self.focus = Focus::Viewer;
                } else {
                    self.preview_selection();
                    if let Some(explorer_rect) = self.layout.explorer {
                        self.explorer
                            .set_viewport_height(explorer_rect.height as usize);
                    }
                }
            }
            Focus::Viewer => self.follow_link(),
            Focus::Terminal => {}
        }
    }

    fn move_left(&mut self) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.collapse_or_parent();
                self.preview_selection();
            }
            Focus::Viewer => {
                if self.focusable(Focus::Explorer) {
                    self.focus = Focus::Explorer;
                }
            }
            Focus::Terminal => {}
        }
    }

    fn move_right(&mut self) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.expand_or_descend();
                self.preview_selection();
            }
            Focus::Viewer | Focus::Terminal => {}
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.move_selection(delta);
                self.preview_selection();
            }
            Focus::Viewer => self.viewer.move_cursor(delta),
            Focus::Terminal => {}
        }
    }

    fn page(&mut self, down: bool) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.move_page(down);
                self.preview_selection();
            }
            Focus::Viewer => self.viewer.move_page(down),
            Focus::Terminal => {}
        }
    }

    fn half_page(&mut self, down: bool) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.move_page(down);
                self.preview_selection();
            }
            Focus::Viewer => self.viewer.move_half_page(down),
            Focus::Terminal => {}
        }
    }

    fn go_top(&mut self) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.go_top();
                self.preview_selection();
            }
            Focus::Viewer => self.viewer.go_top(),
            Focus::Terminal => {}
        }
    }

    fn go_bottom(&mut self) {
        match self.focus {
            Focus::Explorer => {
                self.explorer.go_bottom();
                self.preview_selection();
            }
            Focus::Viewer => {
                self.load_all_viewer();
                self.viewer.go_bottom();
            }
            Focus::Terminal => {}
        }
    }

    // ---------------------------------------------------------------- editor

    /// Path that should be opened in the external editor, if any.
    pub fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.editor_request.take()
    }

    fn edit_selected(&mut self) {
        if self.editor.is_some() {
            self.set_status("Already editing a file");
            return;
        }
        let candidate = match self.focus {
            Focus::Explorer => self.explorer.selected_path(),
            Focus::Viewer | Focus::Terminal => self
                .viewer
                .loaded
                .as_ref()
                .map(|loaded| loaded.path.clone()),
        };
        match candidate.filter(|path| path.is_file()) {
            Some(path) => {
                if self.config.embedded_editor {
                    match self.start_embedded_editor(&path) {
                        Ok(()) => return,
                        Err(error) => {
                            self.set_status(format!("Embedded editor unavailable ({error}); using external"));
                        }
                    }
                }
                self.editor_request = Some(path);
            }
            None => self.set_status("Select a file to edit"),
        }
    }

    /// Run the editor inside the document pane.
    pub fn start_embedded_editor(&mut self, path: &Path) -> anyhow::Result<()> {
        let pane = self.layout.viewer;
        let editor = EmbeddedEditor::spawn(path, pane.height, pane.width)?;
        self.editor = Some(editor);
        self.focus = Focus::Viewer;
        self.set_terminal_area(self.area);
        Ok(())
    }

    /// Close the embedded editor once its process has exited.
    fn poll_editor(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(success) = editor.exit_status() else {
            return;
        };
        let path = editor.path.clone();
        self.editor = None;
        self.after_editor(&path);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_status(if success {
            format!("Edited {name}")
        } else {
            format!("Editor closed with an error ({name})")
        });
    }

    /// Called after the external editor exits: refresh the tree and the open
    /// document so the edits are immediately visible.
    pub fn after_editor(&mut self, path: &Path) {
        self.explorer.refresh();
        if self
            .viewer
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.path == path)
        {
            self.reload_current();
        } else {
            self.load_into_viewer(path.to_path_buf(), None, false);
        }
    }

    // ------------------------------------------------------------------ exit

    pub fn persist(&mut self) {
        self.config.last_directory = Some(self.root.clone());
        self.config.theme = self.theme.slug().to_string();
        self.config.show_hidden = self.explorer.show_hidden();
        self.config.respect_gitignore = self.explorer.respect_gitignore();
        self.config.explorer_width = self.explorer.width_pct();
        self.config.show_explorer = self.show_explorer;
        let _ = self.config.save();
    }

    pub fn quit(&mut self) {
        self.persist();
        self.should_quit = true;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::with_config(None, Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::explorer::DEFAULT_WIDTH_PCT;
    use crossterm::event::KeyEvent;
    use ratatui::layout::Rect;
    use std::fs;

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn press_ctrl(app: &mut App, character: char) {
        app.handle_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::CONTROL,
        ));
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("notes")).unwrap();
        fs::write(
            dir.path().join("README.md"),
            b"# Home\n\nSee [the guide](notes/guide.md) and [[guide]].\n\n## Details\n\nalpha beta alpha\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("notes/guide.md"),
            b"# Guide\n\nBack to [home](../README.md).\n\n## Anchor\n\ntext\n",
        )
        .unwrap();
        fs::write(dir.path().join("plain.txt"), b"hello\nworld\n").unwrap();
        dir
    }

    fn app(dir: &tempfile::TempDir) -> App {
        let mut app = App::with_config(Some(dir.path()), Config::default());
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app
    }

    fn line_of_link(app: &App, dest_fragment: &str) -> usize {
        app.viewer
            .doc
            .links
            .iter()
            .find(|link| link.dest.contains(dest_fragment))
            .map(|link| link.line)
            .unwrap_or_else(|| panic!("no link containing {dest_fragment}"))
    }

    #[test]
    fn opens_readme_on_startup() {
        let dir = project();
        let app = app(&dir);
        let loaded = app.viewer.loaded.as_ref().unwrap();
        assert_eq!(loaded.display_name(), "README.md");
        assert_eq!(app.focus, Focus::Explorer);
        assert_eq!(
            app.explorer.selected_path().unwrap(),
            dir.path().join("README.md")
        );
    }

    #[test]
    fn explorer_navigation_and_open() {
        let dir = project();
        let mut app = app(&dir);
        app.explorer.go_top();
        let before = app.explorer.selected_position();
        press(&mut app, KeyCode::Down);
        assert_ne!(app.explorer.selected_position(), before);

        assert!(
            app.explorer
                .select_visible_path(&dir.path().join("plain.txt"))
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Viewer);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "plain.txt"
        );
    }

    #[test]
    fn space_toggles_folders_in_the_explorer() {
        let dir = project();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs").join("guide.md"), b"# Guide\n").unwrap();
        let mut app = app(&dir);
        app.explorer.refresh();
        assert!(app.explorer.select_visible_path(&dir.path().join("docs")));
        let collapsed = app.explorer.visible_len();

        press(&mut app, KeyCode::Char(' '));
        assert!(
            app.explorer.visible_len() > collapsed,
            "space should unfold"
        );
        assert_eq!(app.focus, Focus::Explorer);

        assert!(app.explorer.select_visible_path(&dir.path().join("docs")));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.explorer.visible_len(), collapsed, "space should fold");
    }

    #[test]
    fn space_opens_a_selected_file() {
        let dir = project();
        let mut app = app(&dir);
        assert!(
            app.explorer
                .select_visible_path(&dir.path().join("plain.txt"))
        );
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "plain.txt"
        );
        assert_eq!(app.focus, Focus::Viewer);
    }

    #[test]
    fn arrow_navigation_previews_files_without_leaving_explorer() {
        let dir = project();
        let mut app = app(&dir);
        app.explorer.go_top();
        press(&mut app, KeyCode::Down); // notes/ directory
        press(&mut app, KeyCode::Down); // plain.txt
        assert_eq!(app.focus, Focus::Explorer);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "plain.txt"
        );
    }

    #[test]
    fn j_and_k_no_longer_move_the_selection() {
        let dir = project();
        let mut app = app(&dir);
        app.explorer.go_top();
        let before = app.explorer.selected_position();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.explorer.selected_position(), before);
    }

    #[test]
    fn e_requests_the_external_editor_when_embedding_is_off() {
        let dir = project();
        let mut app = app(&dir);
        app.config.embedded_editor = false;
        assert!(
            app.explorer
                .select_visible_path(&dir.path().join("plain.txt"))
        );
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(
            app.take_editor_request(),
            Some(dir.path().join("plain.txt"))
        );
        assert!(app.take_editor_request().is_none());
    }

    #[test]
    fn capital_f_cycles_icon_styles() {
        let dir = project();
        let mut app = app(&dir);
        let first = app.config.icon_style();
        press(&mut app, KeyCode::Char('F'));
        assert_ne!(app.config.icon_style(), first);
        press(&mut app, KeyCode::Char('F'));
        press(&mut app, KeyCode::Char('F'));
        assert_eq!(app.config.icon_style(), first);
    }

    #[test]
    fn tab_switches_focus_and_left_returns_to_explorer() {
        let dir = project();
        let mut app = app(&dir);
        assert_eq!(app.focus, Focus::Explorer);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Viewer);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::Explorer);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Viewer);
    }

    #[test]
    fn theme_cycles_and_updates_config() {
        let dir = project();
        let mut app = app(&dir);
        let original = app.theme;
        press(&mut app, KeyCode::Char('T'));
        assert_eq!(app.theme, original.next());
        assert_eq!(app.viewer.rendered_theme, Some(app.theme));
        assert_eq!(app.config.theme, app.theme.slug());
    }

    #[test]
    fn hidden_and_gitignore_toggles() {
        let dir = project();
        let mut app = app(&dir);
        let hidden = app.explorer.show_hidden();
        press(&mut app, KeyCode::Char('H'));
        assert_eq!(app.explorer.show_hidden(), !hidden);
        assert_eq!(app.config.show_hidden, !hidden);

        let gitignore = app.explorer.respect_gitignore();
        press(&mut app, KeyCode::Char('I'));
        assert_eq!(app.explorer.respect_gitignore(), !gitignore);
    }

    #[test]
    fn explorer_can_be_hidden_and_shown() {
        let dir = project();
        let mut app = app(&dir);
        press_ctrl(&mut app, 'r');
        assert!(app.show_explorer, "Ctrl+R alone only starts the prefix");
        press(&mut app, KeyCode::Char('b'));
        assert!(!app.show_explorer);
        assert_eq!(app.focus, Focus::Viewer);
        assert!(app.layout.explorer.is_none());
        press_ctrl(&mut app, 'r');
        press(&mut app, KeyCode::Char('b'));
        assert!(app.show_explorer);
        assert!(app.layout.explorer.is_some());
    }

    #[test]
    fn search_prompt_finds_matches() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('/'));
        for character in "beta".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.search_input.is_none());
        let search = app.viewer.search.as_ref().unwrap();
        assert_eq!(search.query, "beta");
        assert_eq!(search.matches.len(), 1);
        assert!(app.status_text().unwrap().contains("1 match"));
    }

    #[test]
    fn internal_link_opens_target_document() {
        let dir = project();
        let mut app = app(&dir);
        let line = line_of_link(&app, "notes/guide.md");
        app.viewer.jump_to_line(line);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "guide.md"
        );
        assert!(app.history.len() >= 1);
    }

    #[test]
    fn wiki_link_resolves_by_name() {
        let dir = project();
        let mut app = app(&dir);
        let line = line_of_link(&app, "guide");
        app.viewer.jump_to_line(line);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "guide.md"
        );
    }

    #[test]
    fn anchor_links_jump_within_document() {
        let dir = project();
        let mut app = app(&dir);
        let details = app
            .viewer
            .doc
            .headings
            .iter()
            .find(|heading| heading.text == "Details")
            .unwrap()
            .line;
        // A same-document anchor link.
        let line = app.viewer.doc.lines.len().saturating_sub(1);
        app.viewer.jump_to_line(line);
        app.follow_internal_link("#details");
        assert_eq!(app.viewer.cursor, details);
    }

    #[test]
    fn history_back_and_forward_restore_position() {
        let dir = project();
        let mut app = app(&dir);
        let line = line_of_link(&app, "notes/guide.md");
        app.viewer.jump_to_line(line);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "guide.md"
        );

        press_ctrl(&mut app, 'o');
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "README.md"
        );
        press_ctrl(&mut app, 'i');
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "guide.md"
        );
    }

    #[test]
    fn outline_overlay_jumps_to_heading() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('o'));
        let selected = match app.overlay {
            Overlay::Outline { selected } => selected,
            other => panic!("unexpected overlay: {other:?}"),
        };
        assert_eq!(selected, 0);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.viewer.cursor > 0);
    }

    #[test]
    fn help_overlay_closes_on_escape() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('?'));
        assert!(app.overlay.is_open());
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
    }

    /// Esc, then Enter on OPTIONS, then Enter on the first option (Theme).
    fn open_theme_picker(app: &mut App) {
        press(app, KeyCode::Esc);
        press(app, KeyCode::Enter);
        press(app, KeyCode::Enter);
    }

    #[test]
    fn escape_opens_the_menu_and_a_second_escape_closes_it() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::Menu { selected: 0 });
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn menu_navigation_wraps_and_help_returns_to_the_menu() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.overlay, Overlay::Menu { selected: 2 });
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.overlay, Overlay::Menu { selected: 1 });

        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Help { .. }));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::Menu { selected: 1 });
    }

    #[test]
    fn menu_quit_item_quits() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert!(app.should_quit);
    }

    #[test]
    fn theme_picker_previews_live_and_escape_returns_to_options() {
        let dir = project();
        let mut app = app(&dir);
        let original = app.theme;
        open_theme_picker(&mut app);
        assert!(matches!(app.overlay, Overlay::Themes { .. }));

        press(&mut app, KeyCode::Down);
        assert_ne!(app.theme, original);
        assert_eq!(app.config.theme, app.theme.slug());

        // Esc cancels, restores the theme and lands back on the options.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.theme, original);
        assert_eq!(app.config.theme, original.slug());
        assert!(matches!(app.overlay, Overlay::Options { tab: 0, selected: 0 }));

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::Menu { selected: 0 });
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn theme_picker_enter_applies_the_selection() {
        let dir = project();
        let mut app = app(&dir);
        open_theme_picker(&mut app);
        press(&mut app, KeyCode::Down);
        let chosen = app.theme;
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Options { .. }));
        assert_eq!(app.theme, chosen);

        press(&mut app, KeyCode::Enter);
        match app.overlay {
            Overlay::Themes { selected, .. } => {
                assert_eq!(ThemeName::ALL[selected], chosen);
            }
            other => panic!("unexpected overlay: {other:?}"),
        }
    }

    #[test]
    fn options_tabs_and_toggles_work() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);

        // General: Icons is the second entry.
        press(&mut app, KeyCode::Down);
        let before = app.config.icon_style();
        press(&mut app, KeyCode::Enter);
        assert_ne!(app.config.icon_style(), before);

        // Explorer tab: Hidden files is first.
        press(&mut app, KeyCode::Right);
        assert!(matches!(app.overlay, Overlay::Options { tab: 1, selected: 0 }));
        let hidden = app.explorer.show_hidden();
        press(&mut app, KeyCode::Enter);
        assert_ne!(app.explorer.show_hidden(), hidden);

        // Editor tab: embedding can be switched off.
        press(&mut app, KeyCode::Right);
        assert!(app.config.embedded_editor);
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.embedded_editor);

        // Terminal tab, then it wraps back to the first tab.
        press(&mut app, KeyCode::Right);
        assert!(matches!(app.overlay, Overlay::Options { tab: 3, .. }));
        press(&mut app, KeyCode::Right);
        assert!(matches!(app.overlay, Overlay::Options { tab: 0, .. }));
    }

    #[test]
    fn embedded_editor_takes_the_keys_and_closes_when_it_exits() {
        let dir = project();
        let mut app = app(&dir);
        let file = dir.path().join("plain.txt");
        let args = vec!["-c".to_string(), "read line; echo got:$line".to_string()];
        let Ok(editor) = EmbeddedEditor::spawn_with("sh", &args, &file, 10, 40) else {
            return; // no pty in this environment
        };
        app.editor = Some(editor);

        // Keys (even Esc and `q`) go to the editor, never to the app.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.overlay == Overlay::None);
        assert!(!app.should_quit);

        press(&mut app, KeyCode::Enter);
        for _ in 0..200 {
            app.tick();
            if app.editor.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.editor.is_none(), "the pane closes when the editor exits");
        assert!(app.status_text().is_some_and(|text| text.starts_with("Edited")));
    }

    #[test]
    fn q_quits() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let dir = project();
        let mut app = app(&dir);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::End);
        assert_eq!(app.viewer.progress(), 100);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.viewer.cursor, 0);
    }

    #[test]
    fn narrow_terminal_hides_explorer_and_moves_focus() {
        let dir = project();
        let mut app = app(&dir);
        app.set_terminal_area(Rect::new(0, 0, 44, 20));
        assert!(app.layout.explorer.is_none());
        assert_eq!(app.focus, Focus::Viewer);
    }

    #[test]
    fn resizing_reflows_the_document() {
        let dir = project();
        let mut app = app(&dir);
        let before = app.viewer.rendered_width;
        app.set_terminal_area(Rect::new(0, 0, 140, 30));
        assert_ne!(app.viewer.rendered_width, before);
        assert_eq!(app.viewer.rendered_width, app.content_width());
    }

    #[test]
    fn external_links_are_reported_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("README.md"),
            b"# Link\n\n[site](https://example.com)\n",
        )
        .unwrap();
        let mut app = app(&dir);
        let line = line_of_link(&app, "https://example.com");
        app.viewer.jump_to_line(line);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::Enter);
        assert!(app.status_text().unwrap().contains("External link"));
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "README.md"
        );
    }

    #[test]
    fn width_percentage_starts_at_default() {
        let dir = project();
        let mut config = Config::default();
        config.explorer_width = DEFAULT_WIDTH_PCT;
        let app = App::with_config(Some(dir.path()), config);
        assert_eq!(app.explorer.width_pct(), DEFAULT_WIDTH_PCT);
    }

    // ------------------------------------------------ terminal, prefix, keys

    fn prefix_then(app: &mut App, code: KeyCode) {
        press_ctrl(app, 'r');
        press(app, code);
    }

    fn shell_app(dir: &tempfile::TempDir, script: &str) -> Option<App> {
        let mut app = app(dir);
        let args = vec!["-c".to_string(), script.to_string()];
        let pane = PtyPane::spawn_shell_with("sh", &args, dir.path(), 8, 60).ok()?;
        app.term = Some(pane);
        app.focus = Focus::Terminal;
        Some(app)
    }

    fn wait_for_screen(app: &App, needle: &str) -> bool {
        for _ in 0..300 {
            if app.term.as_ref().is_some_and(|term| term.screen_text().contains(needle)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// A program that ignores the polite ways of stopping it.
    const HUNG: &str = "trap '' HUP INT TERM QUIT; while :; do read -r _; done";

    #[test]
    fn force_close_kills_a_hung_embedded_editor() {
        let dir = project();
        let mut app = app(&dir);
        let file = dir.path().join("plain.txt");
        let args = vec!["-c".to_string(), HUNG.to_string()];
        let Ok(editor) = EmbeddedEditor::spawn_with("sh", &args, &file, 10, 40) else {
            return; // no pty in this environment
        };
        app.editor = Some(editor);
        app.focus = Focus::Viewer;

        // Esc, q and Ctrl+C all go to the hung editor, which ignores them.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('q'));
        press_ctrl(&mut app, 'c');
        assert!(app.editor.is_some() && !app.should_quit);

        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.editor.is_none(), "the prefix still reaches the app");
        assert!(!app.should_quit);
        assert!(app.status_text().is_some_and(|text| text.starts_with("Editor killed")));
        assert!(!app.pane_captured(), "the keyboard is back");
    }

    #[test]
    fn force_close_kills_a_hung_terminal_and_hides_the_panel() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, HUNG) else {
            return; // no pty in this environment
        };
        app.show_term = true;
        press_ctrl(&mut app, 'c');
        assert!(app.term.is_some(), "Ctrl+C goes to the shell, which ignores it");

        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.term.is_none());
        assert!(!app.show_term);
        assert_eq!(app.focus, Focus::Viewer);
        assert!(app.status_text().is_some_and(|text| text.starts_with("Terminal killed")));
    }

    #[test]
    fn force_close_reaches_a_hidden_unfocused_shell() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, HUNG) else {
            return;
        };
        app.show_term = false;
        app.focus = Focus::Viewer;
        assert!(!app.pane_captured());
        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.term.is_none(), "a hidden shell is killed too");
    }

    #[test]
    fn force_close_with_nothing_running_only_says_so() {
        let dir = project();
        let mut app = app(&dir);
        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.status_text().is_some_and(|text| text.starts_with("Nothing to kill")));
        assert!(!app.should_quit);
    }

    #[test]
    fn force_close_kills_only_the_focused_pane_first() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, HUNG) else {
            return;
        };
        let file = dir.path().join("plain.txt");
        let args = vec!["-c".to_string(), HUNG.to_string()];
        let Ok(editor) = EmbeddedEditor::spawn_with("sh", &args, &file, 10, 40) else {
            return;
        };
        app.editor = Some(editor);
        app.show_term = true;
        app.focus = Focus::Terminal;

        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.term.is_none() && app.editor.is_some(), "the focused terminal goes first");
        prefix_then(&mut app, KeyCode::Char('x'));
        assert!(app.editor.is_none(), "a second press kills the editor");
    }

    #[test]
    fn prefix_then_arrows_resize_the_left_and_bottom_panels() {
        let dir = project();
        let mut app = app(&dir);
        let explorer = app.explorer.width_pct();
        prefix_then(&mut app, KeyCode::Left);
        assert_eq!(app.explorer.width_pct(), explorer - 2);
        prefix_then(&mut app, KeyCode::Right);
        prefix_then(&mut app, KeyCode::Right);
        assert_eq!(app.explorer.width_pct(), explorer + 2);
        assert_eq!(app.config.explorer_width, explorer + 2);

        let terminal = app.term_pct;
        let before = app.layout.terminal.unwrap().height;
        prefix_then(&mut app, KeyCode::Up);
        assert_eq!(app.term_pct, terminal + TERMINAL_STEP as u16);
        assert!(app.layout.terminal.unwrap().height > before);
        prefix_then(&mut app, KeyCode::Down);
        prefix_then(&mut app, KeyCode::Down);
        assert_eq!(app.term_pct, terminal - TERMINAL_STEP as u16);
        assert_eq!(app.config.terminal_height, app.term_pct);
    }

    #[test]
    fn prefix_arrows_work_with_ctrl_still_held() {
        let dir = project();
        let mut app = app(&dir);
        let explorer = app.explorer.width_pct();
        press_ctrl(&mut app, 'r');
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(app.explorer.width_pct(), explorer - 2);
        assert!(app.pending.is_none());
    }

    #[test]
    fn direct_ctrl_arrows_resize_when_no_pane_is_captured() {
        let dir = project();
        let mut app = app(&dir);
        let explorer = app.explorer.width_pct();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(app.explorer.width_pct(), explorer + 2);
        let terminal = app.term_pct;
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
        assert_eq!(app.term_pct, terminal + 5);
    }

    #[test]
    fn resizing_respects_the_limits() {
        let dir = project();
        let mut app = app(&dir);
        for _ in 0..40 {
            prefix_then(&mut app, KeyCode::Up);
        }
        assert_eq!(app.term_pct, MAX_TERMINAL_PCT);
        for _ in 0..40 {
            prefix_then(&mut app, KeyCode::Down);
        }
        assert_eq!(app.term_pct, MIN_TERMINAL_PCT);
    }

    #[test]
    fn pending_prefix_is_cancelled_by_an_unrelated_key() {
        let dir = project();
        let mut app = app(&dir);
        press_ctrl(&mut app, 'r');
        assert!(app.pending.is_some());
        press(&mut app, KeyCode::Char('z'));
        assert!(app.pending.is_none());
        assert!(!app.should_quit);
        // `q` right after a cancelled prefix is a normal quit key again.
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn the_terminal_can_be_shown_and_hidden() {
        let dir = project();
        let mut app = app(&dir);
        assert!(app.show_term && app.layout.terminal.is_some());
        let with = app.layout.viewer.height;
        prefix_then(&mut app, KeyCode::Char('t'));
        assert!(!app.show_term);
        assert!(app.layout.terminal.is_none());
        assert!(app.layout.viewer.height > with);
        assert!(!app.config.show_terminal, "the choice is persisted");

        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(app.show_term);
        assert_eq!(app.focus, Focus::Terminal, "showing it moves the keyboard there");
        assert!(app.config.show_terminal);
    }

    #[test]
    fn tab_cycles_explorer_document_terminal() {
        let dir = project();
        let mut app = app(&dir);
        assert_eq!(app.focus, Focus::Explorer);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Viewer);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Terminal);
    }

    #[test]
    fn terminal_focus_forwards_all_keys_except_the_prefix() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, "read line; echo got:$line") else {
            return; // no pty in this environment
        };
        assert!(app.pane_captured());
        // `q`, Esc, Tab and Ctrl+C would normally act on runetui.
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.should_quit);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.focus, Focus::Terminal);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        for character in "hi".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        assert!(wait_for_screen(&app, "got:hi"), "keys reached the shell");
    }

    #[test]
    fn prefix_twice_sends_the_prefix_key_to_the_shell() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, "stty -iexten; echo ready; read -r line; printf '%s' \"$line\" | od -An -tx1") else {
            return;
        };
        // Wait until `stty` has run: before that the tty still eats Ctrl+R.
        assert!(wait_for_screen(&app, "ready"), "the shell is ready");
        press_ctrl(&mut app, 'r');
        press_ctrl(&mut app, 'r');
        press(&mut app, KeyCode::Enter);
        assert!(wait_for_screen(&app, "12"), "Ctrl+R (0x12) reached the shell");
    }

    #[test]
    fn prefix_still_works_from_the_terminal() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, "sleep 5") else {
            return;
        };
        let explorer = app.explorer.width_pct();
        prefix_then(&mut app, KeyCode::Left);
        assert_eq!(app.explorer.width_pct(), explorer - 2);
        // The Esc menu opens from the terminal through the prefix.
        prefix_then(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.overlay, Overlay::Menu { .. }));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        // Prefix + o moves the focus out of the terminal.
        prefix_then(&mut app, KeyCode::Char('o'));
        assert_ne!(app.focus, Focus::Terminal);
    }

    #[test]
    fn closing_the_shell_closes_the_panel_without_changing_the_setting() {
        let dir = project();
        let Some(mut app) = shell_app(&dir, "exit 0") else {
            return;
        };
        for _ in 0..300 {
            app.tick();
            if app.term.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.term.is_none());
        assert!(!app.show_term);
        assert!(app.layout.terminal.is_none());
        assert_ne!(app.focus, Focus::Terminal);
        assert!(app.config.show_terminal, "the saved preference is untouched");
    }

    #[test]
    fn editor_keeps_working_with_the_new_capture_rules() {
        let dir = project();
        let mut app = app(&dir);
        let file = dir.path().join("plain.txt");
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        let Ok(editor) = EmbeddedEditor::spawn_with("sh", &args, &file, 10, 40) else {
            return;
        };
        app.editor = Some(editor);
        app.focus = Focus::Viewer;
        // Tab belongs to vim while the editor has focus...
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Viewer);
        // ...and the prefix still reaches runetui.
        prefix_then(&mut app, KeyCode::Down);
        assert_eq!(app.term_pct, crate::ui::layout::DEFAULT_TERMINAL_PCT - TERMINAL_STEP as u16);
        // The explorer is not part of the focus cycle while editing.
        prefix_then(&mut app, KeyCode::Char('o'));
        assert_eq!(app.focus, Focus::Terminal);
        prefix_then(&mut app, KeyCode::Char('o'));
        assert_eq!(app.focus, Focus::Viewer);
    }

    #[test]
    fn explorer_width_is_adjusted_with_enter_then_plus_and_minus() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Right); // Explorer tab
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down); // Explorer width
        let width = app.explorer.width_pct();

        // Enter does not change the value by itself: it starts the adjustment.
        press(&mut app, KeyCode::Enter);
        assert!(app.adjusting);
        assert_eq!(app.explorer.width_pct(), width);

        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.explorer.width_pct(), width + 4);
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.explorer.width_pct(), width + 2);

        // Navigation keys are captured while adjusting; Enter finishes.
        press(&mut app, KeyCode::Tab);
        assert!(matches!(app.overlay, Overlay::Options { tab: 1, selected: 2 }));
        press(&mut app, KeyCode::Enter);
        assert!(!app.adjusting);
        // Esc closes the options as usual.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::Menu { selected: 0 });
    }

    #[test]
    fn escape_stops_adjusting_before_it_closes_the_options() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right); // Terminal tab: height is first
        let height = app.term_pct;
        press(&mut app, KeyCode::Enter);
        assert!(app.adjusting);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.term_pct, height + TERMINAL_STEP as u16);
        press(&mut app, KeyCode::Char('-'));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.term_pct, height - TERMINAL_STEP as u16);
        press(&mut app, KeyCode::Esc);
        assert!(!app.adjusting);
        assert!(matches!(app.overlay, Overlay::Options { tab: 3, .. }), "still in the options");
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::Menu { .. }));
    }

    #[test]
    fn terminal_visibility_and_mouse_are_options() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        for _ in 0..3 {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Enter); // Show terminal
        assert!(!app.show_term);
        assert_eq!(app.focus, Focus::Explorer, "toggling from the options keeps the focus");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // Mouse
        assert!(!app.config.mouse);
    }

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_a_panel_focuses_it() {
        let dir = project();
        let mut app = app(&dir);
        let viewer = app.layout.viewer;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), viewer.x + 3, viewer.y + 1));
        assert_eq!(app.focus, Focus::Viewer);
        let terminal = app.layout.terminal_inner().unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), terminal.x + 2, terminal.y));
        assert_eq!(app.focus, Focus::Terminal);
        let explorer = app.layout.explorer.unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), explorer.x + 2, explorer.y));
        assert_eq!(app.focus, Focus::Explorer);
        assert!(app.selection.is_none(), "the explorer never starts a text selection");
    }

    #[test]
    fn dragging_selects_inside_the_document_and_a_click_selects_nothing() {
        let dir = project();
        let mut app = app(&dir);
        let viewer = app.layout.viewer;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), viewer.x + 1, viewer.y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), viewer.x + 1, viewer.y));
        assert!(app.selection.is_none(), "a plain click clears the selection");

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), viewer.x + 1, viewer.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, viewer.y + 2));
        let selection = app.selection.as_ref().unwrap();
        assert_eq!(selection.area, viewer);
        assert!(selection.head.0 >= viewer.x, "clamped to the pane");
        // Typing ends the selection.
        press(&mut app, KeyCode::Down);
        assert!(app.selection.is_none());
    }

    #[test]
    fn the_wheel_scrolls_the_panel_under_the_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..80).map(|n| format!("line {n}\n\n")).collect();
        fs::write(dir.path().join("README.md"), body).unwrap();
        let mut app = app(&dir);
        let viewer = app.layout.viewer;
        let before = app.viewer.cursor;
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, viewer.x + 2, viewer.y + 2));
        assert!(app.viewer.cursor > before);
        // Over the explorer it moves the file selection instead.
        let explorer = app.layout.explorer.unwrap();
        let cursor = app.viewer.cursor;
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, explorer.x + 1, explorer.y + 1));
        assert_eq!(app.viewer.cursor, cursor);
    }

    #[test]
    fn mouse_input_is_ignored_when_the_mouse_option_is_off() {
        let dir = project();
        let mut config = Config::default();
        config.mouse = false;
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        let viewer = app.layout.viewer;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), viewer.x + 1, viewer.y));
        assert_eq!(app.focus, Focus::Explorer);
        assert!(app.selection.is_none());
    }

    #[test]
    fn shortcuts_follow_the_configuration() {
        let dir = project();
        let mut config = Config::default();
        config.prefix = "ctrl+a".to_string();
        config.keys.insert("quit".into(), crate::keymap::KeySpec::One("x".into()));
        config.keys.insert(
            "toggle_terminal".into(),
            crate::keymap::KeySpec::Many(vec!["f4".into(), "prefix `".into()]),
        );
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));

        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit, "q is no longer bound");
        press(&mut app, KeyCode::F(4));
        assert!(!app.show_term);
        press_ctrl(&mut app, 'a');
        press(&mut app, KeyCode::Left);
        assert_eq!(app.explorer.width_pct(), DEFAULT_WIDTH_PCT - 2, "the new prefix works");
        // The old prefix no longer starts a sequence.
        press_ctrl(&mut app, 'r');
        assert!(app.pending.is_none());
        press(&mut app, KeyCode::Char('x'));
        assert!(app.should_quit);
    }

    #[test]
    fn invalid_bindings_warn_and_fall_back() {
        let dir = project();
        let mut config = Config::default();
        config.keys.insert("help".into(), crate::keymap::KeySpec::One("hyper+h".into()));
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        assert!(app.status_text().unwrap().contains("help"));
        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.overlay, Overlay::Help { .. }));
    }

    // --------------------------------------------------- key binding capture

    fn open_key_bindings(app: &mut App) {
        let tab = OPTION_TABS[0]
            .items
            .iter()
            .position(|item| *item == OptionItem::Keys)
            .unwrap();
        app.overlay = Overlay::Options { tab: 0, selected: tab };
        press(app, KeyCode::Enter);
    }

    #[test]
    fn enter_on_key_bindings_waits_for_the_new_prefix() {
        let dir = project();
        let mut app = app(&dir);
        open_key_bindings(&mut app);
        assert!(app.capturing_prefix);
        let status = app.status.as_ref().unwrap().text.clone();
        assert!(status.contains("Press the new prefix key"), "{status}");
        assert!(!status.contains("config.toml"), "no longer just shows the path");
        // The options screen is still open behind the capture.
        assert!(matches!(app.overlay, Overlay::Options { .. }));
    }

    #[test]
    fn the_pressed_key_becomes_the_prefix() {
        let dir = project();
        let mut app = app(&dir);
        open_key_bindings(&mut app);
        press_ctrl(&mut app, 'a');
        assert!(!app.capturing_prefix);
        assert_eq!(app.config.prefix, "ctrl+a");
        assert_eq!(app.keymap.prefix_label(), "Ctrl+A");
        assert!(app.status.as_ref().unwrap().text.contains("Ctrl+A"));
        // Back in the interface the new prefix works and the old one does not.
        app.overlay = Overlay::None;
        press_ctrl(&mut app, 'a');
        assert!(app.pending.is_some());
        press(&mut app, KeyCode::Char('z'));
        assert!(app.pending.is_none());
        press_ctrl(&mut app, 'r');
        assert!(app.pending.is_none(), "Ctrl+R is a plain key again");
        // Shortcuts written as "prefix <key>" follow the new prefix.
        let explorer = app.explorer.width_pct();
        press_ctrl(&mut app, 'a');
        press(&mut app, KeyCode::Left);
        assert_eq!(app.explorer.width_pct(), explorer - 2);
    }

    #[test]
    fn unusable_keys_are_refused_and_capture_continues() {
        let dir = project();
        let mut app = app(&dir);
        open_key_bindings(&mut app);
        for key in [
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            app.handle_key(key);
            assert!(app.capturing_prefix, "{key:?} must not end the capture");
            assert_eq!(app.config.prefix, "ctrl+r");
        }
        assert!(!app.should_quit, "q and Ctrl+C did not reach the shortcuts");
        assert!(app.status.as_ref().unwrap().text.contains("already used"));
        // A valid key still works afterwards.
        app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert!(!app.capturing_prefix);
        assert_eq!(app.config.prefix, "f9");
    }

    #[test]
    fn backspace_restores_the_default_and_esc_cancels() {
        let dir = project();
        let mut app = app(&dir);
        open_key_bindings(&mut app);
        press_ctrl(&mut app, 'a');
        open_key_bindings(&mut app);
        press(&mut app, KeyCode::Esc);
        assert!(!app.capturing_prefix);
        assert_eq!(app.config.prefix, "ctrl+a", "Esc leaves it as it was");
        assert!(matches!(app.overlay, Overlay::Options { .. }), "Esc only cancels the capture");
        open_key_bindings(&mut app);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.config.prefix, "ctrl+r");
        assert_eq!(app.keymap.prefix_label(), "Ctrl+R");
    }

    #[test]
    fn the_new_prefix_survives_a_config_round_trip() {
        let dir = project();
        let mut app = app(&dir);
        open_key_bindings(&mut app);
        press_ctrl(&mut app, 'a');
        let file = dir.path().join("config.toml");
        app.config.save_to(&file).unwrap();
        let loaded = Config::load_from(&file).unwrap();
        assert_eq!(loaded.prefix, "ctrl+a");
        assert_eq!(Keymap::from_config(&loaded.prefix, &loaded.keys).prefix_label(), "Ctrl+A");
    }

    // ------------------------------------------------- animation and stars

    #[test]
    fn the_banner_animation_can_be_switched_off() {
        let dir = project();
        let mut app = app(&dir);
        app.open_menu();
        assert!(app.menu_animation().is_some());
        app.close_overlay();
        app.config.banner_animation = false;
        app.open_menu();
        assert!(app.menu_animation().is_none(), "the menu opens with a static banner");
    }

    #[test]
    fn options_toggle_the_animation_and_the_stars() {
        let dir = project();
        let mut app = app(&dir);
        let position = |wanted: OptionItem| {
            OPTION_TABS[0].items.iter().position(|item| *item == wanted).unwrap()
        };
        app.overlay = Overlay::Options { tab: 0, selected: position(OptionItem::Animation) };
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.banner_animation);
        press(&mut app, KeyCode::Enter);
        assert!(app.config.banner_animation);

        app.overlay = Overlay::Options { tab: 0, selected: position(OptionItem::GithubStars) };
        assert!(app.config.show_stars);
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.show_stars);
        assert_eq!(app.stars.state(), crate::stars::StarsState::Off);
        press(&mut app, KeyCode::Enter);
        assert!(app.config.show_stars);
        assert_ne!(app.stars.state(), crate::stars::StarsState::Off);
    }

    // ------------------------------------------------ long files, lazy loading

    fn long_file(dir: &tempfile::TempDir, lines: usize) -> PathBuf {
        let mut source = String::new();
        for index in 0..lines {
            source.push_str(&format!("fn item_{index}() {{ let marker_{index} = {index}; }}\n"));
        }
        let path = dir.path().join("long.rs");
        fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn opening_a_long_file_only_highlights_the_first_chunk() {
        let dir = project();
        let mut app = app(&dir);
        let path = long_file(&dir, 6000);
        app.open_file(path, None, true);
        assert!(app.viewer.is_partial());
        assert!(app.viewer.doc.lines.len() < 1000);
        // Scrolling a little does not load anything else...
        app.focus = Focus::Viewer;
        for _ in 0..50 {
            press(&mut app, KeyCode::Down);
            app.tick();
        }
        let (loaded_early, _) = app.viewer.highlighted_lines().unwrap();
        assert_eq!(loaded_early, crate::viewer::INITIAL_LINES);
        // ...but going on towards the end does, a chunk at a time.
        for _ in 0..200 {
            press(&mut app, KeyCode::Down);
            app.tick();
        }
        let (loaded_later, total) = app.viewer.highlighted_lines().unwrap();
        assert!(loaded_later > loaded_early && loaded_later < total);
    }

    #[test]
    fn jumping_to_the_end_and_searching_cover_the_whole_file() {
        let dir = project();
        let mut app = app(&dir);
        app.open_file(long_file(&dir, 6000), None, true);
        app.focus = Focus::Viewer;
        press(&mut app, KeyCode::End);
        assert!(!app.viewer.is_partial());
        assert_eq!(app.viewer.cursor, app.viewer.doc.lines.len() - 1);

        // A fresh view: the search finds text far beyond the first chunk.
        app.open_file(long_file(&dir, 6000), None, true);
        app.viewer.go_top();
        assert!(app.viewer.is_partial() || app.viewer.cursor == 0);
        app.start_search();
        for character in "marker_5990".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        let search = app.viewer.search.as_ref().expect("a search is active");
        assert_eq!(search.matches.len(), 1, "found the line near the end");
        assert!(app.viewer.cursor > 5000);
    }

    #[test]
    fn reloading_a_long_file_returns_to_the_same_place() {
        let dir = project();
        let mut app = app(&dir);
        app.open_file(long_file(&dir, 6000), None, true);
        app.focus = Focus::Viewer;
        app.jump_viewer_to(2500);
        assert_eq!(app.viewer.cursor, 2500);
        app.reload_current();
        assert_eq!(app.viewer.cursor, 2500);
    }
}

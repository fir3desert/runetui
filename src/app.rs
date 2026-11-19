//! Application state, key handling and navigation.
//!
//! Everything here is synchronous and in-memory: files are loaded for display
//! only. Editing runs the user's editor inside the document pane (see
//! `crate::editor`), falling back to an external editor request when the
//! embedded one cannot start. The app owns the explorer, the viewer, the active theme
//! and a small amount of UI state (focus, overlays, history, search prompt).

use std::cell::Cell;
use std::collections::HashSet;
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
use crate::backlinks::BacklinkIndex;
use crate::export::{self, Format};
use crate::finder::Finder;
use crate::state::{self, ProjectState, Toggle};
use crate::filesystem::Explorer;
use crate::filesystem::watcher::{FileWatcher, WatcherState};
use crate::keymap::{Action, Chord, DEFAULT_PREFIX, Keymap, Resolution};
use crate::stars::Stars;
use crate::trace;
use crate::markdown::{DocLink, LinkKind};
use crate::markdown::code_highlighter::CodeHighlighter;
use crate::markdown::toc;
use crate::ui::help;
use crate::selection::Selection;
use crate::ui::layout::{Layout, MAX_TERMINAL_PCT, MIN_TERMINAL_PCT};
use crate::ui::themes::{Theme, ThemeName};
use crate::viewer::{LoadedDoc, Viewer, load_document};

const STATUS_TTL: Duration = Duration::from_secs(5);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(180);
/// Background work shorter than this never shows the "indexing…" indicator,
/// so quick listings do not make the status bar flicker.
const INDEXING_GRACE: Duration = Duration::from_millis(150);
/// Braille spinner frames and the ASCII fallback, advanced every 80 ms.
const SPINNER_UNICODE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_ASCII: [&str; 4] = ["|", "/", "-", "\\"];
const HISTORY_LIMIT: usize = 100;
const WIKI_SCAN_LIMIT: usize = 25_000;
/// How long a pending prefix key waits for the key that completes it.
const PREFIX_TIMEOUT: Duration = Duration::from_secs(3);
/// Lines moved by one mouse-wheel notch.
const WHEEL_LINES: isize = 3;
/// Rows moved by PageUp / PageDown in the file finder.
const FINDER_PAGE: usize = 10;
/// The link index starts this long after launch, once the first frames are out.
const BACKLINKS_START_DELAY: Duration = Duration::from_millis(300);
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

/// The two lists of the bookmarks popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListTab {
    Bookmarks,
    Recent,
}

impl ListTab {
    pub fn other(self) -> Self {
        match self {
            ListTab::Bookmarks => ListTab::Recent,
            ListTab::Recent => ListTab::Bookmarks,
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
    /// Fuzzy file finder (`Ctrl+P`).  The query and the ranked results live in
    /// `App::finder`; `selected` indexes into those results.
    Finder {
        selected: usize,
    },
    /// Files that link to the open file (`B`).
    Backlinks {
        selected: usize,
    },
    /// Bookmarks and recently opened files (`'`), two lists in one popup.
    Bookmarks {
        tab: ListTab,
        selected: usize,
    },
    /// Chooser for a line that holds several links (see `App::link_choices`).
    Links {
        selected: usize,
    },
    /// Actions for the file or folder selected in the explorer, opened with
    /// Enter (see `App::file_menu`).
    FileMenu {
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

/// A clickable region recorded while drawing an overlay, so the mouse handler
/// knows what is under the pointer without recomputing the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Entry of the Esc menu.
    MenuItem(usize),
    /// Tab of the Options screen.
    OptionsTab(usize),
    /// Row of the Options list (label and blanks).
    Option(usize),
    /// The value at the end of an Options row.
    OptionValue(usize),
    /// Row of the theme chooser.
    Theme(usize),
    /// Row of the outline.
    Outline(usize),
    /// Row of the links chooser.
    Link(usize),
    /// Result row of the file finder.
    Finder(usize),
    /// Row of the backlinks popup.
    Backlink(usize),
    /// Tab of the bookmarks popup.
    BookmarkTab(ListTab),
    /// Row of the list shown by the bookmarks popup.
    BookmarkRow(usize),
    /// Row of the file actions menu.
    FileAction(usize),
    /// The frame of a popup: a click outside every panel dismisses it.
    Panel,
}

/// What Enter on an explorer entry can do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    /// Open the file in the viewer, or fold/unfold the folder.
    Open,
    VsCode,
    Terminal,
    DefaultApp,
    Edit,
    CopyPath,
    CopyName,
    /// Opens the Export submenu.
    Export,
    ExportHtml,
    ExportPdf,
}

impl FileAction {
    pub fn label(self, is_dir: bool) -> &'static str {
        match self {
            FileAction::Open if is_dir => "Expand / collapse",
            FileAction::Open => "Open",
            FileAction::VsCode => "Open in VS Code",
            FileAction::Terminal => "Open in terminal",
            FileAction::DefaultApp => "Open with default app",
            FileAction::Edit => "Edit here",
            FileAction::CopyPath => "Copy path",
            FileAction::CopyName => "Copy name",
            FileAction::Export => "Export...",
            FileAction::ExportHtml => "HTML (self-contained page)",
            FileAction::ExportPdf => "PDF",
        }
    }
}

/// The entry the file actions menu is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMenu {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub actions: Vec<FileAction>,
    /// Showing the Export choices (Esc goes back to the actions).
    pub submenu: bool,
    /// Drawn under the explorer row (false: centred, for the shortcut).
    pub anchored: bool,
}

/// Programs the file actions menu starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    VsCode,
    Terminal,
    DefaultApp,
}

/// Start `program` on `path` (a folder, for the terminal).  Replaced in tests.
fn launch_real(program: Launch, path: &Path) -> Result<(), String> {
    match program {
        Launch::VsCode => crate::launcher::open_in_vscode(path),
        Launch::Terminal => crate::launcher::open_in_terminal(path),
        Launch::DefaultApp => crate::launcher::open_with_default(path),
    }
}

/// Two clicks this close in time and place make a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
    /// The selection the last key press ended (read by copy-as-Markdown).
    ended_selection: Option<Selection>,
    /// Options → Key bindings: the next key press becomes the prefix.
    pub capturing_prefix: bool,
    /// GitHub star count shown in the Esc menu.
    pub stars: Stars,
    /// Project file index and results of the `Ctrl+P` finder.
    pub finder: Finder,
    /// Result of the export running on a background thread.
    export_job: Option<std::sync::mpsc::Receiver<export::Outcome>>,
    /// `PATH` to search for PDF converters instead of the real one (tests).
    export_path_var: Option<std::ffi::OsString>,
    /// Which files link to which, built in the background.
    pub backlinks: BacklinkIndex,
    /// Bookmarks and recent files of this project (saved in `state.toml`).
    pub project_state: ProjectState,
    state_path: Option<PathBuf>,

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
    /// Directories whose listing changed since the last refresh.
    dirty_dirs: HashSet<PathBuf>,
    /// What the watcher was last told to watch: tree generation and open file.
    watch_key: Option<(u64, Option<PathBuf>)>,
    /// Why live refresh is limited or off, once the user has been told.
    watcher_problem: Option<String>,
    started_at: Instant,
    /// Where the star count was drawn last frame, for mouse clicks.
    pub star_rect: Cell<Option<Rect>>,
    /// Right-hand part of the header (kind, progress, stars): the Esc menu and
    /// the other overlays leave it undimmed.
    pub header_keep: Cell<Option<Rect>>,
    /// Clickable regions of the overlay drawn last frame.
    pub hits: std::cell::RefCell<Vec<(Rect, Hit)>>,
    /// When and where the last left click happened, for double clicks.
    last_click: Option<(Instant, u16, u16)>,
    /// Opens an address in the browser; replaced in tests.
    pub(crate) opener: fn(&str) -> Result<(), String>,
    /// The links offered by the `Overlay::Links` chooser.
    pub(crate) link_choices: Vec<DocLink>,
    /// The entry the `Overlay::FileMenu` acts on.
    pub(crate) file_menu: Option<FileMenu>,
    /// Starts an external program for the file menu; replaced in tests.
    pub(crate) launcher: fn(Launch, &Path) -> Result<(), String>,
    /// Status text that replaces "Copied N characters" for the next copy.
    clipboard_note: Option<String>,
}

impl App {
    pub fn with_config(cli_path: Option<&Path>, mut config: Config) -> Self {
        let root = trace::span("resolve root", || config::resolve_root(cli_path, &config));
        let initial_file = config::resolve_initial_file(cli_path);
        let theme = config.theme_name();
        let keymap = Keymap::from_config(&config.prefix, &config.keys);
        let term_pct = config
            .terminal_height
            .clamp(MIN_TERMINAL_PCT, MAX_TERMINAL_PCT);
        let mut explorer = trace::span("explorer: list root", || {
            Explorer::new(
                &root,
                config.explorer_width,
                config.show_hidden,
                config.respect_gitignore,
            )
        });

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
            watcher: Some(trace::span("filesystem watcher (start thread)", FileWatcher::start)),
            editor: None,
            term: None,
            show_term: config.show_terminal,
            term_pct,
            keymap,
            pending: None,
            adjusting: false,
            selection: None,
            ended_selection: None,
            capturing_prefix: false,
            stars: trace::span("stars start", || Stars::start(config.show_stars)),
            finder: Finder::new(),
            export_job: None,
            export_path_var: None,
            backlinks: BacklinkIndex::new(),
            project_state: ProjectState::default(),
            state_path: state::default_path(),
            selection_copied: false,
            clipboard: None,
            term_failed: false,
            menu_selected: 0,
            menu_opened: None,
            overlay_stack: Vec::new(),
            highlighter: trace::span("syntax highlighter", CodeHighlighter::new),
            history: Vec::new(),
            forward: Vec::new(),
            editor_request: None,
            refresh_pending: None,
            reload_pending: false,
            dirty_dirs: HashSet::new(),
            watch_key: None,
            watcher_problem: None,
            started_at: Instant::now(),
            star_rect: Cell::new(None),
            header_keep: Cell::new(None),
            hits: std::cell::RefCell::new(Vec::new()),
            last_click: None,
            opener: crate::browser::open_url,
            link_choices: Vec::new(),
            file_menu: None,
            launcher: launch_real,
            clipboard_note: None,
        };

        config.last_directory = Some(root.clone());
        app.config = config;

        let cli_opened_file = initial_file.is_some();
        trace::span("initial document", || {
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
        });
        // Bookmarks and recent files come after the first document, so the
        // README opened by default is not counted as something the user opened.
        app.project_state = match &app.state_path {
            Some(path) => state::load(path, &app.root),
            None => ProjectState::default(),
        };
        if let Some(file) = app.viewer.loaded.as_ref().map(|loaded| loaded.path.clone()) {
            if cli_opened_file {
                app.note_recent(&file);
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
        app.sync_watcher();
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
        let opened = path.clone();
        if self.load_into_viewer(path, anchor, record_history) {
            self.focus = Focus::Viewer;
            self.note_recent(&opened);
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
        // The links of the open file may have changed.
        self.backlinks.invalidate();
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
        self.explorer.poll();
        self.poll_finder();
        self.poll_backlinks();
        self.poll_export();
        self.sync_watcher();
        self.poll_watcher_state();
        if let Some(watcher) = &self.watcher {
            if let Some(event) = watcher.drain() {
                if let Some(current) = &self.viewer.loaded {
                    if event.touches(&current.path) {
                        self.reload_pending = true;
                    }
                }
                self.dirty_dirs.extend(event.dirs);
                // The window opens at the first event, so a steady stream of
                // changes (a build, a download) cannot postpone it forever.
                self.refresh_pending.get_or_insert_with(Instant::now);
            }
        }

        if self
            .refresh_pending
            .is_some_and(|since| since.elapsed() >= WATCH_DEBOUNCE)
        {
            self.refresh_pending = None;
            let dirs: Vec<PathBuf> = std::mem::take(&mut self.dirty_dirs).into_iter().collect();
            if !dirs.is_empty() {
                self.explorer.refresh_dirs(&dirs);
                // Files came or went: the finder's index is out of date.
                self.finder.invalidate();
                self.backlinks.invalidate();
            }
            if std::mem::take(&mut self.reload_pending) {
                self.reload_current();
            }
        }

        if self.status.as_ref().is_some_and(StatusMessage::is_expired) {
            self.status = None;
        }
    }

    // ------------------------------------------------------- background work

    /// Tell the watcher which folders to watch: the ones open in the explorer
    /// and the folder of the open document.  Does nothing unless they changed.
    fn sync_watcher(&mut self) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        let document = self.viewer.loaded.as_ref().map(|loaded| loaded.path.clone());
        let key = (self.explorer.tree.generation(), document.clone());
        if self.watch_key.as_ref() == Some(&key) {
            return;
        }
        self.watch_key = Some(key);
        let mut dirs = self.explorer.tree.open_directories();
        if let Some(parent) = document.as_deref().and_then(Path::parent) {
            if !dirs.iter().any(|dir| dir == parent) {
                dirs.push(parent.to_path_buf());
            }
        }
        watcher.sync(dirs);
    }

    /// Say once when live refresh is limited or unavailable.
    fn poll_watcher_state(&mut self) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        let problem = match watcher.state() {
            WatcherState::Failed(reason) | WatcherState::Limited { reason, .. } => Some(reason),
            WatcherState::Starting | WatcherState::Ready { .. } => None,
        };
        match (problem, self.watcher_problem.is_some()) {
            (Some(reason), false) => {
                let refresh = self
                    .keymap
                    .primary(Action::Refresh)
                    .map(|chord| chord.label())
                    .unwrap_or_else(|| "r".to_string());
                self.set_status(format!(
                    "Live refresh limited ({reason}); press {refresh} to refresh by hand"
                ));
                self.watcher_problem = Some(reason);
            }
            (None, true) => self.watcher_problem = None,
            _ => {}
        }
    }

    /// Why live refresh is limited or off, if it is.
    pub fn watcher_problem(&self) -> Option<&str> {
        self.watcher_problem.as_deref()
    }

    /// Something is running in the background: a directory listing or the
    /// watcher starting up.
    pub fn is_busy(&self) -> bool {
        self.work_started().is_some()
    }

    fn work_started(&self) -> Option<Instant> {
        let watcher_starting = self
            .watcher
            .as_ref()
            .is_some_and(|watcher| watcher.state() == WatcherState::Starting)
            && self.watch_key.is_some();
        let listing = match (self.explorer.busy_since(), self.finder.scan_started()) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        };
        match (watcher_starting.then_some(self.started_at), listing) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        }
    }

    /// Background work has been running long enough to deserve an "indexing…"
    /// indicator.
    pub fn indexing(&self) -> bool {
        self.work_started()
            .is_some_and(|since| since.elapsed() >= INDEXING_GRACE)
    }

    /// Spinner frame for the link index (it has its own clock, as it is not
    /// part of the "indexing…" status the other background work shows).
    pub fn backlink_spinner(&self) -> &'static str {
        let step = self
            .backlinks
            .scan_started()
            .map(|since| since.elapsed().as_millis() / 80)
            .unwrap_or(0) as usize;
        if self.config.icon_style() == crate::ui::icons::IconStyle::Ascii {
            SPINNER_ASCII[step % SPINNER_ASCII.len()]
        } else {
            SPINNER_UNICODE[step % SPINNER_UNICODE.len()]
        }
    }

    /// Current frame of the indexing spinner.
    pub fn spinner(&self) -> &'static str {
        let step = self
            .work_started()
            .map(|since| since.elapsed().as_millis() / 80)
            .unwrap_or(0) as usize;
        if self.config.icon_style() == crate::ui::icons::IconStyle::Ascii {
            SPINNER_ASCII[step % SPINNER_ASCII.len()]
        } else {
            SPINNER_UNICODE[step % SPINNER_UNICODE.len()]
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

    // -------------------------------------------------------------- backlinks

    /// Project-relative path of the open file.
    fn open_relative_path(&self) -> Option<String> {
        let path = self.viewer.loaded.as_ref()?.path.clone();
        self.relative_path(&path)
    }

    /// How many places link to the open file; `None` until the first index is
    /// built or when no file inside the project is open.
    pub fn backlink_count(&self) -> Option<usize> {
        self.backlinks.count(&self.open_relative_path()?)
    }

    /// Keep the link index going: it starts shortly after the first frames (so
    /// opening a huge project stays instant) and follows changes on disk.
    fn poll_backlinks(&mut self) {
        self.backlinks.poll();
        if self.started_at.elapsed() >= BACKLINKS_START_DELAY {
            self.backlinks.ensure(
                &self.root,
                self.explorer.show_hidden(),
                self.explorer.respect_gitignore(),
            );
        }
    }

    fn open_backlinks(&mut self) {
        if self.open_relative_path().is_none() {
            self.set_status(if self.viewer.loaded.is_some() {
                "Only files inside the project have backlinks"
            } else {
                "No file open"
            });
            return;
        }
        // Asked for: do not wait for the start-up delay.
        self.backlinks.ensure(
            &self.root,
            self.explorer.show_hidden(),
            self.explorer.respect_gitignore(),
        );
        self.overlay = Overlay::Backlinks { selected: 0 };
    }

    /// The links to the open file, as listed by the popup.
    pub fn current_backlinks(&self) -> &[crate::backlinks::Backlink] {
        match self.open_relative_path() {
            Some(relative) => self.backlinks.backlinks(&relative),
            None => &[],
        }
    }

    fn choose_backlink(&mut self, index: usize) {
        let Some(link) = self.current_backlinks().get(index).cloned() else {
            return;
        };
        self.close_overlay();
        let path = self.root.join(&link.source);
        if !path.is_file() {
            self.backlinks.invalidate();
            self.set_status(format!("No longer exists: {}", link.source));
            return;
        }
        self.open_file(path, None, true);
        self.jump_to_source_line(link.line);
        // A link inside a wrapped paragraph or a list is mapped to its block:
        // look for the link itself within the block.
        if let Some(row) = self.viewer.row_of_text_near_line(link.line, &link.label) {
            self.jump_viewer_to(row);
        }
        self.set_status(format!("{}:{}", link.source, link.line + 1));
    }

    fn handle_backlinks_key(&mut self, key: KeyEvent, selected: usize) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.current_backlinks().len().saturating_sub(1);
        let mut selected = selected.min(last);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => {
                self.close_overlay();
                return;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.choose_backlink(selected);
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::PageDown => selected = (selected + FINDER_PAGE).min(last),
            KeyCode::PageUp => selected = selected.saturating_sub(FINDER_PAGE),
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = last,
            _ => {}
        }
        self.overlay = Overlay::Backlinks { selected };
    }

    // ------------------------------------------------- bookmarks and recents

    /// `path` relative to the project root, with `/` separators.
    fn relative_path(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.root).ok().map(Path::to_path_buf).or_else(|| {
            let (path, root) = (path.canonicalize().ok()?, self.root.canonicalize().ok()?);
            path.strip_prefix(root).ok().map(Path::to_path_buf)
        })?;
        let text = relative.to_str()?.replace('\\', "/");
        (!text.is_empty()).then_some(text)
    }

    fn save_state(&mut self) {
        let Some(path) = self.state_path.clone() else {
            return;
        };
        if state::save(&path, &self.root, &self.project_state).is_err() {
            self.set_status("Could not save the bookmarks and recent files");
        }
    }

    /// A file was opened on purpose (not just previewed while browsing).
    fn note_recent(&mut self, path: &Path) {
        let Some(relative) = self.relative_path(path) else {
            return;
        };
        if self.project_state.recent.first() == Some(&relative) {
            return;
        }
        self.project_state.note_recent(&relative);
        self.save_state();
    }

    /// Bookmark the current line of the open file, or remove its bookmark.
    fn toggle_bookmark(&mut self) {
        let Some(path) = self.viewer.loaded.as_ref().map(|loaded| loaded.path.clone()) else {
            self.set_status("No file open to bookmark");
            return;
        };
        let Some(relative) = self.relative_path(&path) else {
            self.set_status("Only files inside the project can be bookmarked");
            return;
        };
        let Some(line) = self.viewer.source_line_at(self.viewer.cursor) else {
            self.set_status("Nothing to bookmark here");
            return;
        };
        let text = (line..line + 4)
            .filter_map(|candidate| self.viewer.source_line_text(candidate))
            .find(|text| !text.trim().is_empty())
            .unwrap_or_default();
        let number = line + 1;
        match self.project_state.toggle_bookmark(&relative, number, &text) {
            Toggle::Added => self.set_status(format!("Bookmarked {relative}:{number}")),
            Toggle::Removed => self.set_status(format!("Bookmark removed: {relative}:{number}")),
            Toggle::Full => {
                self.set_status(format!("Too many bookmarks (the limit is {})", state::MAX_BOOKMARKS));
                return;
            }
        }
        self.save_state();
    }

    fn open_bookmarks(&mut self) {
        // Entries of files that were moved or deleted are dropped here.
        let before = (self.project_state.bookmarks.len(), self.project_state.recent.len());
        self.project_state.retain_existing(&self.root);
        if before != (self.project_state.bookmarks.len(), self.project_state.recent.len()) {
            self.save_state();
        }
        self.overlay = Overlay::Bookmarks {
            tab: ListTab::Bookmarks,
            selected: 0,
        };
    }

    pub fn list_len(&self, tab: ListTab) -> usize {
        match tab {
            ListTab::Bookmarks => self.project_state.bookmarks.len(),
            ListTab::Recent => self.project_state.recent.len(),
        }
    }

    /// Move the viewer to a 0-based source line, loading more of a long source
    /// file when the line is further down than what is rendered.
    fn jump_to_source_line(&mut self, line: usize) {
        let mut row = self.viewer.row_of_source_line(line);
        if row.is_none() {
            let theme = self.theme();
            self.viewer.load_all(theme, &mut self.highlighter);
            row = self.viewer.row_of_source_line(line);
        }
        if let Some(row) = row {
            self.jump_viewer_to(row);
        }
    }

    /// Open entry `index` of a list: a bookmark jumps to its line, a recent
    /// file opens at its start.
    fn choose_list_entry(&mut self, tab: ListTab, index: usize) {
        let (file, line) = match tab {
            ListTab::Bookmarks => match self.project_state.bookmarks.get(index) {
                Some(bookmark) => (bookmark.file.clone(), Some(bookmark.line.saturating_sub(1))),
                None => return,
            },
            ListTab::Recent => match self.project_state.recent.get(index) {
                Some(file) => (file.clone(), None),
                None => return,
            },
        };
        self.close_overlay();
        let path = self.root.join(&file);
        if !path.is_file() {
            match tab {
                ListTab::Bookmarks => {
                    self.project_state.remove_bookmark(index);
                }
                ListTab::Recent => {
                    self.project_state.remove_recent(index);
                }
            }
            self.save_state();
            self.set_status(format!("No longer exists: {file}"));
            return;
        }
        let same_file = self.viewer.loaded.as_ref().is_some_and(|loaded| loaded.path == path);
        if same_file && line.is_some() {
            self.push_history();
            self.forward.clear();
            self.focus = Focus::Viewer;
        } else {
            self.open_file(path, None, true);
        }
        if let Some(line) = line {
            self.jump_to_source_line(line);
            self.set_status(format!("{file}:{}", line + 1));
        }
    }

    fn delete_list_entry(&mut self, tab: ListTab, index: usize) {
        let removed = match tab {
            ListTab::Bookmarks => self.project_state.remove_bookmark(index).is_some(),
            ListTab::Recent => self.project_state.remove_recent(index).is_some(),
        };
        if removed {
            self.save_state();
        }
    }

    fn handle_bookmarks_key(&mut self, key: KeyEvent, tab: ListTab, selected: usize) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.list_len(tab);
        let last = count.saturating_sub(1);
        let mut tab = tab;
        let mut selected = selected.min(last);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => {
                self.close_overlay();
                return;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.choose_list_entry(tab, selected);
                return;
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right
            | KeyCode::Char('h') | KeyCode::Char('l')
                if !ctrl =>
            {
                tab = tab.other();
                selected = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::PageDown => selected = (selected + FINDER_PAGE).min(last),
            KeyCode::PageUp => selected = selected.saturating_sub(FINDER_PAGE),
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = last,
            KeyCode::Char('d') | KeyCode::Delete if !ctrl => {
                if count > 0 {
                    self.delete_list_entry(tab, selected);
                    selected = selected.min(self.list_len(tab).saturating_sub(1));
                }
            }
            _ => {}
        }
        self.overlay = Overlay::Bookmarks { tab, selected };
    }

    // ---------------------------------------------------------------- finder

    /// Open the fuzzy file finder with an empty query.  The project index is
    /// built in the background the first time (and again after files change).
    fn open_finder(&mut self) {
        self.finder.query.clear();
        self.finder.ensure(
            &self.root,
            self.explorer.show_hidden(),
            self.explorer.respect_gitignore(),
        );
        self.finder.refresh();
        self.overlay = Overlay::Finder { selected: 0 };
    }

    /// Take in indexed files, keep the index current while the finder is open
    /// and keep its selection inside the results.
    fn poll_finder(&mut self) {
        self.finder.poll();
        if let Overlay::Finder { selected } = self.overlay {
            self.finder.ensure(
                &self.root,
                self.explorer.show_hidden(),
                self.explorer.respect_gitignore(),
            );
            let last = self.finder.hits().len().saturating_sub(1);
            if selected > last {
                self.overlay = Overlay::Finder { selected: last };
            }
        }
    }

    /// Open result `index` in the viewer and reveal it in the explorer.
    fn choose_finder_hit(&mut self, index: usize) {
        let Some(hit) = self.finder.hits().get(index).cloned() else {
            return;
        };
        let path = self.finder.absolute(&hit);
        self.close_overlay();
        if !path.is_file() {
            self.finder.invalidate();
            self.set_status(format!("No longer exists: {}", hit.path));
            return;
        }
        self.open_file(path, None, true);
    }

    fn handle_finder_key(&mut self, key: KeyEvent, selected: usize) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let last = self.finder.hits().len().saturating_sub(1);
        let mut selected = selected.min(last);
        match (key.code, ctrl) {
            (KeyCode::Esc, _) => {
                self.close_overlay();
                return;
            }
            (KeyCode::Enter, _) => {
                self.choose_finder_hit(selected);
                return;
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) => selected = (selected + 1).min(last),
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => selected = selected.saturating_sub(1),
            (KeyCode::PageDown, _) => selected = (selected + FINDER_PAGE).min(last),
            (KeyCode::PageUp, _) => selected = selected.saturating_sub(FINDER_PAGE),
            (KeyCode::Home, _) => selected = 0,
            (KeyCode::End, _) => selected = last,
            (KeyCode::Backspace, _) => {
                self.finder.backspace();
                selected = 0;
            }
            (KeyCode::Char('u'), true) => {
                self.finder.clear_query();
                selected = 0;
            }
            (KeyCode::Char('w'), true) => {
                self.finder.delete_word();
                selected = 0;
            }
            // Every other character is text, `q` included: Esc leaves the
            // finder, and `q` quits again once it is closed.
            (KeyCode::Char(character), false) if !alt => {
                self.finder.type_char(character);
                selected = 0;
            }
            _ => {}
        }
        self.overlay = Overlay::Finder { selected };
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

    /// Follow the link on the cursor line.  One link is followed at once;
    /// several open a chooser.  `quiet` (Space) says nothing when the line has
    /// no link, so the key can be tapped freely; Enter explains.
    /// Open the actions menu for the entry selected in the explorer.
    fn open_file_menu(&mut self) {
        let Some(node) = self.explorer.selected_node() else {
            return;
        };
        let is_dir = node.is_dir;
        let mut actions = vec![
            FileAction::Open,
            FileAction::VsCode,
            FileAction::Terminal,
            FileAction::DefaultApp,
        ];
        if !is_dir {
            actions.push(FileAction::Edit);
        }
        actions.push(FileAction::CopyPath);
        actions.push(FileAction::CopyName);
        if !is_dir {
            actions.push(FileAction::Export);
        }
        self.file_menu = Some(FileMenu {
            path: node.path.clone(),
            name: node.name.clone(),
            is_dir,
            actions,
            submenu: false,
            anchored: true,
        });
        self.overlay = Overlay::FileMenu { selected: 0 };
    }

    fn close_file_menu(&mut self) {
        self.file_menu = None;
        self.overlay = Overlay::None;
    }

    /// Run entry `index` of the file actions menu.
    fn choose_file_action(&mut self, index: usize) {
        let Some(menu) = self.file_menu.clone() else {
            self.overlay = Overlay::None;
            return;
        };
        let Some(action) = menu.actions.get(index).copied() else {
            return;
        };
        if action == FileAction::Export {
            // Stay in the menu, now listing the formats.
            self.file_menu = Some(FileMenu {
                actions: vec![FileAction::ExportHtml, FileAction::ExportPdf],
                submenu: true,
                ..menu
            });
            self.overlay = Overlay::FileMenu { selected: 0 };
            return;
        }
        self.close_file_menu();
        let shown = |what: &str| format!("{what}: {}", menu.name);
        let launched = |app: &mut App, program: Launch, path: &Path, what: &str| {
            match (app.launcher)(program, path) {
                Ok(()) => app.set_status(shown(what)),
                Err(reason) => app.set_status(format!("{what} failed ({reason})")),
            }
        };
        match action {
            FileAction::Open => {
                self.activate();
                self.preview_selection();
            }
            FileAction::VsCode => launched(self, Launch::VsCode, &menu.path, "Opened in VS Code"),
            FileAction::Terminal => {
                let dir = if menu.is_dir {
                    menu.path.clone()
                } else {
                    menu.path.parent().map(Path::to_path_buf).unwrap_or_else(|| menu.path.clone())
                };
                launched(self, Launch::Terminal, &dir, "Opened a terminal in");
            }
            FileAction::DefaultApp => {
                launched(self, Launch::DefaultApp, &menu.path, "Opened with the default app")
            }
            FileAction::Edit => self.edit_selected(),
            FileAction::CopyPath => {
                let text = menu.path.display().to_string();
                self.clipboard_note = Some(format!("Copied path: {text}"));
                self.clipboard = Some(text);
            }
            FileAction::CopyName => {
                self.clipboard_note = Some(format!("Copied name: {}", menu.name));
                self.clipboard = Some(menu.name.clone());
            }
            FileAction::Export => {}
            FileAction::ExportHtml => self.start_export(&menu.path, Format::Html),
            FileAction::ExportPdf => self.start_export(&menu.path, Format::Pdf),
        }
    }

    /// `Y`: copy the Markdown source behind the text selected in the viewer.
    fn copy_selection_as_markdown(&mut self) {
        let Some(selection) = self.ended_selection.take().filter(|selection| selection.done && !selection.is_empty()) else {
            self.set_status("Select text in the document with the mouse first, then press Y");
            return;
        };
        if selection.area != self.layout.viewer {
            self.set_status("The selection is not in the document");
            return;
        }
        let rows = self.viewer.doc.lines.len();
        let top = self.viewer.top;
        let row_of = |y: u16| top + usize::from(y.saturating_sub(selection.area.y));
        let first = row_of(selection.anchor.1.min(selection.head.1));
        let last = row_of(selection.anchor.1.max(selection.head.1));
        if first >= rows {
            self.set_status("The selection is empty");
            return;
        }
        let last = last.min(rows - 1);
        let Some(range) = self.viewer.source_range_of_rows(first, last) else {
            self.set_status("Nothing to copy as Markdown here");
            return;
        };
        let Some(text) = self.viewer.source_lines(range.start, range.end) else {
            self.set_status("Nothing to copy as Markdown here");
            return;
        };
        let (from, to) = (range.start + 1, range.end + 1);
        let place = if from == to { format!("line {from}") } else { format!("lines {from}-{to}") };
        let what = if self.viewer.markdown_file() { "Markdown" } else { "source" };
        self.clipboard_note = Some(if range.exact {
            format!("Copied {what}: {place}")
        } else {
            format!("Copied {place} as whole blocks: the selection could not be mapped exactly")
        });
        self.clipboard = Some(text);
    }

    /// Leave the Export submenu: back to the file's actions, on `Export...`.
    fn back_to_file_actions(&mut self) {
        let Some(menu) = self.file_menu.take() else {
            self.overlay = Overlay::None;
            return;
        };
        let mut actions = vec![
            FileAction::Open,
            FileAction::VsCode,
            FileAction::Terminal,
            FileAction::DefaultApp,
            FileAction::Edit,
            FileAction::CopyPath,
            FileAction::CopyName,
            FileAction::Export,
        ];
        if !menu.anchored {
            // Opened by the shortcut: there is no menu behind it.
            self.overlay = Overlay::None;
            return;
        }
        let selected = actions.len() - 1;
        actions.retain(|action| *action != FileAction::Edit);
        let selected = selected.min(actions.len() - 1);
        self.file_menu = Some(FileMenu {
            actions,
            submenu: false,
            ..menu
        });
        self.overlay = Overlay::FileMenu { selected };
    }

    // ---------------------------------------------------------------- export

    /// Where the export of `source` is written: the configured folder, or the
    /// folder of the original.
    pub fn export_dir_for(&self, source: &Path) -> PathBuf {
        let configured = self
            .config
            .export_dir
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        match configured {
            Some(text) => {
                let expanded = match text.strip_prefix("~/").or_else(|| (text == "~").then_some("")) {
                    Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(text), |home| home.join(rest)),
                    None => PathBuf::from(text),
                };
                if expanded.is_absolute() {
                    expanded
                } else {
                    self.root.join(expanded)
                }
            }
            None => source.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
        }
    }

    /// Export `source` on a background thread; the result goes to the status bar.
    pub fn start_export(&mut self, source: &Path, format: Format) {
        if self.export_job.is_some() {
            self.set_status("An export is already running");
            return;
        }
        let request = export::Request {
            source: source.to_path_buf(),
            root: self.root.clone(),
            format,
            out_dir: self.export_dir_for(source),
            theme: self.theme(),
            path_var: self
                .export_path_var
                .clone()
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default(),
            timeout: export::CONVERTER_TIMEOUT,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("rtui-export".to_string())
            .spawn(move || {
                let _ = sender.send(export::run(&request));
            });
        match spawned {
            Ok(_) => {
                self.export_job = Some(receiver);
                self.set_status(format!("Exporting {}…", format.label()));
            }
            Err(error) => self.set_status(format!("Cannot export: {error}")),
        }
    }

    fn poll_export(&mut self) {
        let Some(receiver) = &self.export_job else {
            return;
        };
        match receiver.try_recv() {
            Ok(outcome) => {
                self.export_job = None;
                if outcome.written.is_some() {
                    // A new file appeared: the explorer and the finder should know.
                    self.finder.invalidate();
                    self.explorer.refresh();
                }
                self.set_status(outcome.message);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.export_job = None;
                self.set_status("The export stopped unexpectedly");
            }
        }
    }

    /// The shortcut: export the open document (HTML or PDF, asked in a menu).
    fn open_export_menu(&mut self) {
        let Some(loaded) = self.viewer.loaded.clone() else {
            self.set_status("No file open to export");
            return;
        };
        let name = loaded
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.file_menu = Some(FileMenu {
            path: loaded.path.clone(),
            name,
            is_dir: false,
            actions: vec![FileAction::ExportHtml, FileAction::ExportPdf],
            submenu: true,
            anchored: false,
        });
        self.overlay = Overlay::FileMenu { selected: 0 };
    }

    /// Copy the code block under the cursor.  Returns false when the cursor is
    /// not on one, so the caller can fall back to its usual action.
    fn copy_code_block(&mut self) -> bool {
        let cursor = self.viewer.cursor;
        let Some(region) = self.viewer.doc.code_block_at(cursor) else {
            return false;
        };
        let code = region.code.clone();
        let lines = code.lines().count().max(1);
        self.clipboard_note = Some(format!(
            "Copied code block ({lines} {})",
            if lines == 1 { "line" } else { "lines" }
        ));
        self.clipboard = Some(code);
        true
    }

    fn follow_link(&mut self, quiet: bool) {
        // A link on the line wins; otherwise a code block under the cursor is
        // copied.
        let cursor = self.viewer.cursor;
        let mut on_line: Vec<DocLink> = self
            .viewer
            .doc
            .links
            .iter()
            .filter(|link| link.line == cursor)
            .cloned()
            .collect();
        on_line.sort_by_key(|link| link.column);

        let first_image = on_line.iter().find(|link| link.image).cloned();
        let mut links: Vec<DocLink> = Vec::new();
        for link in on_line.into_iter().filter(|link| !link.image) {
            // The same target twice on a line is one choice.
            if !links
                .iter()
                .any(|seen| seen.dest == link.dest && seen.kind == link.kind)
            {
                links.push(link);
            }
        }

        match links.len() {
            0 => {
                if first_image.is_none() && self.copy_code_block() {
                    // Copied: main.rs sends it to the terminal's clipboard.
                } else if let Some(image) = first_image {
                    self.set_status(format!(
                        "Image: {} (images are not rendered)",
                        image.dest
                    ));
                } else if !quiet {
                    self.set_status("No link on this line");
                }
            }
            1 => self.follow_one(links.remove(0)),
            _ => {
                self.link_choices = links;
                self.overlay = Overlay::Links { selected: 0 };
            }
        }
    }

    /// Follow a link chosen in the chooser.
    fn choose_link(&mut self, index: usize) {
        let link = self.link_choices.get(index).cloned();
        self.link_choices.clear();
        self.overlay = Overlay::None;
        if let Some(link) = link {
            self.follow_one(link);
        }
    }

    /// Nothing is ever executed from a link: documents inside the project are
    /// opened in the viewer, `http(s)` and `mailto:` addresses go to the
    /// browser, everything else is refused with a message.
    fn follow_one(&mut self, link: DocLink) {
        match link.kind {
            LinkKind::External => {
                let dest = link.dest.trim();
                if crate::browser::is_openable(dest) {
                    self.open_url(dest);
                } else {
                    self.set_status(format!(
                        "Not opening {dest}: only http(s) and mailto links are opened"
                    ));
                }
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
        if has_url_scheme(&path_part) {
            self.set_status(format!(
                "Not opening {dest}: only http(s) and mailto links are opened"
            ));
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

        if candidate.exists() && self.outside_project(&candidate) {
            self.set_status(format!(
                "Link leaves the project folder: {path_part} (not followed)"
            ));
            return;
        }

        if candidate.is_dir() {
            self.focus_folder(&candidate);
        } else if candidate.is_file() {
            self.open_file(candidate, fragment.map(str::to_string), true);
        } else {
            self.set_status(format!("Cannot find linked file: {path_part}"));
        }
    }

    /// Does `path` resolve (through `..` and symlinks) to somewhere outside the
    /// project root?
    fn outside_project(&self, path: &Path) -> bool {
        match (path.canonicalize(), self.root.canonicalize()) {
            (Ok(path), Ok(root)) => !path.starts_with(root),
            _ => false,
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
        // Typing ends a text selection.  `Y` still needs to know what it was.
        self.ended_selection = self.selection.take();

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
        // Pasting into the file finder types into its query.
        if let Overlay::Finder { .. } = self.overlay {
            for character in text.chars().filter(|character| !character.is_control()) {
                self.finder.type_char(character);
            }
            self.overlay = Overlay::Finder { selected: 0 };
            return;
        }
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
            Action::FindFile => self.open_finder(),
            Action::Backlinks => self.open_backlinks(),
            Action::Export => self.open_export_menu(),
            Action::CopyMarkdown => self.copy_selection_as_markdown(),
            Action::Bookmark => self.toggle_bookmark(),
            Action::Bookmarks => self.open_bookmarks(),
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
                    self.finder.invalidate();
                    self.backlinks.invalidate();
                    self.explorer.refresh();
                    self.preview_selection();
                    self.set_status("Project tree refreshed");
                }
                Focus::Viewer => {
                    self.finder.invalidate();
                    self.backlinks.invalidate();
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
            Action::FollowLink => match self.focus {
                Focus::Viewer => self.follow_link(true),
                // In the explorer Space keeps opening files and folding folders.
                Focus::Explorer => {
                    self.activate();
                    self.preview_selection();
                }
                Focus::Terminal => {}
            },
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
            // In the explorer Enter offers actions for the entry; Space still
            // opens files and folds folders.
            KeyCode::Enter if self.focus == Focus::Explorer => self.open_file_menu(),
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
        if !self.config.mouse {
            return;
        }
        let (x, y) = (event.column, event.row);
        // The star count (header or Esc menu) opens the repository.
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self.star_rect.get().is_some_and(|rect| rect.contains((x, y).into()))
        {
            self.open_url(crate::stars::REPO_URL);
            return;
        }
        if self.overlay.is_open() {
            self.handle_overlay_mouse(event);
            return;
        }
        if self.search_input.is_some() {
            return;
        }
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

    /// Record a left click and say whether it completes a double click.
    fn register_click(&mut self, x: u16, y: u16) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(when, last_x, last_y)| {
            now.duration_since(when) <= DOUBLE_CLICK && last_y == y && last_x.abs_diff(x) <= 3
        });
        self.last_click = if double { None } else { Some((now, x, y)) };
        double
    }

    /// Click in the explorer: the chevron of a folder folds it without
    /// touching the open document, the name selects (and previews a file),
    /// and a double click opens a file or folds the folder.
    fn explorer_click(&mut self, x: u16, y: u16, rect: Rect) {
        let double = self.register_click(x, y);
        let position = self.explorer.offset() + (y - rect.y) as usize;
        let Some(&node_index) = self.explorer.visible().get(position) else {
            return;
        };
        let on_chevron = self.explorer.tree.node(node_index).is_some_and(|node| {
            crate::ui::explorer::chevron_columns(node, self.config.icon_style())
                .is_some_and(|(start, width)| x >= rect.x + start && x < rect.x + start + width)
        });
        self.explorer.select_position(position);
        if on_chevron {
            self.last_click = None;
            self.explorer.toggle_selected();
        } else if double {
            self.activate();
            self.preview_selection();
        } else {
            self.preview_selection();
        }
    }

    /// Note a clickable region while drawing.
    pub fn record_hit(&self, rect: Rect, hit: Hit) {
        self.hits.borrow_mut().push((rect, hit));
    }

    fn hit_at(&self, x: u16, y: u16) -> Option<Hit> {
        self.hits
            .borrow()
            .iter()
            .rev()
            .find(|(rect, hit)| *hit != Hit::Panel && rect.contains((x, y).into()))
            .map(|(_, hit)| *hit)
    }

    /// Is the pointer outside every popup frame (and is there one at all)?
    fn outside_popups(&self, x: u16, y: u16) -> bool {
        let hits = self.hits.borrow();
        let mut any = false;
        for (rect, hit) in hits.iter() {
            if *hit == Hit::Panel {
                any = true;
                if rect.contains((x, y).into()) {
                    return false;
                }
            }
        }
        any
    }

    fn press_overlay_key(&mut self, code: KeyCode) {
        self.handle_overlay_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn handle_overlay_mouse(&mut self, event: MouseEvent) {
        // While the prefix key is being captured only a key press counts.
        if self.capturing_prefix {
            return;
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.overlay_wheel(false),
            MouseEventKind::ScrollDown => self.overlay_wheel(true),
            MouseEventKind::Down(MouseButton::Left) => self.overlay_click(event.column, event.row),
            _ => {}
        }
    }

    fn overlay_wheel(&mut self, down: bool) {
        let (code, times) = match (self.overlay, down) {
            (Overlay::Help { .. }, true) => (KeyCode::Down, WHEEL_LINES as usize),
            (Overlay::Help { .. }, false) => (KeyCode::Up, WHEEL_LINES as usize),
            (_, true) => (KeyCode::Down, 1),
            (_, false) => (KeyCode::Up, 1),
        };
        for _ in 0..times {
            self.press_overlay_key(code);
        }
    }

    fn overlay_click(&mut self, x: u16, y: u16) {
        let double = self.register_click(x, y);
        let hit = self.hit_at(x, y);
        match (self.overlay, hit) {
            (Overlay::Menu { .. }, Some(Hit::MenuItem(index))) => {
                self.overlay = Overlay::Menu { selected: index };
                self.press_overlay_key(KeyCode::Enter);
            }
            (Overlay::Options { .. }, Some(Hit::OptionsTab(index))) => {
                self.adjusting = false;
                self.overlay = Overlay::Options { tab: index, selected: 0 };
            }
            (Overlay::Options { tab, .. }, Some(Hit::Option(index))) => {
                self.adjusting = false;
                self.overlay = Overlay::Options { tab, selected: index };
                if double {
                    self.press_overlay_key(KeyCode::Enter);
                }
            }
            (Overlay::Options { tab, .. }, Some(Hit::OptionValue(index))) => {
                self.adjusting = false;
                self.overlay = Overlay::Options { tab, selected: index };
                self.press_overlay_key(KeyCode::Enter);
            }
            (Overlay::Themes { selected, .. }, Some(Hit::Theme(index))) => {
                // Walk there with the arrows so the live preview follows.
                let code = if index >= selected { KeyCode::Down } else { KeyCode::Up };
                for _ in 0..index.abs_diff(selected) {
                    self.press_overlay_key(code);
                }
                self.press_overlay_key(KeyCode::Enter);
            }
            (Overlay::Outline { .. }, Some(Hit::Outline(index))) => {
                self.overlay = Overlay::Outline { selected: index };
                self.press_overlay_key(KeyCode::Enter);
            }
            (Overlay::Finder { .. }, Some(Hit::Finder(index))) => self.choose_finder_hit(index),
            (Overlay::Backlinks { .. }, Some(Hit::Backlink(index))) => self.choose_backlink(index),
            (Overlay::Bookmarks { tab, .. }, Some(Hit::BookmarkRow(index))) => {
                self.choose_list_entry(tab, index)
            }
            (Overlay::Bookmarks { .. }, Some(Hit::BookmarkTab(tab))) => {
                self.overlay = Overlay::Bookmarks { tab, selected: 0 }
            }
            (Overlay::Links { .. }, Some(Hit::Link(index))) => self.choose_link(index),
            (Overlay::FileMenu { .. }, Some(Hit::FileAction(index))) => self.choose_file_action(index),
            (_, None) if self.outside_popups(x, y) => self.press_overlay_key(KeyCode::Esc),
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
            Focus::Explorer => self.explorer_click(x, y, rect),
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
    /// Open a web address in the browser.  Without a browser (a server over
    /// SSH, say) the address is copied with OSC 52 instead, and the status bar
    /// says so.
    pub fn open_url(&mut self, url: &str) {
        match (self.opener)(url) {
            Ok(()) => self.set_status(format!("Opened {url} in the browser")),
            Err(reason) => {
                let note = format!("No browser ({reason}); copied {url}");
                self.clipboard = Some(url.to_string());
                self.clipboard_note = Some(note.clone());
                self.set_status(note);
            }
        }
    }

    /// Status message for a copy just handed to the terminal.
    pub fn copy_status(&mut self, text: &str) -> String {
        self.clipboard_note
            .take()
            .unwrap_or_else(|| format!("Copied {} characters", text.chars().count()))
    }

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
            Overlay::Finder { selected } => self.handle_finder_key(key, selected),
            Overlay::Backlinks { selected } => self.handle_backlinks_key(key, selected),
            Overlay::Bookmarks { tab, selected } => self.handle_bookmarks_key(key, tab, selected),
            Overlay::FileMenu { selected } => {
                let count = self.file_menu.as_ref().map_or(0, |menu| menu.actions.len());
                let submenu = self.file_menu.as_ref().is_some_and(|menu| menu.submenu);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') if submenu => self.back_to_file_actions(),
                    KeyCode::Char('h') if submenu => self.choose_file_action(0),
                    KeyCode::Char('p') if submenu => self.choose_file_action(1),
                    KeyCode::Esc | KeyCode::Char('q') => self.close_file_menu(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.overlay = Overlay::FileMenu {
                            selected: (selected + 1).min(count.saturating_sub(1)),
                        };
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.overlay = Overlay::FileMenu {
                            selected: selected.saturating_sub(1),
                        };
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => self.choose_file_action(selected),
                    KeyCode::Char(digit @ '1'..='9') => {
                        let index = digit as usize - '1' as usize;
                        if index < count {
                            self.choose_file_action(index);
                        }
                    }
                    _ => {}
                }
            }
            Overlay::Links { selected } => {
                let count = self.link_choices.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.link_choices.clear();
                        self.overlay = Overlay::None;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.overlay = Overlay::Links {
                            selected: (selected + 1).min(count.saturating_sub(1)),
                        };
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.overlay = Overlay::Links {
                            selected: selected.saturating_sub(1),
                        };
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => self.choose_link(selected),
                    KeyCode::Char(digit @ '1'..='9') => {
                        let index = digit as usize - '1' as usize;
                        if index < count {
                            self.choose_link(index);
                        }
                    }
                    _ => {}
                }
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
                        self.load_into_viewer(path.clone(), None, true);
                    }
                    self.focus = Focus::Viewer;
                    self.note_recent(&path);
                } else {
                    self.preview_selection();
                    if let Some(explorer_rect) = self.layout.explorer {
                        self.explorer
                            .set_viewport_height(explorer_rect.height as usize);
                    }
                }
            }
            Focus::Viewer => self.follow_link(false),
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
        self.finder.invalidate();
        self.backlinks.invalidate();
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

/// Does a link target start with a URL scheme (`javascript:`, `file:`, …)?
/// Single letters are drive names (`C:\\notes`), not schemes.
pub(crate) fn has_url_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    scheme.len() >= 2
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
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

    fn open_code_doc(app: &mut App, dir: &tempfile::TempDir) {
        let path = dir.path().join("code.md");
        fs::write(
            &path,
            b"# Code\n\nintro text\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nafter\n",
        )
        .unwrap();
        app.load_into_viewer(path, None, true);
        app.focus = Focus::Viewer;
    }

    #[test]
    fn enter_and_space_copy_the_code_block_under_the_cursor() {
        let dir = project();
        let mut app = app(&dir);
        open_code_doc(&mut app, &dir);
        let region = app.viewer.doc.code_blocks.first().cloned().expect("one code block");
        assert_eq!(region.code, "fn main() {\n    println!(\"hi\");\n}");

        for key in [KeyCode::Enter, KeyCode::Char(' ')] {
            // Anywhere in the block, frame included.
            for line in [region.start, region.start + 2, region.end - 1] {
                app.viewer.cursor = line;
                press(&mut app, key);
                assert_eq!(app.take_clipboard().as_deref(), Some(region.code.as_str()), "{key:?} on row {line}");
                assert!(app.clipboard_note.as_deref().unwrap_or("").contains("3 lines"));
                app.clipboard_note = None;
            }
        }

        // Outside the block nothing is copied.
        app.viewer.cursor = 2;
        press(&mut app, KeyCode::Enter);
        assert!(app.take_clipboard().is_none());
    }

    #[test]
    fn ctrl_c_does_not_quit_and_q_does() {
        let dir = project();
        let mut app = app(&dir);
        for focus in [Focus::Explorer, Focus::Viewer] {
            app.focus = focus;
            press_ctrl(&mut app, 'c');
            assert!(!app.should_quit, "Ctrl+C with {focus:?} focused");
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit);
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
        // Enter offers actions; the first one opens the file.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::FileMenu { selected: 0 });
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.focus, Focus::Viewer);
        assert_eq!(
            app.viewer.loaded.as_ref().unwrap().display_name(),
            "plain.txt"
        );
    }

    thread_local! {
        static LAUNCHED: std::cell::RefCell<Vec<(Launch, PathBuf)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn stub_launcher(program: Launch, path: &Path) -> Result<(), String> {
        LAUNCHED.with(|log| log.borrow_mut().push((program, path.to_path_buf())));
        Ok(())
    }
    fn launched() -> Vec<(Launch, PathBuf)> {
        LAUNCHED.with(|log| log.borrow().clone())
    }
    fn failing_launcher(_: Launch, _: &Path) -> Result<(), String> {
        Err("cannot run code: not found".to_string())
    }

    fn menu_app(dir: &tempfile::TempDir, select: &str) -> App {
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs").join("guide.md"), b"# Guide\n").unwrap();
        let mut app = app(dir);
        app.explorer.refresh();
        app.launcher = stub_launcher;
        LAUNCHED.with(|log| log.borrow_mut().clear());
        assert!(app.explorer.select_visible_path(&dir.path().join(select)));
        app
    }

    #[test]
    fn enter_in_the_explorer_opens_the_actions_for_the_entry() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        press(&mut app, KeyCode::Enter);
        let menu = app.file_menu.clone().expect("menu");
        assert!(!menu.is_dir);
        assert_eq!(
            menu.actions,
            [
                FileAction::Open,
                FileAction::VsCode,
                FileAction::Terminal,
                FileAction::DefaultApp,
                FileAction::Edit,
                FileAction::CopyPath,
                FileAction::CopyName,
                FileAction::Export,
            ]
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.file_menu.is_none());
        assert_eq!(app.focus, Focus::Explorer, "Esc only closes the menu");

        // A folder has no Edit entry.
        assert!(app.explorer.select_visible_path(&dir.path().join("docs")));
        press(&mut app, KeyCode::Enter);
        let menu = app.file_menu.clone().unwrap();
        assert!(menu.is_dir);
        assert!(!menu.actions.contains(&FileAction::Edit));
    }

    #[test]
    fn space_still_opens_files_straight_away() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.viewer.loaded.as_ref().unwrap().display_name(), "plain.txt");
    }

    #[test]
    fn the_menu_opens_vs_code_and_a_terminal_in_the_right_place() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        let file = dir.path().join("plain.txt");

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down); // Open in VS Code
        press(&mut app, KeyCode::Enter);
        assert_eq!(launched(), vec![(Launch::VsCode, file.clone())]);
        assert!(app.status.as_ref().unwrap().text.contains("VS Code"));
        assert_eq!(app.overlay, Overlay::None);

        // A terminal for a file starts in its folder (picked with its number).
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(launched()[1], (Launch::Terminal, dir.path().to_path_buf()));

        // ...and for a folder, in the folder itself.
        assert!(app.explorer.select_visible_path(&dir.path().join("docs")));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(launched()[2], (Launch::Terminal, dir.path().join("docs")));
    }

    #[test]
    fn a_failing_program_is_reported_in_the_status_bar() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        app.launcher = failing_launcher;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('2'));
        let status = app.status.as_ref().unwrap().text.clone();
        assert!(status.contains("failed") && status.contains("not found"), "{status}");
    }

    #[test]
    fn the_menu_copies_the_path_and_the_name() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('6'));
        assert_eq!(
            app.take_clipboard().unwrap(),
            dir.path().join("plain.txt").display().to_string()
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('7'));
        assert_eq!(app.take_clipboard().unwrap(), "plain.txt");
    }

    #[test]
    fn clicking_an_action_runs_it() {
        let dir = project();
        let mut app = menu_app(&dir, "plain.txt");
        press(&mut app, KeyCode::Enter);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let rect = app
            .hits
            .borrow()
            .iter()
            .find(|(_, hit)| *hit == Hit::FileAction(1))
            .map(|(rect, _)| *rect)
            .expect("row recorded");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 3, rect.y));
        assert_eq!(launched(), vec![(Launch::VsCode, dir.path().join("plain.txt"))]);
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
        // The README line holds two links: a chooser opens, the first is the
        // relative Markdown link.
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Links { .. }));
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
        // Two links on the line: pick the second, the [[wikilink]], by number.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.overlay, Overlay::None);
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
    fn external_links_open_in_the_browser() {
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
        app.opener = stub_opener;
        OPENED.with(|log| log.borrow_mut().clear());
        press(&mut app, KeyCode::Enter);
        OPENED.with(|log| assert_eq!(*log.borrow(), vec!["https://example.com".to_string()]));
        assert!(app.status_text().unwrap().contains("Opened"));
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

        // Quitting cannot be rebound: `x` does nothing, only `q` quits.
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.should_quit, "quit is not configurable");
        press(&mut app, KeyCode::F(4));
        assert!(!app.show_term);
        press_ctrl(&mut app, 'a');
        press(&mut app, KeyCode::Left);
        assert_eq!(app.explorer.width_pct(), DEFAULT_WIDTH_PCT - 2, "the new prefix works");
        // The old prefix no longer starts a sequence.
        press_ctrl(&mut app, 'r');
        assert!(app.pending.is_none());
        press(&mut app, KeyCode::Char('q'));
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
        ] {
            app.handle_key(key);
            assert!(app.capturing_prefix, "{key:?} must not end the capture");
            assert_eq!(app.config.prefix, "ctrl+r");
        }
        assert!(!app.should_quit, "q did not reach the shortcuts");
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

    fn tick_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
        let started = Instant::now();
        while !done(app) {
            app.tick();
            assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn indexing_shows_only_after_the_grace_period_and_clears_when_done() {
        let dir = project();
        let mut app = app(&dir);
        assert!(!app.is_busy() && !app.indexing());
        app.explorer.tree.set_listing_delay(Duration::from_millis(500));
        app.explorer.tree.set_load_budget(Duration::ZERO);
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        app.explorer.tree.expand(notes);

        assert!(app.is_busy());
        assert!(!app.indexing(), "quick work must not flash the indicator");
        std::thread::sleep(INDEXING_GRACE + Duration::from_millis(30));
        assert!(app.indexing());
        assert!(!app.spinner().is_empty());

        tick_until(&mut app, "the listing", |app| !app.is_busy());
        assert!(!app.indexing());
        assert!(app.explorer.tree.find_path(&dir.path().join("notes/guide.md")).is_some());
    }

    #[test]
    fn the_spinner_turns_and_has_an_ascii_form() {
        let dir = project();
        let mut app = app(&dir);
        app.explorer.tree.set_listing_delay(Duration::from_millis(600));
        app.explorer.tree.set_load_budget(Duration::ZERO);
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        app.explorer.tree.expand(notes);
        let first = app.spinner();
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(app.spinner(), first, "the frame advances");
        app.config.set_icon_style(crate::ui::icons::IconStyle::Ascii);
        assert!(["|", "/", "-", "\\"].contains(&app.spinner()));
    }

    #[test]
    fn a_broken_watcher_is_announced_once_and_marked_in_the_status_bar() {
        let dir = project();
        let mut app = app(&dir);
        app.watcher = Some(FileWatcher::failed("the system's file-watch limit was reached"));
        app.tick();
        let status = app.status_text().expect("a message").to_string();
        assert!(status.contains("Live refresh limited") && status.contains("file-watch limit"), "{status}");
        assert!(status.contains("refresh by hand"));
        assert!(app.watcher_problem().is_some());

        // Said once: a later message is not overwritten.
        app.set_status("something else");
        app.tick();
        assert_eq!(app.status_text(), Some("something else"));

        // The program keeps working: a manual refresh still sees new files.
        fs::write(dir.path().join("after.md"), b"x\n").unwrap();
        app.explorer.refresh();
        assert!(app.explorer.tree.find_path(&dir.path().join("after.md")).is_some());
    }

    #[test]
    fn new_files_show_up_through_the_background_watcher() {
        let dir = project();
        let mut app = app(&dir);
        tick_until(&mut app, "the watcher", |app| {
            app.watcher.as_ref().is_some_and(|w| w.state() != WatcherState::Starting)
        });
        fs::write(dir.path().join("fresh.md"), b"# Fresh\n").unwrap();
        tick_until(&mut app, "the new file", |app| {
            app.explorer.tree.find_path(&dir.path().join("fresh.md")).is_some()
        });
    }

    #[test]
    fn the_watcher_follows_the_folders_that_are_open() {
        let dir = project();
        let mut app = app(&dir);
        let key = app.watch_key.clone().expect("synced at start");
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        app.explorer.tree.expand(notes);
        app.tick();
        assert_ne!(app.watch_key, Some(key), "opening a folder re-syncs the watcher");
        // A change inside the newly opened folder is noticed.
        tick_until(&mut app, "the watcher", |app| {
            app.watcher.as_ref().is_some_and(|w| w.state() == WatcherState::Ready { dirs: 2 })
        });
        fs::write(dir.path().join("notes/extra.md"), b"x\n").unwrap();
        tick_until(&mut app, "the new file", |app| {
            app.explorer.tree.find_path(&dir.path().join("notes/extra.md")).is_some()
        });
    }

    #[test]
    fn a_stream_of_changes_cannot_postpone_the_refresh_forever() {
        let dir = project();
        let mut app = app(&dir);
        tick_until(&mut app, "the watcher", |app| {
            app.watcher.as_ref().is_some_and(|w| w.state() != WatcherState::Starting)
        });
        let started = Instant::now();
        let mut number = 0;
        // Keep touching the folder faster than the debounce window.
        while app.explorer.tree.find_path(&dir.path().join("burst0.md")).is_none() {
            fs::write(dir.path().join(format!("burst{number}.md")), b"x\n").unwrap();
            number += 1;
            std::thread::sleep(Duration::from_millis(40));
            app.tick();
            assert!(started.elapsed() < Duration::from_secs(5), "starved by constant events");
        }
    }

    // ------------------------------------------------- following links (Space)

    thread_local! {
        static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn stub_opener(url: &str) -> Result<(), String> {
        OPENED.with(|log| log.borrow_mut().push(url.to_string()));
        Ok(())
    }
    fn opened() -> Vec<String> {
        OPENED.with(|log| log.borrow().clone())
    }

    /// An app over a temporary project made of `files`, with the first one
    /// open, focus in the document and the browser stubbed out.
    fn linked(files: &[(&str, &str)]) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let mut app = app(&dir);
        app.opener = stub_opener;
        OPENED.with(|log| log.borrow_mut().clear());
        app.focus = Focus::Viewer;
        (dir, app)
    }

    fn goto_link(app: &mut App, dest_fragment: &str) {
        let line = line_of_link(app, dest_fragment);
        app.viewer.jump_to_line(line);
        app.focus = Focus::Viewer;
    }

    fn open_name(app: &App) -> String {
        app.viewer.loaded.as_ref().unwrap().display_name().to_string()
    }

    #[test]
    fn space_is_the_default_binding_and_help_lists_it() {
        let dir = project();
        let app = app(&dir);
        assert_eq!(app.keymap.labels(Action::FollowLink), vec!["Space".to_string()]);
        assert!(crate::ui::help::rendered_text(&app.keymap).contains("Follow the link on the line"));
    }

    #[test]
    fn space_follows_a_relative_link_with_history_and_explorer_sync() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\nGo to [the guide](notes/guide.md).\n"),
            ("notes/guide.md", "# Guide\n"),
        ]);
        goto_link(&mut app, "notes/guide.md");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(open_name(&app), "guide.md");
        assert_eq!(app.history.len(), 1, "the jump is in the history");
        assert_eq!(
            app.explorer.selected_path().unwrap().file_name().unwrap(),
            "guide.md",
            "the explorer follows the document"
        );
        press_ctrl(&mut app, 'o');
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn space_handles_encoded_spaces_and_jumps_to_the_heading() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\nSee [deep](my%20notes/Deep%20Doc.md#section-two).\n"),
            (
                "my notes/Deep Doc.md",
                "# Deep\n\ntext\n\n## Section One\n\nx\n\n## Section Two\n\ny\n",
            ),
        ]);
        goto_link(&mut app, "Deep%20Doc.md");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(open_name(&app), "Deep Doc.md");
        assert_eq!(app.viewer.heading_nearest_cursor().unwrap().text, "Section Two");
    }

    #[test]
    fn space_follows_an_internal_anchor() {
        let (_dir, mut app) = linked(&[(
            "README.md",
            "# Home\n\nJump to [details](#details).\n\n## Details\n\ntext\n",
        )]);
        goto_link(&mut app, "#details");
        let before = app.viewer.cursor;
        press(&mut app, KeyCode::Char(' '));
        assert!(app.viewer.cursor > before);
        assert_eq!(app.viewer.heading_nearest_cursor().unwrap().text, "Details");
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn space_opens_web_and_mail_links_in_the_browser_only() {
        let (_dir, mut app) = linked(&[(
            "README.md",
            "# L\n\n[web](https://example.com/a?b=1)\n\n[mail](mailto:me@example.com)\n\n[ftp](ftp://example.com/x)\n\n[js](javascript:alert(1))\n\n[file](file:///etc/passwd)\n",
        )]);
        goto_link(&mut app, "https://example.com");
        press(&mut app, KeyCode::Char(' '));
        goto_link(&mut app, "mailto:");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            opened(),
            vec!["https://example.com/a?b=1".to_string(), "mailto:me@example.com".to_string()]
        );

        for dest in ["ftp://", "javascript:", "file:///"] {
            goto_link(&mut app, dest);
            press(&mut app, KeyCode::Char(' '));
            let status = app.status_text().unwrap().to_string();
            assert!(status.contains("Not opening"), "{dest}: {status}");
        }
        assert_eq!(opened().len(), 2, "nothing else reached the browser");
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn a_missing_destination_says_so_and_changes_nothing() {
        let (_dir, mut app) = linked(&[("README.md", "# Home\n\n[gone](notes/missing.md)\n")]);
        goto_link(&mut app, "missing.md");
        press(&mut app, KeyCode::Char(' '));
        let status = app.status_text().unwrap().to_string();
        assert!(status.contains("Cannot find linked file"), "{status}");
        assert_eq!(open_name(&app), "README.md");
        assert!(app.history.is_empty());
    }

    #[test]
    fn space_on_a_line_without_a_link_is_silent_but_enter_explains() {
        let (_dir, mut app) = linked(&[("README.md", "# Home\n\nplain text\n")]);
        app.viewer.jump_to_line(2);
        app.set_status("before");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.status_text(), Some("before"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.status_text(), Some("No link on this line"));
    }

    #[test]
    fn links_cannot_lead_out_of_the_project_root() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join("secret.md"), "# Secret\n").unwrap();
        let root = outer.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("README.md"),
            "# Home\n\n[up](../secret.md)\n\n[abs](/../secret.md)\n",
        )
        .unwrap();
        let mut app = App::with_config(Some(&root), Config::default());
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app.focus = Focus::Viewer;
        goto_link(&mut app, "../secret.md");
        press(&mut app, KeyCode::Char(' '));
        let status = app.status_text().unwrap().to_string();
        assert!(status.contains("leaves the project folder"), "{status}");
        assert_eq!(open_name(&app), "README.md");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_project_is_refused_too() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join("secret.md"), "# Secret\n").unwrap();
        let root = outer.path().join("project");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(outer.path().join("secret.md"), root.join("shortcut.md")).unwrap();
        fs::write(root.join("README.md"), "# Home\n\n[sneaky](shortcut.md)\n").unwrap();
        let mut app = App::with_config(Some(&root), Config::default());
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app.focus = Focus::Viewer;
        goto_link(&mut app, "shortcut.md");
        press(&mut app, KeyCode::Char(' '));
        assert!(app.status_text().unwrap().contains("leaves the project folder"));
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn several_links_on_a_line_open_a_chooser() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\n[a](a.md) · [b](b.md) · [site](https://example.com)\n"),
            ("a.md", "# A\n"),
            ("b.md", "# B\n"),
        ]);
        goto_link(&mut app, "a.md");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.overlay, Overlay::Links { selected: 0 });
        assert_eq!(app.link_choices.len(), 3);
        // Navigation inside the chooser, bounded at both ends.
        press(&mut app, KeyCode::Up);
        assert_eq!(app.overlay, Overlay::Links { selected: 0 });
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.overlay, Overlay::Links { selected: 2 });
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.link_choices.is_empty());
        assert_eq!(open_name(&app), "b.md");
    }

    #[test]
    fn the_chooser_can_pick_by_number_and_reach_the_browser_and_can_be_cancelled() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\n[a](a.md) [site](https://example.com)\n"),
            ("a.md", "# A\n"),
        ]);
        goto_link(&mut app, "a.md");
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "README.md");
        assert!(opened().is_empty());

        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('9'));
        assert!(matches!(app.overlay, Overlay::Links { .. }), "no such entry: still open");
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(opened(), vec!["https://example.com".to_string()]);
    }

    #[test]
    fn the_same_target_twice_on_a_line_is_followed_without_asking() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\n[one](a.md) and again [two](a.md)\n"),
            ("a.md", "# A\n"),
        ]);
        goto_link(&mut app, "a.md");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "a.md");
    }

    #[test]
    fn the_chooser_is_drawn_with_labels_and_targets() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\n[alpha](a.md) [beta](https://example.com)\n"),
            ("a.md", "# A\n"),
        ]);
        goto_link(&mut app, "a.md");
        press(&mut app, KeyCode::Char(' '));
        let text = crate::ui::render_to_text_for_test(&mut app, 100, 30);
        assert!(text.contains("Links on this line"), "{text}");
        assert!(text.contains("alpha") && text.contains("a.md"), "{text}");
        assert!(text.contains("beta") && text.contains("https://example.com"), "{text}");
    }

    #[test]
    fn space_in_the_explorer_still_opens_files_and_folds_folders() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n"),
            ("notes/guide.md", "# Guide\n"),
        ]);
        app.focus = Focus::Explorer;
        let notes = app.explorer.tree.find_path(&app.root.join("notes")).unwrap();
        let position = app.explorer.visible().iter().position(|&i| i == notes).unwrap();
        app.explorer.select_position(position);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.explorer.tree.is_expanded(notes));
        press(&mut app, KeyCode::Char(' '));
        assert!(!app.explorer.tree.is_expanded(notes));
    }

    #[test]
    fn space_types_a_space_in_the_search_box_and_does_nothing_in_code_files() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\nalpha beta\n"),
            ("main.rs", "fn main() { let a = \"https://example.com\"; }\n"),
        ]);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.search_input.as_deref(), Some("a b"));
        press(&mut app, KeyCode::Esc);

        app.open_file(app.root.join("main.rs"), None, true);
        app.set_status("before");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.status_text(), Some("before"));
        assert!(opened().is_empty());
    }

    // ------------------------------------------------ mouse in menus/explorer

    fn draw_now(app: &mut App) -> String {
        crate::ui::render_to_text_for_test(app, 120, 40)
    }

    fn send_mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) {
        app.handle_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn rect_of(app: &App, hit: Hit) -> Rect {
        app.hits
            .borrow()
            .iter()
            .find(|(_, candidate)| *candidate == hit)
            .map(|(rect, _)| *rect)
            .unwrap_or_else(|| panic!("{hit:?} was not drawn"))
    }

    fn click_on(app: &mut App, hit: Hit) {
        let rect = rect_of(app, hit);
        send_mouse(app, MouseEventKind::Down(MouseButton::Left), rect.x + rect.width / 2, rect.y);
        send_mouse(app, MouseEventKind::Up(MouseButton::Left), rect.x + rect.width / 2, rect.y);
    }

    fn wheel(app: &mut App, down: bool) {
        let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
        send_mouse(app, kind, 10, 10);
    }

    fn open_menu(app: &mut App) {
        press(app, KeyCode::Esc);
        app.finish_menu_animation();
        draw_now(app);
    }

    #[test]
    fn clicking_a_menu_entry_activates_it() {
        let dir = project();
        let mut app = app(&dir);
        open_menu(&mut app);
        click_on(&mut app, Hit::MenuItem(1));
        assert!(matches!(app.overlay, Overlay::Help { .. }), "{:?}", app.overlay);

        press(&mut app, KeyCode::Esc); // back to the menu
        draw_now(&mut app);
        click_on(&mut app, Hit::MenuItem(0));
        assert!(matches!(app.overlay, Overlay::Options { .. }), "{:?}", app.overlay);

        press(&mut app, KeyCode::Esc);
        draw_now(&mut app);
        click_on(&mut app, Hit::MenuItem(2));
        assert!(app.should_quit);
    }

    #[test]
    fn the_wheel_moves_the_menu_selection_and_clicks_elsewhere_do_nothing() {
        let dir = project();
        let mut app = app(&dir);
        open_menu(&mut app);
        let start = app.overlay;
        wheel(&mut app, true);
        assert_ne!(app.overlay, start);
        let after = app.overlay;
        wheel(&mut app, false);
        assert_eq!(app.overlay, start);
        let _ = after;
        // A click on empty space neither closes nor activates anything.
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 1, 1);
        assert_eq!(app.overlay, start);
        assert!(!app.should_quit);
    }

    #[test]
    fn menu_mouse_works_in_the_compact_layout_too() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Esc);
        app.finish_menu_animation();
        crate::ui::render_to_text_for_test(&mut app, 40, 14);
        click_on(&mut app, Hit::MenuItem(1));
        assert!(matches!(app.overlay, Overlay::Help { .. }));
    }

    fn open_options(app: &mut App) {
        open_menu(app);
        click_on(app, Hit::MenuItem(0));
        draw_now(app);
    }

    #[test]
    fn options_tabs_rows_and_values_respond_to_clicks() {
        let dir = project();
        let mut app = app(&dir);
        open_options(&mut app);

        // Tabs.
        click_on(&mut app, Hit::OptionsTab(1));
        assert_eq!(app.overlay, Overlay::Options { tab: 1, selected: 0 });
        draw_now(&mut app);
        click_on(&mut app, Hit::OptionsTab(0));
        draw_now(&mut app);
        assert_eq!(app.overlay, Overlay::Options { tab: 0, selected: 0 });

        // A single click on a row only selects it.
        let stars_row = OPTION_TABS[0].items.iter().position(|i| *i == OptionItem::GithubStars).unwrap();
        let before = app.config.show_stars;
        click_on(&mut app, Hit::Option(stars_row));
        assert_eq!(app.overlay, Overlay::Options { tab: 0, selected: stars_row });
        assert_eq!(app.config.show_stars, before);

        // Clicking the value changes it, like Enter.
        draw_now(&mut app);
        click_on(&mut app, Hit::OptionValue(stars_row));
        assert_ne!(app.config.show_stars, before);

        // A quick second click on a row (a double click) changes it too.
        draw_now(&mut app);
        let anim_row = OPTION_TABS[0].items.iter().position(|i| *i == OptionItem::Animation).unwrap();
        let animation = app.config.banner_animation;
        let rect = rect_of(&app, Hit::Option(anim_row));
        for _ in 0..2 {
            send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), rect.x + 2, rect.y);
        }
        assert_ne!(app.config.banner_animation, animation);
        assert_eq!(app.overlay, Overlay::Options { tab: 0, selected: anim_row });
    }

    #[test]
    fn two_slow_clicks_on_an_option_do_not_change_it() {
        let dir = project();
        let mut app = app(&dir);
        open_options(&mut app);
        let row = OPTION_TABS[0].items.iter().position(|i| *i == OptionItem::Animation).unwrap();
        let animation = app.config.banner_animation;
        let rect = rect_of(&app, Hit::Option(row));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), rect.x + 2, rect.y);
        std::thread::sleep(DOUBLE_CLICK + Duration::from_millis(60));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), rect.x + 2, rect.y);
        assert_eq!(app.config.banner_animation, animation);
    }

    #[test]
    fn options_wheel_moves_the_selection_and_a_click_outside_goes_back() {
        let dir = project();
        let mut app = app(&dir);
        open_options(&mut app);
        wheel(&mut app, true);
        assert_eq!(app.overlay, Overlay::Options { tab: 0, selected: 1 });
        wheel(&mut app, false);
        assert_eq!(app.overlay, Overlay::Options { tab: 0, selected: 0 });

        // Outside the Options frame: like Esc, back to the menu.
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert!(matches!(app.overlay, Overlay::Menu { .. }), "{:?}", app.overlay);
    }

    #[test]
    fn clicking_a_theme_applies_it_and_the_wheel_previews() {
        let dir = project();
        let mut app = app(&dir);
        open_options(&mut app);
        click_on(&mut app, Hit::Option(0)); // Theme row, selected
        draw_now(&mut app);
        click_on(&mut app, Hit::OptionValue(0)); // opens the theme chooser
        draw_now(&mut app);
        let Overlay::Themes { original, .. } = app.overlay else {
            panic!("expected the theme chooser, got {:?}", app.overlay);
        };

        wheel(&mut app, true);
        assert_ne!(app.theme().name, original, "the wheel previews the next theme");
        draw_now(&mut app);

        let target = ThemeName::ALL[3];
        click_on(&mut app, Hit::Theme(3));
        assert_eq!(app.theme().name, target);
        assert!(!matches!(app.overlay, Overlay::Themes { .. }));
    }

    #[test]
    fn outline_rows_are_clickable_and_a_click_outside_closes_it() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('o'));
        draw_now(&mut app);
        click_on(&mut app, Hit::Outline(1));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.viewer.cursor > 0, "jumped to the second heading");

        press(&mut app, KeyCode::Char('o'));
        draw_now(&mut app);
        let cursor = app.viewer.cursor;
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.viewer.cursor, cursor, "closing does not jump");
    }

    #[test]
    fn help_scrolls_with_the_wheel_and_closes_on_an_outside_click() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('?'));
        draw_now(&mut app);
        wheel(&mut app, true);
        let Overlay::Help { scroll } = app.overlay else { panic!() };
        assert_eq!(scroll, WHEEL_LINES as usize);
        wheel(&mut app, false);
        assert_eq!(app.overlay, Overlay::Help { scroll: 0 });
        // Inside the help: nothing. Outside: closes.
        let panel = rect_of(&app, Hit::Panel);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), panel.x + 3, panel.y + 3);
        assert!(matches!(app.overlay, Overlay::Help { .. }));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn links_chooser_rows_are_clickable() {
        let (_dir, mut app) = linked(&[
            ("README.md", "# Home\n\n[a](a.md) [b](b.md)\n"),
            ("a.md", "# A\n"),
            ("b.md", "# B\n"),
        ]);
        goto_link(&mut app, "a.md");
        press(&mut app, KeyCode::Char(' '));
        draw_now(&mut app);
        click_on(&mut app, Hit::Link(1));
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "b.md");
    }

    #[test]
    fn with_the_mouse_option_off_menus_ignore_clicks_and_the_wheel() {
        let dir = project();
        let mut app = app(&dir);
        open_menu(&mut app);
        app.config.mouse = false;
        let start = app.overlay;
        click_on(&mut app, Hit::MenuItem(2));
        wheel(&mut app, true);
        assert_eq!(app.overlay, start);
        assert!(!app.should_quit);
    }

    // ------------------------------------------------------------ explorer

    fn explorer_row(app: &App, name: &str) -> (u16, u16) {
        let position = app
            .explorer
            .visible()
            .iter()
            .position(|&i| app.explorer.tree.node(i).unwrap().name == name)
            .unwrap_or_else(|| panic!("{name} not visible"));
        let rect = app.layout.explorer.unwrap();
        (rect.x, rect.y + (position - app.explorer.offset()) as u16)
    }

    #[test]
    fn the_chevron_folds_a_folder_without_changing_the_document() {
        let dir = project();
        let mut app = app(&dir);
        draw_now(&mut app);
        let open_before = open_name(&app);
        let (x, y) = explorer_row(&app, "notes");
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        assert!(!app.explorer.tree.is_expanded(notes));
        let (start, _) = crate::ui::explorer::chevron_columns(
            app.explorer.tree.node(notes).unwrap(),
            app.config.icon_style(),
        )
        .unwrap();

        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + start, y);
        assert!(app.explorer.tree.is_expanded(notes), "expanded by the chevron");
        assert_eq!(open_name(&app), open_before, "the document did not change");
        assert_eq!(app.focus, Focus::Explorer);

        draw_now(&mut app);
        let (x, y) = explorer_row(&app, "notes");
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + start + 1, y);
        assert!(!app.explorer.tree.is_expanded(notes), "folded again");
    }

    #[test]
    fn the_chevron_columns_match_what_is_drawn() {
        let dir = project();
        fs::create_dir_all(dir.path().join("notes/deeper")).unwrap();
        fs::write(dir.path().join("notes/deeper/x.md"), "x\n").unwrap();
        let mut app = app(&dir);
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        app.explorer.tree.expand(notes);
        app.explorer.refresh();
        let deeper = app.explorer.tree.find_path(&dir.path().join("notes/deeper")).unwrap();
        app.explorer.tree.expand(deeper);
        app.explorer.refresh();
        let text = draw_now(&mut app);
        for name in ["notes", "deeper"] {
            let (x, y) = explorer_row(&app, name);
            let node = app
                .explorer
                .tree
                .node(app.explorer.tree.find_path(&dir.path().join(if name == "notes" { "notes" } else { "notes/deeper" })).unwrap())
                .unwrap();
            let (start, width) = crate::ui::explorer::chevron_columns(node, app.config.icon_style()).unwrap();
            let row: Vec<char> = text.lines().nth(y as usize).unwrap().chars().collect();
            let drawn: String = row[(x + start) as usize..(x + start + width) as usize].iter().collect();
            assert!(drawn.starts_with('▾') || drawn.starts_with('▸'), "{name}: {drawn:?} in {:?}", row.iter().collect::<String>());
        }
        // The root and files have no chevron.
        let root = app.explorer.tree.node(app.explorer.tree.root_index()).unwrap();
        assert!(crate::ui::explorer::chevron_columns(root, app.config.icon_style()).is_none());
    }

    #[test]
    fn clicking_a_name_selects_and_double_clicking_opens_or_folds() {
        let dir = project();
        let mut app = app(&dir);
        draw_now(&mut app);
        app.focus = Focus::Explorer;

        // A single click on a file previews it and keeps focus in the tree.
        let (x, y) = explorer_row(&app, "plain.txt");
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        assert_eq!(open_name(&app), "plain.txt");
        assert_eq!(app.focus, Focus::Explorer);
        assert!(app.history.is_empty());

        // The second click of a double click opens it for reading.
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        assert_eq!(app.focus, Focus::Viewer);

        // A double click on a folder folds it; a single click only selects.
        draw_now(&mut app);
        app.focus = Focus::Explorer;
        let notes = app.explorer.tree.find_path(&dir.path().join("notes")).unwrap();
        let (x, y) = explorer_row(&app, "notes");
        std::thread::sleep(DOUBLE_CLICK + Duration::from_millis(60));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        assert!(!app.explorer.tree.is_expanded(notes));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        assert!(app.explorer.tree.is_expanded(notes));
    }

    #[test]
    fn clicks_below_the_last_row_do_nothing_and_text_selection_still_works() {
        let dir = project();
        let mut app = app(&dir);
        draw_now(&mut app);
        let selected = app.explorer.selected_position();
        let rect = app.layout.explorer.unwrap();
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), rect.x + 3, rect.bottom() - 1);
        assert_eq!(app.explorer.selected_position(), selected);

        // Drag in the viewer still starts and ends a selection.
        let viewer = app.layout.viewer;
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), viewer.x + 2, viewer.y + 1);
        send_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), viewer.x + 12, viewer.y + 1);
        assert!(app.selection.is_some());
        send_mouse(&mut app, MouseEventKind::Up(MouseButton::Left), viewer.x + 12, viewer.y + 1);
        assert!(app.selection.as_ref().is_some_and(|s| s.done));
    }

    #[test]
    fn explorer_clicks_do_nothing_with_the_mouse_off() {
        let dir = project();
        let mut app = app(&dir);
        draw_now(&mut app);
        app.config.mouse = false;
        let (x, y) = explorer_row(&app, "plain.txt");
        let before = open_name(&app);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 8, y);
        assert_eq!(open_name(&app), before);
    }
    // ------------------------------------------------------------- finder

    fn deep_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/ui")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("README.md"), b"# Home\n\nWelcome.\n").unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();
        fs::write(root.join("src/ui/mod.rs"), b"pub mod x;\n").unwrap();
        fs::write(root.join("docs/guide.md"), b"# Guide\n").unwrap();
        fs::write(root.join("target/junk.rs"), b"x\n").unwrap();
        fs::write(root.join(".hidden.md"), b"# Hidden\n").unwrap();
        fs::write(root.join(".gitignore"), b"target/\n").unwrap();
        dir
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    fn open_finder_and_wait(app: &mut App) {
        press_ctrl(app, 'p');
        assert!(matches!(app.overlay, Overlay::Finder { .. }), "Ctrl+P opens the finder");
        tick_until(app, "the index", |app| !app.finder.is_scanning());
    }

    fn hit_paths(app: &App) -> Vec<String> {
        app.finder.hits().iter().map(|hit| hit.path.clone()).collect()
    }

    fn finder_selected(app: &App) -> usize {
        match app.overlay {
            Overlay::Finder { selected } => selected,
            other => panic!("the finder is not open: {other:?}"),
        }
    }

    #[test]
    fn ctrl_p_opens_the_finder_and_typing_filters_the_files() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        assert_eq!(
            hit_paths(&app),
            vec!["docs/guide.md", "README.md", "src/main.rs", "src/ui/mod.rs"],
            "an empty query lists the project, ignoring target/ and hidden files"
        );
        type_text(&mut app, "mod");
        assert_eq!(hit_paths(&app)[0], "src/ui/mod.rs");
        let screen = draw_now(&mut app);
        assert!(screen.contains("Find file"), "{screen}");
        assert!(screen.contains("src/ui/mod.rs"), "{screen}");
        assert!(screen.contains("mod"), "the query is shown");
    }

    #[test]
    fn enter_opens_the_file_and_reveals_its_row_in_the_explorer() {
        let dir = deep_project();
        let mut app = app(&dir);
        draw_now(&mut app);
        let visible_names = |app: &App| -> Vec<String> {
            app.explorer
                .visible()
                .iter()
                .filter_map(|&i| app.explorer.tree.node(i).map(|node| node.name.clone()))
                .collect()
        };
        assert!(!visible_names(&app).contains(&"mod.rs".to_string()), "src/ui starts folded");
        open_finder_and_wait(&mut app);
        type_text(&mut app, "uimod");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "mod.rs");
        let wanted = dir.path().join("src/ui/mod.rs");
        tick_until(&mut app, "the explorer to reveal the file", |app| {
            app.explorer.selected_path().as_deref() == Some(wanted.as_path())
        });
        assert_eq!(app.focus, Focus::Viewer);
        // The folders on the way were opened.
        let names = visible_names(&app);
        assert!(names.contains(&"ui".to_string()) && names.contains(&"mod.rs".to_string()), "{names:?}");
        // It counts as a navigation: Ctrl+O goes back to the README.
        press_ctrl(&mut app, 'o');
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn escape_closes_the_finder_and_does_not_open_the_menu() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "ma");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.should_quit);
        // A second Esc is the usual one: it opens the main menu.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::Menu { .. }));
        // Opening the finder again starts with an empty query.
        press(&mut app, KeyCode::Esc);
        press_ctrl(&mut app, 'p');
        assert!(app.finder.query.is_empty());
    }

    #[test]
    fn q_is_a_letter_inside_the_finder_and_ctrl_c_does_nothing() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit, "q types a letter here");
        assert_eq!(app.finder.query, "q");
        press_ctrl(&mut app, 'c');
        assert!(!app.should_quit);
        assert!(matches!(app.overlay, Overlay::Finder { .. }), "Ctrl+C does nothing");
        assert_eq!(app.finder.query, "q");
        // Once the finder is closed, q quits as always.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn arrows_stay_inside_the_results_and_typing_goes_back_to_the_top() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        let count = app.finder.hits().len();
        assert_eq!(count, 4);
        press(&mut app, KeyCode::Up);
        assert_eq!(finder_selected(&app), 0, "no wrapping above the first result");
        for _ in 0..10 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(finder_selected(&app), count - 1, "no wrapping below the last one");
        press(&mut app, KeyCode::Home);
        assert_eq!(finder_selected(&app), 0);
        press(&mut app, KeyCode::End);
        assert_eq!(finder_selected(&app), count - 1);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(finder_selected(&app), 0);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(finder_selected(&app), count - 1);
        press_ctrl(&mut app, 'p');
        assert_eq!(finder_selected(&app), count - 2, "Ctrl+P / Ctrl+N move like in fzf");
        press_ctrl(&mut app, 'n');
        assert_eq!(finder_selected(&app), count - 1);
        type_text(&mut app, "s");
        assert_eq!(finder_selected(&app), 0, "a new query starts at the best match");
    }

    #[test]
    fn enter_opens_the_highlighted_result_not_the_first() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "rs");
        let paths = hit_paths(&app);
        assert!(paths.len() >= 2, "{paths:?}");
        press(&mut app, KeyCode::Down);
        let wanted = paths[1].clone();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.viewer.loaded.as_ref().unwrap().path, dir.path().join(wanted));
    }

    #[test]
    fn backspace_ctrl_u_and_ctrl_w_edit_the_query() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "src main");
        assert_eq!(app.finder.query, "src main");
        press_ctrl(&mut app, 'w');
        assert_eq!(app.finder.query, "src ");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.finder.query, "src");
        press_ctrl(&mut app, 'u');
        assert_eq!(app.finder.query, "");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.finder.query, "", "backspace on an empty query is harmless");
        assert_eq!(app.finder.hits().len(), 4);
    }

    #[test]
    fn enter_without_results_keeps_the_finder_open() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "zzzzzz");
        assert!(app.finder.hits().is_empty());
        let screen = draw_now(&mut app);
        assert!(screen.contains("no matches"), "{screen}");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::PageDown);
        assert!(matches!(app.overlay, Overlay::Finder { .. }));
        assert_eq!(open_name(&app), "README.md", "nothing was opened");
    }

    #[test]
    fn an_empty_project_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        let screen = draw_now(&mut app);
        assert!(screen.contains("no files in this project"), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Finder { .. }));
    }

    #[test]
    fn clicking_a_result_opens_it() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "guide");
        draw_now(&mut app);
        click_on(&mut app, Hit::Finder(0));
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "guide.md");
    }

    #[test]
    fn the_wheel_moves_the_selection_and_a_click_outside_closes() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        draw_now(&mut app);
        wheel(&mut app, true);
        wheel(&mut app, true);
        assert_eq!(finder_selected(&app), 2);
        wheel(&mut app, false);
        assert_eq!(finder_selected(&app), 1);
        // The top-left corner of the screen is outside the popup.
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn clicks_on_the_input_or_the_frame_do_not_close_the_finder() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        draw_now(&mut app);
        let frame = rect_of(&app, Hit::Panel);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), frame.x + 5, frame.y + 1);
        assert!(matches!(app.overlay, Overlay::Finder { .. }));
    }

    #[test]
    fn the_mouse_option_off_ignores_clicks_in_the_finder() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        draw_now(&mut app);
        app.config.mouse = false;
        let rect = rect_of(&app, Hit::Finder(0));
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), rect.x + 4, rect.y);
        assert!(matches!(app.overlay, Overlay::Finder { .. }));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None, "the keyboard still works");
    }

    #[test]
    fn indexing_is_shown_while_the_scan_runs_and_results_arrive_after() {
        let dir = deep_project();
        let mut app = app(&dir);
        app.finder.set_scan_limits(Duration::from_millis(400), crate::finder::MAX_INDEXED_FILES);
        press_ctrl(&mut app, 'p');
        assert!(app.finder.is_scanning());
        assert!(app.is_busy(), "the event loop must keep ticking while it indexes");
        let screen = draw_now(&mut app);
        assert!(screen.contains("indexing…"), "{screen}");
        assert!(app.finder.hits().is_empty());
        // The interface is not blocked: keys still work while indexing.
        type_text(&mut app, "mod");
        assert_eq!(app.finder.query, "mod");
        tick_until(&mut app, "the index", |app| !app.finder.is_scanning());
        assert_eq!(hit_paths(&app)[0], "src/ui/mod.rs", "the query typed while indexing is applied");
        let screen = draw_now(&mut app);
        assert!(!screen.contains("indexing…"), "{screen}");
        assert!(!app.is_busy());
    }

    #[test]
    fn the_finder_follows_the_hidden_and_gitignore_settings_of_the_explorer() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        assert!(!hit_paths(&app).contains(&".hidden.md".to_string()));
        press(&mut app, KeyCode::Esc);

        app.toggle_hidden();
        open_finder_and_wait(&mut app);
        assert!(hit_paths(&app).contains(&".hidden.md".to_string()));
        press(&mut app, KeyCode::Esc);

        app.toggle_gitignore();
        open_finder_and_wait(&mut app);
        assert!(hit_paths(&app).contains(&"target/junk.rs".to_string()));
    }

    #[test]
    fn files_created_later_show_up_after_a_refresh_or_a_watcher_event() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        press(&mut app, KeyCode::Esc);

        fs::write(dir.path().join("fresh.md"), b"# fresh\n").unwrap();
        // Without any signal the cached index is reused.
        open_finder_and_wait(&mut app);
        assert!(!hit_paths(&app).contains(&"fresh.md".to_string()));
        press(&mut app, KeyCode::Esc);

        // `r` in the explorer refreshes the tree and the index.
        app.focus = Focus::Explorer;
        press(&mut app, KeyCode::Char('r'));
        open_finder_and_wait(&mut app);
        assert!(hit_paths(&app).contains(&"fresh.md".to_string()));
        press(&mut app, KeyCode::Esc);

        // The background watcher invalidates it too.
        fs::write(dir.path().join("watched.md"), b"# watched\n").unwrap();
        tick_until(&mut app, "the watcher to invalidate the index", |app| {
            app.explorer.tree.find_path(&dir.path().join("watched.md")).is_some()
        });
        open_finder_and_wait(&mut app);
        assert!(hit_paths(&app).contains(&"watched.md".to_string()));
    }

    #[test]
    fn a_file_deleted_after_indexing_is_reported_and_nothing_breaks() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        fs::remove_file(dir.path().join("docs/guide.md")).unwrap();
        type_text(&mut app, "guide");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("No longer exists")));
        assert_eq!(open_name(&app), "README.md");
    }

    #[test]
    fn find_file_can_be_rebound_and_the_prefix_works_too() {
        let dir = deep_project();
        let mut config = Config::default();
        config
            .keys
            .insert("find_file".into(), crate::keymap::KeySpec::One("ctrl+g".into()));
        let mut rebound = App::with_config(Some(dir.path()), config);
        rebound.set_terminal_area(Rect::new(0, 0, 100, 30));
        press_ctrl(&mut rebound, 'p');
        assert_eq!(rebound.overlay, Overlay::None, "the old binding is gone");
        press_ctrl(&mut rebound, 'g');
        assert!(matches!(rebound.overlay, Overlay::Finder { .. }));
        press(&mut rebound, KeyCode::Esc);

        // The default configuration also has `prefix f` (it works from the
        // terminal and the editor, which only let the prefix through).
        let mut default_app = app(&dir);
        prefix_then(&mut default_app, KeyCode::Char('f'));
        assert!(matches!(default_app.overlay, Overlay::Finder { .. }));
    }

    #[test]
    fn pasting_types_into_the_finder_query() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        app.paste("src/\nmain");
        assert_eq!(app.finder.query, "src/main", "control characters are dropped");
        assert_eq!(hit_paths(&app)[0], "src/main.rs");
        assert_eq!(finder_selected(&app), 0);
    }

    #[test]
    fn the_help_lists_the_finder_shortcut() {
        let app = App::default();
        let help = crate::ui::help::rendered_text(&app.keymap);
        assert!(help.contains("Ctrl+P"), "{help}");
        assert!(help.contains("Find a file"), "{help}");
    }

    #[test]
    fn long_paths_keep_their_file_name_and_tiny_terminals_do_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a_very_long_folder_name/another_quite_long_folder/third_folder_here");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("target_file.md"), b"# t\n").unwrap();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "tf");
        let narrow = crate::ui::render_to_text_for_test(&mut app, 50, 20);
        assert!(narrow.contains("target_file.md"), "{narrow}");
        assert!(narrow.contains('…'), "the start of the path is elided\n{narrow}");
        for (width, height) in [(20, 5), (30, 8), (1, 1), (40, 6)] {
            let _ = crate::ui::render_to_text_for_test(&mut app, width, height);
        }
    }

    #[test]
    fn the_matching_characters_are_drawn_highlighted() {
        let dir = deep_project();
        let mut app = app(&dir);
        open_finder_and_wait(&mut app);
        type_text(&mut app, "mod");
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rect = rect_of(&app, Hit::Finder(0));
        let mut highlighted = String::new();
        for x in rect.x..rect.right() {
            let cell = &buffer[(x, rect.y)];
            if cell.modifier.contains(ratatui::style::Modifier::UNDERLINED) {
                highlighted.push_str(cell.symbol());
            }
        }
        assert_eq!(highlighted, "mod");
    }

    // ------------------------------------------------- bookmarks and recents

    fn app_with_state(dir: &tempfile::TempDir, state_file: &Path) -> App {
        let mut app = App::with_config(Some(dir.path()), Config::default());
        app.state_path = Some(state_file.to_path_buf());
        app.project_state = state::load(state_file, &app.root);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app
    }

    fn state_file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("..").join(format!(
            "state-{}.toml",
            dir.path().file_name().unwrap().to_string_lossy()
        ))
    }

    fn bookmarks(app: &App) -> Vec<(String, usize)> {
        app.project_state
            .bookmarks
            .iter()
            .map(|bookmark| (bookmark.file.clone(), bookmark.line))
            .collect()
    }

    fn list_state(app: &App) -> (ListTab, usize) {
        match app.overlay {
            Overlay::Bookmarks { tab, selected } => (tab, selected),
            other => panic!("the bookmarks popup is not open: {other:?}"),
        }
    }

    #[test]
    fn m_bookmarks_the_current_line_and_again_removes_it() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(bookmarks(&app), vec![("README.md".to_string(), 1)]);
        assert_eq!(app.project_state.bookmarks[0].text, "# Home");
        assert!(app.status_text().is_some_and(|text| text.contains("Bookmarked README.md:1")));
        press(&mut app, KeyCode::Char('m'));
        assert!(bookmarks(&app).is_empty());
        assert!(app.status_text().is_some_and(|text| text.contains("Bookmark removed")));
    }

    #[test]
    fn the_bookmark_follows_the_line_of_the_source_not_the_row() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        let details = app.viewer.doc.headings.iter().find(|h| h.text == "Details").unwrap().line;
        app.viewer.jump_to_line(details);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(bookmarks(&app), vec![("README.md".to_string(), 5)], "`## Details` is line 5");
        assert_eq!(app.project_state.bookmarks[0].text, "## Details");
        // A different width flows the document differently; the line stays.
        app.set_terminal_area(Rect::new(0, 0, 50, 30));
        let row = app.viewer.row_of_source_line(4).unwrap();
        assert_eq!(app.viewer.source_line_at(row), Some(4));
    }

    #[test]
    fn plain_and_code_files_bookmark_their_real_lines_even_when_they_wrap() {
        let dir = tempfile::tempdir().unwrap();
        let long = "word ".repeat(60);
        fs::write(dir.path().join("long.txt"), format!("first\n{long}\ntail\n")).unwrap();
        let code: String = (1..=12).map(|n| format!("let value_{n} = {n};\n")).collect();
        fs::write(dir.path().join("code.rs"), code).unwrap();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);

        app.open_file(dir.path().join("long.txt"), None, true);
        let last_row = app.viewer.doc.lines.len() - 1;
        app.viewer.jump_to_line(last_row);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(bookmarks(&app), vec![("long.txt".to_string(), 3)], "`tail` is line 3");
        // A row in the middle of the wrapped line is line 2.
        let middle = app.viewer.row_of_source_line(1).unwrap() + 1;
        app.viewer.jump_to_line(middle);
        press(&mut app, KeyCode::Char('m'));
        assert!(bookmarks(&app).contains(&("long.txt".to_string(), 2)));

        app.open_file(dir.path().join("code.rs"), None, true);
        let row = app.viewer.row_of_source_line(6).expect("line 7 is rendered");
        app.viewer.jump_to_line(row);
        press(&mut app, KeyCode::Char('m'));
        assert!(bookmarks(&app).contains(&("code.rs".to_string(), 7)));
        let bookmark = app.project_state.bookmarks.iter().find(|b| b.file == "code.rs").unwrap();
        assert_eq!(bookmark.text, "let value_7 = 7;");
    }

    #[test]
    fn bookmarks_need_an_open_file_inside_the_project() {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("elsewhere.md"), "# Elsewhere\n").unwrap();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(outside.path().join("elsewhere.md"), None, true);
        press(&mut app, KeyCode::Char('m'));
        assert!(bookmarks(&app).is_empty());
        assert!(app.status_text().is_some_and(|text| text.contains("inside the project")));
        assert!(app.project_state.recent.is_empty(), "files outside the project are not recent");

        let mut app = app_with_state(&dir, &file);
        app.viewer.loaded = None;
        app.show_welcome();
        press(&mut app, KeyCode::Char('m'));
        assert!(bookmarks(&app).is_empty());
        assert!(app.status_text().is_some_and(|text| text.contains("No file open")));
    }

    #[test]
    fn bookmarks_and_recent_files_persist_relative_to_the_project() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("notes/guide.md"), None, true);
        press(&mut app, KeyCode::Char('m'));
        drop(app);

        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("notes/guide.md"), "{text}");
        assert!(!text.contains(&format!("{}/notes", dir.path().display())), "no absolute file paths: {text}");

        let again = app_with_state(&dir, &file);
        assert_eq!(bookmarks(&again), vec![("notes/guide.md".to_string(), 1)]);
        assert_eq!(again.project_state.recent, vec!["notes/guide.md"]);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn a_corrupt_state_file_never_stops_the_app() {
        let dir = project();
        let file = state_file(&dir);
        fs::write(&file, "][ not toml").unwrap();
        let mut app = app_with_state(&dir, &file);
        assert!(app.project_state.bookmarks.is_empty());
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(bookmarks(&app).len(), 1);
        let again = app_with_state(&dir, &file);
        assert_eq!(bookmarks(&again).len(), 1, "the state was rewritten");
        let _ = fs::remove_file(&file);
        let _ = fs::remove_file(file.with_extension("toml.corrupt"));
    }

    #[test]
    fn files_that_disappeared_are_dropped_when_the_state_is_read() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("notes/guide.md"), None, true);
        press(&mut app, KeyCode::Char('m'));
        drop(app);
        fs::remove_file(dir.path().join("notes/guide.md")).unwrap();
        let again = app_with_state(&dir, &file);
        assert!(again.project_state.bookmarks.is_empty());
        assert!(!again.project_state.recent.contains(&"notes/guide.md".to_string()));
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn the_popup_lists_bookmarks_and_enter_jumps_to_the_file_and_line() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("plain.txt"), None, true);
        let row = app.viewer.row_of_source_line(1).unwrap();
        app.viewer.jump_to_line(row);
        press(&mut app, KeyCode::Char('m'));
        app.open_file(dir.path().join("README.md"), None, true);
        assert_eq!(open_name(&app), "README.md");

        press(&mut app, KeyCode::Char('\''));
        assert_eq!(list_state(&app), (ListTab::Bookmarks, 0));
        let screen = draw_now(&mut app);
        assert!(screen.contains("plain.txt:2"), "{screen}");
        assert!(screen.contains("world"), "the excerpt of the line is shown\n{screen}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "plain.txt");
        assert_eq!(app.viewer.source_line_at(app.viewer.cursor), Some(1));
        assert_eq!(app.focus, Focus::Viewer);
        // The jump is a navigation: Ctrl+O comes back.
        press_ctrl(&mut app, 'o');
        assert_eq!(open_name(&app), "README.md");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn jumping_inside_the_same_file_moves_the_cursor_and_keeps_history() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        let details = app.viewer.doc.headings.iter().find(|h| h.text == "Details").unwrap().line;
        app.viewer.jump_to_line(details);
        press(&mut app, KeyCode::Char('m'));
        app.viewer.jump_to_line(0);
        press(&mut app, KeyCode::Char('\''));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.viewer.cursor, details);
        press_ctrl(&mut app, 'o');
        assert_eq!(app.viewer.cursor, 0, "back to where the jump started");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn d_deletes_a_bookmark_and_the_selection_stays_in_range() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        for name in ["README.md", "plain.txt", "notes/guide.md"] {
            app.open_file(dir.path().join(name), None, true);
            press(&mut app, KeyCode::Char('m'));
        }
        assert_eq!(app.project_state.bookmarks.len(), 3);
        press(&mut app, KeyCode::Char('\''));
        press(&mut app, KeyCode::End);
        assert_eq!(list_state(&app).1, 2);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.project_state.bookmarks.len(), 2);
        assert_eq!(list_state(&app).1, 1, "the selection moved up with the list");
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.project_state.bookmarks.is_empty());
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(list_state(&app), (ListTab::Bookmarks, 0), "d on an empty list is harmless");
        let screen = draw_now(&mut app);
        assert!(screen.contains("No bookmarks yet"), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Bookmarks { .. }));
        // The deletions were saved.
        let again = app_with_state(&dir, &file);
        assert!(again.project_state.bookmarks.is_empty());
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn recent_files_are_most_recent_first_and_limited_to_twenty() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..25 {
            fs::write(dir.path().join(format!("f{index:02}.md")), format!("# {index}\n")).unwrap();
        }
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        assert!(app.project_state.recent.is_empty(), "the README-less start opens nothing");
        for index in 0..25 {
            app.open_file(dir.path().join(format!("f{index:02}.md")), None, true);
        }
        assert_eq!(app.project_state.recent.len(), 20);
        assert_eq!(app.project_state.recent[0], "f24.md");
        assert_eq!(app.project_state.recent[19], "f05.md");
        // Opening one again moves it to the front without duplicating it.
        app.open_file(dir.path().join("f10.md"), None, true);
        assert_eq!(app.project_state.recent[0], "f10.md");
        assert_eq!(app.project_state.recent.iter().filter(|f| *f == "f10.md").count(), 1);

        press(&mut app, KeyCode::Char('\''));
        press(&mut app, KeyCode::Tab);
        assert_eq!(list_state(&app), (ListTab::Recent, 0));
        let screen = draw_now(&mut app);
        assert!(screen.contains("f10.md") && screen.contains("Recent (20)"), "{screen}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(open_name(&app), "f24.md");
        assert_eq!(app.project_state.recent[0], "f24.md");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn tab_switches_lists_in_both_directions_and_d_forgets_a_recent_file() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("plain.txt"), None, true);
        press(&mut app, KeyCode::Char('\''));
        press(&mut app, KeyCode::Tab);
        assert_eq!(list_state(&app).0, ListTab::Recent);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(list_state(&app).0, ListTab::Bookmarks);
        press(&mut app, KeyCode::Right);
        assert_eq!(list_state(&app).0, ListTab::Recent);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.project_state.recent.is_empty());
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn browsing_with_the_arrows_does_not_count_as_opening_a_file() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        draw_now(&mut app);
        app.focus = Focus::Explorer;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert!(app.project_state.recent.is_empty(), "previews are not opens");
        // Space opens: that does.
        let (x, y) = explorer_row(&app, "plain.txt");
        app.explorer.select_position(app.explorer.visible().iter().position(|&i| app.explorer.tree.node(i).unwrap().name == "plain.txt").unwrap());
        let _ = (x, y);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.project_state.recent, vec!["plain.txt"]);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn only_a_file_given_on_the_command_line_counts_at_startup() {
        let dir = project();
        // The README opened by default is not something the user chose.
        let app = app(&dir);
        assert!(app.project_state.recent.is_empty());
        // An explicit file is.
        let file = state_file(&dir);
        let mut app = App::with_config(Some(&dir.path().join("plain.txt")), Config::default());
        app.state_path = Some(file.clone());
        assert_eq!(open_name(&app), "plain.txt");
        app.note_recent(&dir.path().join("plain.txt"));
        assert_eq!(app.project_state.recent, vec!["plain.txt"]);
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn choosing_an_entry_whose_file_vanished_removes_it_and_says_so() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("plain.txt"), None, true);
        press(&mut app, KeyCode::Char('m'));
        app.open_file(dir.path().join("README.md"), None, true);
        press(&mut app, KeyCode::Char('\''));
        // The file disappears while the popup is already open.
        fs::remove_file(dir.path().join("plain.txt")).unwrap();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("No longer exists: plain.txt")));
        assert!(app.project_state.bookmarks.is_empty());
        assert_eq!(open_name(&app), "README.md");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn the_popup_closes_with_esc_or_q_and_ctrl_c_does_nothing() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('\''));
        press_ctrl(&mut app, 'c');
        assert!(matches!(app.overlay, Overlay::Bookmarks { .. }));
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        press(&mut app, KeyCode::Char('\''));
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.should_quit, "q closes the popup, as in every popup");
        let screen = {
            press(&mut app, KeyCode::Char('\''));
            draw_now(&mut app)
        };
        assert!(screen.contains("No bookmarks yet"), "{screen}");
        press(&mut app, KeyCode::Tab);
        let screen = draw_now(&mut app);
        assert!(screen.contains("No recent files yet"), "{screen}");
    }

    #[test]
    fn mouse_picks_tabs_and_rows_and_the_wheel_moves() {
        let dir = project();
        let file = state_file(&dir);
        let mut app = app_with_state(&dir, &file);
        app.open_file(dir.path().join("plain.txt"), None, true);
        app.open_file(dir.path().join("notes/guide.md"), None, true);
        press(&mut app, KeyCode::Char('\''));
        draw_now(&mut app);
        click_on(&mut app, Hit::BookmarkTab(ListTab::Recent));
        assert_eq!(list_state(&app), (ListTab::Recent, 0));
        draw_now(&mut app);
        wheel(&mut app, true);
        assert_eq!(list_state(&app).1, 1);
        click_on(&mut app, Hit::BookmarkRow(1));
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "plain.txt");

        press(&mut app, KeyCode::Char('\''));
        draw_now(&mut app);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(app.overlay, Overlay::None, "a click outside closes it");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn the_keys_are_configurable_and_listed_in_the_help() {
        let dir = project();
        let mut config = Config::default();
        config.keys.insert("bookmark".into(), crate::keymap::KeySpec::One("b".into()));
        config.keys.insert("bookmarks".into(), crate::keymap::KeySpec::One("ctrl+b".into()));
        let mut custom = App::with_config(Some(dir.path()), config);
        custom.set_terminal_area(Rect::new(0, 0, 100, 30));
        press(&mut custom, KeyCode::Char('m'));
        assert!(custom.project_state.bookmarks.is_empty(), "m is no longer bound");
        press(&mut custom, KeyCode::Char('b'));
        assert_eq!(custom.project_state.bookmarks.len(), 1);
        press_ctrl(&mut custom, 'b');
        assert!(matches!(custom.overlay, Overlay::Bookmarks { .. }));

        let default = App::default();
        let help = crate::ui::help::rendered_text(&default.keymap);
        assert!(help.contains("Bookmark the current line"), "{help}");
        assert!(help.contains("Bookmarks and recent files"), "{help}");
    }

    #[test]
    fn m_and_the_apostrophe_are_text_in_the_search_prompt_and_the_finder() {
        let dir = project();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Char('\''));
        assert_eq!(app.search_input.as_deref(), Some("m'"));
        assert!(app.project_state.bookmarks.is_empty());
        press(&mut app, KeyCode::Esc);
        press_ctrl(&mut app, 'p');
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Char('\''));
        assert_eq!(app.finder.query, "m'");
        assert!(app.project_state.bookmarks.is_empty());
    }

    // ----------------------------------------------------------- backlinks

    fn link_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("README.md"),
            "# Home\n\nIntro text.\n\n- [the guide](docs/guide.md) is first\n- see also [[guide|alias]] and [other](docs/other.md#top)\n",
        )
        .unwrap();
        fs::write(root.join("docs/guide.md"), "# Guide\n\nBack to [home](../README.md).\n").unwrap();
        fs::write(
            root.join("docs/other.md"),
            "# Other\n\nSee [guide](guide.md) and [missing](nope.md).\n",
        )
        .unwrap();
        fs::write(root.join("orphan.md"), "# Orphan\n").unwrap();
        dir
    }

    fn index_links(app: &mut App) {
        app.started_at = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or_else(Instant::now);
        tick_until(app, "the link index", |app| {
            app.backlinks.is_ready() && !app.backlinks.is_scanning()
        });
    }

    fn open_in(app: &mut App, dir: &tempfile::TempDir, file: &str) {
        app.open_file(dir.path().join(file), None, true);
        assert_eq!(app.viewer.loaded.as_ref().map(|l| l.path.clone()), Some(dir.path().join(file)));
    }

    fn backlink_selected(app: &App) -> usize {
        match app.overlay {
            Overlay::Backlinks { selected } => selected,
            other => panic!("the backlinks popup is not open: {other:?}"),
        }
    }

    #[test]
    fn the_status_bar_counts_the_backlinks_of_the_open_file() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        assert_eq!(app.backlink_count(), None, "nothing is known before the first build");
        assert!(!draw_now(&mut app).contains("backlink"));
        index_links(&mut app);
        assert_eq!(app.backlink_count(), Some(3));
        assert!(draw_now(&mut app).contains("3 backlinks"));
        open_in(&mut app, &dir, "README.md");
        assert_eq!(app.backlink_count(), Some(1));
        assert!(draw_now(&mut app).contains("1 backlink "), "singular");
        open_in(&mut app, &dir, "orphan.md");
        assert!(draw_now(&mut app).contains("0 backlinks"));
    }

    #[test]
    fn b_lists_who_links_here_with_the_line_of_each_link() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        assert_eq!(backlink_selected(&app), 0);
        let entries: Vec<(String, usize)> =
            app.current_backlinks().iter().map(|b| (b.source.clone(), b.line)).collect();
        assert_eq!(
            entries,
            vec![
                ("docs/other.md".to_string(), 2),
                ("README.md".to_string(), 4),
                ("README.md".to_string(), 5),
            ],
            "wikilinks with an alias count, the broken link is ignored"
        );
        let screen = draw_now(&mut app);
        assert!(screen.contains("Backlinks") && screen.contains("docs/guide.md"), "{screen}");
        assert!(screen.contains("docs/other.md:3"), "lines are shown 1-based\n{screen}");
        assert!(screen.contains("README.md:5") && screen.contains("README.md:6"), "{screen}");
        assert!(screen.contains("[the guide](docs/guide.md) is first"), "the text of the line\n{screen}");
        assert!(screen.contains("3 links"), "{screen}");
    }

    #[test]
    fn enter_opens_the_source_file_at_the_line_of_the_link() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(backlink_selected(&app), 2);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(open_name(&app), "README.md");
        let row = app.viewer.doc.lines[app.viewer.cursor].text();
        assert!(row.contains("alias"), "the cursor is on the list item with the [[wikilink]]: {row:?}");
        assert_eq!(app.focus, Focus::Viewer);
        // It is a navigation: Ctrl+O goes back to the guide.
        press_ctrl(&mut app, 'o');
        assert_eq!(open_name(&app), "guide.md");
    }

    #[test]
    fn navigation_keys_stay_in_range_and_the_popup_closes_with_esc_or_q() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        press(&mut app, KeyCode::Up);
        assert_eq!(backlink_selected(&app), 0);
        press(&mut app, KeyCode::End);
        assert_eq!(backlink_selected(&app), 2);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(backlink_selected(&app), 2);
        press(&mut app, KeyCode::Home);
        assert_eq!(backlink_selected(&app), 0);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(backlink_selected(&app), 0);
        press_ctrl(&mut app, 'c');
        assert!(matches!(app.overlay, Overlay::Backlinks { .. }) && !app.should_quit);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        press(&mut app, KeyCode::Char('B'));
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.should_quit);
    }

    #[test]
    fn a_file_nobody_links_to_says_so() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "orphan.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        let screen = draw_now(&mut app);
        assert!(screen.contains("No other file links to this one."), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Backlinks { .. }), "Enter on an empty list does nothing");
    }

    #[test]
    fn the_popup_shows_indexing_until_the_index_is_ready_and_then_fills_in() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        app.backlinks.set_walk_delay(Duration::from_millis(400));
        press(&mut app, KeyCode::Char('B'));
        assert!(app.backlinks.is_scanning(), "asking for it starts the build at once");
        let screen = draw_now(&mut app);
        assert!(screen.contains("indexing"), "{screen}");
        assert!(app.current_backlinks().is_empty());
        press(&mut app, KeyCode::Down);
        assert!(matches!(app.overlay, Overlay::Backlinks { .. }), "the interface stays responsive");
        tick_until(&mut app, "the index", |app| app.backlinks.is_ready());
        let screen = draw_now(&mut app);
        assert!(!screen.contains("indexing"), "{screen}");
        assert!(screen.contains("docs/other.md:3"), "{screen}");
    }

    #[test]
    fn the_start_up_build_waits_for_the_first_frames() {
        let dir = link_project();
        let mut app = app(&dir);
        app.tick();
        assert!(!app.backlinks.is_scanning(), "nothing runs in the first moments");
        index_links(&mut app);
        assert!(app.backlinks.is_ready());
    }

    #[test]
    fn mouse_picks_a_backlink_and_a_click_outside_closes() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        draw_now(&mut app);
        wheel(&mut app, true);
        assert_eq!(backlink_selected(&app), 1);
        click_on(&mut app, Hit::Backlink(0));
        assert_eq!(open_name(&app), "other.md");
        assert_eq!(app.viewer.source_line_at(app.viewer.cursor), Some(2));

        open_in(&mut app, &dir, "docs/guide.md");
        press(&mut app, KeyCode::Char('B'));
        draw_now(&mut app);
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn editing_links_updates_the_count_after_a_refresh() {
        let dir = link_project();
        let mut app = app(&dir);
        app.backlinks.set_test_limits(Duration::ZERO, crate::backlinks::Limits::default());
        open_in(&mut app, &dir, "orphan.md");
        index_links(&mut app);
        assert_eq!(app.backlink_count(), Some(0));

        fs::write(dir.path().join("docs/new.md"), "# New\n\n[orphan](../orphan.md)\n").unwrap();
        app.focus = Focus::Explorer;
        press(&mut app, KeyCode::Char('r'));
        tick_until(&mut app, "the rebuilt index", |app| app.backlink_count() == Some(1));
        assert_eq!(app.current_backlinks()[0].source, "docs/new.md");

        // Removing the link takes the backlink away again.
        fs::write(dir.path().join("docs/new.md"), "# New, with no link at all\n").unwrap();
        app.focus = Focus::Explorer;
        press(&mut app, KeyCode::Char('r'));
        tick_until(&mut app, "the rebuilt index", |app| app.backlink_count() == Some(0));
    }

    #[test]
    fn the_watcher_invalidates_the_link_index_too() {
        let dir = link_project();
        let mut app = app(&dir);
        app.backlinks.set_test_limits(Duration::ZERO, crate::backlinks::Limits::default());
        open_in(&mut app, &dir, "orphan.md");
        index_links(&mut app);
        fs::write(dir.path().join("fresh.md"), "[o](orphan.md)\n").unwrap();
        tick_until(&mut app, "the watcher to report it", |app| app.backlink_count() == Some(1));
    }

    #[test]
    fn b_needs_a_file_inside_the_project() {
        let dir = link_project();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("far.md"), "# Far\n").unwrap();
        let mut app = app(&dir);
        app.show_welcome();
        press(&mut app, KeyCode::Char('B'));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("No file open")));
        app.open_file(outside.path().join("far.md"), None, true);
        press(&mut app, KeyCode::Char('B'));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("inside the project")));
        assert_eq!(app.backlink_count(), None);
    }

    #[test]
    fn a_source_that_vanished_is_reported() {
        let dir = link_project();
        let mut app = app(&dir);
        open_in(&mut app, &dir, "docs/guide.md");
        index_links(&mut app);
        press(&mut app, KeyCode::Char('B'));
        fs::remove_file(dir.path().join("docs/other.md")).unwrap();
        press(&mut app, KeyCode::Enter);
        assert!(app.status_text().is_some_and(|text| text.contains("No longer exists: docs/other.md")));
        assert_eq!(open_name(&app), "guide.md");
    }

    #[test]
    fn the_key_is_configurable_and_a_letter_in_text_prompts() {
        let dir = link_project();
        let mut config = Config::default();
        config.keys.insert("backlinks".into(), crate::keymap::KeySpec::One("ctrl+b".into()));
        let mut custom = App::with_config(Some(dir.path()), config);
        custom.set_terminal_area(Rect::new(0, 0, 100, 30));
        press(&mut custom, KeyCode::Char('B'));
        assert_eq!(custom.overlay, Overlay::None, "B is no longer bound");
        press_ctrl(&mut custom, 'b');
        assert!(matches!(custom.overlay, Overlay::Backlinks { .. }));

        let mut plain = app(&dir);
        press(&mut plain, KeyCode::Char('/'));
        press(&mut plain, KeyCode::Char('B'));
        assert_eq!(plain.search_input.as_deref(), Some("B"));
        assert_eq!(plain.overlay, Overlay::None);

        let help = crate::ui::help::rendered_text(&App::default().keymap);
        assert!(help.contains("Files that link to the open file"), "{help}");
    }

    // ------------------------------------------------------ export and copy

    fn export_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("guide.md"), "# Guide\n\nSome *text* and a [link](other.md).\n\n```rust\nfn main() {}\n```\n").unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir(dir.path().join("folder")).unwrap();
        fs::write(dir.path().join("folder/inner.md"), "# Inner\n").unwrap();
        dir
    }

    fn select_in_explorer(app: &mut App, name: &str) {
        draw_now(app);
        app.focus = Focus::Explorer;
        let position = app
            .explorer
            .visible()
            .iter()
            .position(|&index| app.explorer.tree.node(index).is_some_and(|node| node.name == name))
            .unwrap_or_else(|| panic!("{name} is not in the explorer"));
        app.explorer.select_position(position);
    }

    fn menu_actions(app: &App) -> Vec<FileAction> {
        app.file_menu.as_ref().expect("a file menu").actions.clone()
    }

    fn wait_for_export(app: &mut App) -> String {
        tick_until(app, "the export", |app| app.export_job.is_none());
        app.status_text().unwrap_or_default().to_string()
    }

    fn without_converters() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_file_menu_has_an_export_entry_with_html_and_pdf_choices() {
        let dir = export_project();
        let mut app = app(&dir);
        select_in_explorer(&mut app, "guide.md");
        press(&mut app, KeyCode::Enter);
        assert_eq!(menu_actions(&app).last(), Some(&FileAction::Export));
        let screen = draw_now(&mut app);
        assert!(screen.contains("Export..."), "{screen}");

        let index = menu_actions(&app).len() - 1;
        press(&mut app, KeyCode::End);
        for _ in 0..index {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::FileMenu { selected: 0 }, "the menu stays open on the formats");
        assert_eq!(menu_actions(&app), vec![FileAction::ExportHtml, FileAction::ExportPdf]);
        let screen = draw_now(&mut app);
        assert!(screen.contains("HTML") && screen.contains("PDF") && screen.contains("Export: guide.md"), "{screen}");
    }

    #[test]
    fn folders_have_no_export_entry() {
        let dir = export_project();
        let mut app = app(&dir);
        select_in_explorer(&mut app, "folder");
        press(&mut app, KeyCode::Enter);
        assert!(!menu_actions(&app).contains(&FileAction::Export));
    }

    #[test]
    fn esc_in_the_export_choices_goes_back_to_the_actions_and_then_closes() {
        let dir = export_project();
        let mut app = app(&dir);
        select_in_explorer(&mut app, "guide.md");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('8'));
        assert!(app.file_menu.as_ref().unwrap().submenu);
        press(&mut app, KeyCode::Esc);
        assert!(!app.file_menu.as_ref().unwrap().submenu, "back to the file's actions");
        assert!(menu_actions(&app).contains(&FileAction::Export));
        let Overlay::FileMenu { selected } = app.overlay else { panic!("the menu closed") };
        assert_eq!(menu_actions(&app)[selected], FileAction::Export, "the cursor is on Export...");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.file_menu.is_none());
    }

    #[test]
    fn exporting_html_from_the_menu_writes_next_to_the_original_and_says_where() {
        let dir = export_project();
        let mut app = app(&dir);
        select_in_explorer(&mut app, "guide.md");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("Exporting HTML")));
        let status = wait_for_export(&mut app);
        let written = dir.path().join("guide.html");
        assert!(status.contains("Exported HTML") && status.contains(&written.display().to_string()), "{status}");
        let html = fs::read_to_string(&written).unwrap();
        assert!(html.contains("<h1 id=\"guide\">Guide</h1>") && html.contains("<style>"));
        // The explorer and the finder pick the new file up.
        app.tick();
        let names: Vec<String> = app
            .explorer
            .visible()
            .iter()
            .filter_map(|&i| app.explorer.tree.node(i).map(|node| node.name.clone()))
            .collect();
        assert!(names.contains(&"guide.html".to_string()), "{names:?}");
    }

    #[test]
    fn h_and_p_pick_a_format_in_the_export_choices_and_clicks_work() {
        let dir = export_project();
        let none = without_converters();
        let mut app = app(&dir);
        app.export_path_var = Some(none.path().as_os_str().to_os_string());
        select_in_explorer(&mut app, "guide.md");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('h'));
        assert!(wait_for_export(&mut app).contains("Exported HTML"));

        select_in_explorer(&mut app, "guide.md");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('8'));
        draw_now(&mut app);
        click_on(&mut app, Hit::FileAction(1));
        let status = wait_for_export(&mut app);
        assert!(status.contains("PDF export needs"), "p / the second row is PDF: {status}");
    }

    #[test]
    fn pdf_without_a_converter_says_what_is_missing_and_keeps_the_html() {
        let dir = export_project();
        let none = without_converters();
        let mut app = app(&dir);
        app.export_path_var = Some(none.path().as_os_str().to_os_string());
        app.start_export(&dir.path().join("guide.md"), Format::Pdf);
        let status = wait_for_export(&mut app);
        for program in ["wkhtmltopdf", "chromium", "weasyprint"] {
            assert!(status.contains(program), "{status}");
        }
        assert!(status.contains("Wrote HTML instead") && status.contains("guide.html"), "{status}");
        assert!(dir.path().join("guide.html").is_file());
        assert!(!dir.path().join("guide.pdf").exists());
    }

    #[cfg(unix)]
    #[test]
    fn pdf_uses_the_converter_found_in_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = export_project();
        let bin = tempfile::tempdir().unwrap();
        let script = bin.path().join("wkhtmltopdf");
        fs::write(&script, "#!/bin/sh\nfor last; do :; done\nprintf '%%PDF-1.4 fake' > \"$last\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let mut app = app(&dir);
        app.export_path_var = Some(bin.path().as_os_str().to_os_string());
        app.start_export(&dir.path().join("guide.md"), Format::Pdf);
        assert!(app.status_text().is_some_and(|text| text.contains("Exporting PDF")));
        let status = wait_for_export(&mut app);
        assert!(status.starts_with("Exported PDF (wkhtmltopdf)"), "{status}");
        assert!(status.contains(&dir.path().join("guide.pdf").display().to_string()));
        assert!(fs::read(dir.path().join("guide.pdf")).unwrap().starts_with(b"%PDF"));
    }

    #[test]
    fn the_shortcut_exports_the_open_document() {
        let dir = export_project();
        let none = without_converters();
        let mut app = app(&dir);
        app.export_path_var = Some(none.path().as_os_str().to_os_string());
        app.open_file(dir.path().join("guide.md"), None, true);
        press(&mut app, KeyCode::Char('X'));
        assert!(matches!(app.overlay, Overlay::FileMenu { .. }));
        let screen = draw_now(&mut app);
        assert!(screen.contains("Export: guide.md"), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(wait_for_export(&mut app).contains("Exported HTML"));
        assert!(dir.path().join("guide.html").is_file());

        // Esc closes the menu: there is no actions menu behind the shortcut.
        press(&mut app, KeyCode::Char('X'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.file_menu.is_none());
    }

    #[test]
    fn the_export_shortcut_needs_an_open_file_and_is_configurable() {
        let dir = export_project();
        let mut app = app(&dir);
        app.show_welcome();
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status_text().is_some_and(|text| text.contains("No file open to export")));

        let mut config = Config::default();
        config.keys.insert("export".into(), crate::keymap::KeySpec::One("ctrl+g".into()));
        let mut custom = App::with_config(Some(dir.path()), config);
        custom.set_terminal_area(Rect::new(0, 0, 100, 30));
        custom.open_file(dir.path().join("guide.md"), None, true);
        press(&mut custom, KeyCode::Char('X'));
        assert_eq!(custom.overlay, Overlay::None, "X is no longer bound");
        press_ctrl(&mut custom, 'g');
        assert!(matches!(custom.overlay, Overlay::FileMenu { .. }));
        let help = crate::ui::help::rendered_text(&App::default().keymap);
        assert!(help.contains("Export the open document"), "{help}");
        assert!(help.contains("Copy the source Markdown"), "{help}");
    }

    #[test]
    fn the_export_folder_is_configurable() {
        let dir = export_project();
        let mut config = Config::default();
        config.export_dir = Some("exports/out".to_string());
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        let source = dir.path().join("folder/inner.md");
        assert_eq!(app.export_dir_for(&source), dir.path().join("exports/out"), "relative: from the project root");
        app.start_export(&source, Format::Html);
        let status = wait_for_export(&mut app);
        assert!(status.contains(&dir.path().join("exports/out/inner.html").display().to_string()), "{status}");
        assert!(dir.path().join("exports/out/inner.html").is_file());

        let absolute = tempfile::tempdir().unwrap();
        app.config.export_dir = Some(absolute.path().display().to_string());
        assert_eq!(app.export_dir_for(&source), absolute.path());
        if let Some(home) = dirs::home_dir() {
            app.config.export_dir = Some("~/rtui-exports".into());
            assert_eq!(app.export_dir_for(&source), home.join("rtui-exports"));
        }
        app.config.export_dir = Some("   ".into());
        assert_eq!(app.export_dir_for(&source), dir.path().join("folder"), "blank means next to the original");
        app.config.export_dir = None;
        assert_eq!(app.export_dir_for(&source), dir.path().join("folder"));
    }

    #[test]
    fn source_files_export_and_a_second_export_waits_for_the_first() {
        let dir = export_project();
        let mut app = app(&dir);
        app.start_export(&dir.path().join("main.rs"), Format::Html);
        app.start_export(&dir.path().join("main.rs"), Format::Html);
        assert!(app.status_text().is_some_and(|text| text.contains("already running")));
        let status = wait_for_export(&mut app);
        assert!(status.contains("main.rs.html"), "{status}");
        assert!(fs::read_to_string(dir.path().join("main.rs.html")).unwrap().contains("fn"));
        // Once it finished another one can start.
        app.start_export(&dir.path().join("guide.md"), Format::Html);
        assert!(app.export_job.is_some());
        wait_for_export(&mut app);
    }

    #[test]
    fn a_failed_export_is_reported_not_hidden() {
        let dir = export_project();
        let mut app = app(&dir);
        app.start_export(&dir.path().join("gone.md"), Format::Html);
        let status = wait_for_export(&mut app);
        assert!(status.contains("Cannot export gone.md"), "{status}");
    }

    // ---------------------------------------------------- copy as Markdown

    const COPY_SOURCE: &str = "# Title\n\nPlain paragraph.\n\n- apple\n- banana\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\nSee [the guide](docs/guide.md) now.\n";

    fn doc_row(app: &App, needle: &str) -> usize {
        app.viewer
            .doc
            .lines
            .iter()
            .position(|line| line.text().contains(needle))
            .unwrap_or_else(|| panic!("no row shows {needle:?}"))
    }

    /// Drag from document row `first` to row `last` (both visible).
    fn select_rows(app: &mut App, first: usize, last: usize) {
        let viewer = app.layout.viewer;
        let top = app.viewer.top;
        let y = |row: usize| viewer.y + (row - top) as u16;
        send_mouse(app, MouseEventKind::Down(MouseButton::Left), viewer.x + 1, y(first));
        send_mouse(app, MouseEventKind::Drag(MouseButton::Left), viewer.x + 10, y(last));
        send_mouse(app, MouseEventKind::Up(MouseButton::Left), viewer.x + 10, y(last));
    }

    /// Select from the row showing `from` to the row showing `to`.
    fn select_between(app: &mut App, from: &str, to: &str) {
        let (first, last) = (doc_row(app, from), doc_row(app, to));
        select_rows(app, first, last);
    }

    fn copy_project() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("copy.md"), COPY_SOURCE).unwrap();
        let mut app = app(&dir);
        app.open_file(dir.path().join("copy.md"), None, true);
        draw_now(&mut app);
        let _ = app.take_clipboard();
        (dir, app)
    }

    fn copied(app: &mut App) -> (String, String) {
        let status = app.status_text().map(str::to_string);
        let text = app.take_clipboard().unwrap_or_else(|| panic!("nothing was copied; status: {status:?}"));
        let note = app.copy_status(&text);
        (text, note)
    }

    #[test]
    fn y_copies_the_original_markdown_not_the_rendered_text() {
        let (_dir, mut app) = copy_project();
        // Two lines inside the code block: exactly those source lines.
        select_between(&mut app, "let x", "let y");
        let _ = app.take_clipboard();
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, "let x = 1;\nlet y = 2;");
        assert_eq!(note, "Copied Markdown: lines 9-10");

        // A heading and the paragraph after it: both source lines, as written.
        select_between(&mut app, "Title", "Plain paragraph");
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, "# Title\n\nPlain paragraph.");
        assert_eq!(note, "Copied Markdown: lines 1-3");
    }

    #[test]
    fn list_items_keep_their_markers_and_fences_come_along_when_selected() {
        let (_dir, mut app) = copy_project();
        select_between(&mut app, "apple", "banana");
        press(&mut app, KeyCode::Char('Y'));
        let (text, _) = copied(&mut app);
        assert_eq!(text, "- apple\n- banana");

        // From the top border of the code block to its bottom border.
        let top = doc_row(&app, "let x") - 1;
        let bottom = doc_row(&app, "let y") + 1;
        select_rows(&mut app, top, bottom);
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, "```rust\nlet x = 1;\nlet y = 2;\n```");
        assert_eq!(note, "Copied Markdown: lines 8-11");
    }

    #[test]
    fn a_link_keeps_its_destination_and_the_note_says_the_block_was_copied_whole() {
        let (_dir, mut app) = copy_project();
        let row = doc_row(&app, "the guide");
        select_rows(&mut app, row, row);
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, "See [the guide](docs/guide.md) now.");
        assert!(note.contains("whole blocks") && note.contains("could not be mapped exactly"), "{note}");
        assert!(note.contains("line 13"), "{note}");
    }

    #[test]
    fn a_selection_made_upwards_or_past_the_end_still_works() {
        let (_dir, mut app) = copy_project();
        select_between(&mut app, "banana", "apple");
        press(&mut app, KeyCode::Char('Y'));
        assert_eq!(copied(&mut app).0, "- apple\n- banana");

        // Dragging into the empty area below the document clamps to its end.
        let viewer = app.layout.viewer;
        let row = doc_row(&app, "the guide");
        let y = viewer.y + (row - app.viewer.top) as u16;
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), viewer.x + 1, y);
        send_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), viewer.x + 8, viewer.bottom() - 1);
        send_mouse(&mut app, MouseEventKind::Up(MouseButton::Left), viewer.x + 8, viewer.bottom() - 1);
        press(&mut app, KeyCode::Char('Y'));
        assert_eq!(copied(&mut app).0, "See [the guide](docs/guide.md) now.");
    }

    #[test]
    fn y_without_a_usable_selection_explains_what_to_do() {
        let (_dir, mut app) = copy_project();
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.status_text().is_some_and(|text| text.contains("Select text in the document")));
        assert!(app.take_clipboard().is_none());
        // A click without a drag selects nothing.
        let viewer = app.layout.viewer;
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), viewer.x + 2, viewer.y + 1);
        send_mouse(&mut app, MouseEventKind::Up(MouseButton::Left), viewer.x + 2, viewer.y + 1);
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.take_clipboard().is_none());
        // A drag still in progress is not a selection yet.
        send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), viewer.x + 2, viewer.y + 1);
        send_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), viewer.x + 9, viewer.y + 3);
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.take_clipboard().is_none());
        // A selection that belongs to another pane is refused.
        let other = Rect::new(0, 0, 5, 5);
        let mut selection = crate::selection::Selection::new(other, 0, 0);
        selection.drag_to(4, 3);
        selection.done = true;
        app.selection = Some(selection);
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.status_text().is_some_and(|text| text.contains("not in the document")));
        assert!(app.selection.is_none(), "the key press ended the selection");
        assert!(app.take_clipboard().is_none());
    }

    #[test]
    fn wrapped_plain_text_copies_the_whole_source_line() {
        let dir = tempfile::tempdir().unwrap();
        let long = "alpha ".repeat(60);
        fs::write(dir.path().join("long.txt"), format!("first\n{long}\nlast\n")).unwrap();
        let mut app = app(&dir);
        app.open_file(dir.path().join("long.txt"), None, true);
        draw_now(&mut app);
        let _ = app.take_clipboard();
        let wrapped = app.viewer.row_of_source_line(1).unwrap() + 1;
        select_rows(&mut app, wrapped, wrapped);
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, long, "the full line, not the row");
        assert!(note.contains("whole blocks") && note.contains("line 2"), "{note}");
    }

    #[test]
    fn source_code_files_copy_their_lines() {
        let dir = tempfile::tempdir().unwrap();
        let code: String = (1..=12).map(|n| format!("let value_{n} = {n};\n")).collect();
        fs::write(dir.path().join("code.rs"), code).unwrap();
        let mut app = app(&dir);
        app.open_file(dir.path().join("code.rs"), None, true);
        draw_now(&mut app);
        let _ = app.take_clipboard();
        let (first, last) = (app.viewer.row_of_source_line(2).unwrap(), app.viewer.row_of_source_line(4).unwrap());
        select_rows(&mut app, first, last);
        press(&mut app, KeyCode::Char('Y'));
        let (text, note) = copied(&mut app);
        assert_eq!(text, "let value_3 = 3;\nlet value_4 = 4;\nlet value_5 = 5;");
        assert_eq!(note, "Copied source: lines 3-5");
    }

    #[test]
    fn rows_are_mapped_through_the_scroll_position() {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (1..=80).map(|n| format!("- item {n:02}\n")).collect();
        fs::write(dir.path().join("long.md"), text).unwrap();
        let mut app = app(&dir);
        app.open_file(dir.path().join("long.md"), None, true);
        draw_now(&mut app);
        app.viewer.jump_to_line(60);
        draw_now(&mut app);
        assert!(app.viewer.top > 0);
        let _ = app.take_clipboard();
        let (first, last) = (doc_row(&app, "item 50"), doc_row(&app, "item 52"));
        select_rows(&mut app, first, last);
        press(&mut app, KeyCode::Char('Y'));
        assert_eq!(copied(&mut app).0, "- item 50\n- item 51\n- item 52");
    }

    #[test]
    fn y_is_a_letter_in_the_search_prompt_and_configurable() {
        let (_dir, mut app) = copy_project();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('Y'));
        assert_eq!(app.search_input.as_deref(), Some("Y"));
        press(&mut app, KeyCode::Esc);

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "- one\n- two\n").unwrap();
        let mut config = Config::default();
        config.keys.insert("copy_markdown".into(), crate::keymap::KeySpec::One("ctrl+y".into()));
        let mut custom = App::with_config(Some(dir.path()), config);
        custom.set_terminal_area(Rect::new(0, 0, 100, 30));
        custom.open_file(dir.path().join("a.md"), None, true);
        draw_now(&mut custom);
        select_rows(&mut custom, 0, 1);
        let _ = custom.take_clipboard();
        press(&mut custom, KeyCode::Char('Y'));
        assert!(custom.take_clipboard().is_none(), "Y is no longer bound");
        // Any key ends a selection, so select again for the bound key.
        select_rows(&mut custom, 0, 1);
        let _ = custom.take_clipboard();
        press_ctrl(&mut custom, 'y');
        assert_eq!(copied(&mut custom).0, "- one\n- two");
    }
}

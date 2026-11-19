//! Document loading, rendering lifecycle, scrolling and search.
//!
//! The viewer opens files with `fs::read`, parses them in memory, and never
//! writes them itself. Editing is delegated to an external editor requested by
//! the application layer.

pub mod syntax;
pub mod text;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::markdown::code_highlighter::{
    CodeHighlighter, HighlightSession, canonical_label, sniff_language,
};
use crate::markdown::parser::Block;
use crate::markdown::{Heading, LineSpan, RSpan, RenderedDoc, SourceRange, parse_spanned, renderer, toc};
use crate::ui::themes::{Theme, ThemeName};

/// Preview limit.  Larger files are reported as unsupported instead of
/// freezing the interface.
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

/// Source files with at least this many lines are highlighted lazily.
pub const LAZY_MIN_LINES: usize = 400;
/// Source lines highlighted when a long file is opened.
pub const INITIAL_LINES: usize = 200;
/// Source lines highlighted each time the reader gets close to the end of
/// what is already shown.
pub const CHUNK_LINES: usize = 300;
/// How many rendered rows below the viewport are kept ready.
pub const LOOKAHEAD_ROWS: usize = 60;

/// Highlighting progress of a long source file: only the first `spans.len()`
/// lines are coloured and rendered, the rest waits until it is scrolled to.
#[derive(Debug)]
pub struct LazyCode {
    session: HighlightSession,
    theme: ThemeName,
    spans: Vec<Vec<RSpan>>,
    total: usize,
}

impl LazyCode {
    fn is_complete(&self) -> bool {
        self.spans.len() >= self.total
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocKind {
    Markdown,
    Text,
    Code(String),
    Binary,
    TooLarge,
    Empty,
    Error,
}

impl DocKind {
    pub fn badge(&self) -> &str {
        match self {
            DocKind::Markdown => "markdown",
            DocKind::Text => "text",
            DocKind::Code(label) => label,
            DocKind::Binary => "binary",
            DocKind::TooLarge => "too large",
            DocKind::Empty => "empty",
            DocKind::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ParsedContent {
    Markdown(MarkdownDoc),
    Plain(Vec<String>),
    Code { lang: String, lines: Vec<String> },
    Message { title: String, detail: String },
}

/// A parsed Markdown file: its blocks, the source lines each came from and the
/// source text itself (for copying and exporting the original Markdown).
#[derive(Debug, Clone)]
pub struct MarkdownDoc {
    pub blocks: Vec<Block>,
    pub spans: Vec<LineSpan>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct LoadedDoc {
    pub path: PathBuf,
    pub kind: DocKind,
    pub content: ParsedContent,
    pub bytes: u64,
}

impl LoadedDoc {
    pub fn error(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: DocKind::Error,
            content: ParsedContent::Message {
                title: "Unable to open file".to_string(),
                detail: detail.into(),
            },
            bytes: 0,
        }
    }

    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn badge(&self) -> String {
        match self.kind {
            DocKind::Text => text_badge(&self.path),
            _ => self.kind.badge().to_string(),
        }
    }
}

/// Badge of a plain-text document.  A bare "text" next to a name such as
/// `.gitignore` reads like an extension, so say what the file is: "text" for
/// `.txt`, otherwise the extension (`text · .rst`) or the whole name
/// (`text · .gitignore`, `text · LICENSE`).
fn text_badge(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let short = |text: &str| -> String {
        if text.chars().count() > 24 {
            format!("{}…", text.chars().take(23).collect::<String>())
        } else {
            text.to_string()
        }
    };
    if name.is_empty() {
        return "text".to_string();
    }
    if name.starts_with('.') {
        return format!("text · {}", short(&name));
    }
    match path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()) {
        Some(ext) if ext == "txt" || ext == "text" => "text".to_string(),
        Some(ext) => format!("text · .{}", short(&ext)),
        None => format!("text · {}", short(&name)),
    }
}

/// Load and parse a file.  The file is read once, in full, and never written.
pub fn load_document(path: &Path) -> Result<LoadedDoc> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("cannot read metadata for {}", path.display()))?;
    if metadata.is_dir() {
        bail!("{} is a directory", path.display());
    }

    let bytes = metadata.len();

    if bytes == 0 {
        return Ok(LoadedDoc {
            path: path.to_path_buf(),
            kind: DocKind::Empty,
            content: ParsedContent::Message {
                title: "Empty file".to_string(),
                detail: "There is nothing to preview in this file.".to_string(),
            },
            bytes,
        });
    }

    if bytes > MAX_FILE_SIZE {
        return Ok(LoadedDoc {
            path: path.to_path_buf(),
            kind: DocKind::TooLarge,
            content: ParsedContent::Message {
                title: "File too large to preview".to_string(),
                detail: format!(
                    "{} is {}. runetui previews files up to {} to keep scrolling responsive.",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    format_size(bytes),
                    format_size(MAX_FILE_SIZE)
                ),
            },
            bytes,
        });
    }

    let raw = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;

    // Binary detection: NUL bytes or invalid UTF-8.
    if raw.iter().take(8192).any(|byte| *byte == 0) {
        return Ok(binary_doc(path, bytes, "It contains binary data."));
    }
    let source = match String::from_utf8(raw) {
        Ok(source) => source,
        Err(_) => {
            return Ok(binary_doc(path, bytes, "It is not valid UTF-8 text."));
        }
    };

    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Ok(LoadedDoc {
            path: path.to_path_buf(),
            kind: DocKind::Markdown,
            content: {
                let parsed = parse_spanned(&source);
                ParsedContent::Markdown(MarkdownDoc {
                    blocks: parsed.blocks,
                    spans: parsed.spans,
                    source,
                })
            },
            bytes,
        }),
        "txt" | "text" | "log" | "rst" | "adoc" | "org" => Ok(LoadedDoc {
            path: path.to_path_buf(),
            kind: DocKind::Text,
            content: ParsedContent::Plain(split_lines(&source)),
            bytes,
        }),
        "" => {
            // Extensionless files: honour a shebang when possible, otherwise
            // display them as plain text.
            let trimmed = source.trim_start();
            let sample = &trimmed[..trimmed.len().min(1024)];
            if trimmed.starts_with("#!") {
                if let Some(lang) = sniff_language(sample) {
                    return Ok(LoadedDoc {
                        path: path.to_path_buf(),
                        kind: DocKind::Code(lang.to_string()),
                        content: ParsedContent::Code {
                            lang: lang.to_string(),
                            lines: split_lines(&source),
                        },
                        bytes,
                    });
                }
            }
            // A well-known name (`Makefile`) that syntect has a syntax for.
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !file_name.starts_with('.') {
                if let Some(token) = crate::markdown::code_highlighter::syntax_token_for_file_name(&file_name) {
                    return Ok(LoadedDoc {
                        path: path.to_path_buf(),
                        kind: DocKind::Code(token.clone()),
                        content: ParsedContent::Code {
                            lang: token,
                            lines: split_lines(&source),
                        },
                        bytes,
                    });
                }
            }
            Ok(LoadedDoc {
                path: path.to_path_buf(),
                kind: DocKind::Text,
                content: ParsedContent::Plain(split_lines(&source)),
                bytes,
            })
        }
        _ => {
            let label = canonical_label(&extension);
            Ok(LoadedDoc {
                path: path.to_path_buf(),
                kind: DocKind::Code(label.clone()),
                content: ParsedContent::Code {
                    lang: extension,
                    lines: split_lines(&source),
                },
                bytes,
            })
        }
    }
}

fn binary_doc(path: &Path, bytes: u64, reason: &str) -> LoadedDoc {
    LoadedDoc {
        path: path.to_path_buf(),
        kind: DocKind::Binary,
        content: ParsedContent::Message {
            title: "Binary file".to_string(),
            detail: format!(
                "{reason} runetui only previews text documents, and will never modify the original file."
            ),
        },
        bytes,
    }
}

fn split_lines(source: &str) -> Vec<String> {
    let mut lines: Vec<String> = source
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

pub fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Render a loaded document for the given width and theme.
#[allow(dead_code)]
pub fn render_loaded(
    loaded: &LoadedDoc,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    render_document(loaded, width, theme, highlighter, None).0
}

/// Like [`render_loaded`], but long source files are only highlighted up to
/// the point already reached (`previous`) or the first chunk, and the state to
/// continue from is returned.
fn render_document(
    loaded: &LoadedDoc,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
    previous: Option<LazyCode>,
) -> (RenderedDoc, Option<LazyCode>) {
    match &loaded.content {
        ParsedContent::Markdown(markdown) => (
            renderer::render_with_source(
                &markdown.blocks,
                &markdown.spans,
                &markdown.source,
                width,
                theme,
                highlighter,
            ),
            None,
        ),
        ParsedContent::Plain(lines) => (text::render_plain(lines, width, theme), None),
        ParsedContent::Code { lang, lines, .. } if lines.len() >= LAZY_MIN_LINES => {
            let total = lines.len();
            let lazy = match previous {
                // Same theme: the colours are still valid, only re-wrap.
                Some(lazy) if lazy.theme == theme.name && lazy.total == total => lazy,
                other => {
                    let keep = other
                        .map_or(INITIAL_LINES, |lazy| lazy.spans.len())
                        .clamp(1, total);
                    let sample = lines
                        .iter()
                        .take(syntax::SAMPLE_LINES)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut session = highlighter.start_session(lang, &sample, &theme);
                    let spans = highlighter.highlight_chunk(&mut session, &lines[..keep], &theme);
                    LazyCode {
                        session,
                        theme: theme.name,
                        spans,
                        total,
                    }
                }
            };
            let layout = syntax::code_layout(total, width);
            let mut doc = RenderedDoc::default();
            doc.lines = syntax::render_code_range(&lazy.spans, 0, layout, theme);
            if lazy.is_complete() {
                doc.lines.extend(syntax::closing_lines(layout, theme));
            }
            (doc, Some(lazy))
        }
        ParsedContent::Code { lang, lines, .. } => (
            syntax::render_code(lines, lang, width, theme, highlighter),
            None,
        ),
        ParsedContent::Message { title, detail } => {
            (text::render_message(title, detail, width, theme), None)
        }
    }
}

// ------------------------------------------------------------------- search

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub line: usize,
    /// Character index (not bytes) into the line's plain text.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub current: Option<usize>,
}

impl SearchState {
    pub fn current_match(&self) -> Option<SearchMatch> {
        self.current
            .and_then(|index| self.matches.get(index))
            .copied()
    }
}

fn lower_char(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// Case-insensitive search over the rendered document.
pub fn find_matches(doc: &RenderedDoc, query: &str) -> Vec<SearchMatch> {
    let needle: Vec<char> = query.chars().map(lower_char).collect();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (line_index, line) in doc.lines.iter().enumerate() {
        let text = line.text();
        let haystack: Vec<char> = text.chars().map(lower_char).collect();
        if haystack.len() < needle.len() {
            continue;
        }
        let mut start = 0;
        while start + needle.len() <= haystack.len() {
            if haystack[start..start + needle.len()] == needle[..] {
                matches.push(SearchMatch {
                    line: line_index,
                    start,
                    end: start + needle.len(),
                });
                start += needle.len().max(1);
            } else {
                start += 1;
            }
        }
    }
    matches
}

// ------------------------------------------------------------------- viewer

#[derive(Debug)]
pub struct Viewer {
    pub loaded: Option<Arc<LoadedDoc>>,
    pub doc: RenderedDoc,
    pub cursor: usize,
    pub top: usize,
    pub viewport_height: usize,
    pub rendered_width: usize,
    pub rendered_theme: Option<ThemeName>,
    pub search: Option<SearchState>,
    pub title: String,
    pub kind_label: String,
    welcome_source: Option<String>,
    /// Set while a long source file is only partly highlighted.
    lazy: Option<LazyCode>,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            loaded: None,
            doc: RenderedDoc::default(),
            cursor: 0,
            top: 0,
            viewport_height: 20,
            rendered_width: 0,
            rendered_theme: None,
            search: None,
            title: "Welcome".to_string(),
            kind_label: "welcome".to_string(),
            welcome_source: None,
            lazy: None,
        }
    }
}

impl Viewer {
    pub fn has_document(&self) -> bool {
        self.loaded.is_some() || self.welcome_source.is_some()
    }

    pub fn show(
        &mut self,
        loaded: Arc<LoadedDoc>,
        width: usize,
        theme: Theme,
        highlighter: &mut CodeHighlighter,
        anchor: Option<&str>,
    ) {
        self.title = loaded.display_name();
        self.kind_label = loaded.badge();
        self.welcome_source = None;
        let (doc, lazy) = render_document(&loaded, width, theme, highlighter, None);
        self.lazy = lazy;
        let cursor = anchor
            .and_then(|anchor| toc::resolve_anchor(&doc.headings, anchor))
            .unwrap_or(0);
        self.loaded = Some(loaded);
        self.doc = doc;
        self.cursor = cursor;
        self.top = 0;
        self.rendered_width = width;
        self.rendered_theme = Some(theme.name);
        self.search = None;
        self.ensure_visible();
    }

    pub fn show_welcome(
        &mut self,
        project_name: &str,
        project_path: &str,
        width: usize,
        theme: Theme,
        highlighter: &mut CodeHighlighter,
    ) {
        let name = project_name.replace('`', "'");
        let path = project_path.replace('`', "'");
        let source = format!(
            "# runetui\n\n**A Markdown and source-code viewer.**\n\n`{path}`\n\n## Welcome\n\n\
             Use the arrow keys in the explorer to preview a document as you browse. Press `Space` \
             or `Enter` to move into the document, and `e` to edit the selected file right here in vim.\n\n\
             ## Quick keys\n\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `↑` / `↓` | Move selection or document |\n\
             | `Space` / `Enter` | Open the file and focus the document |\n\
             | `←` / `→` | Collapse / expand folders, or return to the explorer |\n\
             | `Ctrl+D` / `Ctrl+U` | Half-page scroll |\n\
             | `Tab` | Switch explorer / viewer |\n\
             | `e` | Edit the selected file here (vim in the pane) |\n\
             | `/` | Search in document |\n\
             | `o` | Document outline |\n\
             | `Esc` | Menu: options, help, quit (btop style) |\n\
             | `T` | Cycle theme |\n\
             | `?` | All key bindings |\n\n\
             ## Project\n\n\
             **{name}** is open. Moving through the explorer previews files automatically. Use \
             `Ctrl+R` then `b` to hide or show the explorer and `H` to include hidden files. Internal \
             Markdown links, `[[wikilinks]]` and anchors can be opened with `Enter` when the cursor \
             is on their line.\n"
        );
        let parsed = parse_spanned(&source);
        self.loaded = None;
        self.lazy = None;
        self.doc = renderer::render_with_source(
            &parsed.blocks,
            &parsed.spans,
            &source,
            width,
            theme,
            highlighter,
        );
        self.title = "Welcome".to_string();
        self.kind_label = "welcome".to_string();
        self.cursor = 0;
        self.top = 0;
        self.rendered_width = width;
        self.rendered_theme = Some(theme.name);
        self.search = None;
        self.welcome_source = Some(source);
    }

    pub fn rerender(&mut self, width: usize, theme: Theme, highlighter: &mut CodeHighlighter) {
        let old_len = self.doc.lines.len().max(1);
        let ratio = self.cursor as f32 / old_len as f32;
        let old_search_line = self
            .search
            .as_ref()
            .and_then(SearchState::current_match)
            .map(|item| item.line);

        if let Some(loaded) = self.loaded.clone() {
            let previous = self.lazy.take();
            let (doc, lazy) = render_document(&loaded, width, theme, highlighter, previous);
            self.doc = doc;
            self.lazy = lazy;
        } else if let Some(source) = &self.welcome_source {
            let parsed = parse_spanned(source);
            self.doc = renderer::render_with_source(
                &parsed.blocks,
                &parsed.spans,
                source,
                width,
                theme,
                highlighter,
            );
        } else {
            return;
        }

        self.rendered_width = width;
        self.rendered_theme = Some(theme.name);

        let new_len = self.doc.lines.len();
        self.cursor = ((ratio * new_len as f32).round() as usize).min(new_len.saturating_sub(1));

        if let Some(mut search) = self.search.take() {
            search.matches = find_matches(&self.doc, &search.query);
            search.current = match (old_search_line, search.matches.is_empty()) {
                (_, true) => None,
                (Some(line), false) => Some(
                    search
                        .matches
                        .iter()
                        .position(|item| item.line >= line)
                        .unwrap_or(search.matches.len() - 1),
                ),
                (None, false) => Some(0),
            };
            if !search.matches.is_empty() {
                self.search = Some(search);
            }
        }

        self.ensure_visible();
    }

    // ---------------------------------------------------- lazy highlighting

    /// True while a long source file is only partly highlighted.
    pub fn is_partial(&self) -> bool {
        self.lazy.as_ref().is_some_and(|lazy| !lazy.is_complete())
    }

    /// The reader is close to the end of what has been highlighted so far.
    pub fn needs_more(&self) -> bool {
        self.is_partial()
            && self.cursor + self.viewport_height + LOOKAHEAD_ROWS >= self.doc.lines.len()
    }

    /// Highlight and append the next chunk.  Returns false when there was
    /// nothing to do.
    pub fn load_more(&mut self, theme: Theme, highlighter: &mut CodeHighlighter) -> bool {
        let Some(loaded) = self.loaded.clone() else {
            return false;
        };
        let ParsedContent::Code { lines, .. } = &loaded.content else {
            return false;
        };
        let Some(lazy) = self.lazy.as_mut() else {
            return false;
        };
        if lazy.is_complete() || lazy.theme != theme.name {
            return false;
        }
        let from = lazy.spans.len();
        let to = (from + CHUNK_LINES).min(lazy.total);
        let chunk = highlighter.highlight_chunk(&mut lazy.session, &lines[from..to], &theme);
        let layout = syntax::code_layout(lazy.total, self.rendered_width);
        self.doc
            .lines
            .extend(syntax::render_code_range(&chunk, from, layout, theme));
        lazy.spans.extend(chunk);
        if lazy.is_complete() {
            self.doc.lines.extend(syntax::closing_lines(layout, theme));
        }
        true
    }

    /// Highlight the whole file (search, jump to the end...).
    pub fn load_all(&mut self, theme: Theme, highlighter: &mut CodeHighlighter) {
        while self.load_more(theme, highlighter) {}
    }

    /// Make sure rendered row `line` exists, loading chunks as needed.
    pub fn load_through(&mut self, line: usize, theme: Theme, highlighter: &mut CodeHighlighter) {
        while self.doc.lines.len() <= line && self.load_more(theme, highlighter) {}
    }

    /// Number of source lines highlighted so far and the file's total.
    #[allow(dead_code)]
    pub fn highlighted_lines(&self) -> Option<(usize, usize)> {
        self.lazy.as_ref().map(|lazy| (lazy.spans.len(), lazy.total))
    }

    pub fn heading_nearest_cursor(&self) -> Option<&Heading> {
        self.doc
            .headings
            .iter()
            .rev()
            .find(|heading| heading.line <= self.cursor)
    }

    pub fn outline(&self) -> Vec<toc::OutlineItem> {
        toc::outline(&self.doc.headings)
    }

    // --------------------------------------------------------- navigation

    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
        self.ensure_visible();
    }

    pub fn ensure_visible(&mut self) {
        let height = self.viewport_height.max(1);
        let max_top = self.doc.lines.len().saturating_sub(height);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + height {
            self.top = self.cursor + 1 - height;
        }
        self.top = self.top.min(max_top);
        if self.doc.lines.is_empty() {
            self.cursor = 0;
            self.top = 0;
        } else {
            self.cursor = self.cursor.min(self.doc.lines.len() - 1);
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.doc.lines.is_empty() {
            return;
        }
        let last = self.doc.lines.len().saturating_sub(1);
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
        self.ensure_visible();
    }

    pub fn move_page(&mut self, down: bool) {
        let delta = (self.viewport_height.saturating_sub(2).max(1)) as isize;
        self.move_cursor(if down { delta } else { -delta });
    }

    pub fn move_half_page(&mut self, down: bool) {
        let delta = (self.viewport_height / 2).max(1) as isize;
        self.move_cursor(if down { delta } else { -delta });
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
        self.ensure_visible();
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.doc.lines.len().saturating_sub(1);
        self.ensure_visible();
    }

    pub fn jump_to_line(&mut self, line: usize) {
        self.cursor = line;
        self.ensure_visible();
    }

    pub fn progress(&self) -> usize {
        let total = self.total_lines();
        if total <= 1 {
            return 100;
        }
        (self.cursor * 100 / (total - 1)).min(100)
    }

    /// Rendered rows; for a partly highlighted file an estimate of the whole.
    pub fn total_lines(&self) -> usize {
        match &self.lazy {
            Some(lazy) if !lazy.is_complete() && !lazy.spans.is_empty() => {
                self.doc.lines.len() * lazy.total / lazy.spans.len()
            }
            _ => self.doc.lines.len(),
        }
    }

    // ------------------------------------------------------------- search

    /// Apply a search query, jumping to the first match at or after the
    /// cursor.  Returns the number of matches.
    pub fn apply_search(&mut self, query: &str) -> usize {
        let matches = find_matches(&self.doc, query);
        if matches.is_empty() {
            self.search = Some(SearchState {
                query: query.to_string(),
                matches,
                current: None,
            });
            return 0;
        }

        let index = matches
            .iter()
            .position(|item| item.line >= self.cursor)
            .unwrap_or(0);
        self.cursor = matches[index].line;
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: Some(index),
        });
        self.ensure_visible();
        self.search
            .as_ref()
            .map(|search| search.matches.len())
            .unwrap_or(0)
    }

    pub fn search_next(&mut self) -> bool {
        let Some(search) = &mut self.search else {
            return false;
        };
        if search.matches.is_empty() {
            return false;
        }
        let next = search
            .current
            .map(|index| (index + 1) % search.matches.len())
            .unwrap_or(0);
        search.current = Some(next);
        let line = search.matches[next].line;
        self.cursor = line;
        self.ensure_visible();
        true
    }

    pub fn search_previous(&mut self) -> bool {
        let Some(search) = &mut self.search else {
            return false;
        };
        if search.matches.is_empty() {
            return false;
        }
        let previous = search
            .current
            .map(|index| {
                if index == 0 {
                    search.matches.len() - 1
                } else {
                    index - 1
                }
            })
            .unwrap_or(0);
        search.current = Some(previous);
        let line = search.matches[previous].line;
        self.cursor = line;
        self.ensure_visible();
        true
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    /// Character ranges to highlight on a line: `(start, end, is_current)`.
    pub fn matches_on_line(&self, line: usize) -> Vec<(usize, usize, bool)> {
        let Some(search) = &self.search else {
            return Vec::new();
        };
        let current = search.current;
        search
            .matches
            .iter()
            .enumerate()
            .filter(|(_, item)| item.line == line)
            .map(|(index, item)| (item.start, item.end, Some(index) == current))
            .collect()
    }
}

// ------------------------------------------------------------- source lines

impl Viewer {
    /// Source line (0-based) shown at rendered row `row` of the open file.  For
    /// a Markdown block that cannot be followed row by row this is the line
    /// the block starts on.  `None` for messages and the welcome screen.
    pub fn source_line_at(&self, row: usize) -> Option<usize> {
        match &self.loaded.as_ref()?.content {
            ParsedContent::Code { .. } => syntax::source_line_of_row(&self.doc, row),
            ParsedContent::Markdown(_) | ParsedContent::Plain(_) => self.doc.source_line_at(row),
            ParsedContent::Message { .. } => None,
        }
    }

    /// First rendered row that shows source line `line` (0-based).  A long
    /// source file may need [`Viewer::load_all`] before its far end is found.
    pub fn row_of_source_line(&self, line: usize) -> Option<usize> {
        match &self.loaded.as_ref()?.content {
            ParsedContent::Code { .. } => syntax::row_of_source_line(&self.doc, line),
            ParsedContent::Markdown(_) | ParsedContent::Plain(_) => {
                self.doc.row_of_source_line(line)
            }
            ParsedContent::Message { .. } => None,
        }
    }

    /// Row of the open Markdown file where `label` is drawn inside the block
    /// that holds source line `line`.
    pub fn row_of_text_near_line(&self, line: usize, label: &str) -> Option<usize> {
        match &self.loaded.as_ref()?.content {
            ParsedContent::Markdown(_) => self.doc.row_of_text_in_block(line, label),
            _ => None,
        }
    }

    /// Text of source line `line` (0-based) of the open file.
    pub fn source_line_text(&self, line: usize) -> Option<String> {
        match &self.loaded.as_ref()?.content {
            ParsedContent::Markdown(markdown) => markdown.source.lines().nth(line).map(str::to_string),
            ParsedContent::Plain(lines) | ParsedContent::Code { lines, .. } => lines.get(line).cloned(),
            ParsedContent::Message { .. } => None,
        }
    }

    /// Source lines behind the rendered rows `first..=last` of the open file.
    /// For a source-code view each row maps to the line numbered in its gutter;
    /// a row that only continues a wrapped line makes the range inexact.
    pub fn source_range_of_rows(&self, first: usize, last: usize) -> Option<SourceRange> {
        let (first, last) = if first <= last { (first, last) } else { (last, first) };
        match &self.loaded.as_ref()?.content {
            ParsedContent::Markdown(_) | ParsedContent::Plain(_) => self.doc.source_range(first, last),
            ParsedContent::Code { .. } => {
                let start = syntax::source_line_of_row(&self.doc, first)?;
                let end = syntax::source_line_of_row(&self.doc, last)?.max(start);
                let starts_a_line = |row: usize, line: usize| syntax::row_of_source_line(&self.doc, line) == Some(row);
                Some(SourceRange {
                    start,
                    end,
                    exact: starts_a_line(first, start) && starts_a_line(last, end),
                })
            }
            ParsedContent::Message { .. } => None,
        }
    }

    /// Is the open file a Markdown document?
    pub fn markdown_file(&self) -> bool {
        matches!(self.loaded.as_ref().map(|loaded| &loaded.content), Some(ParsedContent::Markdown(_)))
    }

    /// Source lines `start..=end` (0-based) of the open file, as they are in
    /// the file, joined with newlines.
    pub fn source_lines(&self, start: usize, end: usize) -> Option<String> {
        match &self.loaded.as_ref()?.content {
            ParsedContent::Markdown(markdown) => {
                let lines: Vec<&str> = markdown.source.lines().skip(start).take(end + 1 - start).collect();
                (!lines.is_empty()).then(|| lines.join("\n"))
            }
            ParsedContent::Plain(lines) | ParsedContent::Code { lines, .. } => {
                let slice = lines.get(start..(end + 1).min(lines.len()))?;
                (!slice.is_empty()).then(|| slice.join("\n"))
            }
            ParsedContent::Message { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::catppuccin_mocha;
    use std::io::Write;

    fn temp_markdown(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new()
            .prefix("runetui-test")
            .suffix(".md")
            .tempfile()
            .unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    }

    #[test]
    fn loads_markdown_and_detects_kind() {
        let file = temp_markdown("# Hello\n\nBody\n");
        let loaded = load_document(file.path()).unwrap();
        assert_eq!(loaded.kind, DocKind::Markdown);
        assert!(loaded.content_has_heading());
    }

    trait ContentHeading {
        fn content_has_heading(&self) -> bool;
    }
    impl ContentHeading for LoadedDoc {
        fn content_has_heading(&self) -> bool {
            matches!(&self.content, ParsedContent::Markdown(markdown) if !markdown.blocks.is_empty())
        }
    }

    #[test]
    fn detects_binary_files() {
        let mut file = tempfile::Builder::new().suffix(".bin").tempfile().unwrap();
        file.write_all(&[0, 1, 2, 3, 0, 255]).unwrap();
        let loaded = load_document(file.path()).unwrap();
        assert_eq!(loaded.kind, DocKind::Binary);
    }

    #[test]
    fn loads_plain_text_and_code() {
        let mut txt = tempfile::Builder::new().suffix(".txt").tempfile().unwrap();
        txt.write_all(b"line one\nline two\n").unwrap();
        let loaded = load_document(txt.path()).unwrap();
        assert_eq!(loaded.kind, DocKind::Text);
        assert!(matches!(&loaded.content, ParsedContent::Plain(lines) if lines.len() == 2));

        let mut rs = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
        rs.write_all(b"fn main() {}\n").unwrap();
        let loaded = load_document(rs.path()).unwrap();
        assert_eq!(loaded.kind, DocKind::Code("rust".into()));
    }

    fn shown_viewer(source: &str, width: usize) -> Viewer {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let file = temp_markdown(source);
        let loaded = load_document(file.path()).unwrap();
        let mut viewer = Viewer::default();
        viewer.set_viewport_height(10);
        viewer.show(Arc::new(loaded), width, theme, &mut highlighter, None);
        viewer
    }

    #[test]
    fn search_finds_matches_and_navigates() {
        let mut viewer = shown_viewer(
            "# One\n\nalpha beta\n\n## Two\n\nalpha again\n\n## Three\n\nfinal alpha\n",
            60,
        );
        let count = viewer.apply_search("alpha");
        assert_eq!(count, 3);
        assert_eq!(
            viewer.cursor,
            viewer.search.as_ref().unwrap().matches[0].line
        );
        assert!(viewer.search_next());
        let second = viewer.search.as_ref().unwrap().current_match().unwrap();
        assert!(second.line > 0);
        assert!(viewer.search_previous());
        assert_eq!(
            viewer
                .search
                .as_ref()
                .unwrap()
                .current_match()
                .unwrap()
                .line,
            viewer.search.as_ref().unwrap().matches[0].line
        );
        let ranges = viewer.matches_on_line(second.line);
        assert!(!ranges.is_empty() || second.line != 0);
    }

    #[test]
    fn navigation_clamps_and_tracks_progress() {
        let mut viewer = shown_viewer("# A\n\n text\n".repeat(50).as_str(), 40);
        assert!(viewer.total_lines() > 5);
        viewer.go_bottom();
        assert_eq!(viewer.progress(), 100);
        viewer.move_cursor(100);
        assert_eq!(viewer.progress(), 100);
        viewer.go_top();
        assert_eq!(viewer.cursor, 0);
        viewer.move_half_page(true);
        assert!(viewer.cursor > 0);
    }

    #[test]
    fn anchor_jump_lands_on_heading() {
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let file = temp_markdown("# Intro\n\ntext\n\n## Details\n\nmore\n");
        let doc = render_loaded(
            &load_document(file.path()).unwrap(),
            60,
            theme,
            &mut highlighter,
        );
        let details_line = doc
            .headings
            .iter()
            .find(|heading| heading.text == "Details")
            .unwrap()
            .line;
        let mut viewer = Viewer::default();
        viewer.set_viewport_height(5);
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            60,
            theme,
            &mut highlighter,
            Some("details"),
        );
        assert_eq!(viewer.cursor, details_line);
    }

    #[test]
    fn oversized_files_are_refused() {
        let mut file = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        let data = vec![b'a'; (MAX_FILE_SIZE + 1) as usize];
        file.write_all(&data).unwrap();
        let loaded = load_document(file.path()).unwrap();
        assert_eq!(loaded.kind, DocKind::TooLarge);
    }

    fn long_rust_source(lines: usize) -> String {
        let mut source = String::new();
        for index in 0..lines {
            source.push_str(&format!("fn function_{index}() -> usize {{ {index} }}\n"));
        }
        source
    }

    fn temp_rust(lines: usize) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
        file.write_all(long_rust_source(lines).as_bytes()).unwrap();
        file
    }

    fn rows_with_colours(doc: &RenderedDoc) -> Vec<(String, Vec<Option<ratatui::style::Color>>)> {
        doc.lines
            .iter()
            .map(|line| (line.text(), line.spans.iter().map(|span| span.style.fg).collect()))
            .collect()
    }

    #[test]
    fn long_source_files_are_only_highlighted_up_to_the_first_chunk() {
        let file = temp_rust(5000);
        let mut highlighter = CodeHighlighter::new();
        let mut viewer = Viewer::default();
        viewer.set_viewport_height(30);
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            80,
            catppuccin_mocha(),
            &mut highlighter,
            None,
        );
        assert_eq!(viewer.highlighted_lines(), Some((INITIAL_LINES, 5000)));
        assert!(viewer.is_partial());
        assert!(viewer.doc.lines.len() < 400, "only the first chunk is rendered");
        // The gutter is sized for the whole file from the start.
        assert!(viewer.doc.lines[0].text().starts_with("   1 │"));
        // The scroll position still reflects the size of the file.
        assert!(viewer.total_lines() > 4000);
        // Not near the end yet: nothing more is loaded.
        assert!(!viewer.needs_more());
    }

    #[test]
    fn short_source_files_are_rendered_in_one_go() {
        let file = temp_rust(LAZY_MIN_LINES - 1);
        let mut highlighter = CodeHighlighter::new();
        let mut viewer = Viewer::default();
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            80,
            catppuccin_mocha(),
            &mut highlighter,
            None,
        );
        assert!(!viewer.is_partial());
        assert_eq!(viewer.highlighted_lines(), None);
    }

    #[test]
    fn scrolling_towards_the_end_loads_more_chunks() {
        let file = temp_rust(5000);
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let mut viewer = Viewer::default();
        viewer.set_viewport_height(30);
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            80,
            theme,
            &mut highlighter,
            None,
        );
        // 200 rows are ready; the viewport (30) plus the look-ahead (60) must fit.
        viewer.move_cursor(100);
        assert!(!viewer.needs_more());
        viewer.move_cursor(20);
        assert!(viewer.needs_more());
        assert!(viewer.load_more(theme, &mut highlighter));
        assert_eq!(
            viewer.highlighted_lines(),
            Some((INITIAL_LINES + CHUNK_LINES, 5000))
        );
        assert!(!viewer.needs_more());
    }

    #[test]
    fn chunked_highlighting_matches_highlighting_the_whole_file() {
        let file = temp_rust(900);
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let loaded = Arc::new(load_document(file.path()).unwrap());
        let mut viewer = Viewer::default();
        viewer.show(loaded.clone(), 80, theme, &mut highlighter, None);
        viewer.load_all(theme, &mut highlighter);
        assert!(!viewer.is_partial());

        let ParsedContent::Code { lang, lines } = &loaded.content else {
            panic!("expected code");
        };
        let eager = syntax::render_code(lines, lang, 80, theme, &mut highlighter);
        assert_eq!(rows_with_colours(&viewer.doc), rows_with_colours(&eager));
    }

    #[test]
    fn load_through_reaches_a_far_row() {
        let file = temp_rust(5000);
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let mut viewer = Viewer::default();
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            80,
            theme,
            &mut highlighter,
            None,
        );
        viewer.load_through(1500, theme, &mut highlighter);
        assert!(viewer.doc.lines.len() > 1500);
        assert!(viewer.is_partial(), "the rest still waits");
    }

    #[test]
    fn resizing_keeps_the_progress_and_changing_theme_recolours_it() {
        let file = temp_rust(5000);
        let theme = catppuccin_mocha();
        let mut highlighter = CodeHighlighter::new();
        let mut viewer = Viewer::default();
        viewer.set_viewport_height(30);
        viewer.show(
            Arc::new(load_document(file.path()).unwrap()),
            80,
            theme,
            &mut highlighter,
            None,
        );
        viewer.load_more(theme, &mut highlighter);
        let reached = viewer.highlighted_lines();
        viewer.rerender(60, theme, &mut highlighter);
        assert_eq!(viewer.highlighted_lines(), reached);
        assert_eq!(viewer.rendered_width, 60);

        let other = crate::ui::themes::ThemeName::TokyoNight.theme();
        viewer.rerender(60, other, &mut highlighter);
        assert_eq!(viewer.highlighted_lines(), reached);
        assert_eq!(viewer.rendered_theme, Some(other.name));
    }

    /// Manual benchmark: `cargo test --release bench_ -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_first_chunk_versus_whole_file() {
        let source = include_str!("../app.rs");
        let mut file = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
        file.write_all(source.as_bytes()).unwrap();
        let theme = catppuccin_mocha();
        let loaded = Arc::new(load_document(file.path()).unwrap());
        let mut highlighter = CodeHighlighter::new();
        // Warm the syntax set and the theme.
        let mut warm = Viewer::default();
        warm.show(loaded.clone(), 100, theme, &mut highlighter, None);

        let started = std::time::Instant::now();
        let mut viewer = Viewer::default();
        viewer.show(loaded.clone(), 100, theme, &mut highlighter, None);
        let first = started.elapsed();

        let started = std::time::Instant::now();
        viewer.load_all(theme, &mut highlighter);
        let rest = started.elapsed();

        let ParsedContent::Code { lang, lines } = &loaded.content else {
            panic!("code expected");
        };
        let started = std::time::Instant::now();
        let _ = syntax::render_code(lines, lang, 100, theme, &mut highlighter);
        let whole = started.elapsed();
        println!(
            "{} lines: first chunk {first:?}, remaining chunks {rest:?}, whole file at once {whole:?}",
            lines.len()
        );
    }

    // ------------------------------------------------------------ type badge

    fn badge_of(name: &str, contents: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        load_document(&path).unwrap().badge()
    }

    #[test]
    fn plain_text_says_which_file_it_is_instead_of_looking_like_an_extension() {
        assert_eq!(badge_of(".gitignore", "target/\n"), "text · .gitignore");
        assert_eq!(badge_of(".env", "A=1\n"), "text · .env");
        assert_eq!(badge_of("LICENSE", "MIT\n"), "text · LICENSE");
        assert_eq!(badge_of("Dockerfile", "FROM x\n"), "text · Dockerfile");
        assert_eq!(badge_of("notes.txt", "hi\n"), "text");
        assert_eq!(badge_of("readme.text", "hi\n"), "text");
        assert_eq!(badge_of("manual.rst", "hi\n"), "text · .rst");
        assert_eq!(badge_of("server.LOG", "hi\n"), "text · .log");
    }

    #[test]
    fn long_names_are_cut_in_the_badge() {
        let name = format!("{}", "a".repeat(40));
        let badge = badge_of(&name, "hi\n");
        assert!(badge.starts_with("text · "));
        assert!(badge.ends_with('…'));
        assert!(badge.chars().count() < 40);
    }

    #[test]
    fn well_known_names_with_a_syntect_syntax_are_code_not_plain_text() {
        // syntect ships a Makefile syntax…
        assert_eq!(badge_of("Makefile", "all:\n\techo hi\n"), "makefile");
        // …but none for these, which therefore stay plain text.
        for name in [".gitignore", "Dockerfile", "LICENSE", ".editorconfig"] {
            assert!(
                crate::markdown::code_highlighter::syntax_token_for_file_name(name).is_none(),
                "{name} unexpectedly has a syntax"
            );
        }
        assert!(crate::markdown::code_highlighter::syntax_token_for_file_name("Makefile").is_some());
        assert!(crate::markdown::code_highlighter::syntax_token_for_file_name("").is_none());
    }

    #[test]
    fn dotfiles_are_never_looked_up_as_syntaxes() {
        // `.sh` as a file name must not be mistaken for the `sh` extension.
        assert_eq!(badge_of(".sh", "echo hi\n"), "text · .sh");
    }

    #[test]
    fn the_empty_viewer_is_labelled_welcome_not_home() {
        let viewer = Viewer::default();
        assert_eq!(viewer.kind_label, "welcome");
    }
}

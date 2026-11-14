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

use crate::markdown::code_highlighter::{CodeHighlighter, canonical_label, sniff_language};
use crate::markdown::parser::Block;
use crate::markdown::{Heading, RenderedDoc, parse, renderer, toc};
use crate::ui::themes::{Theme, ThemeName};

/// Preview limit.  Larger files are reported as unsupported instead of
/// freezing the interface.
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

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
    Markdown(Vec<Block>),
    Plain(Vec<String>),
    Code { lang: String, lines: Vec<String> },
    Message { title: String, detail: String },
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
        self.kind.badge().to_string()
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
            content: ParsedContent::Markdown(parse(&source)),
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
pub fn render_loaded(
    loaded: &LoadedDoc,
    width: usize,
    theme: Theme,
    highlighter: &mut CodeHighlighter,
) -> RenderedDoc {
    match &loaded.content {
        ParsedContent::Markdown(blocks) => renderer::render(blocks, width, theme, highlighter),
        ParsedContent::Plain(lines) => text::render_plain(lines, width, theme),
        ParsedContent::Code { lang, lines, .. } => {
            syntax::render_code(lines, lang, width, theme, highlighter)
        }
        ParsedContent::Message { title, detail } => {
            text::render_message(title, detail, width, theme)
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
            kind_label: "home".to_string(),
            welcome_source: None,
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
        let doc = render_loaded(&loaded, width, theme, highlighter);
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
        let blocks = parse(&source);
        self.loaded = None;
        self.doc = renderer::render(&blocks, width, theme, highlighter);
        self.title = "Welcome".to_string();
        self.kind_label = "home".to_string();
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

        if let Some(loaded) = &self.loaded {
            self.doc = render_loaded(loaded, width, theme, highlighter);
        } else if let Some(source) = &self.welcome_source {
            let blocks = parse(source);
            self.doc = renderer::render(&blocks, width, theme, highlighter);
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
        let total = self.doc.lines.len();
        if total <= 1 {
            return 100;
        }
        (self.cursor * 100 / (total - 1)).min(100)
    }

    pub fn total_lines(&self) -> usize {
        self.doc.lines.len()
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
            matches!(&self.content, ParsedContent::Markdown(blocks) if !blocks.is_empty())
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
}

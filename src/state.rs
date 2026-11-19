//! Per-project state kept between sessions: bookmarks and recently opened
//! files, stored in `state.toml` next to `config.toml`.
//!
//! The file holds one table per project root (`[projects."/path/to/root"]`), so
//! several projects (and several running instances) share it.  Paths inside a
//! project are stored relative to its root, with `/` separators.  Reading is
//! forgiving on purpose: a corrupt file, a malformed entry or a path that no
//! longer exists is dropped silently, never an error.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::CONFIG_DIR;

pub const STATE_FILE: &str = "state.toml";
/// Recently opened files kept per project.
pub const MAX_RECENT: usize = 20;
/// Bookmarks kept per project.
pub const MAX_BOOKMARKS: usize = 500;
/// Longest line excerpt stored with a bookmark, in characters.
const EXCERPT_CHARS: usize = 100;

/// Where the state is kept: next to the configuration file.  Tests never touch
/// the real file (they point the app at a temporary one when they need it).
pub fn default_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    dirs::config_dir().map(|directory| directory.join(CONFIG_DIR).join(STATE_FILE))
}

/// A marked line of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// Path relative to the project root, with `/` separators.
    pub file: String,
    /// 1-based source line.
    pub line: usize,
    /// The text of that line, to recognise the bookmark in the list.
    pub text: String,
}

/// What [`ProjectState::toggle_bookmark`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Added,
    Removed,
    /// There are already [`MAX_BOOKMARKS`] bookmarks.
    Full,
}

/// Bookmarks and recent files of one project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectState {
    /// Sorted by file, then line.
    pub bookmarks: Vec<Bookmark>,
    /// Most recent first, at most [`MAX_RECENT`].
    pub recent: Vec<String>,
}

impl ProjectState {
    pub fn is_bookmarked(&self, file: &str, line: usize) -> bool {
        self.bookmarks
            .iter()
            .any(|bookmark| bookmark.file == file && bookmark.line == line)
    }

    /// Mark the line, or unmark it when it is already marked.
    pub fn toggle_bookmark(&mut self, file: &str, line: usize, text: &str) -> Toggle {
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.file == file && bookmark.line == line)
        {
            self.bookmarks.remove(index);
            return Toggle::Removed;
        }
        if self.bookmarks.len() >= MAX_BOOKMARKS {
            return Toggle::Full;
        }
        self.bookmarks.push(Bookmark {
            file: file.to_string(),
            line,
            text: excerpt(text),
        });
        self.bookmarks
            .sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
        Toggle::Added
    }

    pub fn remove_bookmark(&mut self, index: usize) -> Option<Bookmark> {
        (index < self.bookmarks.len()).then(|| self.bookmarks.remove(index))
    }

    /// Put `file` at the front of the recent list.
    pub fn note_recent(&mut self, file: &str) {
        self.recent.retain(|entry| entry != file);
        self.recent.insert(0, file.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, index: usize) -> Option<String> {
        (index < self.recent.len()).then(|| self.recent.remove(index))
    }

    /// Forget every file that is gone: a moved or deleted file leaves no stale
    /// entry behind.
    pub fn retain_existing(&mut self, root: &Path) {
        let exists = |file: &str| root.join(file).is_file();
        self.bookmarks.retain(|bookmark| exists(&bookmark.file));
        self.recent.retain(|file| exists(file));
    }
}

/// One-line excerpt of a source line: whitespace collapsed, cut to
/// [`EXCERPT_CHARS`] characters.
pub fn excerpt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > EXCERPT_CHARS {
        let mut cut: String = collapsed.chars().take(EXCERPT_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

/// A usable relative path: inside the project, never absolute, no `..`.
fn clean_relative(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let path = path.trim();
    let escapes = path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|part| part == "..")
        || path.chars().nth(1) == Some(':');
    (!escapes).then(|| path.to_string())
}

fn project_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn read_table(path: &Path) -> Option<toml::Table> {
    let text = fs::read_to_string(path).ok()?;
    text.parse::<toml::Table>().ok()
}

/// Read the state of the project at `root`.  Never fails: whatever cannot be
/// read is simply not there.
pub fn load(path: &Path, root: &Path) -> ProjectState {
    let mut state = ProjectState::default();
    let Some(table) = read_table(path) else {
        return state;
    };
    let Some(project) = table
        .get("projects")
        .and_then(toml::Value::as_table)
        .and_then(|projects| projects.get(&project_key(root)))
        .and_then(toml::Value::as_table)
    else {
        return state;
    };

    if let Some(recent) = project.get("recent").and_then(toml::Value::as_array) {
        for value in recent {
            let Some(file) = value.as_str().and_then(clean_relative) else {
                continue;
            };
            if !state.recent.contains(&file) && state.recent.len() < MAX_RECENT {
                state.recent.push(file);
            }
        }
    }
    if let Some(bookmarks) = project.get("bookmarks").and_then(toml::Value::as_array) {
        for value in bookmarks {
            let Some(entry) = value.as_table() else {
                continue;
            };
            let file = entry.get("file").and_then(toml::Value::as_str).and_then(clean_relative);
            let line = entry
                .get("line")
                .and_then(toml::Value::as_integer)
                .and_then(|line| usize::try_from(line).ok())
                .filter(|line| *line >= 1);
            let (Some(file), Some(line)) = (file, line) else {
                continue;
            };
            let text = entry.get("text").and_then(toml::Value::as_str).unwrap_or("");
            if state.bookmarks.len() < MAX_BOOKMARKS && !state.is_bookmarked(&file, line) {
                state.toggle_bookmark(&file, line, text);
            }
        }
    }
    state.retain_existing(root);
    state
}

/// Write the state of the project at `root`, keeping what the file holds for
/// other projects.  A file that cannot be parsed is moved aside (to
/// `state.toml.corrupt`) rather than silently destroyed.
pub fn save(path: &Path, root: &Path, state: &ProjectState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut table = match fs::read_to_string(path) {
        Ok(text) => match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(_) => {
                let _ = fs::rename(path, path.with_extension("toml.corrupt"));
                toml::Table::new()
            }
        },
        Err(_) => toml::Table::new(),
    };

    let mut project = toml::Table::new();
    project.insert(
        "recent".to_string(),
        toml::Value::Array(
            state
                .recent
                .iter()
                .map(|file| toml::Value::String(file.clone()))
                .collect(),
        ),
    );
    project.insert(
        "bookmarks".to_string(),
        toml::Value::Array(
            state
                .bookmarks
                .iter()
                .map(|bookmark| {
                    let mut entry = toml::Table::new();
                    entry.insert("file".to_string(), toml::Value::String(bookmark.file.clone()));
                    entry.insert("line".to_string(), toml::Value::Integer(bookmark.line as i64));
                    entry.insert("text".to_string(), toml::Value::String(bookmark.text.clone()));
                    toml::Value::Table(entry)
                })
                .collect(),
        ),
    );

    let mut projects = match table.remove("projects") {
        Some(toml::Value::Table(projects)) => projects,
        _ => toml::Table::new(),
    };
    if state.recent.is_empty() && state.bookmarks.is_empty() {
        projects.remove(&project_key(root));
    } else {
        projects.insert(project_key(root), toml::Value::Table(project));
    }
    table.insert("projects".to_string(), toml::Value::Table(projects));

    let text = toml::to_string_pretty(&table).map_err(|error| io::Error::other(error.to_string()))?;
    // Write a sibling file and rename it over the real one, so a crash never
    // leaves half a file behind.
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("notes")).unwrap();
        for file in ["a.md", "b.md", "notes/c.md"] {
            fs::write(dir.path().join(file), "x\n").unwrap();
        }
        dir
    }

    #[test]
    fn toggling_adds_sorts_and_removes_bookmarks() {
        let mut state = ProjectState::default();
        assert_eq!(state.toggle_bookmark("b.md", 3, "  some   text "), Toggle::Added);
        assert_eq!(state.toggle_bookmark("a.md", 9, "x"), Toggle::Added);
        assert_eq!(state.toggle_bookmark("a.md", 2, "y"), Toggle::Added);
        let order: Vec<_> = state.bookmarks.iter().map(|b| (b.file.as_str(), b.line)).collect();
        assert_eq!(order, vec![("a.md", 2), ("a.md", 9), ("b.md", 3)]);
        assert_eq!(state.bookmarks[2].text, "some text", "whitespace is collapsed");
        assert!(state.is_bookmarked("a.md", 9));
        assert_eq!(state.toggle_bookmark("a.md", 9, "x"), Toggle::Removed);
        assert!(!state.is_bookmarked("a.md", 9));
        assert_eq!(state.bookmarks.len(), 2);
    }

    #[test]
    fn the_number_of_bookmarks_is_bounded() {
        let mut state = ProjectState::default();
        for line in 1..=MAX_BOOKMARKS {
            assert_eq!(state.toggle_bookmark("a.md", line, ""), Toggle::Added);
        }
        assert_eq!(state.toggle_bookmark("a.md", MAX_BOOKMARKS + 1, ""), Toggle::Full);
        assert_eq!(state.toggle_bookmark("a.md", 1, ""), Toggle::Removed, "unmarking still works");
    }

    #[test]
    fn recent_files_are_most_recent_first_without_duplicates_and_capped() {
        let mut state = ProjectState::default();
        for index in 0..30 {
            state.note_recent(&format!("f{index}.md"));
        }
        assert_eq!(state.recent.len(), MAX_RECENT);
        assert_eq!(state.recent[0], "f29.md");
        assert_eq!(state.recent[MAX_RECENT - 1], "f10.md");
        state.note_recent("f20.md");
        assert_eq!(state.recent[0], "f20.md");
        assert_eq!(state.recent.iter().filter(|file| *file == "f20.md").count(), 1);
        assert_eq!(state.remove_recent(0).as_deref(), Some("f20.md"));
        assert!(state.remove_recent(999).is_none());
    }

    #[test]
    fn excerpts_are_one_short_line() {
        assert_eq!(excerpt("  a \t b\n c "), "a b c");
        let long = "é".repeat(300);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(excerpt(""), "");
    }

    #[test]
    fn state_round_trips_and_is_relative_to_the_root() {
        let dir = project();
        let file = dir.path().join("cfg/state.toml");
        let mut state = ProjectState::default();
        state.toggle_bookmark("notes/c.md", 7, "seven");
        state.toggle_bookmark("a.md", 1, "");
        state.note_recent("b.md");
        state.note_recent("notes/c.md");
        save(&file, dir.path(), &state).unwrap();

        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("[projects."), "{text}");
        assert!(text.contains("notes/c.md"), "{text}");
        let loaded = load(&file, dir.path());
        assert_eq!(loaded, state);
    }

    #[test]
    fn projects_do_not_see_each_other_and_saving_keeps_the_others() {
        let one = project();
        let two = project();
        let file = one.path().join("state.toml");
        let mut first = ProjectState::default();
        first.note_recent("a.md");
        let mut second = ProjectState::default();
        second.note_recent("b.md");
        second.toggle_bookmark("b.md", 2, "two");
        save(&file, one.path(), &first).unwrap();
        save(&file, two.path(), &second).unwrap();
        assert_eq!(load(&file, one.path()), first);
        assert_eq!(load(&file, two.path()), second);
        // Emptying a project removes its table and leaves the other alone.
        save(&file, one.path(), &ProjectState::default()).unwrap();
        assert_eq!(load(&file, one.path()), ProjectState::default());
        assert_eq!(load(&file, two.path()), second);
    }

    #[test]
    fn a_missing_file_gives_an_empty_state() {
        let dir = project();
        assert_eq!(load(&dir.path().join("nope.toml"), dir.path()), ProjectState::default());
    }

    #[test]
    fn a_corrupt_file_is_ignored_and_set_aside_on_the_next_save() {
        let dir = project();
        let file = dir.path().join("state.toml");
        fs::write(&file, "this is [not toml {{{").unwrap();
        assert_eq!(load(&file, dir.path()), ProjectState::default());

        let mut state = ProjectState::default();
        state.note_recent("a.md");
        save(&file, dir.path(), &state).unwrap();
        assert_eq!(load(&file, dir.path()), state);
        assert_eq!(
            fs::read_to_string(dir.path().join("state.toml.corrupt")).unwrap(),
            "this is [not toml {{{",
            "the broken file is kept, not destroyed"
        );
    }

    #[test]
    fn malformed_entries_and_vanished_files_are_dropped_without_error() {
        let dir = project();
        let file = dir.path().join("state.toml");
        let key = project_key(dir.path());
        let text = format!(
            r#"
[projects."{key}"]
recent = ["a.md", 5, "gone.md", "../outside.md", "/etc/passwd", "a.md", "C:/x.md", "notes\\c.md"]

[[projects."{key}".bookmarks]]
file = "b.md"
line = 4
text = "kept"

[[projects."{key}".bookmarks]]
file = "gone.md"
line = 1

[[projects."{key}".bookmarks]]
file = "a.md"
line = 0

[[projects."{key}".bookmarks]]
file = "a.md"
line = "three"

[[projects."{key}".bookmarks]]
line = 2

[[projects."{key}".bookmarks]]
file = "../escape.md"
line = 2

[[projects."{key}".bookmarks]]
file = "b.md"
line = 4
text = "duplicate"
"#
        );
        fs::write(&file, text).unwrap();
        let state = load(&file, dir.path());
        assert_eq!(state.recent, vec!["a.md", "notes/c.md"]);
        assert_eq!(
            state.bookmarks,
            vec![Bookmark { file: "b.md".into(), line: 4, text: "kept".into() }]
        );
    }

    #[test]
    fn a_project_entry_of_the_wrong_type_is_ignored() {
        let dir = project();
        let file = dir.path().join("state.toml");
        let key = project_key(dir.path());
        fs::write(&file, format!("[projects]\n\"{key}\" = 5\n")).unwrap();
        assert_eq!(load(&file, dir.path()), ProjectState::default());
        fs::write(&file, "projects = 3\n").unwrap();
        assert_eq!(load(&file, dir.path()), ProjectState::default());
        // And saving over such a file works.
        let mut state = ProjectState::default();
        state.note_recent("a.md");
        save(&file, dir.path(), &state).unwrap();
        assert_eq!(load(&file, dir.path()), state);
    }

    #[test]
    fn paths_are_validated() {
        assert_eq!(clean_relative("a/b.md").as_deref(), Some("a/b.md"));
        assert_eq!(clean_relative("a\\b.md").as_deref(), Some("a/b.md"));
        assert_eq!(clean_relative(""), None);
        assert_eq!(clean_relative("/abs"), None);
        assert_eq!(clean_relative("a/../../b"), None);
        assert_eq!(clean_relative("D:/x"), None);
    }

    #[test]
    fn retain_existing_forgets_deleted_files() {
        let dir = project();
        let mut state = ProjectState::default();
        state.note_recent("a.md");
        state.note_recent("deleted.md");
        state.toggle_bookmark("deleted.md", 1, "");
        state.toggle_bookmark("b.md", 1, "");
        state.retain_existing(dir.path());
        assert_eq!(state.recent, vec!["a.md"]);
        assert_eq!(state.bookmarks.len(), 1);
    }
}

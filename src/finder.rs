//! Fuzzy file finder (`Ctrl+P`): the matcher and the background project index.
//!
//! * [`fuzzy_match`] / [`Matcher`] score a query against a path as a
//!   *subsequence* match.  The score favours consecutive characters, characters
//!   that start a word (after `/`, `_`, `-`, `.`, a space or a camelCase hump)
//!   and matches inside the file name over matches in the folders.
//! * [`Finder`] owns the list of project files.  It is filled by a background
//!   thread (so a huge project never blocks the interface), follows the same
//!   hidden-file and `.gitignore` settings as the explorer, and keeps the
//!   ranked results of the current query up to date as batches arrive.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;

/// Files kept in the index.  A bigger project is cut here (the finder says so).
pub const MAX_INDEXED_FILES: usize = 200_000;
/// Ranked results kept for display (the list scrolls through them).
pub const MAX_RESULTS: usize = 200;
/// Paths sent from the indexing thread to the interface at a time.
const BATCH: usize = 512;

// ------------------------------------------------------------------ matching

const SCORE_MATCH: i32 = 16;
/// First character of the path or of a folder / file name.
const BONUS_START: i32 = 10;
/// Character after `_`, `-`, `.` or a space.
const BONUS_SEPARATOR: i32 = 8;
/// Upper-case letter after a lower-case one (`fooBar`).
const BONUS_CAMEL: i32 = 7;
/// Character after other punctuation.
const BONUS_OTHER: i32 = 6;
/// A matched character right after the previous matched one.
const BONUS_CONSECUTIVE: i32 = 9;
/// Every matched character that lies in the file name.
const BONUS_NAME_CHAR: i32 = 3;
/// The whole query matched inside the file name.
const BONUS_ALL_IN_NAME: i32 = 30;
/// Cost of every character skipped between two matches.
const GAP: i32 = 2;
const NEG: i32 = -1_000_000_000;

fn valid(score: i32) -> bool {
    score > NEG / 2
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// Character (not byte) indexes of the candidate that matched the query.
    pub positions: Vec<usize>,
}

/// Turn what the user typed into the characters that are matched: spaces are
/// ignored, `\` is a `/` and case does not matter.
pub fn normalize_query(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '\\' { '/' } else { fold(character) })
        .collect()
}

fn fold(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// Reusable buffers for scoring many candidates without allocating each time.
#[derive(Default)]
pub struct Matcher {
    chars: Vec<char>,
    folded: Vec<char>,
    bonus: Vec<i32>,
    /// `ends[i * n + j]`: best score with query char `i` matched at `j`.
    ends: Vec<i32>,
    /// `upto[i * n + j]`: best score with query chars `0..=i` matched at or
    /// before `j`, minus the gap cost since the last match.
    upto: Vec<i32>,
}

impl Matcher {
    /// Score `candidate` for an already normalised `query` (see
    /// [`normalize_query`]).  `None` when the query is not a subsequence.
    pub fn score(&mut self, query: &[char], candidate: &str) -> Option<FuzzyMatch> {
        if query.is_empty() {
            return Some(FuzzyMatch {
                score: 0,
                positions: Vec::new(),
            });
        }
        self.chars.clear();
        self.chars.extend(candidate.chars());
        let n = self.chars.len();
        let m = query.len();
        if m > n {
            return None;
        }
        self.folded.clear();
        self.folded.extend(self.chars.iter().map(|c| fold(*c)));

        // Cheap rejection first: most candidates are not a subsequence.
        let mut next = 0;
        for character in &self.folded {
            if next < m && *character == query[next] {
                next += 1;
            }
        }
        if next < m {
            return None;
        }

        let name_start = self
            .chars
            .iter()
            .rposition(|c| *c == '/')
            .map_or(0, |slash| slash + 1);

        self.bonus.clear();
        for j in 0..n {
            let current = self.chars[j];
            let bonus = if j == 0 {
                BONUS_START
            } else {
                let previous = self.chars[j - 1];
                if previous == '/' {
                    BONUS_START
                } else if matches!(previous, '_' | '-' | '.' | ' ') {
                    BONUS_SEPARATOR
                } else if previous.is_lowercase() && current.is_uppercase() {
                    BONUS_CAMEL
                } else if !previous.is_alphanumeric() {
                    BONUS_OTHER
                } else {
                    0
                }
            };
            self.bonus.push(bonus);
        }

        self.ends.clear();
        self.ends.resize(m * n, NEG);
        self.upto.clear();
        self.upto.resize(m * n, NEG);

        for i in 0..m {
            for j in i..n {
                let at = i * n + j;
                if self.folded[j] == query[i] {
                    let name_bonus = if j >= name_start { BONUS_NAME_CHAR } else { 0 };
                    let best = if i == 0 {
                        self.bonus[j]
                    } else if j > 0 {
                        let before = (i - 1) * n + j - 1;
                        let mut best = NEG;
                        if valid(self.ends[before]) {
                            best = self.ends[before] + self.bonus[j].max(BONUS_CONSECUTIVE);
                        }
                        if valid(self.upto[before]) {
                            best = best.max(self.upto[before] + self.bonus[j]);
                        }
                        best
                    } else {
                        NEG
                    };
                    if valid(best) {
                        self.ends[at] = best + SCORE_MATCH + name_bonus;
                    }
                }
                let carried = if j > 0 && valid(self.upto[at - 1]) {
                    self.upto[at - 1] - GAP
                } else {
                    NEG
                };
                self.upto[at] = self.ends[at].max(carried);
            }
        }

        // The best alignment ends wherever the last query char scores most.
        let last = (m - 1) * n;
        let mut best_end = None;
        let mut best_score = NEG;
        for j in 0..n {
            let score = self.ends[last + j];
            if valid(score) && score > best_score {
                best_score = score;
                best_end = Some(j);
            }
        }
        let mut j = best_end?;

        // Walk back to find which characters were matched.
        let mut positions = vec![0usize; m];
        positions[m - 1] = j;
        for i in (1..m).rev() {
            let name_bonus = if j >= name_start { BONUS_NAME_CHAR } else { 0 };
            let target = self.ends[i * n + j];
            let before = (i - 1) * n + j - 1;
            let consecutive = valid(self.ends[before])
                && self.ends[before] + self.bonus[j].max(BONUS_CONSECUTIVE) + SCORE_MATCH + name_bonus
                    == target;
            if consecutive {
                j -= 1;
            } else {
                let carried = self.upto[before];
                let mut found = None;
                for previous in (i - 1..j).rev() {
                    let score = self.ends[(i - 1) * n + previous];
                    if valid(score) && score - GAP * (j - 1 - previous) as i32 == carried {
                        found = Some(previous);
                        break;
                    }
                }
                j = found?;
            }
            positions[i - 1] = j;
        }

        let mut score = best_score;
        if positions.iter().all(|position| *position >= name_start) {
            score += BONUS_ALL_IN_NAME;
        }
        Some(FuzzyMatch { score, positions })
    }
}

/// Score `candidate` against a raw query string.
#[cfg(test)]
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    Matcher::default().score(&normalize_query(query), candidate)
}

// --------------------------------------------------------------------- index

/// One ranked result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Path relative to the project root, with `/` separators.
    pub path: String,
    pub score: i32,
    /// Character indexes of `path` to highlight.
    pub positions: Vec<usize>,
}

/// How the project is walked.  Mirrors the explorer's own settings so every
/// result can be revealed in the tree.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub show_hidden: bool,
    pub respect_gitignore: bool,
    pub max_files: usize,
    /// Pause before each batch (tests use it to observe the indexing state).
    pub batch_delay: Duration,
}

enum Message {
    Batch(Vec<String>),
    Done { truncated: bool },
}

/// Walk `root`, handing the relative paths of its files to `send` in batches.
/// Stops early when `cancel` is set or `send` returns false.
fn scan_project(
    root: &Path,
    options: &ScanOptions,
    cancel: &AtomicBool,
    mut send: impl FnMut(Message) -> bool,
) {
    let mut builder = WalkBuilder::new(root);
    if options.respect_gitignore {
        builder
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true);
    } else {
        builder.standard_filters(false);
    }
    builder
        .hidden(!options.show_hidden)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git");

    let pause = |cancel: &AtomicBool| {
        if !options.batch_delay.is_zero() {
            thread::sleep(options.batch_delay);
        }
        cancel.load(Ordering::Relaxed)
    };

    let mut batch: Vec<String> = Vec::with_capacity(BATCH);
    let mut total = 0usize;
    let mut truncated = false;
    for result in builder.build() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = result else {
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if total >= options.max_files {
            truncated = true;
            break;
        }
        batch.push(relative.replace('\\', "/"));
        total += 1;
        if batch.len() >= BATCH {
            if pause(cancel) || !send(Message::Batch(std::mem::take(&mut batch))) {
                return;
            }
        }
    }
    if pause(cancel) {
        return;
    }
    if !batch.is_empty() && !send(Message::Batch(batch)) {
        return;
    }
    send(Message::Done { truncated });
}

/// List every file of a project, sorted, the way the finder indexes it.  The
/// flag says the list was cut at `options.max_files`.  `None` when `cancel`
/// was set before the walk finished.
pub fn list_files(
    root: &Path,
    options: &ScanOptions,
    cancel: &AtomicBool,
) -> Option<(Vec<String>, bool)> {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut finished = false;
    scan_project(root, options, cancel, |message| {
        match message {
            Message::Batch(batch) => files.extend(batch),
            Message::Done { truncated: cut } => {
                truncated = cut;
                finished = true;
            }
        }
        true
    });
    if !finished {
        return None;
    }
    files.sort_by_cached_key(|path| path.to_lowercase());
    Some((files, truncated))
}

struct Scan {
    receiver: Receiver<Message>,
    cancel: Arc<AtomicBool>,
    since: Instant,
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The project's file list plus the ranked results of the current query.
pub struct Finder {
    /// The text in the input field.
    pub query: String,
    root: PathBuf,
    files: Vec<String>,
    /// Paths of a re-scan, swapped in when it finishes so the list the user is
    /// looking at does not flicker.
    incoming: Vec<String>,
    replacing: bool,
    scan: Option<Scan>,
    scanned: bool,
    stale: bool,
    truncated: bool,
    options: ScanOptions,

    matcher: Matcher,
    /// Every file that matched `scored_query`, best first: `(score, index)`.
    matches: Vec<(i32, usize)>,
    scored_query: Vec<char>,
    scored_files: usize,
    hits: Vec<Hit>,
}

impl Default for Finder {
    fn default() -> Self {
        Self::new()
    }
}

impl Finder {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            root: PathBuf::new(),
            files: Vec::new(),
            incoming: Vec::new(),
            replacing: false,
            scan: None,
            scanned: false,
            stale: false,
            truncated: false,
            options: ScanOptions {
                show_hidden: false,
                respect_gitignore: true,
                max_files: MAX_INDEXED_FILES,
                batch_delay: Duration::ZERO,
            },
            matcher: Matcher::default(),
            matches: Vec::new(),
            scored_query: Vec::new(),
            scored_files: 0,
            hits: Vec::new(),
        }
    }

    /// Test hook: pause before every batch, and index at most `max_files`.
    #[cfg(test)]
    pub fn set_scan_limits(&mut self, batch_delay: Duration, max_files: usize) {
        self.options.batch_delay = batch_delay;
        self.options.max_files = max_files;
    }

    /// Make sure the index describes `root` with these settings, starting a
    /// background scan when it does not (or when files changed on disk).
    pub fn ensure(&mut self, root: &Path, show_hidden: bool, respect_gitignore: bool) {
        let different = self.root != root
            || self.options.show_hidden != show_hidden
            || self.options.respect_gitignore != respect_gitignore;
        if different {
            self.options.show_hidden = show_hidden;
            self.options.respect_gitignore = respect_gitignore;
            if self.root != root {
                self.files.clear();
                self.matches.clear();
                self.scored_query.clear();
                self.scored_files = 0;
                self.hits.clear();
                self.scanned = false;
            }
            self.root = root.to_path_buf();
            self.start_scan();
        } else if self.scan.is_none() && (self.stale || !self.scanned) {
            self.start_scan();
        }
    }

    /// Files changed on disk: the next `ensure` indexes again.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    fn start_scan(&mut self) {
        // Dropping the old scan tells its thread to stop.
        self.scan = None;
        self.stale = false;
        self.incoming.clear();
        self.replacing = !self.files.is_empty();

        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let root = self.root.clone();
        let options = self.options.clone();
        let spawned = thread::Builder::new()
            .name("rtui-finder".to_string())
            .spawn(move || {
                scan_project(&root, &options, &thread_cancel, |message| {
                    sender.send(message).is_ok()
                });
            });
        if spawned.is_ok() {
            self.scan = Some(Scan {
                receiver,
                cancel,
                since: Instant::now(),
            });
        } else {
            // No thread, no index: the finder shows an empty list.
            self.scanned = true;
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    pub fn scan_started(&self) -> Option<Instant> {
        self.scan.as_ref().map(|scan| scan.since)
    }

    /// Take in what the indexing thread has produced.  True when the list of
    /// results may have changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut finished = None;
        if let Some(scan) = &self.scan {
            loop {
                match scan.receiver.try_recv() {
                    Ok(Message::Batch(batch)) => {
                        if self.replacing {
                            self.incoming.extend(batch);
                        } else {
                            self.files.extend(batch);
                            changed = true;
                        }
                    }
                    Ok(Message::Done { truncated }) => {
                        finished = Some(truncated);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = Some(self.truncated);
                        break;
                    }
                }
            }
        }
        if let Some(truncated) = finished {
            self.scan = None;
            self.scanned = true;
            self.truncated = truncated;
            if self.replacing {
                self.files = std::mem::take(&mut self.incoming);
                self.replacing = false;
            }
            self.files.sort_by_cached_key(|path| path.to_lowercase());
            // The order changed: indexes held by the old matches are void.
            self.matches.clear();
            self.scored_query.clear();
            self.scored_files = 0;
            changed = true;
        }
        if changed {
            self.refresh();
        }
        changed
    }

    // ------------------------------------------------------------- the query

    pub fn type_char(&mut self, character: char) {
        self.query.push(character);
        self.refresh();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refresh();
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.refresh();
    }

    /// Delete the last word of the query (`Ctrl+W`).
    pub fn delete_word(&mut self) {
        let trimmed = self.query.trim_end().len();
        self.query.truncate(trimmed);
        while let Some(last) = self.query.chars().last() {
            if last.is_alphanumeric() {
                self.query.pop();
            } else {
                break;
            }
        }
        self.refresh();
    }

    /// Rank the files for the current query.  Only the new work is done: a
    /// longer query rescored among the old matches, new files among new files.
    pub fn refresh(&mut self) {
        let query = normalize_query(&self.query);
        if query.is_empty() {
            self.matches.clear();
            self.scored_query.clear();
            self.scored_files = self.files.len();
            self.hits = self
                .files
                .iter()
                .take(MAX_RESULTS)
                .map(|path| Hit {
                    path: path.clone(),
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
            return;
        }

        let narrowing = !self.scored_query.is_empty()
            && query.starts_with(&self.scored_query)
            && self.scored_files <= self.files.len();
        let Self {
            matcher,
            files,
            matches,
            scored_query,
            scored_files,
            ..
        } = self;

        let first_new = if narrowing { *scored_files } else { 0 };
        let mut next: Vec<(i32, usize)> = Vec::new();
        if narrowing {
            if *scored_query == query {
                next = std::mem::take(matches);
            } else {
                for (_, index) in std::mem::take(matches) {
                    if let Some(found) = matcher.score(&query, &files[index]) {
                        next.push((found.score, index));
                    }
                }
            }
        }
        for index in first_new..files.len() {
            if let Some(found) = matcher.score(&query, &files[index]) {
                next.push((found.score, index));
            }
        }
        next.sort_by_key(|(score, index)| (Reverse(*score), files[*index].len(), *index));

        let hits = next
            .iter()
            .take(MAX_RESULTS)
            .filter_map(|(_, index)| {
                let found = matcher.score(&query, &files[*index])?;
                Some(Hit {
                    path: files[*index].clone(),
                    score: found.score,
                    positions: found.positions,
                })
            })
            .collect();

        *matches = next;
        *scored_query = query;
        *scored_files = files.len();
        self.hits = hits;
    }

    // ------------------------------------------------------------- read-only

    /// Ranked results, best first (at most [`MAX_RESULTS`]).
    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// Files that match the query (or all files when it is empty).
    pub fn match_count(&self) -> usize {
        if self.scored_query.is_empty() {
            self.files.len()
        } else {
            self.matches.len()
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The project had more files than the index keeps.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Absolute path of a result.
    pub fn absolute(&self, hit: &Hit) -> PathBuf {
        self.root.join(&hit.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn score(query: &str, candidate: &str) -> Option<i32> {
        fuzzy_match(query, candidate).map(|found| found.score)
    }

    fn positions(query: &str, candidate: &str) -> Vec<usize> {
        fuzzy_match(query, candidate).expect("it matches").positions
    }

    #[test]
    fn only_subsequences_match() {
        assert!(fuzzy_match("mrs", "src/main.rs").is_some());
        assert!(fuzzy_match("rsm", "src/main.rs").is_none(), "order matters");
        assert!(fuzzy_match("xyz", "src/main.rs").is_none());
        assert!(fuzzy_match("mainn", "main").is_none(), "a longer query cannot match");
        assert!(fuzzy_match("aa", "a").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything_without_highlights() {
        let found = fuzzy_match("", "anything").unwrap();
        assert_eq!(found.score, 0);
        assert!(found.positions.is_empty());
        assert!(fuzzy_match("   ", "anything").is_some(), "spaces are ignored");
    }

    #[test]
    fn case_and_backslashes_do_not_matter() {
        assert!(fuzzy_match("README", "readme.md").is_some());
        assert!(fuzzy_match("readme", "README.md").is_some());
        assert_eq!(positions("src\\main", "src/main.rs"), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(score("r m", "readme.md"), score("rm", "readme.md"));
    }

    #[test]
    fn consecutive_characters_beat_scattered_ones() {
        let tight = score("foo", "foo.rs").unwrap();
        let loose = score("foo", "f_o_o.rs").unwrap();
        assert!(tight > loose, "{tight} vs {loose}");
    }

    #[test]
    fn word_starts_beat_the_middle_of_a_word() {
        let boundary = score("fb", "foo_bar.rs").unwrap();
        let middle = score("fb", "fabric.rs").unwrap();
        assert!(boundary > middle, "{boundary} vs {middle}");
        let camel = score("fb", "fooBar.rs").unwrap();
        let flat = score("fb", "foobar.rs").unwrap();
        assert!(camel > flat, "{camel} vs {flat}");
    }

    #[test]
    fn the_file_name_beats_the_folders() {
        let name = score("main", "src/main.rs").unwrap();
        let folder = score("main", "docs/main_notes/readme.md").unwrap();
        assert!(name > folder, "{name} vs {folder}");
    }

    #[test]
    fn highlights_point_at_the_matched_characters() {
        assert_eq!(positions("mrs", "src/main.rs"), vec![4, 9, 10]);
        let found = fuzzy_match("rdm", "docs/README.md").unwrap();
        for position in &found.positions {
            let character = "docs/README.md".chars().nth(*position).unwrap();
            assert!("rdm".contains(character.to_ascii_lowercase()));
        }
        assert_eq!(found.positions.len(), 3);
        assert!(found.positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn positions_are_character_indexes_not_bytes() {
        // `é` and `ñ` take two bytes each.
        assert_eq!(positions("ni", "café/niño.md"), vec![5, 6]);
    }

    #[test]
    fn a_match_that_could_pick_a_bad_alignment_picks_the_good_one() {
        // `ab` appears early and scattered, and later as a word start.
        let found = fuzzy_match("ab", "a_x_x_x/ab.md").unwrap();
        assert_eq!(found.positions, vec![8, 9]);
    }

    // ----------------------------------------------------------- the index

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/ui")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join("README.md"), "# readme\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("src/ui/mod.rs"), "\n").unwrap();
        fs::write(root.join("docs/guide.md"), "# guide\n").unwrap();
        fs::write(root.join("target/debug/junk.rs"), "\n").unwrap();
        fs::write(root.join(".hidden.md"), "\n").unwrap();
        fs::write(root.join(".git/objects/blob"), "\n").unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        dir
    }

    fn wait(finder: &mut Finder) {
        let started = Instant::now();
        while finder.is_scanning() {
            finder.poll();
            assert!(started.elapsed() < Duration::from_secs(10), "the scan never finished");
            thread::sleep(Duration::from_millis(5));
        }
        finder.poll();
    }

    fn indexed(root: &Path, hidden: bool, gitignore: bool) -> Vec<String> {
        let mut finder = Finder::new();
        finder.ensure(root, hidden, gitignore);
        wait(&mut finder);
        finder.hits().iter().map(|hit| hit.path.clone()).collect()
    }

    #[test]
    fn the_index_respects_gitignore_and_hidden_files_and_skips_dot_git() {
        let dir = project();
        let files = indexed(dir.path(), false, true);
        assert_eq!(
            files,
            vec!["docs/guide.md", "README.md", "src/main.rs", "src/ui/mod.rs"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            "sorted, relative, ignoring target/, hidden files and .git"
        );

        let with_hidden = indexed(dir.path(), true, true);
        assert!(with_hidden.contains(&".hidden.md".to_string()));
        assert!(with_hidden.contains(&".gitignore".to_string()));
        assert!(!with_hidden.iter().any(|path| path.starts_with(".git/")));
        assert!(!with_hidden.iter().any(|path| path.starts_with("target/")));

        let without_ignore = indexed(dir.path(), false, false);
        assert!(without_ignore.contains(&"target/debug/junk.rs".to_string()));
    }

    #[test]
    fn typing_ranks_the_results_and_narrowing_matches_a_fresh_search() {
        let dir = project();
        let mut finder = Finder::new();
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);

        for character in "mod".chars() {
            finder.type_char(character);
        }
        let narrowed: Vec<_> = finder.hits().to_vec();
        assert_eq!(narrowed.first().map(|hit| hit.path.as_str()), Some("src/ui/mod.rs"));

        // The same query typed in one go gives the same ranking.
        let mut fresh = Finder::new();
        fresh.ensure(dir.path(), false, true);
        wait(&mut fresh);
        fresh.query = "mod".to_string();
        fresh.refresh();
        assert_eq!(fresh.hits(), narrowed.as_slice());

        finder.backspace();
        finder.backspace();
        finder.backspace();
        assert_eq!(finder.hits().len(), 4, "an empty query lists every file again");
        finder.query = "zzz".to_string();
        finder.refresh();
        assert!(finder.hits().is_empty());
        assert_eq!(finder.match_count(), 0);
    }

    #[test]
    fn delete_word_and_clear_edit_the_query() {
        let mut finder = Finder::new();
        finder.query = "src main".to_string();
        finder.delete_word();
        assert_eq!(finder.query, "src ");
        finder.delete_word();
        assert_eq!(finder.query, "");
        finder.query = "abc".to_string();
        finder.clear_query();
        assert!(finder.query.is_empty());
    }

    #[test]
    fn the_index_is_cut_at_the_limit_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..12 {
            fs::write(dir.path().join(format!("file{index:02}.txt")), "x").unwrap();
        }
        let mut finder = Finder::new();
        finder.set_scan_limits(Duration::ZERO, 5);
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);
        assert_eq!(finder.file_count(), 5);
        assert!(finder.truncated());
    }

    #[test]
    fn an_empty_project_has_no_results() {
        let dir = tempfile::tempdir().unwrap();
        let mut finder = Finder::new();
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);
        assert_eq!(finder.file_count(), 0);
        finder.type_char('a');
        assert!(finder.hits().is_empty());
    }

    #[test]
    fn a_rescan_replaces_the_list_only_when_it_is_complete() {
        let dir = project();
        let mut finder = Finder::new();
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);
        let before = finder.file_count();

        fs::write(dir.path().join("new.md"), "x").unwrap();
        finder.invalidate();
        finder.set_scan_limits(Duration::from_millis(150), MAX_INDEXED_FILES);
        finder.ensure(dir.path(), false, true);
        assert!(finder.is_scanning());
        finder.poll();
        assert_eq!(finder.file_count(), before, "the old list stays until the new one is ready");
        wait(&mut finder);
        assert_eq!(finder.file_count(), before + 1);
    }

    #[test]
    fn changing_a_setting_or_the_root_starts_a_new_scan() {
        let dir = project();
        let mut finder = Finder::new();
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);
        finder.ensure(dir.path(), false, true);
        assert!(!finder.is_scanning(), "nothing changed, nothing to do");
        finder.ensure(dir.path(), true, true);
        assert!(finder.is_scanning(), "showing hidden files needs a new index");
        wait(&mut finder);
        assert!(finder.hits().iter().any(|hit| hit.path == ".hidden.md"));
    }

    #[test]
    fn absolute_joins_the_root_and_the_relative_path() {
        let dir = project();
        let mut finder = Finder::new();
        finder.ensure(dir.path(), false, true);
        wait(&mut finder);
        let hit = finder.hits()[0].clone();
        assert_eq!(finder.absolute(&hit), dir.path().join(&hit.path));
    }
}

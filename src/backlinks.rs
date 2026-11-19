//! Backlinks: which files of the project link to a given file.
//!
//! The index is built on a background thread.  It walks the project the way
//! the finder does, reads every Markdown file, collects its links with the
//! same parser the viewer uses ([`scan_links`]) and resolves each link to a
//! file with the same rules the viewer follows when you open it:
//!
//! * relative links (`../notes/a.md`, `a`, `a.md#section`, `/from-the-root.md`)
//!   are resolved from the folder of the file that holds them; a link without
//!   extension that matches nothing gets `.md`;
//! * `[[wikilinks]]` (with `|alias` and `#section`) are looked up next to the
//!   file, then from the project root, then by name anywhere in the project
//!   (case-insensitive; if several files match, the shallowest one wins, then
//!   alphabetical order);
//! * `#anchor`-only links, external addresses, links that point outside the
//!   project or at something that does not exist are ignored.
//!
//! Files that did not change since the last build are not read again.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::app::has_url_scheme;
use crate::finder::{self, ScanOptions};
use crate::markdown::{LinkKind, SourceLink, scan_links, toc};
use crate::state::excerpt;
use crate::viewer::MAX_FILE_SIZE;

/// Markdown files read per build; a bigger project is indexed partially.
pub const MAX_MARKDOWN_FILES: usize = 20_000;
/// Bytes read per build (files that did not change cost nothing).
pub const MAX_BYTES_READ: u64 = 256 * 1024 * 1024;
/// A change on disk waits this long before the index is rebuilt, so a burst of
/// saves causes one rebuild.
pub const REBUILD_DELAY: Duration = Duration::from_secs(2);

const MARKDOWN_EXTENSIONS: [&str; 5] = ["md", "markdown", "mdown", "mkd", "mkdn"];

/// One place where a file is linked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    /// The file that holds the link: path relative to the project root, `/`
    /// separated.
    pub source: String,
    /// 0-based line of the link in `source`.
    pub line: usize,
    /// That line, trimmed and shortened.
    pub text: String,
    /// What the link shows in the rendered document (to find it again when
    /// the line is part of a larger block).
    pub label: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_files: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: MAX_MARKDOWN_FILES,
            max_bytes: MAX_BYTES_READ,
        }
    }
}

/// A link as found in a file, before it is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawLink {
    line: usize,
    text: String,
    dest: String,
    kind: LinkKind,
    label: String,
}

/// What was read from a file the last time, valid while it does not change.
#[derive(Debug, Clone)]
struct CachedFile {
    modified: Option<SystemTime>,
    len: u64,
    links: Vec<RawLink>,
}

type Cache = HashMap<String, CachedFile>;
type Links = HashMap<String, Vec<Backlink>>;

pub fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

// ------------------------------------------------------------- resolution

/// Turns the destination of a link into the project file it points to.
struct Resolver<'a> {
    root: &'a Path,
    canonical_root: Option<PathBuf>,
    /// Lower-case file name to the files that have it, for wikilinks.
    by_name: HashMap<String, Vec<&'a str>>,
}

impl<'a> Resolver<'a> {
    fn new(root: &'a Path, files: &'a [String]) -> Self {
        let mut by_name: HashMap<String, Vec<&'a str>> = HashMap::new();
        for file in files {
            let name = file.rsplit('/').next().unwrap_or(file).to_lowercase();
            by_name.entry(name).or_default().push(file.as_str());
        }
        Self {
            root,
            canonical_root: root.canonicalize().ok(),
            by_name,
        }
    }

    fn is_file(&self, relative: &str) -> bool {
        self.root.join(relative).is_file()
    }

    /// Does the file really live inside the project (symlinks included)?
    fn inside(&self, relative: &str) -> bool {
        match (&self.canonical_root, self.root.join(relative).canonicalize()) {
            (Some(root), Ok(path)) => path.starts_with(root),
            _ => true,
        }
    }

    fn resolve(&self, source: &str, link: &RawLink) -> Option<String> {
        let found = match link.kind {
            LinkKind::Wiki => self.resolve_wiki(source, &link.dest),
            LinkKind::Internal => self.resolve_internal(source, &link.dest),
            LinkKind::Anchor | LinkKind::External => None,
        }?;
        (found != source).then_some(found)
    }

    fn resolve_internal(&self, source: &str, dest: &str) -> Option<String> {
        let (path, _fragment) = toc::split_fragment(dest);
        let decoded = toc::decode_fragment(path).replace('\\', "/");
        let path = decoded.split('?').next().unwrap_or(&decoded).trim();
        if path.is_empty() || has_url_scheme(path) {
            return None;
        }
        let joined = match path.strip_prefix('/') {
            Some(from_root) => from_root.to_string(),
            None => join(parent(source), path),
        };
        let mut relative = normalize(&joined)?;
        if !self.is_file(&relative) && Path::new(&relative).extension().is_none() {
            relative.push_str(".md");
        }
        (self.is_file(&relative) && self.inside(&relative)).then_some(relative)
    }

    fn resolve_wiki(&self, source: &str, dest: &str) -> Option<String> {
        let (path, _fragment) = toc::split_fragment(dest);
        let decoded = toc::decode_fragment(path);
        let target = decoded.split('|').next().unwrap_or(&decoded).trim().replace('\\', "/");
        if target.is_empty() {
            return None;
        }
        let without_extension = Path::new(&target).extension().is_none();
        let variants: Vec<String> = if without_extension {
            vec![target.clone(), format!("{target}.md")]
        } else {
            vec![target.clone()]
        };

        for variant in &variants {
            for candidate in [join(parent(source), variant), variant.clone()] {
                if let Some(relative) = normalize(&candidate) {
                    if self.is_file(&relative) && self.inside(&relative) {
                        return Some(relative);
                    }
                }
            }
        }

        // By name, anywhere in the project.
        let lower = target.to_lowercase();
        let wanted: Vec<String> = if without_extension {
            vec![lower.clone(), format!("{lower}.md")]
        } else {
            vec![lower]
        };
        let mut best: Option<&str> = None;
        for want in &wanted {
            let name = want.rsplit('/').next().unwrap_or(want);
            let Some(candidates) = self.by_name.get(name) else {
                continue;
            };
            for candidate in candidates {
                let lowered = candidate.to_lowercase();
                let matches = lowered == *want || lowered.ends_with(&format!("/{want}"));
                if matches && best.is_none_or(|current| rank(candidate) < rank(current)) {
                    best = Some(candidate);
                }
            }
        }
        best.map(str::to_string)
    }
}

/// Shallowest first, then alphabetical.
fn rank(path: &str) -> (usize, String) {
    (path.matches('/').count(), path.to_lowercase())
}

fn parent(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn join(folder: &str, path: &str) -> String {
    if folder.is_empty() {
        path.to_string()
    } else {
        format!("{folder}/{path}")
    }
}

/// Resolve `.` and `..` without touching the disk.  `None` when the path
/// climbs out of the project.
fn normalize(relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

// ------------------------------------------------------------------ build

fn read_links(path: &Path, bytes: u64) -> Option<Vec<RawLink>> {
    if bytes > MAX_FILE_SIZE {
        return None;
    }
    let raw = fs::read(path).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let found: Vec<SourceLink> = scan_links(&text);
    if found.is_empty() {
        return Some(Vec::new());
    }
    let lines: Vec<&str> = text.lines().collect();
    Some(
        found
            .into_iter()
            .map(|link| RawLink {
                line: link.line,
                text: excerpt(lines.get(link.line).copied().unwrap_or("")),
                dest: link.dest,
                kind: link.kind,
                label: link.label,
            })
            .collect(),
    )
}

/// The links of every Markdown file of `files`, resolved.  `None` when
/// `cancel` was set.  Returns whether the project was only partly read.
fn build(
    root: &Path,
    files: &[String],
    cache: &mut Cache,
    limits: Limits,
    cancel: &AtomicBool,
) -> Option<(Links, bool)> {
    let resolver = Resolver::new(root, files);
    let mut links: Links = HashMap::new();
    let mut partial = false;
    let mut read = 0u64;
    let mut seen_files = 0usize;
    let mut alive: HashSet<&str> = HashSet::new();

    for file in files.iter().filter(|file| is_markdown(file)) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if seen_files >= limits.max_files {
            partial = true;
            break;
        }
        seen_files += 1;
        alive.insert(file);

        let path = root.join(file);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        let modified = metadata.modified().ok();
        let fresh = cache
            .get(file)
            .is_some_and(|cached| cached.len == metadata.len() && cached.modified == modified && modified.is_some());
        if !fresh {
            if read >= limits.max_bytes {
                partial = true;
                continue;
            }
            read += metadata.len();
            match read_links(&path, metadata.len()) {
                Some(found) => {
                    cache.insert(
                        file.clone(),
                        CachedFile {
                            modified,
                            len: metadata.len(),
                            links: found,
                        },
                    );
                }
                None => {
                    cache.remove(file);
                    continue;
                }
            }
        }
        let Some(cached) = cache.get(file) else {
            continue;
        };
        for link in &cached.links {
            let Some(target) = resolver.resolve(file, link) else {
                continue;
            };
            let entry = links.entry(target).or_default();
            // Two links to the same file on one line count once.
            if !entry.iter().any(|known| known.source == *file && known.line == link.line) {
                entry.push(Backlink {
                    source: file.clone(),
                    line: link.line,
                    text: link.text.clone(),
                    label: link.label.clone(),
                });
            }
        }
    }
    cache.retain(|file, _| alive.contains(file.as_str()));
    for entry in links.values_mut() {
        entry.sort_by(|a, b| (a.source.to_lowercase(), a.line).cmp(&(b.source.to_lowercase(), b.line)));
    }
    Some((links, partial))
}

// ------------------------------------------------------------------ index

struct Built {
    links: Links,
    cache: Cache,
    partial: bool,
}

struct Job {
    receiver: Receiver<Option<Built>>,
    cancel: Arc<AtomicBool>,
    since: Instant,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The project's links, kept up to date in the background.
pub struct BacklinkIndex {
    root: PathBuf,
    options: ScanOptions,
    limits: Limits,
    rebuild_delay: Duration,
    links: Links,
    cache: Cache,
    built: bool,
    partial: bool,
    stale_since: Option<Instant>,
    job: Option<Job>,
    empty: Vec<Backlink>,
}

impl Default for BacklinkIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl BacklinkIndex {
    pub fn new() -> Self {
        Self {
            root: PathBuf::new(),
            options: ScanOptions {
                show_hidden: false,
                respect_gitignore: true,
                max_files: finder::MAX_INDEXED_FILES,
                batch_delay: Duration::ZERO,
            },
            limits: Limits::default(),
            rebuild_delay: REBUILD_DELAY,
            links: HashMap::new(),
            cache: HashMap::new(),
            built: false,
            partial: false,
            stale_since: None,
            job: None,
            empty: Vec::new(),
        }
    }

    /// Test hook: rebuild at once after a change, and read at most `limits`.
    #[cfg(test)]
    pub fn set_test_limits(&mut self, rebuild_delay: Duration, limits: Limits) {
        self.rebuild_delay = rebuild_delay;
        self.limits = limits;
    }

    /// Test hook: pause before each batch of the file walk.
    #[cfg(test)]
    pub fn set_walk_delay(&mut self, delay: Duration) {
        self.options.batch_delay = delay;
    }

    /// Start a build when there is none and one is due: the first one, one
    /// for another project or other settings, or one after files changed.
    pub fn ensure(&mut self, root: &Path, show_hidden: bool, respect_gitignore: bool) {
        let different = self.root != root
            || self.options.show_hidden != show_hidden
            || self.options.respect_gitignore != respect_gitignore;
        if different {
            if self.root != root {
                self.links.clear();
                self.cache.clear();
                self.built = false;
            }
            self.root = root.to_path_buf();
            self.options.show_hidden = show_hidden;
            self.options.respect_gitignore = respect_gitignore;
            self.start();
            return;
        }
        if self.job.is_some() {
            return;
        }
        let due = match self.stale_since {
            Some(since) => since.elapsed() >= self.rebuild_delay,
            None => false,
        };
        if !self.built || due {
            self.start();
        }
    }

    /// Files changed on disk: rebuild soon (a burst of changes waits together).
    pub fn invalidate(&mut self) {
        self.stale_since = Some(Instant::now());
    }

    fn start(&mut self) {
        self.job = None;
        self.stale_since = None;
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let root = self.root.clone();
        let options = self.options.clone();
        let limits = self.limits;
        let mut cache = std::mem::take(&mut self.cache);
        let spawned = thread::Builder::new()
            .name("rtui-backlinks".to_string())
            .spawn(move || {
                let built = finder::list_files(&root, &options, &thread_cancel).and_then(
                    |(files, _truncated)| {
                        let (links, partial) = build(&root, &files, &mut cache, limits, &thread_cancel)?;
                        Some(Built {
                            links,
                            cache,
                            partial,
                        })
                    },
                );
                let _ = sender.send(built);
            });
        if spawned.is_ok() {
            self.job = Some(Job {
                receiver,
                cancel,
                since: Instant::now(),
            });
        } else {
            self.built = true;
        }
    }

    /// Take in a finished build.  True when the links changed.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        match job.receiver.try_recv() {
            Ok(built) => {
                self.job = None;
                if let Some(built) = built {
                    self.links = built.links;
                    self.cache = built.cache;
                    self.partial = built.partial;
                    self.built = true;
                    return true;
                }
                false
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                false
            }
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.job.is_some()
    }

    pub fn scan_started(&self) -> Option<Instant> {
        self.job.as_ref().map(|job| job.since)
    }

    /// The first build is done (later builds keep showing the previous links).
    pub fn is_ready(&self) -> bool {
        self.built
    }

    /// The project was too big to be read completely.
    pub fn is_partial(&self) -> bool {
        self.partial
    }

    /// Who links to `target` (path relative to the root, `/` separated).
    pub fn backlinks(&self, target: &str) -> &[Backlink] {
        self.links.get(target).map_or(self.empty.as_slice(), Vec::as_slice)
    }

    /// How many places link to `target`, once the first build is done.
    pub fn count(&self, target: &str) -> Option<usize> {
        self.built.then(|| self.backlinks(target).len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn options() -> ScanOptions {
        ScanOptions {
            show_hidden: false,
            respect_gitignore: true,
            max_files: finder::MAX_INDEXED_FILES,
            batch_delay: Duration::ZERO,
        }
    }

    fn index(root: &Path) -> Links {
        let (files, _) = finder::list_files(root, &options(), &AtomicBool::new(false)).unwrap();
        build(root, &files, &mut Cache::new(), Limits::default(), &AtomicBool::new(false))
            .unwrap()
            .0
    }

    fn sources(links: &Links, target: &str) -> Vec<(String, usize)> {
        links
            .get(target)
            .map(|entries| entries.iter().map(|b| (b.source.clone(), b.line)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn normalize_resolves_dots_and_refuses_to_leave_the_project() {
        assert_eq!(normalize("a/./b/../c.md").as_deref(), Some("a/c.md"));
        assert_eq!(normalize("a//b").as_deref(), Some("a/b"));
        assert_eq!(normalize("../x.md"), None);
        assert_eq!(normalize("a/../../x.md"), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("."), None);
    }

    #[test]
    fn relative_markdown_links_resolve_from_the_folder_of_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "README.md", "# Home\n\nSee [guide](docs/guide.md) and [api](docs/api/index.md).\n");
        write(root, "docs/guide.md", "# Guide\n\nBack to [home](../README.md).\nAlso [sibling](./other.md).\n");
        write(root, "docs/other.md", "# Other\n\n[root-relative](/docs/guide.md)\n");
        write(root, "docs/api/index.md", "# API\n\n[up](../guide.md)\n");
        let links = index(root);
        assert_eq!(
            sources(&links, "docs/guide.md"),
            vec![
                ("docs/api/index.md".to_string(), 2),
                ("docs/other.md".to_string(), 2),
                ("README.md".to_string(), 2),
            ],
            "sorted by path, ignoring case"
        );
        assert_eq!(sources(&links, "README.md"), vec![("docs/guide.md".to_string(), 2)]);
        assert_eq!(sources(&links, "docs/other.md"), vec![("docs/guide.md".to_string(), 3)]);
        assert_eq!(sources(&links, "docs/api/index.md"), vec![("README.md".to_string(), 2)]);
    }

    #[test]
    fn anchors_queries_percent_encoding_and_missing_extensions_are_understood() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[one](b.md#section)\n[two](b)\n[three](b.md?raw=1)\n[four](my%20note.md)\n[five](#local)\n");
        write(root, "b.md", "# B\n");
        write(root, "my note.md", "# Spaces\n");
        let links = index(root);
        assert_eq!(
            sources(&links, "b.md"),
            vec![("a.md".to_string(), 0), ("a.md".to_string(), 1), ("a.md".to_string(), 2)]
        );
        assert_eq!(sources(&links, "my note.md"), vec![("a.md".to_string(), 3)]);
        assert!(!links.contains_key("a.md"), "an #anchor in the same file is not a backlink");
    }

    #[test]
    fn wikilinks_with_aliases_and_anchors_resolve_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "index.md",
            "[[guide]]\n[[guide|read the guide]]\n[[guide#intro]]\n[[Notes/Ideas]]\n[[missing]]\n",
        );
        write(root, "docs/guide.md", "# Guide\n");
        write(root, "notes/ideas.md", "# Ideas\n");
        let links = index(root);
        assert_eq!(
            sources(&links, "docs/guide.md"),
            vec![("index.md".to_string(), 0), ("index.md".to_string(), 1), ("index.md".to_string(), 2)]
        );
        assert_eq!(sources(&links, "notes/ideas.md"), vec![("index.md".to_string(), 3)], "the folder part matches case-insensitively");
        assert!(!links.values().flatten().any(|b| b.line == 4), "broken wikilinks are ignored");
    }

    #[test]
    fn a_wikilink_to_a_name_that_exists_twice_picks_the_shallowest_then_alphabetical() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "index.md", "[[dup]]\n");
        write(root, "z/deep/dup.md", "# deep\n");
        write(root, "b/dup.md", "# b\n");
        write(root, "a/dup.md", "# a\n");
        let links = index(root);
        assert_eq!(sources(&links, "a/dup.md"), vec![("index.md".to_string(), 0)]);
        assert!(!links.contains_key("b/dup.md") && !links.contains_key("z/deep/dup.md"));
    }

    #[test]
    fn a_wikilink_prefers_the_file_next_to_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "docs/page.md", "[[twin]]\n");
        write(root, "docs/twin.md", "# near\n");
        write(root, "aaa/twin.md", "# far\n");
        let links = index(root);
        assert_eq!(sources(&links, "docs/twin.md"), vec![("docs/page.md".to_string(), 0)]);
        assert!(!links.contains_key("aaa/twin.md"));
    }

    #[test]
    fn broken_external_and_escaping_links_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        write(&root, "a.md", "[gone](gone.md)\n[web](https://example.com/a.md)\n[mail](mailto:a@b.c)\n[js](javascript:alert(1))\n[up](../outside.md)\n[dir](docs)\n[drive](C:/x.md)\n");
        write(&root, "docs/x.md", "# x\n");
        write(dir.path(), "outside.md", "# outside\n");
        let links = index(&root);
        assert!(links.is_empty(), "{links:?}");
    }

    #[test]
    fn links_inside_code_and_images_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "```\n[no](b.md)\n```\n\n`[no](b.md)`\n\n![image](b.md)\n\n    [no](b.md)\n");
        write(root, "b.md", "# B\n");
        assert!(index(root).is_empty());
    }

    #[test]
    fn the_line_is_where_the_link_starts_and_each_line_counts_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "a.md",
            "# T\n\nfirst [x](b.md) and again [y](b.md) here\n\nmulti-line paragraph\nwith [a link](b.md)\nover lines\n",
        );
        write(root, "b.md", "# B\n");
        let links = index(root);
        let entries = &links["b.md"];
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(entries[0].line, 2);
        assert_eq!(entries[0].text, "first [x](b.md) and again [y](b.md) here");
        assert_eq!(entries[1].line, 5);
        assert_eq!(entries[1].text, "with [a link](b.md)");
    }

    #[test]
    fn a_file_linking_to_itself_is_not_a_backlink() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "[me](a.md) [me too](./a.md#x) [[a]]\n");
        assert!(index(dir.path()).is_empty());
    }

    #[test]
    fn non_markdown_files_can_be_targets_but_are_never_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[code](src/main.rs)\n[text](notes.txt)\n");
        write(root, "src/main.rs", "// [x](a.md)\n");
        write(root, "notes.txt", "[y](a.md)\n");
        let links = index(root);
        assert_eq!(sources(&links, "src/main.rs"), vec![("a.md".to_string(), 0)]);
        assert_eq!(sources(&links, "notes.txt"), vec![("a.md".to_string(), 1)]);
        assert!(!links.contains_key("a.md"));
    }

    #[test]
    fn ignored_and_hidden_files_are_not_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", "build/\n");
        write(root, "build/gen.md", "[a](../a.md)\n");
        write(root, ".hidden.md", "[a](a.md)\n");
        write(root, "a.md", "# A\n");
        assert!(index(root).is_empty());
    }

    #[test]
    fn unchanged_files_are_not_read_again_and_changed_ones_are() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[b](b.md)\n");
        write(root, "b.md", "# B\n");
        let (files, _) = finder::list_files(root, &options(), &AtomicBool::new(false)).unwrap();
        let mut cache = Cache::new();
        let cancel = AtomicBool::new(false);
        let (first, _) = build(root, &files, &mut cache, Limits::default(), &cancel).unwrap();
        assert_eq!(first["b.md"].len(), 1);

        // With no byte budget left, only the cache can answer.
        let none = Limits { max_files: 100, max_bytes: 0 };
        let (again, partial) = build(root, &files, &mut cache, none, &cancel).unwrap();
        assert_eq!(again["b.md"].len(), 1, "served from the cache");
        assert!(!partial);

        // A changed file is read again (the size differs).
        write(root, "a.md", "[b](b.md)\n[again](b.md#x)\n\n[third](b.md)\n");
        let (changed, _) = build(root, &files, &mut cache, Limits::default(), &cancel).unwrap();
        assert_eq!(changed["b.md"].len(), 3, "the changed file was read again: lines 0, 1 and 3");
        // A file that vanished leaves the cache.
        fs::remove_file(root.join("a.md")).unwrap();
        let (gone, _) = build(root, &files, &mut cache, Limits::default(), &cancel).unwrap();
        assert!(gone.is_empty());
    }

    #[test]
    fn big_projects_are_indexed_partially_and_say_so() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..6 {
            write(dir.path(), &format!("f{index}.md"), "# x\n");
        }
        let (files, _) = finder::list_files(dir.path(), &options(), &AtomicBool::new(false)).unwrap();
        let limits = Limits { max_files: 3, max_bytes: u64::MAX };
        let (_, partial) = build(dir.path(), &files, &mut Cache::new(), limits, &AtomicBool::new(false)).unwrap();
        assert!(partial);
        let limits = Limits { max_files: 100, max_bytes: 0 };
        let (_, partial) = build(dir.path(), &files, &mut Cache::new(), limits, &AtomicBool::new(false)).unwrap();
        assert!(partial, "no bytes to read means nothing was read");
    }

    #[test]
    fn a_cancelled_build_gives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "# x\n");
        let (files, _) = finder::list_files(dir.path(), &options(), &AtomicBool::new(false)).unwrap();
        assert!(build(dir.path(), &files, &mut Cache::new(), Limits::default(), &AtomicBool::new(true)).is_none());
        assert!(finder::list_files(dir.path(), &options(), &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn binary_and_oversized_files_are_skipped_quietly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[b](b.md)\n");
        write(root, "b.md", "# B\n");
        fs::write(root.join("binary.md"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
        fs::write(root.join("huge.md"), vec![b'a'; (MAX_FILE_SIZE + 1) as usize]).unwrap();
        let links = index(root);
        assert_eq!(sources(&links, "b.md"), vec![("a.md".to_string(), 0)]);
    }

    // ----------------------------------------------------- the live index

    fn wait(index: &mut BacklinkIndex) {
        let started = Instant::now();
        while index.is_scanning() {
            index.poll();
            assert!(started.elapsed() < Duration::from_secs(10), "the build never finished");
            thread::sleep(Duration::from_millis(5));
        }
        index.poll();
    }

    #[test]
    fn the_index_builds_in_the_background_and_answers_counts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[b](b.md)\n");
        write(root, "c.md", "[[b]]\n");
        write(root, "b.md", "# B\n");
        let mut index = BacklinkIndex::new();
        assert_eq!(index.count("b.md"), None, "nothing is known before the first build");
        index.ensure(root, false, true);
        assert!(index.is_scanning());
        wait(&mut index);
        assert!(index.is_ready());
        assert_eq!(index.count("b.md"), Some(2));
        assert_eq!(index.count("a.md"), Some(0));
        assert_eq!(index.backlinks("b.md")[1].source, "c.md");
        assert!(!index.is_partial());
        // Nothing changed: no new build.
        index.ensure(root, false, true);
        assert!(!index.is_scanning());
    }

    #[test]
    fn invalidating_rebuilds_after_the_delay_and_keeps_the_old_answer_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", "[b](b.md)\n");
        write(root, "b.md", "# B\n");
        let mut index = BacklinkIndex::new();
        index.set_test_limits(Duration::from_millis(150), Limits::default());
        index.ensure(root, false, true);
        wait(&mut index);
        assert_eq!(index.count("b.md"), Some(1));

        write(root, "c.md", "[b](b.md)\n");
        index.invalidate();
        index.ensure(root, false, true);
        assert!(!index.is_scanning(), "a burst of changes waits");
        assert_eq!(index.count("b.md"), Some(1), "the previous answer stays");
        thread::sleep(Duration::from_millis(200));
        index.ensure(root, false, true);
        assert!(index.is_scanning());
        wait(&mut index);
        assert_eq!(index.count("b.md"), Some(2));
    }

    #[test]
    fn changing_the_settings_or_the_root_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        write(dir.path(), ".hidden.md", "[b](b.md)\n");
        write(dir.path(), "b.md", "# B\n");
        write(other.path(), "x.md", "# X\n");
        let mut index = BacklinkIndex::new();
        index.ensure(dir.path(), false, true);
        wait(&mut index);
        assert_eq!(index.count("b.md"), Some(0));
        index.ensure(dir.path(), true, true);
        assert!(index.is_scanning());
        wait(&mut index);
        assert_eq!(index.count("b.md"), Some(1));
        index.ensure(other.path(), false, true);
        assert_eq!(index.count("b.md"), None, "another project: the old answers are gone");
        wait(&mut index);
        assert_eq!(index.count("x.md"), Some(0));
    }
}

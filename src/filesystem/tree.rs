//! Lazy directory tree stored in an arena.
//!
//! Only directories the user expands are read from disk, so opening a huge
//! repository stays instant.  `.gitignore` / `.ignore` rules are honoured by
//! default through the `ignore` crate.
//!
//! Directories are listed on background threads.  An interactive operation
//! (expanding a folder, revealing a file) waits for the listing for at most
//! [`LOAD_BUDGET`]: normal directories are ready within that time and behave
//! as if the read were synchronous, while a huge directory, a cold disk or a
//! network mount lets the interface carry on and the result is applied by
//! [`FileTree::poll`] when it arrives.  Re-listing a directory merges the new
//! entries into the existing nodes, so expanded folders keep their state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;

use crate::trace;

const MAX_ENTRIES_PER_DIRECTORY: usize = 10_000;

/// How long an interactive operation waits for a directory listing before it
/// lets the listing finish in the background.
pub const LOAD_BUDGET: Duration = Duration::from_millis(60);

/// Listings running at the same time; the rest wait in a queue.
const MAX_CONCURRENT_LISTINGS: usize = 8;

#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    pub loaded: bool,
    /// The first listing of this directory is still running in the background.
    pub loading: bool,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
struct DirEntryInfo {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

/// A directory listing waiting to run.
#[derive(Debug, Clone)]
struct Job {
    path: PathBuf,
    show_hidden: bool,
    respect_gitignore: bool,
    epoch: u64,
    #[cfg(test)]
    delay: Duration,
}

/// A finished listing on its way back to the tree.
#[derive(Debug)]
struct Listing {
    path: PathBuf,
    epoch: u64,
    entries: Vec<DirEntryInfo>,
}

impl Job {
    fn run(self) -> Listing {
        #[cfg(test)]
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
        }
        let started = Instant::now();
        let entries = list_directory(&self.path, self.show_hidden, self.respect_gitignore);
        if trace::enabled() {
            trace::mark(&format!(
                "listing {} took {:.1} ms ({} entries)",
                self.path.display(),
                started.elapsed().as_secs_f64() * 1000.0,
                entries.len()
            ));
        }
        Listing {
            path: self.path,
            epoch: self.epoch,
            entries,
        }
    }
}

#[derive(Debug)]
struct Loader {
    sender: Sender<Listing>,
    receiver: Receiver<Listing>,
    /// Directories requested and not yet applied, whether running or queued.
    in_flight: HashSet<PathBuf>,
    /// Directories asked for again while their listing was still running.
    stale: HashSet<PathBuf>,
    queue: VecDeque<Job>,
    running: usize,
    budget: Duration,
    /// When the oldest pending listing was requested.
    busy_since: Option<Instant>,
    #[cfg(test)]
    delay: Duration,
}

impl Loader {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            in_flight: HashSet::new(),
            stale: HashSet::new(),
            queue: VecDeque::new(),
            running: 0,
            // Unit tests must not depend on how busy the machine is.
            budget: if cfg!(test) {
                Duration::from_secs(10)
            } else {
                LOAD_BUDGET
            },
            busy_since: None,
            #[cfg(test)]
            delay: Duration::ZERO,
        }
    }

    /// Start queued listings while there is room.
    fn pump(&mut self) {
        while self.running < MAX_CONCURRENT_LISTINGS {
            let Some(job) = self.queue.pop_front() else {
                break;
            };
            self.running += 1;
            let sender = self.sender.clone();
            let thread_job = job.clone();
            let spawned = thread::Builder::new()
                .name("rtui-listing".into())
                .spawn(move || {
                    let _ = sender.send(thread_job.run());
                });
            if spawned.is_err() {
                // No thread available: list right here rather than not at all.
                let listing = job.run();
                let _ = self.sender.send(listing);
            }
        }
    }
}

#[derive(Debug)]
pub struct FileTree {
    nodes: Vec<FileNode>,
    /// Node index by path, for the nodes that are part of the tree.
    index: HashMap<PathBuf, usize>,
    root: usize,
    pub show_hidden: bool,
    pub respect_gitignore: bool,
    loader: Loader,
    /// Bumped when the listing settings change: older results are discarded.
    epoch: u64,
    /// Bumped when the set of expanded directories may have changed.
    generation: u64,
}

impl FileTree {
    pub fn new(root: &Path, show_hidden: bool, respect_gitignore: bool) -> Self {
        Self::with_loader(root, show_hidden, respect_gitignore, Loader::new())
    }

    /// A tree whose listings sleep for `delay` first and whose interactive
    /// operations wait at most `budget`, to exercise the background path.
    #[cfg(test)]
    pub fn new_slow(
        root: &Path,
        show_hidden: bool,
        respect_gitignore: bool,
        delay: Duration,
        budget: Duration,
    ) -> Self {
        let mut loader = Loader::new();
        loader.delay = delay;
        loader.budget = budget;
        Self::with_loader(root, show_hidden, respect_gitignore, loader)
    }

    fn with_loader(
        root: &Path,
        show_hidden: bool,
        respect_gitignore: bool,
        loader: Loader,
    ) -> Self {
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let root_node = FileNode {
            path: root.to_path_buf(),
            name,
            is_dir: true,
            depth: 0,
            expanded: true,
            loaded: false,
            loading: false,
            children: Vec::new(),
        };
        let mut index = HashMap::new();
        index.insert(root.to_path_buf(), 0);
        let mut tree = Self {
            nodes: vec![root_node],
            index,
            root: 0,
            show_hidden,
            respect_gitignore,
            loader,
            epoch: 0,
            generation: 0,
        };
        tree.load_children(0);
        tree
    }

    pub fn root_index(&self) -> usize {
        self.root
    }

    pub fn root_path(&self) -> &Path {
        &self.nodes[self.root].path
    }

    pub fn node(&self, index: usize) -> Option<&FileNode> {
        self.nodes.get(index)
    }

    /// Changes whenever the set of expanded directories may have changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Directories being listed in the background right now.
    pub fn pending(&self) -> usize {
        self.loader.in_flight.len()
    }

    /// When the oldest background listing started, if any is still running.
    pub fn busy_since(&self) -> Option<Instant> {
        self.loader.busy_since
    }

    #[cfg(test)]
    pub fn set_load_budget(&mut self, budget: Duration) {
        self.loader.budget = budget;
    }

    #[cfg(test)]
    pub fn set_listing_delay(&mut self, delay: Duration) {
        self.loader.delay = delay;
    }

    // ---------------------------------------------------------------- loading

    /// Ask for a listing of the directory at `index`.  With `force` a listing
    /// that is already running is followed by another one, because the
    /// directory changed again in the meantime.
    fn request_listing(&mut self, index: usize, force: bool) {
        let Some(node) = self.nodes.get_mut(index) else {
            return;
        };
        if !node.is_dir {
            return;
        }
        let path = node.path.clone();
        if self.loader.in_flight.contains(&path) {
            if force {
                self.loader.stale.insert(path);
            }
            return;
        }
        node.loading = !node.loaded;
        self.loader.in_flight.insert(path.clone());
        self.loader.busy_since.get_or_insert_with(Instant::now);
        self.loader.queue.push_back(Job {
            path,
            show_hidden: self.show_hidden,
            respect_gitignore: self.respect_gitignore,
            epoch: self.epoch,
            #[cfg(test)]
            delay: self.loader.delay,
        });
        self.loader.pump();
    }

    /// Wait until none of `paths` is being listed, or until `deadline`.
    fn wait_for(&mut self, paths: &[PathBuf], deadline: Instant) {
        while paths
            .iter()
            .any(|path| self.loader.in_flight.contains(path))
        {
            let Some(remaining) = deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
            else {
                break;
            };
            match self.loader.receiver.recv_timeout(remaining) {
                Ok(listing) => {
                    self.apply(listing);
                }
                Err(_) => break,
            }
        }
        self.poll();
    }

    fn deadline(&self) -> Instant {
        Instant::now() + self.loader.budget
    }

    /// Make sure the directory at `index` has been listed, waiting for it at
    /// most until `deadline`.
    fn load_children_until(&mut self, index: usize, deadline: Instant) {
        let Some(node) = self.nodes.get(index) else {
            return;
        };
        if !node.is_dir || node.loaded {
            return;
        }
        let path = node.path.clone();
        self.request_listing(index, false);
        self.wait_for(&[path], deadline);
    }

    pub fn load_children(&mut self, index: usize) {
        let deadline = self.deadline();
        self.load_children_until(index, deadline);
    }

    /// Apply the listings that have finished.  Returns `true` when the tree
    /// changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(listing) = self.loader.receiver.try_recv() {
            changed |= self.apply(listing);
        }
        changed
    }

    fn apply(&mut self, listing: Listing) -> bool {
        self.loader.running = self.loader.running.saturating_sub(1);
        self.loader.in_flight.remove(&listing.path);
        if self.loader.in_flight.is_empty() {
            self.loader.busy_since = None;
        }
        let stale = self.loader.stale.remove(&listing.path);
        self.loader.pump();

        let Some(index) = self.index.get(&listing.path).copied() else {
            return false;
        };
        if listing.epoch != self.epoch {
            // The listing settings changed while this ran: list again.
            self.request_listing(index, true);
            return false;
        }
        self.merge_children(index, listing.entries);
        if stale {
            self.request_listing(index, true);
        }
        true
    }

    /// Replace the children of `index` with `entries`, keeping the nodes (and
    /// so their expanded/loaded state) of entries that are still there.
    fn merge_children(&mut self, index: usize, entries: Vec<DirEntryInfo>) {
        let old_children = std::mem::take(&mut self.nodes[index].children);
        let mut remaining: HashMap<PathBuf, usize> = old_children
            .into_iter()
            .map(|child| (self.nodes[child].path.clone(), child))
            .collect();
        let depth = self.nodes[index].depth + 1;
        let mut children = Vec::with_capacity(entries.len());
        let mut dropped = false;

        for entry in entries {
            if let Some(existing) = remaining.remove(&entry.path) {
                if self.nodes[existing].is_dir == entry.is_dir {
                    children.push(existing);
                    continue;
                }
                // A file became a directory or the other way round.
                self.forget_subtree(existing);
                dropped = true;
            }
            let child = self.nodes.len();
            self.index.insert(entry.path.clone(), child);
            self.nodes.push(FileNode {
                path: entry.path,
                name: entry.name,
                is_dir: entry.is_dir,
                depth,
                expanded: false,
                loaded: false,
                loading: false,
                children: Vec::new(),
            });
            children.push(child);
        }
        for (_, gone) in remaining {
            self.forget_subtree(gone);
            dropped = true;
        }

        let node = &mut self.nodes[index];
        node.children = children;
        node.loaded = true;
        node.loading = false;
        if dropped {
            self.generation += 1;
        }
    }

    /// Remove a node and everything below it from the path index.  The nodes
    /// stay in the arena, unreachable.
    fn forget_subtree(&mut self, index: usize) {
        let mut stack = vec![index];
        while let Some(current) = stack.pop() {
            let node = &self.nodes[current];
            if self.index.get(&node.path) == Some(&current) {
                self.index.remove(&node.path);
            }
            stack.extend(node.children.iter().copied());
        }
    }

    // ------------------------------------------------------------- navigation

    /// Depth-first list of visible nodes.
    pub fn visible(&self) -> Vec<usize> {
        let mut result = Vec::with_capacity(self.nodes.len().min(4096));
        fn walk(tree: &FileTree, index: usize, result: &mut Vec<usize>) {
            result.push(index);
            let node = &tree.nodes[index];
            if node.is_dir && node.expanded && node.loaded {
                for child in &node.children {
                    walk(tree, *child, result);
                }
            }
        }
        if !self.nodes.is_empty() {
            walk(self, self.root, &mut result);
        }
        result
    }

    /// Expanded directories that can currently be seen: the root and every
    /// expanded folder whose ancestors are all expanded.
    pub fn open_directories(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut stack = vec![self.root];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            if !node.is_dir || !node.expanded {
                continue;
            }
            result.push(node.path.clone());
            stack.extend(node.children.iter().copied());
        }
        result
    }

    pub fn expand(&mut self, index: usize) {
        let deadline = self.deadline();
        self.expand_until(index, deadline);
    }

    fn expand_until(&mut self, index: usize, deadline: Instant) {
        self.load_children_until(index, deadline);
        if let Some(node) = self.nodes.get_mut(index) {
            if node.is_dir && !node.expanded {
                node.expanded = true;
                self.generation += 1;
            }
        }
    }

    pub fn collapse(&mut self, index: usize) {
        if let Some(node) = self.nodes.get_mut(index) {
            if node.is_dir && node.expanded {
                node.expanded = false;
                self.generation += 1;
            }
        }
    }

    pub fn is_expanded(&self, index: usize) -> bool {
        self.nodes
            .get(index)
            .map(|node| node.is_dir && node.expanded)
            .unwrap_or(false)
    }

    pub fn find_path(&self, path: &Path) -> Option<usize> {
        self.index.get(path).copied()
    }

    /// Expand every ancestor of `path` and return its node index.  `None`
    /// when the path is outside the tree or one of its folders is still being
    /// listed in the background.
    pub fn reveal_path(&mut self, path: &Path) -> Option<usize> {
        let root = self.root_path().to_path_buf();
        if !path.starts_with(&root) {
            return None;
        }

        let mut ancestors = Vec::new();
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent == root {
                break;
            }
            if !parent.starts_with(&root) {
                return None;
            }
            ancestors.push(parent.to_path_buf());
            current = parent.parent();
        }
        ancestors.reverse();

        // One budget for the whole walk down, not one per folder.
        let deadline = self.deadline();
        self.expand_until(self.root, deadline);
        for ancestor in ancestors {
            if let Some(index) = self.find_path(&ancestor) {
                self.expand_until(index, deadline);
            }
        }

        self.find_path(path)
    }

    // ---------------------------------------------------------------- refresh

    /// List the directories at `paths` again, keeping the expanded state.
    /// Directories that were never listed are left alone: they will be read
    /// when opened.
    pub fn refresh_dirs(&mut self, paths: &[PathBuf]) {
        let mut requested = Vec::new();
        for path in paths {
            let Some(index) = self.find_path(path) else {
                continue;
            };
            if !self.nodes[index].loaded {
                continue;
            }
            self.request_listing(index, true);
            requested.push(path.clone());
        }
        let deadline = self.deadline();
        self.wait_for(&requested, deadline);
    }

    /// List every directory that has been read again.
    pub fn refresh(&mut self) {
        let loaded: Vec<PathBuf> = self
            .index
            .values()
            .filter_map(|&index| self.nodes.get(index))
            .filter(|node| node.is_dir && node.loaded)
            .map(|node| node.path.clone())
            .collect();
        self.refresh_dirs(&loaded);
    }

    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        if self.show_hidden == show_hidden {
            return;
        }
        self.show_hidden = show_hidden;
        self.epoch += 1;
        self.refresh();
    }

    pub fn set_respect_gitignore(&mut self, respect_gitignore: bool) {
        if self.respect_gitignore == respect_gitignore {
            return;
        }
        self.respect_gitignore = respect_gitignore;
        self.epoch += 1;
        self.refresh();
    }
}

fn list_directory(path: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<DirEntryInfo> {
    let mut entries = if respect_gitignore {
        read_with_gitignore(path, show_hidden)
    } else {
        read_plain(path, show_hidden)
    };
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    entries
}

fn read_with_gitignore(path: &Path, show_hidden: bool) -> Vec<DirEntryInfo> {
    let mut builder = WalkBuilder::new(path);
    builder
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .follow_links(false);

    let mut entries = Vec::new();
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        if entry.path() == path {
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(DirEntryInfo {
            path: entry.into_path(),
            name,
            is_dir,
        });
        // A directory this big is shown truncated; do not read the rest.
        if entries.len() >= MAX_ENTRIES_PER_DIRECTORY {
            break;
        }
    }
    entries
}

fn read_plain(path: &Path, show_hidden: bool) -> Vec<DirEntryInfo> {
    let Ok(directory) = fs::read_dir(path) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for result in directory {
        let Ok(entry) = result else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        entries.push(DirEntryInfo {
            path: entry.path(),
            name,
            is_dir,
        });
        if entries.len() >= MAX_ENTRIES_PER_DIRECTORY {
            break;
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::create_dir(dir.path().join(".hidden-dir")).unwrap();
        fs::write(dir.path().join("README.md"), b"# Hi\n").unwrap();
        fs::write(dir.path().join("src/main.rs"), b"fn main() {}\n").unwrap();
        fs::write(dir.path().join("docs/guide.md"), b"# Guide\n").unwrap();
        fs::write(dir.path().join(".env"), b"SECRET=1\n").unwrap();
        dir
    }

    #[test]
    fn lists_directories_first_and_filters_hidden() {
        let dir = project();
        let tree = FileTree::new(dir.path(), false, false);
        let visible = tree.visible();
        let names: Vec<&str> = visible
            .iter()
            .map(|index| tree.node(*index).unwrap().name.as_str())
            .collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"README.md"));
        assert!(!names.contains(&".env"));
        let src = names.iter().position(|name| *name == "src").unwrap();
        let readme = names.iter().position(|name| *name == "README.md").unwrap();
        assert!(src < readme);
    }

    #[test]
    fn expands_and_collapses_lazily() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        assert_eq!(tree.node(src).unwrap().children.len(), 0);
        tree.expand(src);
        assert!(tree.is_expanded(src));
        assert_eq!(tree.node(src).unwrap().children.len(), 1);
        let visible_with_child = tree.visible();
        assert!(visible_with_child.len() > 4);
        tree.collapse(src);
        assert!(
            !tree
                .visible()
                .contains(&tree.node(src).unwrap().children[0])
        );
    }

    #[test]
    fn gitignore_is_respected() {
        let dir = project();
        let mut ignore = fs::File::create(dir.path().join(".gitignore")).unwrap();
        ignore.write_all(b"ignored.md\nsecret/\n").unwrap();
        fs::write(dir.path().join("ignored.md"), b"no\n").unwrap();
        fs::create_dir(dir.path().join("secret")).unwrap();
        fs::write(dir.path().join("secret/file.md"), b"no\n").unwrap();

        let tree = FileTree::new(dir.path(), false, true);
        let visible = tree.visible();
        let names: Vec<&str> = visible
            .iter()
            .map(|index| tree.node(*index).unwrap().name.as_str())
            .collect();
        assert!(!names.contains(&"ignored.md"));
        assert!(!names.contains(&"secret"));
        assert!(names.contains(&"README.md"));
    }

    #[test]
    fn hidden_toggle_rebuilds_tree() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let visible = tree.visible();
        assert!(
            !visible
                .iter()
                .any(|index| tree.node(*index).unwrap().name == ".env")
        );
        tree.set_show_hidden(true);
        let visible = tree.visible();
        assert!(
            visible
                .iter()
                .any(|index| tree.node(*index).unwrap().name == ".env")
        );
    }

    #[test]
    fn reveal_path_expands_ancestors() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let target = dir.path().join("docs/guide.md");
        let index = tree.reveal_path(&target).unwrap();
        assert_eq!(tree.node(index).unwrap().name, "guide.md");
        assert!(tree.visible().contains(&index));
    }

    #[test]
    fn refresh_keeps_expanded_directories() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        fs::write(dir.path().join("package.md"), b"new\n").unwrap();
        tree.refresh();
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        assert!(tree.is_expanded(src));
        assert!(tree.find_path(&dir.path().join("package.md")).is_some());
    }

    // ----------------------------------------------------- background listing

    const SLOW: Duration = Duration::from_millis(250);

    /// Poll until nothing is pending, failing the test after a few seconds.
    fn settle(tree: &mut FileTree) {
        let started = Instant::now();
        while tree.pending() > 0 {
            tree.poll();
            assert!(started.elapsed() < Duration::from_secs(10), "listing never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn names(tree: &FileTree) -> Vec<String> {
        tree.visible()
            .iter()
            .map(|index| tree.node(*index).unwrap().name.clone())
            .collect()
    }

    #[test]
    fn a_slow_root_listing_does_not_hold_up_construction() {
        let dir = project();
        let started = Instant::now();
        let mut tree = FileTree::new_slow(dir.path(), false, false, SLOW, Duration::ZERO);
        assert!(
            started.elapsed() < SLOW,
            "construction waited for the listing: {:?}",
            started.elapsed()
        );
        let root = tree.node(tree.root_index()).unwrap();
        assert!(!root.loaded && root.loading);
        assert_eq!(tree.visible().len(), 1, "only the root until the listing arrives");
        assert_eq!(tree.pending(), 1);
        assert!(tree.busy_since().is_some());

        settle(&mut tree);
        let root = tree.node(tree.root_index()).unwrap();
        assert!(root.loaded && !root.loading);
        assert!(names(&tree).contains(&"README.md".to_string()));
        assert!(tree.busy_since().is_none());
    }

    #[test]
    fn expanding_a_slow_directory_returns_at_once_and_fills_in_later() {
        let dir = project();
        let mut tree = FileTree::new_slow(dir.path(), false, false, SLOW, Duration::ZERO);
        settle(&mut tree);
        let src = tree.find_path(&dir.path().join("src")).unwrap();

        let started = Instant::now();
        tree.expand(src);
        assert!(started.elapsed() < SLOW);
        assert!(tree.is_expanded(src), "the folder opens right away");
        assert!(tree.node(src).unwrap().loading);
        assert!(tree.node(src).unwrap().children.is_empty());

        settle(&mut tree);
        let node = tree.node(src).unwrap();
        assert!(node.loaded && !node.loading);
        assert_eq!(node.children.len(), 1);
        assert!(names(&tree).contains(&"main.rs".to_string()));
    }

    #[test]
    fn a_generous_budget_makes_expansion_behave_synchronously() {
        let dir = project();
        let mut tree = FileTree::new_slow(
            dir.path(),
            false,
            false,
            Duration::from_millis(20),
            Duration::from_secs(5),
        );
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        assert_eq!(tree.node(src).unwrap().children.len(), 1);
        assert_eq!(tree.pending(), 0);
    }

    #[test]
    fn revealing_a_path_gives_up_politely_while_folders_load() {
        let dir = project();
        let mut tree = FileTree::new_slow(dir.path(), false, false, SLOW, Duration::ZERO);
        settle(&mut tree);
        let target = dir.path().join("docs/guide.md");

        assert!(tree.reveal_path(&target).is_none());
        assert!(tree.pending() > 0);
        settle(&mut tree);
        let index = tree.reveal_path(&target).expect("loaded by now");
        assert!(tree.visible().contains(&index));
    }

    #[test]
    fn relisting_keeps_surviving_nodes_and_forgets_removed_ones() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        let readme = tree.find_path(&dir.path().join("README.md")).unwrap();

        fs::remove_file(dir.path().join("README.md")).unwrap();
        fs::write(dir.path().join("added.md"), b"x\n").unwrap();
        tree.refresh_dirs(&[dir.path().to_path_buf()]);

        // The folder kept its node, its state and its children.
        assert_eq!(tree.find_path(&dir.path().join("src")), Some(src));
        assert!(tree.is_expanded(src));
        assert_eq!(tree.node(src).unwrap().children.len(), 1);
        assert!(tree.find_path(&dir.path().join("added.md")).is_some());
        assert!(tree.find_path(&dir.path().join("README.md")).is_none());
        assert!(!tree.visible().contains(&readme));
    }

    #[test]
    fn removing_an_expanded_folder_forgets_everything_below_it() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        assert!(tree.find_path(&dir.path().join("src/main.rs")).is_some());
        let before = tree.generation();

        fs::remove_dir_all(dir.path().join("src")).unwrap();
        tree.refresh_dirs(&[dir.path().to_path_buf()]);

        assert!(tree.find_path(&dir.path().join("src")).is_none());
        assert!(tree.find_path(&dir.path().join("src/main.rs")).is_none());
        assert!(tree.generation() > before, "watchers must be told");
        assert!(!tree.open_directories().contains(&dir.path().join("src")));
    }

    #[test]
    fn a_file_turning_into_a_directory_gets_a_fresh_node() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let path = dir.path().join("thing");
        fs::write(&path, b"x\n").unwrap();
        tree.refresh();
        assert!(!tree.node(tree.find_path(&path).unwrap()).unwrap().is_dir);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        tree.refresh();
        assert!(tree.node(tree.find_path(&path).unwrap()).unwrap().is_dir);
    }

    #[test]
    fn open_directories_lists_the_root_and_expanded_folders_only() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        assert_eq!(tree.open_directories(), vec![dir.path().to_path_buf()]);

        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        let mut open = tree.open_directories();
        open.sort();
        assert_eq!(open, vec![dir.path().to_path_buf(), dir.path().join("src")]);

        // Collapsed folders (and what is inside them) are not watched.
        tree.collapse(src);
        assert_eq!(tree.open_directories(), vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn expanding_and_collapsing_change_the_generation() {
        let dir = project();
        let mut tree = FileTree::new(dir.path(), false, false);
        let start = tree.generation();
        let src = tree.find_path(&dir.path().join("src")).unwrap();
        tree.expand(src);
        let opened = tree.generation();
        assert!(opened > start);
        tree.expand(src);
        assert_eq!(tree.generation(), opened, "no change, no new generation");
        tree.collapse(src);
        assert!(tree.generation() > opened);
    }

    #[test]
    fn toggling_hidden_files_while_a_listing_runs_ends_up_consistent() {
        let dir = project();
        let mut tree = FileTree::new_slow(dir.path(), false, false, SLOW, Duration::ZERO);
        settle(&mut tree);
        assert!(!names(&tree).contains(&".env".to_string()));

        // Flip twice while listings are in flight: the answers computed for
        // older settings must not win.
        tree.set_show_hidden(true);
        tree.set_show_hidden(false);
        tree.set_show_hidden(true);
        settle(&mut tree);
        assert!(names(&tree).contains(&".env".to_string()));
        assert!(names(&tree).contains(&".hidden-dir".to_string()));
    }

    #[test]
    fn asking_again_while_a_listing_runs_picks_up_later_changes() {
        let dir = project();
        let mut tree = FileTree::new_slow(dir.path(), false, false, SLOW, Duration::ZERO);
        settle(&mut tree);
        tree.refresh_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(tree.pending(), 1);
        // Same folder asked for again while the first listing runs.
        tree.refresh_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(tree.pending(), 1, "one listing in flight, not two");

        fs::write(dir.path().join("late.md"), b"x\n").unwrap();
        settle(&mut tree);
        assert!(tree.find_path(&dir.path().join("late.md")).is_some());
    }

    #[test]
    fn at_most_a_few_listings_run_at_once() {
        let dir = tempfile::tempdir().unwrap();
        for number in 0..24 {
            let sub = dir.path().join(format!("d{number:02}"));
            fs::create_dir(&sub).unwrap();
            fs::write(sub.join("f.md"), b"x\n").unwrap();
        }
        let mut tree = FileTree::new_slow(
            dir.path(),
            false,
            false,
            Duration::from_millis(60),
            Duration::ZERO,
        );
        settle(&mut tree);
        let indexes: Vec<usize> = (0..24)
            .map(|number| tree.find_path(&dir.path().join(format!("d{number:02}"))).unwrap())
            .collect();
        for index in &indexes {
            tree.expand(*index);
        }
        assert_eq!(tree.pending(), 24);
        assert!(tree.loader.running <= MAX_CONCURRENT_LISTINGS);
        assert!(!tree.loader.queue.is_empty(), "the rest wait their turn");

        settle(&mut tree);
        for index in indexes {
            assert_eq!(tree.node(index).unwrap().children.len(), 1);
        }
    }

    #[test]
    fn a_huge_directory_is_cut_short_instead_of_read_to_the_end() {
        let dir = tempfile::tempdir().unwrap();
        for number in 0..(MAX_ENTRIES_PER_DIRECTORY + 500) {
            fs::write(dir.path().join(format!("f{number}.txt")), b"").unwrap();
        }
        for gitignore in [false, true] {
            let tree = FileTree::new(dir.path(), false, gitignore);
            let children = tree.node(tree.root_index()).unwrap().children.len();
            assert_eq!(children, MAX_ENTRIES_PER_DIRECTORY, "gitignore={gitignore}");
        }
    }

    /// Manual benchmark: `cargo test --release bench_ -- --ignored --nocapture`.
    ///
    /// Builds (or reuses, with `RTUI_BENCH_DIR`) a big tree and prints how long
    /// each start-up step takes, including what the old recursive watcher cost.
    /// `RTUI_BENCH_DIRS` sets the number of directories to generate.
    #[test]
    #[ignore]
    fn bench_startup_on_a_large_tree() {
        use crate::filesystem::watcher::{FileWatcher, WatcherState};
        use notify::{RecommendedWatcher, RecursiveMode, Watcher};

        let generated;
        let root: PathBuf = match std::env::var_os("RTUI_BENCH_DIR") {
            Some(path) => PathBuf::from(path),
            None => {
                let wanted: usize = std::env::var("RTUI_BENCH_DIRS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(60_000);
                generated = tempfile::tempdir().unwrap();
                let base = generated.path();
                let per_branch = 40;
                for branch in 0..(wanted / (per_branch * 3)).max(1) {
                    for leaf in 0..per_branch {
                        let path = base.join(format!("pkg{branch}/node_modules/m{leaf}/lib"));
                        fs::create_dir_all(&path).unwrap();
                        fs::write(path.join("index.js"), b"").unwrap();
                    }
                }
                base.to_path_buf()
            }
        };
        let directories = ignore::WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(false)
            .build()
            .filter(|entry| entry.as_ref().is_ok_and(|e| e.path().is_dir()))
            .count();
        println!("tree: {} ({directories} directories)", root.display());

        // What the program does before its first draw.
        let started = Instant::now();
        let tree = FileTree::new(&root, false, true);
        println!(
            "FileTree::new (budget {:?}): {:?}, root has {} entries",
            LOAD_BUDGET,
            started.elapsed(),
            tree.node(tree.root_index()).unwrap().children.len()
        );

        let started = Instant::now();
        let watcher = FileWatcher::start();
        watcher.sync(tree.open_directories());
        let spawn = started.elapsed();
        while watcher.state() == WatcherState::Starting {
            std::thread::sleep(Duration::from_millis(1));
        }
        println!(
            "FileWatcher start+sync: caller blocked {:?}, ready after {:?}, state {:?}",
            spawn,
            started.elapsed(),
            watcher.state()
        );

        // What it used to do on the main thread: one recursive watch.
        let started = Instant::now();
        let mut recursive = RecommendedWatcher::new(|_| {}, notify::Config::default()).unwrap();
        let outcome = recursive.watch(&root, RecursiveMode::Recursive);
        println!(
            "old recursive watch of the whole tree: {:?} ({})",
            started.elapsed(),
            if outcome.is_ok() { "ok".to_string() } else { format!("{outcome:?}") }
        );
    }
}

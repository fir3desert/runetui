//! Cursor and viewport state for the project explorer.
//!
//! [`Explorer`] wraps a [`FileTree`] with a flattened visible list, a
//! selection, a scroll offset and a preferred pane width.  All movement keys
//! operate on the flattened list so behaviour matches a Neovim file tree.

use std::path::{Path, PathBuf};

use super::tree::{FileNode, FileTree};

/// Smallest and largest explorer width as a percentage of the terminal.
pub const MIN_WIDTH_PCT: u16 = 14;
pub const MAX_WIDTH_PCT: u16 = 60;
pub const DEFAULT_WIDTH_PCT: u16 = 28;

#[derive(Debug)]
pub struct Explorer {
    pub tree: FileTree,
    visible: Vec<usize>,
    selected: usize,
    offset: usize,
    width_pct: u16,
    viewport_height: usize,
    /// A file to reveal as soon as the folders on its way finish loading.
    reveal_pending: Option<PathBuf>,
}

impl Explorer {
    pub fn new(root: &Path, width_pct: u16, show_hidden: bool, respect_gitignore: bool) -> Self {
        let mut explorer = Self {
            tree: FileTree::new(root, show_hidden, respect_gitignore),
            visible: Vec::new(),
            selected: 0,
            offset: 0,
            width_pct: width_pct.clamp(MIN_WIDTH_PCT, MAX_WIDTH_PCT),
            viewport_height: 20,
            reveal_pending: None,
        };
        explorer.rebuild_visible();
        explorer
    }

    /// An explorer over an existing tree (to test slow listings).
    #[cfg(test)]
    pub fn from_tree(tree: FileTree) -> Self {
        let mut explorer = Self {
            tree,
            visible: Vec::new(),
            selected: 0,
            offset: 0,
            width_pct: DEFAULT_WIDTH_PCT,
            viewport_height: 20,
            reveal_pending: None,
        };
        explorer.rebuild_visible();
        explorer
    }

    // ------------------------------------------------------------- accessors

    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    #[cfg(test)]
    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn selected_position(&self) -> usize {
        self.selected
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn width_pct(&self) -> u16 {
        self.width_pct
    }

    pub fn show_hidden(&self) -> bool {
        self.tree.show_hidden
    }

    pub fn respect_gitignore(&self) -> bool {
        self.tree.respect_gitignore
    }

    pub fn root_name(&self) -> String {
        self.tree
            .node(self.tree.root_index())
            .map(|node| node.name.clone())
            .unwrap_or_else(|| self.tree.root_path().display().to_string())
    }

    pub fn selected_node(&self) -> Option<&FileNode> {
        let index = *self.visible.get(self.selected)?;
        self.tree.node(index)
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected_node().map(|node| node.path.clone())
    }

    // -------------------------------------------------------------- lifecycle

    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
        self.ensure_visible();
    }

    pub fn rebuild_visible(&mut self) {
        let previous = self.selected_path();
        self.visible = self.tree.visible();
        if self.visible.is_empty() {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected = previous
            .and_then(|path| {
                self.visible.iter().position(|index| {
                    self.tree
                        .node(*index)
                        .map(|node| node.path == path)
                        .unwrap_or(false)
                })
            })
            .unwrap_or_else(|| self.selected.min(self.visible.len() - 1));
        self.ensure_visible();
    }

    /// List every folder that has been read again.  Slow folders finish in the
    /// background; see [`Explorer::poll`].
    pub fn refresh(&mut self) {
        let selected_path = self.selected_path();
        self.tree.refresh();
        self.rebuild_visible();
        if let Some(path) = selected_path {
            self.select_path(&path);
        }
    }

    /// List only the folders in `dirs` again (the ones a filesystem event
    /// touched).
    pub fn refresh_dirs(&mut self, dirs: &[PathBuf]) {
        self.tree.refresh_dirs(dirs);
        self.rebuild_visible();
    }

    /// Apply background listings that have finished.  Returns `true` when the
    /// tree changed.
    pub fn poll(&mut self) -> bool {
        if !self.tree.poll() {
            return false;
        }
        self.rebuild_visible();
        if let Some(path) = self.reveal_pending.clone() {
            if self.select_path(&path) || self.tree.pending() == 0 {
                self.reveal_pending = None;
            }
        }
        true
    }

    /// When the current background work started.
    pub fn busy_since(&self) -> Option<std::time::Instant> {
        self.tree.busy_since()
    }

    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        if self.tree.show_hidden == show_hidden {
            return;
        }
        let selected_path = self.selected_path();
        self.tree.set_show_hidden(show_hidden);
        self.rebuild_visible();
        if let Some(path) = selected_path {
            self.select_path(&path);
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.set_show_hidden(!self.tree.show_hidden);
    }

    pub fn set_respect_gitignore(&mut self, respect_gitignore: bool) {
        if self.tree.respect_gitignore == respect_gitignore {
            return;
        }
        let selected_path = self.selected_path();
        self.tree.set_respect_gitignore(respect_gitignore);
        self.rebuild_visible();
        if let Some(path) = selected_path {
            self.select_path(&path);
        }
    }

    pub fn toggle_gitignore(&mut self) {
        self.set_respect_gitignore(!self.tree.respect_gitignore);
    }

    // -------------------------------------------------------------- selection

    pub fn select_position(&mut self, position: usize) {
        self.reveal_pending = None;
        if self.visible.is_empty() {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected = position.min(self.visible.len() - 1);
        self.ensure_visible();
    }

    /// Select `path`, expanding its ancestors when needed.
    pub fn select_path(&mut self, path: &Path) -> bool {
        let Some(index) = self.tree.reveal_path(path) else {
            // A folder on the way is still loading: finish when it arrives.
            self.reveal_pending = (self.tree.pending() > 0).then(|| path.to_path_buf());
            self.rebuild_visible();
            return false;
        };
        self.reveal_pending = None;
        self.visible = self.tree.visible();
        match self
            .visible
            .iter()
            .position(|candidate| *candidate == index)
        {
            Some(position) => {
                self.selected = position;
                self.ensure_visible();
                true
            }
            None => false,
        }
    }

    /// Select the node that owns `path` without expanding it, if visible.
    pub fn select_visible_path(&mut self, path: &Path) -> bool {
        let Some(position) = self.visible.iter().position(|index| {
            self.tree
                .node(*index)
                .map(|node| node.path == path)
                .unwrap_or(false)
        }) else {
            return false;
        };
        self.selected = position;
        self.ensure_visible();
        true
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.reveal_pending = None;
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        self.ensure_visible();
    }

    pub fn move_page(&mut self, down: bool) {
        let delta = (self.viewport_height.saturating_sub(2).max(1)) as isize;
        self.move_selection(if down { delta } else { -delta });
    }

    pub fn go_top(&mut self) {
        self.select_position(0);
    }

    pub fn go_bottom(&mut self) {
        if !self.visible.is_empty() {
            self.select_position(self.visible.len() - 1);
        }
    }

    fn ensure_visible(&mut self) {
        if self.visible.is_empty() {
            self.offset = 0;
            return;
        }
        let height = self.viewport_height.max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
        let max_offset = self.visible.len().saturating_sub(height);
        self.offset = self.offset.min(max_offset);
    }

    // ------------------------------------------------------------ activation

    /// Toggle a directory open/closed.  Returns `true` when the tree changed.
    pub fn toggle_selected(&mut self) -> bool {
        let Some(index) = self.selected_index() else {
            return false;
        };
        if !self
            .tree
            .node(index)
            .map(|node| node.is_dir)
            .unwrap_or(false)
        {
            return false;
        }
        if self.tree.is_expanded(index) {
            self.tree.collapse(index);
        } else {
            self.tree.expand(index);
        }
        self.rebuild_visible();
        self.select_visible_index(index);
        true
    }

    /// `l` / right: expand a collapsed directory, or descend into it.
    pub fn expand_or_descend(&mut self) -> bool {
        let Some(index) = self.selected_index() else {
            return false;
        };
        let Some(node) = self.tree.node(index) else {
            return false;
        };
        if !node.is_dir {
            return false;
        }
        if !node.expanded {
            self.tree.expand(index);
            self.rebuild_visible();
            self.select_visible_index(index);
            return true;
        }
        if let Some(&child) = self.tree.node(index).and_then(|node| node.children.first()) {
            self.select_visible_index(child);
        }
        true
    }

    /// `h` / left: collapse the selected directory, otherwise jump to the
    /// parent directory.  The root never moves.
    pub fn collapse_or_parent(&mut self) -> bool {
        let Some(index) = self.selected_index() else {
            return false;
        };
        if index == self.tree.root_index() {
            return false;
        }
        let is_dir = self
            .tree
            .node(index)
            .map(|node| node.is_dir)
            .unwrap_or(false);
        if is_dir && self.tree.is_expanded(index) {
            self.tree.collapse(index);
            self.rebuild_visible();
            self.select_visible_index(index);
            return true;
        }
        let Some(parent) = self
            .tree
            .node(index)
            .map(|node| node.path.parent().map(Path::to_path_buf))
        else {
            return false;
        };
        let Some(parent) = parent else {
            return false;
        };
        if self.select_visible_path(&parent) {
            return true;
        }
        // Parent may still be collapsed in the visible list (only possible for
        // the root, which is always expanded), so fall back to revealing it.
        self.select_path(&parent)
    }

    /// Enter activates the selection.  Files return their path; directories
    /// are toggled and return `None`.
    pub fn activate_selected(&mut self) -> Option<PathBuf> {
        let Some(index) = self.selected_index() else {
            return None;
        };
        let path = self.tree.node(index)?.path.clone();
        let is_dir = self
            .tree
            .node(index)
            .map(|node| node.is_dir)
            .unwrap_or(false);
        if is_dir {
            self.toggle_selected();
            None
        } else {
            Some(path)
        }
    }

    fn select_visible_index(&mut self, node_index: usize) {
        if let Some(position) = self.visible.iter().position(|index| *index == node_index) {
            self.selected = position;
            self.ensure_visible();
        }
    }

    // ----------------------------------------------------------------- width

    pub fn set_width_pct(&mut self, width_pct: u16) {
        self.width_pct = width_pct.clamp(MIN_WIDTH_PCT, MAX_WIDTH_PCT);
    }

    pub fn resize(&mut self, delta: i16) {
        let next = (self.width_pct as i16 + delta).clamp(MIN_WIDTH_PCT as i16, MAX_WIDTH_PCT as i16)
            as u16;
        self.width_pct = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("README.md"), b"# Hi\n").unwrap();
        fs::write(dir.path().join("docs/guide.md"), b"# Guide\n").unwrap();
        fs::write(dir.path().join("src/main.rs"), b"fn main() {}\n").unwrap();
        dir
    }

    fn explorer() -> (tempfile::TempDir, Explorer) {
        let dir = project();
        let explorer = Explorer::new(dir.path(), DEFAULT_WIDTH_PCT, false, false);
        (dir, explorer)
    }

    fn name_of(explorer: &Explorer) -> String {
        explorer
            .selected_node()
            .map(|node| node.name.clone())
            .unwrap_or_default()
    }

    #[test]
    fn moves_and_clamps_selection() {
        let (_dir, mut explorer) = explorer();
        explorer.set_viewport_height(3);
        explorer.go_top();
        assert_eq!(name_of(&explorer), explorer.root_name());
        explorer.move_selection(-5);
        assert_eq!(explorer.selected_position(), 0);
        explorer.go_bottom();
        assert_eq!(
            explorer.selected_position(),
            explorer.visible_len().saturating_sub(1)
        );
        explorer.move_selection(50);
        assert_eq!(
            explorer.selected_position(),
            explorer.visible_len().saturating_sub(1)
        );
    }

    #[test]
    fn toggles_directories_and_tracks_selection() {
        let (dir, mut explorer) = explorer();
        let docs = dir.path().join("docs");
        assert!(explorer.select_visible_path(&docs));
        assert!(explorer.toggle_selected());
        assert!(
            explorer
                .visible()
                .iter()
                .any(|index| explorer.tree.node(*index).unwrap().name == "guide.md")
        );
        // Collapsing again hides the child.
        assert!(explorer.select_visible_path(&docs));
        assert!(explorer.toggle_selected());
        assert!(
            !explorer
                .visible()
                .iter()
                .any(|index| explorer.tree.node(*index).unwrap().name == "guide.md")
        );
    }

    #[test]
    fn files_activate_and_directories_do_not() {
        let (dir, mut explorer) = explorer();
        let readme = dir.path().join("README.md");
        assert!(explorer.select_visible_path(&readme));
        assert_eq!(explorer.activate_selected(), Some(readme));

        let docs = dir.path().join("docs");
        assert!(explorer.select_visible_path(&docs));
        assert_eq!(explorer.activate_selected(), None);
        assert!(
            explorer
                .tree
                .is_expanded(explorer.selected_index().unwrap())
        );
    }

    #[test]
    fn left_collapses_then_moves_to_parent() {
        let (dir, mut explorer) = explorer();
        let docs = dir.path().join("docs");
        explorer.select_visible_path(&docs);
        explorer.expand_or_descend();
        let guide = dir.path().join("docs/guide.md");
        assert!(explorer.select_visible_path(&guide));
        // First `h` on a child file moves to its parent, still expanded.
        assert!(explorer.collapse_or_parent());
        assert_eq!(explorer.selected_path().unwrap(), docs);
        assert!(
            explorer
                .tree
                .is_expanded(explorer.selected_index().unwrap())
        );
        // Second `h` collapses the directory.
        assert!(explorer.collapse_or_parent());
        assert!(
            !explorer
                .tree
                .is_expanded(explorer.selected_index().unwrap())
        );
        // Third `h` moves to the root.
        assert!(explorer.collapse_or_parent());
        assert_eq!(explorer.selected_path().unwrap(), dir.path());
    }

    #[test]
    fn selection_survives_refresh_and_hidden_toggle() {
        let (dir, mut explorer) = explorer();
        let docs = dir.path().join("docs");
        explorer.select_visible_path(&docs);
        explorer.refresh();
        assert_eq!(explorer.selected_path().unwrap(), docs);
        explorer.toggle_hidden();
        assert!(explorer.show_hidden());
        explorer.toggle_hidden();
        assert!(!explorer.show_hidden());
        assert_eq!(explorer.selected_path().unwrap(), docs);
    }

    #[test]
    fn select_path_expands_ancestors() {
        let (dir, mut explorer) = explorer();
        let guide = dir.path().join("docs/guide.md");
        assert!(explorer.select_path(&guide));
        assert_eq!(explorer.selected_path().unwrap(), guide);
        assert!(
            explorer
                .visible()
                .contains(&explorer.selected_index().unwrap())
        );
    }

    #[test]
    fn width_is_clamped() {
        let (_dir, mut explorer) = explorer();
        explorer.resize(-1000);
        assert_eq!(explorer.width_pct(), MIN_WIDTH_PCT);
        explorer.resize(1000);
        assert_eq!(explorer.width_pct(), MAX_WIDTH_PCT);
        explorer.set_width_pct(1);
        assert_eq!(explorer.width_pct(), MIN_WIDTH_PCT);
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..40 {
            fs::write(dir.path().join(format!("file-{index:02}.md")), b"x\n").unwrap();
        }
        let mut explorer = Explorer::new(dir.path(), DEFAULT_WIDTH_PCT, false, false);
        explorer.set_viewport_height(5);
        explorer.go_bottom();
        assert!(explorer.selected_position() >= explorer.offset());
        assert!(explorer.selected_position() < explorer.offset() + 5);
        explorer.go_top();
        assert_eq!(explorer.offset(), 0);
    }

    fn settle(explorer: &mut Explorer, done: impl Fn(&Explorer) -> bool) {
        let started = std::time::Instant::now();
        while !done(explorer) {
            explorer.poll();
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "never settled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_slow_root_leaves_a_usable_explorer_and_selects_the_file_when_it_arrives() {
        let dir = project();
        let tree = FileTree::new_slow(
            dir.path(),
            false,
            false,
            std::time::Duration::from_millis(120),
            std::time::Duration::ZERO,
        );
        let mut explorer = Explorer::from_tree(tree);
        assert_eq!(explorer.visible_len(), 1, "just the root for now");

        let target = dir.path().join("docs/guide.md");
        assert!(!explorer.select_path(&target), "cannot be selected yet");
        assert!(explorer.reveal_pending.is_some());

        // Each arriving folder moves the reveal one level further.
        settle(&mut explorer, |explorer| explorer.selected_path().as_deref() == Some(target.as_path()));
        assert!(explorer.reveal_pending.is_none());
        assert_eq!(explorer.tree.pending(), 0);
    }

    #[test]
    fn moving_the_selection_cancels_a_pending_reveal() {
        let dir = project();
        let tree = FileTree::new_slow(
            dir.path(),
            false,
            false,
            std::time::Duration::from_millis(120),
            std::time::Duration::ZERO,
        );
        let mut explorer = Explorer::from_tree(tree);
        explorer.select_path(&dir.path().join("docs/guide.md"));
        assert!(explorer.reveal_pending.is_some());
        explorer.move_selection(1);
        assert!(explorer.reveal_pending.is_none());
        settle(&mut explorer, |explorer| explorer.tree.pending() == 0);
        // The user's own movement was not overridden.
        assert_ne!(explorer.selected_path().as_deref(), Some(dir.path().join("docs/guide.md").as_path()));
    }

    #[test]
    fn refreshing_only_the_named_folders_keeps_the_selection() {
        let (dir, mut explorer) = explorer();
        explorer.select_path(&dir.path().join("docs/guide.md"));
        fs::write(dir.path().join("docs/new.md"), b"x\n").unwrap();
        explorer.refresh_dirs(&[dir.path().join("docs")]);
        assert_eq!(
            explorer.selected_path().as_deref(),
            Some(dir.path().join("docs/guide.md").as_path())
        );
        assert!(explorer.tree.find_path(&dir.path().join("docs/new.md")).is_some());
    }
}

//! Lazy directory tree stored in an arena.
//!
//! Only directories the user expands are read from disk, so opening a huge
//! repository stays instant.  `.gitignore` / `.ignore` rules are honoured by
//! default through the `ignore` crate.

use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

const MAX_ENTRIES_PER_DIRECTORY: usize = 10_000;

#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    pub loaded: bool,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
struct DirEntryInfo {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct FileTree {
    nodes: Vec<FileNode>,
    root: usize,
    pub show_hidden: bool,
    pub respect_gitignore: bool,
}

impl FileTree {
    pub fn new(root: &Path, show_hidden: bool, respect_gitignore: bool) -> Self {
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
            children: Vec::new(),
        };
        let mut tree = Self {
            nodes: vec![root_node],
            root: 0,
            show_hidden,
            respect_gitignore,
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

    pub fn load_children(&mut self, index: usize) {
        let Some(node) = self.nodes.get(index) else {
            return;
        };
        if !node.is_dir || node.loaded {
            return;
        }

        let directories = list_directory(&node.path, self.show_hidden, self.respect_gitignore);
        let parent_depth = node.depth;
        let mut children = Vec::with_capacity(directories.len());

        for entry in directories {
            let child = FileNode {
                path: entry.path,
                name: entry.name,
                is_dir: entry.is_dir,
                depth: parent_depth + 1,
                expanded: false,
                loaded: false,
                children: Vec::new(),
            };
            children.push(self.nodes.len());
            self.nodes.push(child);
        }

        if let Some(node) = self.nodes.get_mut(index) {
            node.loaded = true;
            node.children = children;
        }
    }

    /// Depth-first list of visible nodes.
    pub fn visible(&self) -> Vec<usize> {
        let mut result = Vec::with_capacity(self.nodes.len());
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

    pub fn expand(&mut self, index: usize) {
        self.load_children(index);
        if let Some(node) = self.nodes.get_mut(index) {
            if node.is_dir {
                node.expanded = true;
            }
        }
    }

    pub fn collapse(&mut self, index: usize) {
        if let Some(node) = self.nodes.get_mut(index) {
            if node.is_dir {
                node.expanded = false;
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
        self.nodes.iter().position(|node| node.path == path)
    }

    /// Expand every ancestor of `path` and return its node index.
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

        self.expand(self.root);
        for ancestor in ancestors {
            if let Some(index) = self.find_path(&ancestor) {
                self.expand(index);
            }
        }

        self.find_path(path)
    }

    /// Rebuild the tree from disk while keeping expanded/loaded directories.
    pub fn refresh(&mut self) {
        let root = self.root_path().to_path_buf();
        let mut expanded = Vec::new();
        let mut loaded = Vec::new();
        for node in &self.nodes {
            if node.is_dir {
                if node.expanded {
                    expanded.push(node.path.clone());
                }
                if node.loaded {
                    loaded.push(node.path.clone());
                }
            }
        }

        let show_hidden = self.show_hidden;
        let respect_gitignore = self.respect_gitignore;
        *self = FileTree::new(&root, show_hidden, respect_gitignore);

        expanded.sort_by_key(|path| path.components().count());
        loaded.sort_by_key(|path| path.components().count());

        for path in expanded {
            if let Some(index) = self.find_path(&path) {
                self.expand(index);
            }
        }
        for path in loaded {
            if let Some(index) = self.find_path(&path) {
                self.load_children(index);
            }
        }
    }

    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        if self.show_hidden == show_hidden {
            return;
        }
        self.show_hidden = show_hidden;
        self.refresh();
    }

    pub fn set_respect_gitignore(&mut self, respect_gitignore: bool) {
        if self.respect_gitignore == respect_gitignore {
            return;
        }
        self.respect_gitignore = respect_gitignore;
        self.refresh();
    }
}

fn list_directory(path: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<DirEntryInfo> {
    let mut entries = if respect_gitignore {
        read_with_gitignore(path, show_hidden)
    } else {
        read_plain(path, show_hidden)
    };
    entries.truncate(MAX_ENTRIES_PER_DIRECTORY);
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
}

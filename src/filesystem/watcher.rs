//! Filesystem notifications.
//!
//! The reader only needs to know that *something* changed so it can reload the
//! tree and the open document.  Raw `notify` events are filtered (no `.git`
//! noise) and forwarded through a channel; the app debounces them so a single
//! editor save does not cause a refresh storm.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub paths: Vec<PathBuf>,
}

impl WatchEvent {
    /// Does this event touch `path` (or one of its ancestors)?
    pub fn touches(&self, path: &Path) -> bool {
        self.paths
            .iter()
            .any(|candidate| candidate == path || candidate.starts_with(path))
    }
}

pub struct FileWatcher {
    // Kept alive for as long as the app runs; dropping it stops watching.
    _watcher: RecommendedWatcher,
    receiver: Receiver<WatchEvent>,
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher").finish_non_exhaustive()
    }
}

impl FileWatcher {
    pub fn new(root: &Path) -> Result<Self> {
        let (sender, receiver) = channel();
        let handler_sender: Sender<WatchEvent> = sender;
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                let Ok(event) = result else {
                    return;
                };
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                let paths: Vec<PathBuf> = event
                    .paths
                    .into_iter()
                    .filter(|path| !path.components().any(|part| part.as_os_str() == ".git"))
                    .collect();
                if paths.is_empty() {
                    return;
                }
                // A closed receiver simply means the app is shutting down.
                let _ = handler_sender.send(WatchEvent { paths });
            },
            notify::Config::default(),
        )
        .context("cannot start filesystem watcher")?;

        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("cannot watch {}", root.display()))?;

        Ok(Self {
            _watcher: watcher,
            receiver,
        })
    }

    /// Drain all queued events into one combined event.
    pub fn drain(&self) -> Option<WatchEvent> {
        let mut paths = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            paths.extend(event.paths);
        }
        if paths.is_empty() {
            None
        } else {
            paths.sort();
            paths.dedup();
            Some(WatchEvent { paths })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_matching_understands_ancestors() {
        let event = WatchEvent {
            paths: vec![PathBuf::from("/tmp/project/docs/guide.md")],
        };
        assert!(event.touches(Path::new("/tmp/project/docs/guide.md")));
        assert!(event.touches(Path::new("/tmp/project")));
        assert!(!event.touches(Path::new("/tmp/other")));
    }

    #[test]
    fn watcher_reports_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::new(dir.path()).unwrap();
        std::fs::write(dir.path().join("new.md"), b"# New\n").unwrap();

        let mut seen = false;
        for _ in 0..40 {
            if let Some(event) = watcher.drain() {
                if event.touches(&dir.path().join("new.md")) {
                    seen = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(seen, "watcher did not report the new file");
    }
}

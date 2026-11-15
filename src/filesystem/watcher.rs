//! Filesystem notifications.
//!
//! The reader only needs to know that *something* changed so it can reload the
//! tree and the open document.  Raw `notify` events are filtered (no `.git`
//! noise) and forwarded through a channel; the app debounces them so a single
//! editor save does not cause a refresh storm.
//!
//! The watcher never walks the project.  It runs on its own thread and watches
//! only the directories it is told about (the folders that are open in the
//! explorer and the folder of the open document), each non-recursively, so
//! starting it costs the same in a scratch folder and in a home directory with
//! millions of files, and the interface does not wait for it.  If the system
//! refuses (no inotify instance, watch limit reached) the program keeps
//! working and [`FileWatcher::state`] says what happened.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::trace;

/// Most directories watched at once; beyond this the watcher reports itself
/// as limited instead of eating the system's inotify budget.
const MAX_WATCHED_DIRECTORIES: usize = 2048;

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub paths: Vec<PathBuf>,
    /// Directories whose listing changed (an entry was created, removed or
    /// renamed in them).  Plain content writes do not appear here.
    pub dirs: Vec<PathBuf>,
}

impl WatchEvent {
    /// Does this event touch `path` (or one of its ancestors)?
    pub fn touches(&self, path: &Path) -> bool {
        self.paths
            .iter()
            .any(|candidate| candidate == path || candidate.starts_with(path))
    }
}

/// What the background watcher is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatcherState {
    /// The thread has not finished its first round of watches.
    Starting,
    /// Every requested directory is watched.
    Ready { dirs: usize },
    /// Some directories could not be watched; the rest are.
    Limited { dirs: usize, reason: String },
    /// Nothing can be watched: live refresh is off.
    Failed(String),
}

enum Command {
    Sync(Vec<PathBuf>),
    Stop,
}

pub struct FileWatcher {
    commands: Sender<Command>,
    receiver: Receiver<WatchEvent>,
    state: Arc<Mutex<WatcherState>>,
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher").finish_non_exhaustive()
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

impl FileWatcher {
    /// Start the watcher thread.  Returns at once; nothing is watched until
    /// [`FileWatcher::sync`] names directories.
    pub fn start() -> Self {
        let (command_sender, command_receiver) = channel();
        let (event_sender, event_receiver) = channel();
        let state = Arc::new(Mutex::new(WatcherState::Starting));

        let worker_state = Arc::clone(&state);
        let spawned = thread::Builder::new()
            .name("rtui-watcher".into())
            .spawn(move || worker(command_receiver, event_sender, worker_state));
        if let Err(error) = spawned {
            set_state(
                &state,
                WatcherState::Failed(format!("cannot start the watcher thread: {error}")),
            );
        }

        Self {
            commands: command_sender,
            receiver: event_receiver,
            state,
        }
    }

    /// A watcher that is already out of order, for tests of the fallback.
    #[cfg(test)]
    pub fn failed(reason: &str) -> Self {
        let (commands, _unused) = channel();
        let (_events, receiver) = channel();
        Self {
            commands,
            receiver,
            state: Arc::new(Mutex::new(WatcherState::Failed(reason.to_string()))),
        }
    }

    /// Watch exactly these directories (non-recursively), dropping any other.
    pub fn sync(&self, dirs: Vec<PathBuf>) {
        let _ = self.commands.send(Command::Sync(dirs));
    }

    pub fn state(&self) -> WatcherState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Drain all queued events into one combined event.
    pub fn drain(&self) -> Option<WatchEvent> {
        let mut paths = Vec::new();
        let mut dirs = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            paths.extend(event.paths);
            dirs.extend(event.dirs);
        }
        if paths.is_empty() {
            None
        } else {
            paths.sort();
            paths.dedup();
            dirs.sort();
            dirs.dedup();
            Some(WatchEvent { paths, dirs })
        }
    }
}

fn set_state(state: &Mutex<WatcherState>, value: WatcherState) {
    match state.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

fn worker(commands: Receiver<Command>, events: Sender<WatchEvent>, state: Arc<Mutex<WatcherState>>) {
    let started = Instant::now();
    let handler_events = events;
    let created = RecommendedWatcher::new(
        move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let listing_changed = !matches!(
                event.kind,
                EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_))
            );
            let paths: Vec<PathBuf> = event
                .paths
                .into_iter()
                .filter(|path| !path.components().any(|part| part.as_os_str() == ".git"))
                .collect();
            if paths.is_empty() {
                return;
            }
            let dirs = if listing_changed {
                paths
                    .iter()
                    .filter_map(|path| path.parent().map(Path::to_path_buf))
                    .collect()
            } else {
                Vec::new()
            };
            // A closed receiver simply means the app is shutting down.
            let _ = handler_events.send(WatchEvent { paths, dirs });
        },
        notify::Config::default(),
    );
    let mut watcher = match created {
        Ok(watcher) => watcher,
        Err(error) => {
            trace::mark(&format!("watcher: cannot start: {error}"));
            set_state(
                &state,
                WatcherState::Failed(format!("cannot start the filesystem watcher: {error}")),
            );
            return;
        }
    };
    trace::mark(&format!(
        "watcher: created in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    ));

    let mut active: HashSet<PathBuf> = HashSet::new();
    while let Ok(command) = commands.recv() {
        let mut wanted = match command {
            Command::Sync(dirs) => dirs,
            Command::Stop => break,
        };
        // Only the latest request matters.
        let mut stop = false;
        while let Ok(next) = commands.try_recv() {
            match next {
                Command::Sync(dirs) => wanted = dirs,
                Command::Stop => stop = true,
            }
        }
        if stop {
            break;
        }

        let round = Instant::now();
        let wanted: HashSet<PathBuf> = wanted.into_iter().collect();
        for path in active.difference(&wanted).cloned().collect::<Vec<_>>() {
            let _ = watcher.unwatch(&path);
            active.remove(&path);
        }

        let mut problem: Option<String> = None;
        let mut to_add: Vec<PathBuf> = wanted.difference(&active).cloned().collect();
        to_add.sort_by_key(|path| path.components().count());
        for path in to_add {
            if active.len() >= MAX_WATCHED_DIRECTORIES {
                problem.get_or_insert_with(|| {
                    format!("watching only {MAX_WATCHED_DIRECTORIES} folders")
                });
                break;
            }
            match watcher.watch(&path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    active.insert(path);
                }
                Err(error) => match &error.kind {
                    // The folder is already gone: the next listing says so.
                    notify::ErrorKind::PathNotFound => {}
                    notify::ErrorKind::MaxFilesWatch => {
                        problem.get_or_insert_with(|| {
                            "the system's file-watch limit was reached".to_string()
                        });
                        break;
                    }
                    _ => {
                        problem.get_or_insert_with(|| error.to_string());
                    }
                },
            }
        }

        let next_state = match problem {
            None => WatcherState::Ready { dirs: active.len() },
            Some(reason) if active.is_empty() => WatcherState::Failed(reason),
            Some(reason) => WatcherState::Limited {
                dirs: active.len(),
                reason,
            },
        };
        trace::mark(&format!(
            "watcher: {} folders watched, {:?} (round took {:.1} ms)",
            active.len(),
            next_state,
            round.elapsed().as_secs_f64() * 1000.0
        ));
        set_state(&state, next_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait_for_state(watcher: &FileWatcher, done: impl Fn(&WatcherState) -> bool) -> WatcherState {
        for _ in 0..200 {
            let state = watcher.state();
            if done(&state) {
                return state;
            }
            thread::sleep(Duration::from_millis(10));
        }
        watcher.state()
    }

    fn wait_for_event(watcher: &FileWatcher, path: &Path) -> Option<WatchEvent> {
        for _ in 0..80 {
            if let Some(event) = watcher.drain() {
                if event.touches(path) {
                    return Some(event);
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        None
    }

    #[test]
    fn event_matching_understands_ancestors() {
        let event = WatchEvent {
            paths: vec![PathBuf::from("/tmp/project/docs/guide.md")],
            dirs: Vec::new(),
        };
        assert!(event.touches(Path::new("/tmp/project/docs/guide.md")));
        assert!(event.touches(Path::new("/tmp/project")));
        assert!(!event.touches(Path::new("/tmp/other")));
    }

    #[test]
    fn starting_the_watcher_does_not_block_and_reports_starting() {
        let started = Instant::now();
        let watcher = FileWatcher::start();
        assert!(started.elapsed() < Duration::from_millis(500));
        // Nothing is requested yet, so it stays in its initial state.
        assert_eq!(watcher.state(), WatcherState::Starting);
    }

    #[test]
    fn watcher_reports_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::start();
        watcher.sync(vec![dir.path().to_path_buf()]);
        let state = wait_for_state(&watcher, |state| *state != WatcherState::Starting);
        assert_eq!(state, WatcherState::Ready { dirs: 1 });

        std::fs::write(dir.path().join("new.md"), b"# New\n").unwrap();
        let event = wait_for_event(&watcher, &dir.path().join("new.md"))
            .expect("watcher did not report the new file");
        assert!(event.dirs.contains(&dir.path().to_path_buf()));
    }

    #[test]
    fn watching_is_not_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        let watcher = FileWatcher::start();
        watcher.sync(vec![dir.path().to_path_buf()]);
        wait_for_state(&watcher, |state| *state != WatcherState::Starting);

        // A change two levels down is invisible: only the named directory is
        // watched, which is what keeps a home directory affordable.
        std::fs::write(deep.join("deep.md"), b"x\n").unwrap();
        thread::sleep(Duration::from_millis(300));
        assert!(watcher.drain().is_none());
    }

    #[test]
    fn sync_adds_and_removes_directories() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::start();
        watcher.sync(vec![first.path().to_path_buf()]);
        wait_for_state(&watcher, |state| *state != WatcherState::Starting);

        watcher.sync(vec![second.path().to_path_buf()]);
        wait_for_state(&watcher, |state| {
            matches!(state, WatcherState::Ready { dirs: 1 })
        });
        // Give the worker a moment to finish unwatching the first directory.
        thread::sleep(Duration::from_millis(100));
        let _ = watcher.drain();

        std::fs::write(first.path().join("ignored.md"), b"x\n").unwrap();
        std::fs::write(second.path().join("seen.md"), b"x\n").unwrap();
        let event = wait_for_event(&watcher, &second.path().join("seen.md"))
            .expect("the new directory is not watched");
        assert!(!event.touches(&first.path().join("ignored.md")));
    }

    #[test]
    fn a_missing_directory_is_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::start();
        watcher.sync(vec![
            dir.path().to_path_buf(),
            dir.path().join("does-not-exist"),
        ]);
        let state = wait_for_state(&watcher, |state| *state != WatcherState::Starting);
        assert_eq!(state, WatcherState::Ready { dirs: 1 });
    }

    #[test]
    fn content_writes_do_not_mark_a_listing_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log.txt");
        std::fs::write(&file, b"one\n").unwrap();
        let watcher = FileWatcher::start();
        watcher.sync(vec![dir.path().to_path_buf()]);
        wait_for_state(&watcher, |state| *state != WatcherState::Starting);
        let _ = watcher.drain();

        use std::io::Write;
        let mut handle = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        handle.write_all(b"two\n").unwrap();
        handle.sync_all().unwrap();
        let event = wait_for_event(&watcher, &file).expect("write not reported");
        assert!(event.touches(&file));
        assert!(event.dirs.is_empty(), "an append is not a listing change");
    }
}

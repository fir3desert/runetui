//! GitHub star count of the runetui repository, shown in the Esc menu.
//!
//! This is the only network request runetui makes.  It runs once at start-up
//! on a background thread through the system `curl`, never blocks the
//! interface, is cached for an hour and can be switched off with the
//! `show_stars` option (Options → General, or `show_stars = false` in
//! `config.toml`).  When the request fails (offline, rate limited, private
//! repository) the menu simply shows a dash.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Repository whose stars are shown.
pub const REPO: &str = "fir3desert/runetui";
/// How long a cached count is trusted before asking GitHub again.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarsState {
    /// Switched off in the options: nothing is shown.
    Off,
    /// The request is still running and there is nothing cached.
    Loading,
    /// The count could not be obtained.
    Unavailable,
    Count(u64),
}

#[derive(Debug)]
pub struct Stars {
    state: StarsState,
    receiver: Option<Receiver<Option<u64>>>,
}

impl Stars {
    /// Start looking up the star count when `enabled`.  A fresh cached value is
    /// used as is; a stale one is shown while a new one is requested.
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::off();
        }
        // Unit tests never touch the network or the user's cache.
        if cfg!(test) {
            return Self {
                state: StarsState::Loading,
                receiver: None,
            };
        }
        let cache = cache_path();
        let cached = cache.as_deref().and_then(|path| read_cache(path, now()));
        if let Some((count, fresh)) = cached {
            if fresh {
                return Self {
                    state: StarsState::Count(count),
                    receiver: None,
                };
            }
        }
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let count = fetch_count(REPO);
            if let (Some(count), Some(path)) = (count, cache_path()) {
                write_cache(&path, count, now());
            }
            let _ = sender.send(count);
        });
        Self {
            state: cached.map_or(StarsState::Loading, |(count, _)| StarsState::Count(count)),
            receiver: Some(receiver),
        }
    }

    pub fn off() -> Self {
        Self {
            state: StarsState::Off,
            receiver: None,
        }
    }

    /// Pick up the result of the background request, if it has arrived.
    pub fn poll(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Some(count)) => {
                self.state = StarsState::Count(count);
                self.receiver = None;
            }
            Ok(None) => {
                // Keep a stale cached count rather than replacing it by a dash.
                if !matches!(self.state, StarsState::Count(_)) {
                    self.state = StarsState::Unavailable;
                }
                self.receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                if !matches!(self.state, StarsState::Count(_)) {
                    self.state = StarsState::Unavailable;
                }
                self.receiver = None;
            }
        }
    }

    pub fn state(&self) -> StarsState {
        self.state
    }

    /// Switch the feature on or off from the options screen.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.state == StarsState::Off {
            *self = Self::start(true);
        } else if !enabled {
            *self = Self::off();
        }
    }

    #[cfg(test)]
    pub fn set_for_test(&mut self, state: StarsState) {
        self.state = state;
    }
}

/// Text drawn in the menu corner, e.g. `★ 42` or `★ 1.2k`.
pub fn label(state: StarsState) -> Option<String> {
    match state {
        StarsState::Off => None,
        StarsState::Loading => Some("★ …".to_string()),
        StarsState::Unavailable => Some("★ —".to_string()),
        StarsState::Count(count) => Some(format!("★ {}", compact(count))),
    }
}

fn compact(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    let tenths = count / 100;
    let (whole, fraction) = (tenths / 10, tenths % 10);
    if fraction == 0 || whole >= 100 {
        format!("{whole}k")
    } else {
        format!("{whole}.{fraction}k")
    }
}

/// Extract `stargazers_count` from the GitHub API's JSON answer.
pub fn parse_star_count(json: &str) -> Option<u64> {
    let key = "\"stargazers_count\"";
    let after = &json[json.find(key)? + key.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn fetch_count(repo: &str) -> Option<u64> {
    let url = format!("https://api.github.com/repos/{repo}");
    let output = Command::new("curl")
        .args([
            "--silent",
            "--fail",
            "--location",
            "--max-time",
            "6",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "User-Agent: runetui",
            &url,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_star_count(&String::from_utf8_lossy(&output.stdout))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|directory| directory.join("runetui").join("stars.txt"))
}

/// Cached `(count, is_fresh)`; the file holds `"<count> <unix seconds>"`.
fn read_cache(path: &Path, now: u64) -> Option<(u64, bool)> {
    let text = fs::read_to_string(path).ok()?;
    let mut parts = text.split_whitespace();
    let count: u64 = parts.next()?.parse().ok()?;
    let saved: u64 = parts.next()?.parse().ok()?;
    Some((count, now.saturating_sub(saved) < CACHE_TTL.as_secs()))
}

fn write_cache(path: &Path, count: u64, now: u64) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{count} {now}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_count_from_the_api_answer() {
        let json = r#"{"id":1,"name":"runetui","stargazers_count": 128,"watchers_count":128}"#;
        assert_eq!(parse_star_count(json), Some(128));
        assert_eq!(parse_star_count("{\n  \"stargazers_count\":0\n}"), Some(0));
    }

    #[test]
    fn rejects_answers_without_a_count() {
        assert_eq!(parse_star_count(""), None);
        assert_eq!(parse_star_count(r#"{"message":"Not Found"}"#), None);
        assert_eq!(parse_star_count(r#"{"stargazers_count":"many"}"#), None);
    }

    #[test]
    fn counts_are_shortened() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1k");
        assert_eq!(compact(1_234), "1.2k");
        assert_eq!(compact(15_900), "15.9k");
        assert_eq!(compact(120_400), "120k");
    }

    #[test]
    fn labels_cover_every_state() {
        assert_eq!(label(StarsState::Off), None);
        assert_eq!(label(StarsState::Loading).as_deref(), Some("★ …"));
        assert_eq!(label(StarsState::Unavailable).as_deref(), Some("★ —"));
        assert_eq!(label(StarsState::Count(42)).as_deref(), Some("★ 42"));
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("stars.txt");
        assert_eq!(read_cache(&path, 1_000), None);
        write_cache(&path, 77, 1_000);
        assert_eq!(read_cache(&path, 1_000 + 60), Some((77, true)));
        assert_eq!(read_cache(&path, 1_000 + CACHE_TTL.as_secs() + 1), Some((77, false)));
        fs::write(&path, "garbage").unwrap();
        assert_eq!(read_cache(&path, 1_000), None);
    }

    #[test]
    fn a_failed_request_keeps_the_cached_count() {
        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Count(9),
            receiver: Some(receiver),
        };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(9));

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Loading,
            receiver: Some(receiver),
        };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Unavailable);

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Loading,
            receiver: Some(receiver),
        };
        sender.send(Some(5)).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(5));
    }

    #[test]
    fn disabled_means_nothing_is_shown_or_fetched() {
        let stars = Stars::start(false);
        assert_eq!(stars.state(), StarsState::Off);
        assert!(stars.receiver.is_none());
    }
}

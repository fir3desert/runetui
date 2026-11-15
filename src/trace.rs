//! Optional start-up tracing, for finding out where the time goes on a huge
//! directory.
//!
//! Set `RTUI_TRACE=1` to append timings to `rtui-trace.log` in the system
//! temporary directory, `RTUI_TRACE=<path>` to use another file, or
//! `RTUI_TRACE=stderr` to write to stderr (`rtui 2>trace.log`).  The TUI owns
//! the terminal, so a file is the default.  When tracing is off every call is
//! a single atomic load.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static DESTINATION: OnceLock<String> = OnceLock::new();

enum Sink {
    File(File),
    Stderr,
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Stderr,
    File(PathBuf),
}

/// Where `RTUI_TRACE=<value>` sends the trace, or `None` when it is off.
fn parse_target(value: &str) -> Option<Target> {
    let value = value.trim();
    if value.is_empty() || value == "0" || value.eq_ignore_ascii_case("false") {
        None
    } else if value.eq_ignore_ascii_case("stderr") {
        Some(Target::Stderr)
    } else if value == "1" || value.eq_ignore_ascii_case("true") {
        Some(Target::File(std::env::temp_dir().join("rtui-trace.log")))
    } else {
        Some(Target::File(PathBuf::from(value)))
    }
}

/// Read `RTUI_TRACE` and start the clock.  Call once, first thing in `main`.
pub fn init() {
    let _ = START.set(Instant::now());
    let Ok(value) = std::env::var("RTUI_TRACE") else {
        return;
    };
    let (sink, label) = match parse_target(&value) {
        None => return,
        Some(Target::Stderr) => (Sink::Stderr, "stderr".to_string()),
        Some(Target::File(path)) => {
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => (Sink::File(file), path.display().to_string()),
                Err(_) => return,
            }
        }
    };
    let _ = DESTINATION.set(label);
    if let Ok(mut guard) = SINK.lock() {
        *guard = Some(sink);
    }
    ENABLED.store(true, Ordering::Relaxed);
    write_line("---- rtui start ----");
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Where the trace goes, when tracing is on.
pub fn destination() -> Option<&'static str> {
    DESTINATION.get().map(String::as_str).filter(|_| enabled())
}

fn elapsed() -> Duration {
    START.get().map(Instant::elapsed).unwrap_or_default()
}

fn write_line(text: &str) {
    let line = format!("{:>9.1} ms  [{}] {}\n", elapsed().as_secs_f64() * 1000.0, thread_label(), text);
    if let Ok(mut guard) = SINK.lock() {
        match guard.as_mut() {
            Some(Sink::File(file)) => {
                let _ = file.write_all(line.as_bytes());
            }
            Some(Sink::Stderr) => {
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
            None => {}
        }
    }
}

fn thread_label() -> String {
    let current = std::thread::current();
    current.name().unwrap_or("main").to_string()
}

/// Record that a start-up phase was reached.
pub fn mark(phase: &str) {
    if enabled() {
        write_line(phase);
    }
}

/// Run `work` and record how long it took.
pub fn span<T>(phase: &str, work: impl FnOnce() -> T) -> T {
    if !enabled() {
        return work();
    }
    let started = Instant::now();
    let value = work();
    write_line(&format!(
        "{phase} took {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    ));
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_picks_the_destination() {
        for off in ["", "  ", "0", "false", "FALSE"] {
            assert_eq!(parse_target(off), None, "{off:?}");
        }
        assert_eq!(parse_target("stderr"), Some(Target::Stderr));
        assert_eq!(parse_target("STDERR"), Some(Target::Stderr));
        assert_eq!(
            parse_target("1"),
            Some(Target::File(std::env::temp_dir().join("rtui-trace.log")))
        );
        assert_eq!(
            parse_target("true"),
            Some(Target::File(std::env::temp_dir().join("rtui-trace.log")))
        );
        assert_eq!(
            parse_target("/var/tmp/x.log"),
            Some(Target::File(PathBuf::from("/var/tmp/x.log")))
        );
    }

    #[test]
    fn span_returns_the_value_and_is_silent_when_off() {
        // Tracing is off in unit tests (no RTUI_TRACE): nothing is written.
        assert!(!enabled());
        assert_eq!(span("x", || 7), 7);
        mark("ignored");
        assert_eq!(destination(), None);
    }
}

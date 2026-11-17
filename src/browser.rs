//! Opening a web address in the user's browser.
//!
//! Only `http://`, `https://` and `mailto:` addresses without blanks or
//! control characters are ever handed to the system opener, and they are
//! passed as a single argument to a fixed program: never through a shell.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to watch the opener before assuming it worked.  A missing
/// browser (a server over SSH) makes `xdg-open` fail within a few tens of
/// milliseconds.
const OPENER_PATIENCE: Duration = Duration::from_millis(250);

/// May this address be opened outside the program?
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:");
    scheme_ok
        && url.len() > "mailto:".len()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The program and fixed arguments that open an address on this system.
pub(crate) fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        // Not `cmd /c start`: cmd would interpret `&` and friends in the URL.
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else {
        ("xdg-open", &[])
    }
}

/// Open `url` in the browser.  `Err` carries a short reason, in which case
/// the caller should offer the address another way (the clipboard).
pub fn open_url(url: &str) -> Result<(), String> {
    if !is_openable(url) {
        return Err("not a web address".to_string());
    }
    let (program, args) = opener();
    run(program, args, url)
}

fn run(program: &str, args: &[&str], url: &str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) if started.elapsed() < OPENER_PATIENCE => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                // Still running: some openers stay until the browser closes.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(error) => return Err(format!("{program}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_addresses_are_openable() {
        for good in [
            "https://github.com/fir3desert/runetui",
            "http://example.com/a?b=c&d=e#frag",
            "HTTPS://EXAMPLE.COM",
            "mailto:someone@example.com",
        ] {
            assert!(is_openable(good), "{good}");
        }
        for bad in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com",
            "x-terminal://run",
            "/usr/bin/evil",
            "notes/guide.md",
            "https://example.com/a b",
            "https://example.com/\nsecond",
            "mailto:",
            "-oProxyCommand=evil",
        ] {
            assert!(!is_openable(bad), "{bad:?}");
        }
    }

    #[test]
    fn refuses_to_open_what_is_not_a_web_address() {
        assert!(open_url("file:///etc/passwd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_successful_opener_is_ok_and_a_failing_one_is_reported() {
        assert!(run("true", &[], "https://example.com").is_ok());
        let error = run("false", &[], "https://example.com").unwrap_err();
        assert!(error.contains("false"), "{error}");
        let error = run("sh", &["-c", "exit 3", "sh"], "https://example.com").unwrap_err();
        assert!(error.contains("exited"), "{error}");
    }

    #[test]
    fn a_missing_opener_is_reported() {
        let error = run("rtui-no-such-opener", &[], "https://example.com").unwrap_err();
        assert!(error.contains("cannot run"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_url_is_a_single_argument_never_a_shell_command() {
        // If the address went through a shell, this would create the marker.
        let marker = std::env::temp_dir().join(format!("rtui-inject-{}", std::process::id()));
        let url = format!("https://example.com/;touch${{IFS}}{}", marker.display());
        let _ = run("true", &[], &url);
        assert!(!marker.exists());
    }
}

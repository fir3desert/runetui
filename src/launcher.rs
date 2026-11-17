//! Opening the selected file or folder in other programs: VS Code, a new
//! terminal window, the system's default application.
//!
//! Programs are started directly, never through a shell, and the path is a
//! single argument.  Everything is detached: closing runetui does not close
//! them.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to watch a program before assuming it started properly.  One
/// that does not exist or cannot open a window fails well within this.
const PATIENCE: Duration = Duration::from_millis(300);

/// An absolute version of `path`, so it can never be read as an option.
fn absolute(path: &Path) -> PathBuf {
    #[cfg(not(windows))]
    {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
    #[cfg(windows)]
    {
        path.to_path_buf()
    }
}

/// Start `program` and wait a moment to see whether it works.
fn spawn(program: &str, args: &[OsString], dir: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) if started.elapsed() < PATIENCE => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                // Still running: it is a window that stays until closed.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(error) => return Err(format!("{program}: {error}")),
        }
    }
}

/// Open a file or folder in Visual Studio Code.
pub fn open_in_vscode(path: &Path) -> Result<(), String> {
    let path = absolute(path);
    let candidates: &[&str] = if cfg!(windows) {
        &["code.cmd", "code"]
    } else {
        &["code", "code-insiders", "codium"]
    };
    let args = [OsString::from(path.as_os_str())];
    let mut last = String::from("VS Code was not found");
    for program in candidates {
        match spawn(program, &args, None) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Open a file or folder with the system's default application.
pub fn open_with_default(path: &Path) -> Result<(), String> {
    let path = absolute(path);
    let (program, fixed) = crate::browser::opener();
    let mut args: Vec<OsString> = fixed.iter().map(OsString::from).collect();
    args.push(path.into_os_string());
    spawn(program, &args, None)
}

/// Terminal emulators tried on Linux and the BSDs, most specific first.
const TERMINALS: &[&str] = &[
    "x-terminal-emulator",
    "gnome-terminal",
    "konsole",
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "xfce4-terminal",
    "tilix",
    "xterm",
];

/// The arguments that make terminal `program` start in `dir`.  Those that
/// would otherwise ignore the directory of the process (a server such as
/// gnome-terminal) get it explicitly; the others also inherit it.
fn terminal_args(program: &str, dir: &Path) -> Vec<OsString> {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    let flag = |text: &str| -> Vec<OsString> {
        vec![OsString::from(format!("{text}{}", dir.display()))]
    };
    let pair = |text: &str| -> Vec<OsString> {
        vec![OsString::from(text), OsString::from(dir.as_os_str())]
    };
    match name {
        "gnome-terminal" | "xfce4-terminal" | "tilix" | "foot" => flag("--working-directory="),
        "konsole" => pair("--workdir"),
        "kitty" => pair("--directory"),
        "alacritty" => pair("--working-directory"),
        "wezterm" => vec![
            OsString::from("start"),
            OsString::from("--cwd"),
            OsString::from(dir.as_os_str()),
        ],
        _ => Vec::new(),
    }
}

/// Open a new terminal window in the folder `dir`.
pub fn open_in_terminal(dir: &Path) -> Result<(), String> {
    let dir = absolute(dir);

    #[cfg(target_os = "macos")]
    {
        let args = [OsString::from("-a"), OsString::from("Terminal"), dir.into_os_string()];
        return spawn("open", &args, None);
    }

    #[cfg(windows)]
    {
        let wt = [OsString::from("-d"), OsString::from(dir.as_os_str())];
        if spawn("wt", &wt, None).is_ok() {
            return Ok(());
        }
        let start: Vec<OsString> = ["/c", "start", "", "/D"]
            .iter()
            .map(OsString::from)
            .chain([dir.into_os_string(), OsString::from("cmd")])
            .collect();
        return spawn("cmd", &start, None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(chosen) = std::env::var("TERMINAL")
            .ok()
            .and_then(|value| value.split_whitespace().next().map(str::to_string))
        {
            candidates.push(chosen);
        }
        candidates.extend(TERMINALS.iter().map(|name| name.to_string()));
        for program in candidates {
            let args = terminal_args(&program, &dir);
            if spawn(&program, &args, Some(&dir)).is_ok() {
                return Ok(());
            }
        }
        Err("no terminal emulator could be started".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn terminals_get_the_folder_in_their_own_syntax() {
        let dir = Path::new("/home/u/my project");
        assert_eq!(
            text(terminal_args("gnome-terminal", dir)),
            ["--working-directory=/home/u/my project"]
        );
        assert_eq!(text(terminal_args("konsole", dir)), ["--workdir", "/home/u/my project"]);
        assert_eq!(text(terminal_args("/usr/bin/kitty", dir)), ["--directory", "/home/u/my project"]);
        assert_eq!(
            text(terminal_args("wezterm", dir)),
            ["start", "--cwd", "/home/u/my project"]
        );
        assert!(terminal_args("xterm", dir).is_empty(), "xterm inherits the directory");
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_program_is_reported_and_a_working_one_is_ok() {
        let error = spawn("rtui-no-such-program", &[], None).unwrap_err();
        assert!(error.contains("cannot run"), "{error}");
        assert!(spawn("true", &[], None).is_ok());
        assert!(spawn("false", &[], None).unwrap_err().contains("exited"));
    }

    #[cfg(unix)]
    #[test]
    fn the_path_is_one_argument_even_with_shell_characters() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pwned");
        let odd = dir.path().join(format!("a;touch {}", marker.display().to_string().replace('/', "_")));
        std::fs::create_dir_all(&odd).unwrap();
        let args = [OsString::from(absolute(&odd).into_os_string())];
        let _ = spawn("true", &args, None);
        assert!(!marker.exists());
    }
}

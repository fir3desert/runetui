//! Command-line smoke tests.
//!
//! These only exercise paths that terminate before the terminal is
//! initialised, so they can run in a headless test environment.

use std::process::Command;

fn rtui() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtui"))
}

#[test]
fn help_lists_path_and_theme_options() {
    let output = rtui().arg("--help").output().expect("run rtui");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PATH"));
    assert!(stdout.contains("--theme"));
    assert!(stdout.contains("catppuccin-mocha"));
}

#[test]
fn version_is_reported() {
    let output = rtui().arg("--version").output().expect("run rtui");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_theme_is_rejected_with_available_names() {
    let output = rtui()
        .args(["--theme", "not-a-theme"])
        .output()
        .expect("run rtui");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown theme"));
    assert!(stderr.contains("catppuccin-mocha"));
    assert!(stderr.contains("gruvbox"));
}

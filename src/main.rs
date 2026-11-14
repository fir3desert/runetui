#![forbid(unsafe_code)]
//! runetui — a Markdown and source-code viewer for the terminal.
//!
//! Documents are only written when you explicitly edit the selected file with
//! `e`, which runs your editor inside the document pane.  The program also creates its own settings file
//! under the platform configuration directory.

mod app;
mod clipboard;
mod config;
mod editor;
mod events;
mod filesystem;
mod keymap;
mod markdown;
mod selection;
mod ui;
mod viewer;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};
use clap::Parser;

use crate::app::App;
use crate::config::Config;
use crate::events::Event;
use crate::ui::icons::IconStyle;
use crate::ui::themes::ThemeName;

/// Markdown and source-code viewer with an Obsidian-style document pane and a
/// Neovim-style project explorer.
#[derive(Debug, Parser)]
#[command(
    name = "rtui",
    version,
    about = "Markdown and source-code viewer for the terminal",
    after_help = "Keys: ↑/↓ move · Space/Enter open · ←/→ tree · Tab switch panel · e edit here (vim) · Ctrl+T terminal · Ctrl+R then ←→↑↓ resize panels · Esc menu · / search · o outline · T theme · F icons · ? help · q quit (all shortcuts: [keys] in config.toml)"
)]
struct Cli {
    /// Directory or document to open
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Colour theme (catppuccin-mocha, tokyo-night, nord, dracula, gruvbox, black, white, snow, default-dark)
    #[arg(short = 't', long, value_name = "THEME")]
    theme: Option<String>,

    /// Explorer icon style (nerd, unicode, ascii)
    #[arg(short = 'i', long, value_name = "STYLE")]
    icons: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = Config::load();
    if let Some(theme) = &cli.theme {
        let Some(name) = ThemeName::from_slug(theme) else {
            let available = ThemeName::ALL
                .iter()
                .map(|theme| theme.slug())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("unknown theme “{theme}”. Available themes: {available}");
        };
        config.set_theme(name);
    }

    if let Some(icons) = &cli.icons {
        let Some(style) = IconStyle::from_slug(icons) else {
            bail!("unknown icon style “{icons}”. Available styles: nerd, unicode, ascii");
        };
        config.set_icon_style(style);
    }

    let mut app = App::with_config(cli.path.as_deref(), config);

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    let result = event_loop(&mut terminal, &mut app);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();

    if result.is_ok() {
        app.persist();
    }
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut mouse_captured = false;
    loop {
        app.tick();
        terminal.draw(|frame| ui::draw(frame, app))?;
        // The shell starts once the first draw has computed the layout.
        app.ensure_terminal();

        // Mouse capture follows the Options → Mouse setting.
        if app.config.mouse != mouse_captured {
            mouse_captured = app.config.mouse;
            let _ = if mouse_captured {
                crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
            } else {
                crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
            };
        }

        // A finished mouse selection goes to the clipboard through OSC 52.
        if let Some(text) = app.take_clipboard() {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(clipboard::osc52(&text).as_bytes());
            let _ = out.flush();
            app.set_status(format!("Copied {} characters", text.chars().count()));
        }

        let timeout = if app.menu_animation().is_some() {
            events::ANIMATION_TICK
        } else if app.editor.is_some() || app.focus == app::Focus::Terminal {
            events::EDITOR_TICK
        } else if app.term.is_some() && app.layout.terminal.is_some() {
            events::TERMINAL_TICK
        } else {
            events::TICK
        };
        match events::next_event(timeout)? {
            Event::Key(key) => {
                app.handle_key(key);
                if let Some(path) = app.take_editor_request() {
                    match run_editor(terminal, &path) {
                        Ok(()) => {
                            app.after_editor(&path);
                            app.set_status(format!("Edited {}", path.display()));
                        }
                        Err(error) => {
                            // The editor may still have saved changes before
                            // exiting with a non-zero status.
                            app.after_editor(&path);
                            app.set_status(format!("Editor: {error}"));
                        }
                    }
                }
            }
            Event::Paste(text) => app.paste(&text),
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            // The next draw pass recalculates layout and reflows the document.
            Event::Resize | Event::Tick => {}
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Temporarily leave the TUI, run the user's editor on `path`, then restore the
/// terminal.  `$VISUAL` and `$EDITOR` are honoured, falling back to `vim`.
fn run_editor(terminal: &mut ratatui::DefaultTerminal, path: &std::path::Path) -> Result<()> {
    ratatui::restore();
    let result = launch_editor(path);
    *terminal = ratatui::init();
    terminal.clear()?;
    result
}

fn launch_editor(path: &std::path::Path) -> Result<()> {
    let (program, args) = editor::editor_command();
    let status = Command::new(&program)
        .args(args)
        .arg(path)
        .status()
        .map_err(|error| anyhow::anyhow!("cannot run {program}: {error}"))?;
    if !status.success() {
        bail!("{program} exited with status {status}");
    }
    Ok(())
}

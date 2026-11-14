//! Programs running inside a pseudo-terminal pane.
//!
//! Instead of suspending the TUI and handing the terminal to `vim`, the user's
//! editor runs inside a pseudo-terminal whose screen is drawn in the document
//! pane.  The editor is the real program (`$VISUAL`, `$EDITOR` or `vim`), so
//! the user's own configuration and muscle memory keep working.  The terminal
//! panel at the bottom is the same machinery running the user's shell.
//!
//! Output from the child is parsed by `vt100`; keys are translated back into
//! the byte sequences a terminal would send.  A pane closes by itself when
//! its program exits (`:wq`, `:q`, `ZZ`, `exit`...).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::ui::themes::Theme;

/// Editor command taken from `$VISUAL`, then `$EDITOR`, falling back to `vim`.
pub fn editor_command() -> (String, Vec<String>) {
    let raw = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vim".to_string());
    let mut parts = raw.split_whitespace();
    let program = parts.next().unwrap_or("vim").to_string();
    (program, parts.map(str::to_string).collect())
}

/// A program running in a pseudo-terminal pane.  The embedded editor and the
/// terminal panel are both instances of it.
pub struct PtyPane {
    /// The file being edited, or the working directory of a shell.
    pub path: PathBuf,
    pub program: String,
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
}

/// The embedded editor is a pane running `$EDITOR`.
pub type EmbeddedEditor = PtyPane;

/// Shell used by the terminal panel: `$SHELL`, else `COMSPEC` on Windows or
/// `/bin/sh` elsewhere.
pub fn shell_command() -> (String, Vec<String>) {
    let raw = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            if cfg!(windows) {
                std::env::var("COMSPEC").ok().filter(|value| !value.trim().is_empty())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });
    let mut parts = raw.split_whitespace();
    let program = parts.next().unwrap_or("sh").to_string();
    (program, parts.map(str::to_string).collect())
}

/// Lines of scrollback kept for shells.
const SHELL_SCROLLBACK: usize = 2000;

impl PtyPane {
    /// Start the user's shell in `cwd`, in a `rows` x `cols` pseudo-terminal.
    pub fn spawn_shell(cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let (program, args) = shell_command();
        Self::spawn_shell_with(&program, &args, cwd, rows, cols)
    }

    /// Like [`spawn_shell`](Self::spawn_shell) with an explicit program.
    pub fn spawn_shell_with(
        program: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        Self::spawn_inner(program, args, None, Some(cwd), cwd, rows, cols, SHELL_SCROLLBACK)
    }

    /// Start the editor on `path` in a `rows` x `cols` pseudo-terminal.
    pub fn spawn(path: &Path, rows: u16, cols: u16) -> Result<Self> {
        let (program, args) = editor_command();
        Self::spawn_with(&program, &args, path, rows, cols)
    }

    /// Like [`spawn`](Self::spawn) with an explicit program and arguments.
    pub fn spawn_with(
        program: &str,
        args: &[String],
        path: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let cwd = path.parent().filter(|parent| parent.is_dir());
        Self::spawn_inner(program, args, Some(path), cwd, path, rows, cols, 0)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_inner(
        program: &str,
        args: &[String],
        file_argument: Option<&Path>,
        cwd: Option<&Path>,
        anchor: &Path,
        rows: u16,
        cols: u16,
        scrollback: usize,
    ) -> Result<Self> {
        let rows = rows.max(2);
        let cols = cols.max(10);
        let program = program.to_string();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| anyhow!("cannot open a pseudo-terminal: {error}"))?;

        let mut command = CommandBuilder::new(&program);
        command.args(args);
        if let Some(path) = file_argument {
            command.arg(path);
        }
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        if let Some(directory) = cwd {
            command.cwd(directory);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| anyhow!("cannot run {program}: {error}"))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| anyhow!("cannot read from {program}: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| anyhow!("cannot write to {program}: {error}"))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback)));
        let sink = Arc::clone(&parser);
        thread::Builder::new()
            .name("runetui-editor-reader".into())
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            if let Ok(mut parser) = sink.lock() {
                                parser.process(&buffer[..count]);
                            }
                        }
                    }
                }
            })
            .context("cannot start the editor reader thread")?;

        Ok(Self {
            path: anchor.to_path_buf(),
            program,
            parser,
            master: pair.master,
            writer,
            child,
            size: (rows, cols),
        })
    }

    /// Resize the pseudo-terminal when the pane changes size.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(2);
        let cols = cols.max(10);
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut parser) = self.parser.lock() {
            parser.screen_mut().set_size(rows, cols);
        }
    }

    /// Scroll the view through the scrollback: positive goes back in time.
    pub fn scroll_view(&mut self, lines: isize) {
        if let Ok(mut parser) = self.parser.lock() {
            let current = parser.screen().scrollback();
            let next = if lines >= 0 {
                current.saturating_add(lines as usize)
            } else {
                current.saturating_sub(lines.unsigned_abs())
            };
            parser.screen_mut().set_scrollback(next);
        }
    }

    /// Lines the view is scrolled back from the live screen.
    #[allow(dead_code)]
    pub fn scrollback_offset(&self) -> usize {
        self.parser
            .lock()
            .map(|parser| parser.screen().scrollback())
            .unwrap_or(0)
    }

    fn reset_scroll(&mut self) {
        if let Ok(mut parser) = self.parser.lock() {
            if parser.screen().scrollback() > 0 {
                parser.screen_mut().set_scrollback(0);
            }
        }
    }

    /// Forward a key press to the program.
    pub fn send_key(&mut self, key: KeyEvent) {
        self.reset_scroll();
        let application_cursor = self
            .parser
            .lock()
            .map(|parser| parser.screen().application_cursor())
            .unwrap_or(false);
        let bytes = key_bytes(key, application_cursor);
        if !bytes.is_empty() {
            let _ = self.writer.write_all(&bytes);
            let _ = self.writer.flush();
        }
    }

    /// Forward pasted text, using bracketed paste when the editor asked for it.
    pub fn send_paste(&mut self, text: &str) {
        self.reset_scroll();
        let bracketed = self
            .parser
            .lock()
            .map(|parser| parser.screen().bracketed_paste())
            .unwrap_or(false);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        let _ = self.writer.write_all(&bytes);
        let _ = self.writer.flush();
    }

    /// `Some(success)` once the editor process has exited.
    pub fn exit_status(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Text currently on the editor screen, one string per row.
    #[cfg(test)]
    pub fn screen_text(&self) -> String {
        self.parser
            .lock()
            .map(|parser| parser.screen().contents())
            .unwrap_or_default()
    }

    /// Draw the screen into `area`.  Only the focused pane places the
    /// terminal cursor.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let Ok(parser) = self.parser.lock() else {
            return;
        };
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let height = area.height.min(rows);
        let width = area.width.min(cols);
        let base = Style::default().fg(theme.fg).bg(theme.bg);

        for row in 0..height {
            for col in 0..width {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let Some(target) = frame.buffer_mut().cell_mut((area.x + col, area.y + row))
                else {
                    continue;
                };
                let contents = cell.contents();
                target.set_symbol(if contents.is_empty() { " " } else { contents });
                target.set_style(cell_style(cell, base));
            }
        }

        if focused && !screen.hide_cursor() && screen.scrollback() == 0 {
            let (row, col) = screen.cursor_position();
            if row < height && col < width {
                frame.set_cursor_position((area.x + col, area.y + row));
            }
        }
    }
}

impl Drop for PtyPane {
    fn drop(&mut self) {
        self.kill();
    }
}

fn cell_style(cell: &vt100::Cell, base: Style) -> Style {
    let mut style = base;
    if let Some(color) = convert_color(cell.fgcolor()) {
        style = style.fg(color);
    }
    if let Some(color) = convert_color(cell.bgcolor()) {
        style = style.bg(color);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn convert_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// Bytes a terminal would send for `key`.
pub fn key_bytes(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let mut bytes: Vec<u8> = match key.code {
        KeyCode::Char(character) => {
            if ctrl {
                match control_byte(character) {
                    Some(byte) => vec![byte],
                    None => return Vec::new(),
                }
            } else {
                let mut buffer = [0u8; 4];
                character.encode_utf8(&mut buffer).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_key(b'A', application_cursor, ctrl, shift, alt),
        KeyCode::Down => cursor_key(b'B', application_cursor, ctrl, shift, alt),
        KeyCode::Right => cursor_key(b'C', application_cursor, ctrl, shift, alt),
        KeyCode::Left => cursor_key(b'D', application_cursor, ctrl, shift, alt),
        KeyCode::Home => cursor_key(b'H', application_cursor, ctrl, shift, alt),
        KeyCode::End => cursor_key(b'F', application_cursor, ctrl, shift, alt),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(number) => match function_key(number) {
            Some(sequence) => sequence.to_vec(),
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };

    // Alt+key is sent as an ESC prefix (cursor keys already carry the modifier).
    if alt && !matches!(
        key.code,
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
    ) {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn control_byte(character: char) -> Option<u8> {
    match character {
        'a'..='z' => Some(character as u8 - b'a' + 1),
        'A'..='Z' => Some(character as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

fn cursor_key(final_byte: u8, application: bool, ctrl: bool, shift: bool, alt: bool) -> Vec<u8> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    if modifier > 1 {
        return format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes();
    }
    let introducer = if application { b'O' } else { b'[' };
    vec![0x1b, introducer, final_byte]
}

fn function_key(number: u8) -> Option<&'static [u8]> {
    Some(match number {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn plain_characters_and_utf8_are_sent_as_text() {
        assert_eq!(key_bytes(key(KeyCode::Char('i'), KeyModifiers::NONE), false), b"i");
        assert_eq!(
            key_bytes(key(KeyCode::Char('ñ'), KeyModifiers::NONE), false),
            "ñ".as_bytes()
        );
    }

    #[test]
    fn control_and_alt_combinations() {
        assert_eq!(
            key_bytes(key(KeyCode::Char('w'), KeyModifiers::CONTROL), false),
            vec![0x17]
        );
        assert_eq!(
            key_bytes(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            vec![0x1b, b'x']
        );
    }

    #[test]
    fn escape_enter_and_backspace() {
        assert_eq!(key_bytes(key(KeyCode::Esc, KeyModifiers::NONE), false), vec![0x1b]);
        assert_eq!(key_bytes(key(KeyCode::Enter, KeyModifiers::NONE), false), vec![b'\r']);
        assert_eq!(
            key_bytes(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            vec![0x7f]
        );
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(key_bytes(key(KeyCode::Up, KeyModifiers::NONE), false), b"\x1b[A");
        assert_eq!(key_bytes(key(KeyCode::Up, KeyModifiers::NONE), true), b"\x1bOA");
        assert_eq!(
            key_bytes(key(KeyCode::Left, KeyModifiers::CONTROL), false),
            b"\x1b[1;5D"
        );
    }

    #[test]
    fn function_and_navigation_keys() {
        assert_eq!(key_bytes(key(KeyCode::F(1), KeyModifiers::NONE), false), b"\x1bOP");
        assert_eq!(key_bytes(key(KeyCode::PageDown, KeyModifiers::NONE), false), b"\x1b[6~");
        assert_eq!(key_bytes(key(KeyCode::Delete, KeyModifiers::NONE), false), b"\x1b[3~");
    }

    #[test]
    fn editor_command_splits_arguments() {
        // Only the parsing is checked; the environment is left untouched.
        let mut parts = "nvim -u NONE".split_whitespace();
        assert_eq!(parts.next(), Some("nvim"));
        assert_eq!(parts.collect::<Vec<_>>(), vec!["-u", "NONE"]);
    }

    fn wait_for(editor: &mut EmbeddedEditor) -> Option<bool> {
        for _ in 0..200 {
            if let Some(status) = editor.exit_status() {
                return Some(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn program_output_reaches_the_screen_and_exit_is_detected() {
        // `cat` stands in for an editor: it prints the file and exits.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "hello from the file\n").unwrap();

        let Ok(mut editor) = EmbeddedEditor::spawn_with("cat", &[], &file, 5, 40) else {
            return; // no pty in this environment
        };
        assert_eq!(wait_for(&mut editor), Some(true));
        // Give the reader thread a moment to drain the last bytes.
        for _ in 0..100 {
            if editor.screen_text().contains("hello from the file") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(editor.screen_text().contains("hello from the file"));
    }

    #[test]
    fn keys_reach_the_program_and_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "").unwrap();

        // `sh -c 'exit 3'` ends with a failing status.
        let args = vec!["-c".to_string(), "exit 3".to_string()];
        let Ok(mut failing) = EmbeddedEditor::spawn_with("sh", &args, &file, 5, 40) else {
            return;
        };
        assert_eq!(wait_for(&mut failing), Some(false));

        // The shell reads one line typed on the keyboard, prints it and exits
        // (the file path becomes `$0`).
        let args = vec!["-c".to_string(), "read line; echo got:$line".to_string()];
        let mut reader = EmbeddedEditor::spawn_with("sh", &args, &file, 5, 40).unwrap();
        for character in "typed\n".chars() {
            let code = if character == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(character)
            };
            reader.send_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert_eq!(wait_for(&mut reader), Some(true));
        for _ in 0..100 {
            if reader.screen_text().contains("got:typed") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(reader.screen_text().contains("got:typed"));
    }

    #[test]
    fn shell_pane_runs_in_its_directory_and_closes_when_it_exits() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec!["-c".to_string(), "pwd; read line; echo got:$line".to_string()];
        let Ok(mut shell) = PtyPane::spawn_shell_with("sh", &args, dir.path(), 6, 80) else {
            return; // no pty in this environment
        };
        assert_eq!(shell.path, dir.path());
        for _ in 0..200 {
            if shell.screen_text().contains(&dir.path().display().to_string()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // No file argument is appended for shells: `pwd` shows the directory.
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let screen = shell.screen_text();
        assert!(
            screen.contains(&canonical.display().to_string())
                || screen.contains(&dir.path().display().to_string()),
            "{screen}"
        );
        for character in "ok\n".chars() {
            let code = if character == '\n' { KeyCode::Enter } else { KeyCode::Char(character) };
            shell.send_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert_eq!(wait_for(&mut shell), Some(true));
    }

    #[test]
    fn shell_scrollback_can_be_viewed_and_resets_on_typing() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "-c".to_string(),
            "i=1; while [ $i -le 40 ]; do echo line$i; i=$((i+1)); done; sleep 2".to_string(),
        ];
        let Ok(mut shell) = PtyPane::spawn_shell_with("sh", &args, dir.path(), 5, 40) else {
            return;
        };
        for _ in 0..300 {
            if shell.screen_text().contains("line40") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(shell.screen_text().contains("line40"));
        assert_eq!(shell.scrollback_offset(), 0);
        shell.scroll_view(10);
        assert_eq!(shell.scrollback_offset(), 10);
        assert!(!shell.screen_text().contains("line40"), "the view moved back");
        shell.send_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(shell.scrollback_offset(), 0, "typing returns to the live screen");
        shell.scroll_view(-5);
        assert_eq!(shell.scrollback_offset(), 0, "cannot scroll past the live screen");
    }

    #[test]
    fn shell_command_falls_back_to_a_shell() {
        let (program, _) = shell_command();
        assert!(!program.is_empty());
    }

    #[test]
    fn real_vim_edits_and_saves_through_the_pane() {
        if std::process::Command::new("vim").arg("--version").output().is_err() {
            return; // vim is not installed here
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "old\n").unwrap();

        let args = vec!["-u".to_string(), "NONE".to_string(), "-N".to_string()];
        let Ok(mut editor) = EmbeddedEditor::spawn_with("vim", &args, &file, 12, 60) else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(400));
        // Insert text, leave insert mode with Esc, then save and quit.
        for character in "ihello ".chars() {
            editor.send_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        editor.send_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        for character in ":wq".chars() {
            editor.send_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        editor.send_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(wait_for(&mut editor), Some(true));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello old\n");
    }
}

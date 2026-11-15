//! Terminal event reading.
//!
//! Keyboard input, mouse clicks/drags/wheel and resizes matter.  The 100 ms tick keeps filesystem
//! change notifications and status-message expiry flowing even when the user
//! is not pressing keys.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
};

pub const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Resize,
    Mouse(MouseEvent),
    Paste(String),
    Tick,
}

/// Poll interval while an embedded editor is open, so its output (and the
/// echo of every key) shows up without a noticeable delay.
pub const EDITOR_TICK: Duration = Duration::from_millis(12);

/// Poll interval while the terminal panel is visible but not focused, so
/// output from a running command still shows up promptly.
pub const TERMINAL_TICK: Duration = Duration::from_millis(40);

/// Poll interval while the menu banner animates (about 60 frames per second).
pub const ANIMATION_TICK: Duration = Duration::from_millis(16);

/// Poll interval while something runs in the background (a big directory
/// listing), so the "indexing…" spinner turns and the result shows up promptly.
pub const SPINNER_TICK: Duration = Duration::from_millis(80);

/// Poll the terminal for one event, returning [`Event::Tick`] on timeout.
pub fn next_event(timeout: Duration) -> Result<Event> {
    if !event::poll(timeout)? {
        return Ok(Event::Tick);
    }
    match event::read()? {
        CrosstermEvent::Key(key) if key.kind != KeyEventKind::Release => Ok(Event::Key(key)),
        CrosstermEvent::Resize(_, _) => Ok(Event::Resize),
        CrosstermEvent::Paste(text) => Ok(Event::Paste(text)),
        // Plain pointer movement is reported constantly while the mouse is
        // captured and means nothing here.
        CrosstermEvent::Mouse(mouse) if mouse.kind != MouseEventKind::Moved => {
            Ok(Event::Mouse(mouse))
        }
        // Focus changes, key releases and pointer motion are ignored, but the
        // caller still gets a chance to run its periodic work.
        _ => Ok(Event::Tick),
    }
}

//! Configurable key bindings.
//!
//! Every global shortcut is an [`Action`] with one or more bindings.  A binding
//! is either a single key (`"ctrl+t"`, `"T"`, `"f5"`) or a two-key sequence
//! (`"prefix left"`, `"ctrl+x b"`), tmux style.  The word `prefix` stands for
//! the configurable prefix key (`Ctrl+R` by default).
//!
//! Bindings live in the `[keys]` table of `config.toml`:
//!
//! ```toml
//! prefix = "ctrl+r"
//!
//! [keys]
//! quit = ["q", "ctrl+c"]
//! toggle_terminal = "ctrl+t"
//! explorer_shrink = ["prefix left", "ctrl+left"]
//! ```
//!
//! Arrow keys inside the panes, Enter, Space, Home/End and PageUp/PageDown
//! are not configurable: they are navigation, not shortcuts.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Menu,
    Help,
    Outline,
    FollowLink,
    Search,
    SearchNext,
    SearchPrev,
    FocusNext,
    FocusPrev,
    Edit,
    ToggleExplorer,
    ToggleTerminal,
    CycleTheme,
    ToggleHidden,
    ToggleGitignore,
    CycleIcons,
    Refresh,
    HistoryBack,
    HistoryForward,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    ExplorerShrink,
    ExplorerGrow,
    TerminalGrow,
    TerminalShrink,
    ForceClose,
}

struct Meta {
    action: Action,
    name: &'static str,
    short: &'static str,
    #[allow(dead_code)]
    description: &'static str,
    defaults: &'static [&'static str],
}

const META: &[Meta] = &[
    Meta {
        action: Action::Quit,
        name: "quit",
        short: "quit",
        description: "Quit runetui",
        defaults: &["q", "ctrl+c", "prefix q"],
    },
    Meta {
        action: Action::Menu,
        name: "menu",
        short: "menu",
        description: "Open the menu (OPTIONS, HELP, QUIT), or go back",
        defaults: &["esc", "prefix m"],
    },
    Meta {
        action: Action::Help,
        name: "help",
        short: "help",
        description: "Show or hide this help",
        defaults: &["?", "prefix ?"],
    },
    Meta {
        action: Action::Outline,
        name: "outline",
        short: "outline",
        description: "Open the document outline",
        defaults: &["o"],
    },
    Meta {
        action: Action::FollowLink,
        name: "follow_link",
        short: "follow link",
        description: "Follow the link on the current line (Space also opens files in the explorer)",
        defaults: &["space"],
    },
    Meta {
        action: Action::Search,
        name: "search",
        short: "search",
        description: "Search in the document",
        defaults: &["/"],
    },
    Meta {
        action: Action::SearchNext,
        name: "search_next",
        short: "next match",
        description: "Next search match",
        defaults: &["n"],
    },
    Meta {
        action: Action::SearchPrev,
        name: "search_prev",
        short: "prev match",
        description: "Previous search match",
        defaults: &["N"],
    },
    Meta {
        action: Action::FocusNext,
        name: "focus_next",
        short: "next panel",
        description: "Switch focus between panels (explorer, document, terminal)",
        defaults: &["tab", "prefix o"],
    },
    Meta {
        action: Action::FocusPrev,
        name: "focus_prev",
        short: "prev panel",
        description: "Switch focus to the previous panel",
        defaults: &["backtab", "prefix p"],
    },
    Meta {
        action: Action::Edit,
        name: "edit",
        short: "edit",
        description: "Edit the current file inside runetui (vim)",
        defaults: &["e", "ctrl+e"],
    },
    Meta {
        action: Action::ToggleExplorer,
        name: "toggle_explorer",
        short: "explorer",
        description: "Show or hide the explorer",
        defaults: &["prefix b"],
    },
    Meta {
        action: Action::ToggleTerminal,
        name: "toggle_terminal",
        short: "terminal",
        description: "Show or hide the terminal",
        defaults: &["ctrl+t", "prefix t"],
    },
    Meta {
        action: Action::CycleTheme,
        name: "cycle_theme",
        short: "theme",
        description: "Cycle the colour theme",
        defaults: &["T"],
    },
    Meta {
        action: Action::ToggleHidden,
        name: "toggle_hidden",
        short: "hidden",
        description: "Toggle hidden files",
        defaults: &["H"],
    },
    Meta {
        action: Action::ToggleGitignore,
        name: "toggle_gitignore",
        short: "gitignore",
        description: "Toggle .gitignore filtering",
        defaults: &["I"],
    },
    Meta {
        action: Action::CycleIcons,
        name: "cycle_icons",
        short: "icons",
        description: "Cycle icon style (nerd / unicode / ascii)",
        defaults: &["F"],
    },
    Meta {
        action: Action::Refresh,
        name: "refresh",
        short: "refresh",
        description: "Refresh the project tree / reload the document",
        defaults: &["r"],
    },
    Meta {
        action: Action::HistoryBack,
        name: "history_back",
        short: "back",
        description: "History back",
        defaults: &["ctrl+o"],
    },
    Meta {
        action: Action::HistoryForward,
        name: "history_forward",
        short: "forward",
        description: "History forward",
        defaults: &["ctrl+i"],
    },
    Meta {
        action: Action::HalfPageDown,
        name: "half_page_down",
        short: "half page down",
        description: "Scroll half a page down",
        defaults: &["ctrl+d"],
    },
    Meta {
        action: Action::HalfPageUp,
        name: "half_page_up",
        short: "half page up",
        description: "Scroll half a page up",
        defaults: &["ctrl+u"],
    },
    Meta {
        action: Action::PageDown,
        name: "page_down",
        short: "page down",
        description: "Scroll a page down (PageUp / PageDown always work)",
        defaults: &["ctrl+f"],
    },
    Meta {
        action: Action::ExplorerShrink,
        name: "explorer_shrink",
        short: "explorer −",
        description: "Shrink the explorer (left panel)",
        defaults: &["prefix left", "ctrl+left", "<"],
    },
    Meta {
        action: Action::ExplorerGrow,
        name: "explorer_grow",
        short: "explorer +",
        description: "Grow the explorer (left panel)",
        defaults: &["prefix right", "ctrl+right", ">"],
    },
    Meta {
        action: Action::TerminalGrow,
        name: "terminal_grow",
        short: "terminal +",
        description: "Grow the terminal (bottom panel)",
        defaults: &["prefix up", "ctrl+up"],
    },
    Meta {
        action: Action::TerminalShrink,
        name: "terminal_shrink",
        short: "terminal −",
        description: "Shrink the terminal (bottom panel)",
        defaults: &["prefix down", "ctrl+down"],
    },
    Meta {
        action: Action::ForceClose,
        name: "force_close",
        short: "kill pane",
        description: "Kill the focused editor or terminal, even if it is hung",
        defaults: &["prefix x"],
    },
];

impl Action {
    pub fn all() -> impl Iterator<Item = Action> {
        META.iter().map(|meta| meta.action)
    }

    fn meta(self) -> &'static Meta {
        META.iter()
            .find(|meta| meta.action == self)
            .expect("every action has metadata")
    }

    /// Name used in `config.toml`.
    pub fn name(self) -> &'static str {
        self.meta().name
    }

    /// One or two words, used in hints.
    pub fn short(self) -> &'static str {
        self.meta().short
    }

    #[allow(dead_code)]
    pub fn description(self) -> &'static str {
        self.meta().description
    }

    pub fn defaults(self) -> &'static [&'static str] {
        self.meta().defaults
    }

    pub fn from_name(name: &str) -> Option<Action> {
        META.iter()
            .find(|meta| meta.name == name.trim())
            .map(|meta| meta.action)
    }
}

// ------------------------------------------------------------------- chords

/// One key press with its modifiers.  For characters the case carries the
/// shift state (`'T'` is Shift+T), so `shift` only matters for named keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Chord {
    /// Parse `"ctrl+b"`, `"alt+x"`, `"shift+tab"`, `"f5"`, `"T"`, `"esc"`...
    pub fn parse(spec: &str) -> Result<Chord, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty key".into());
        }
        // A literal plus sign, alone or as the key after modifiers.
        let (modifier_part, key_part) = if spec == "+" {
            ("", "+")
        } else if let Some(rest) = spec.strip_suffix("++") {
            (rest, "+")
        } else if let Some((rest, key)) = spec.rsplit_once('+') {
            (rest, key)
        } else {
            ("", spec)
        };

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        for modifier in modifier_part.split('+').filter(|part| !part.is_empty()) {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "ctl" => ctrl = true,
                "alt" | "meta" | "opt" | "option" => alt = true,
                "shift" => shift = true,
                other => return Err(format!("unknown modifier “{other}”")),
            }
        }

        let lower = key_part.to_ascii_lowercase();
        let code = match lower.as_str() {
            "esc" | "escape" => KeyCode::Esc,
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            _ => {
                if let Some(number) = lower.strip_prefix('f') {
                    match number.parse::<u8>() {
                        Ok(number) if (1..=24).contains(&number) => KeyCode::F(number),
                        _ => single_char(key_part)?,
                    }
                } else {
                    single_char(key_part)?
                }
            }
        };

        // Shift on a letter is expressed through its case; with Ctrl the
        // terminal always reports the lowercase letter.
        if let KeyCode::Char(character) = code {
            if ctrl && character.is_ascii_uppercase() {
                return Ok(Chord {
                    code: KeyCode::Char(character.to_ascii_lowercase()),
                    ctrl,
                    alt,
                    shift: false,
                });
            }
            if shift && character.is_ascii_lowercase() {
                return Ok(Chord {
                    code: KeyCode::Char(character.to_ascii_uppercase()),
                    ctrl,
                    alt,
                    shift: false,
                });
            }
            return Ok(Chord {
                code,
                ctrl,
                alt,
                shift: false,
            });
        }
        Ok(Chord {
            code,
            ctrl,
            alt,
            shift,
        })
    }

    /// Build the chord of a key event, folding Shift into letter case.
    pub fn from_event(key: &KeyEvent) -> Chord {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char(character) => {
                let character = if ctrl {
                    character.to_ascii_lowercase()
                } else if shift && character.is_ascii_lowercase() {
                    character.to_ascii_uppercase()
                } else {
                    character
                };
                Chord {
                    code: KeyCode::Char(character),
                    ctrl,
                    alt,
                    shift: false,
                }
            }
            // Terminals report Shift+Tab as BackTab, sometimes with SHIFT.
            KeyCode::BackTab => Chord {
                code: KeyCode::BackTab,
                ctrl,
                alt,
                shift: false,
            },
            code => Chord {
                code,
                ctrl,
                alt,
                shift,
            },
        }
    }

    /// Exact match against a key event.
    pub fn matches(&self, key: &KeyEvent) -> bool {
        *self == Chord::from_event(key)
    }

    /// Match for the second key of a sequence.  Named keys ignore Ctrl and
    /// Shift unless the binding asks for them, so `Ctrl+B` followed by
    /// `Ctrl+←` (modifier still held) works like `Ctrl+B` then `←`.
    fn matches_second(&self, key: &KeyEvent) -> bool {
        let other = Chord::from_event(key);
        if matches!(self.code, KeyCode::Char(_)) {
            return *self == other;
        }
        self.code == other.code
            && self.alt == other.alt
            && (self.ctrl == other.ctrl || !self.ctrl)
            && (self.shift == other.shift || !self.shift)
    }

    /// The key event this chord stands for (used to pass a key through).
    pub fn to_event(self) -> KeyEvent {
        let mut modifiers = KeyModifiers::NONE;
        if self.ctrl {
            modifiers |= KeyModifiers::CONTROL;
        }
        if self.alt {
            modifiers |= KeyModifiers::ALT;
        }
        if self.shift {
            modifiers |= KeyModifiers::SHIFT;
        }
        KeyEvent::new(self.code, modifiers)
    }

    /// Config-file spelling of this key, the inverse of [`Chord::parse`]:
    /// `ctrl+r`, `alt+x`, `f5`, `T`.
    pub fn spec(&self) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push_str("ctrl+");
        }
        if self.alt {
            text.push_str("alt+");
        }
        if self.shift {
            text.push_str("shift+");
        }
        match self.code {
            KeyCode::Char(' ') => text.push_str("space"),
            KeyCode::Char(character) => text.push(character),
            KeyCode::Esc => text.push_str("esc"),
            KeyCode::Enter => text.push_str("enter"),
            KeyCode::Tab => text.push_str("tab"),
            KeyCode::BackTab => text.push_str("backtab"),
            KeyCode::Backspace => text.push_str("backspace"),
            KeyCode::Delete => text.push_str("delete"),
            KeyCode::Insert => text.push_str("insert"),
            KeyCode::Home => text.push_str("home"),
            KeyCode::End => text.push_str("end"),
            KeyCode::PageUp => text.push_str("pageup"),
            KeyCode::PageDown => text.push_str("pagedown"),
            KeyCode::Up => text.push_str("up"),
            KeyCode::Down => text.push_str("down"),
            KeyCode::Left => text.push_str("left"),
            KeyCode::Right => text.push_str("right"),
            KeyCode::F(number) => text.push_str(&format!("f{number}")),
            other => text.push_str(&format!("{other:?}").to_ascii_lowercase()),
        }
        text
    }

    /// Human-readable label: `Ctrl+B`, `Tab`, `←`, `T`.
    pub fn label(&self) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push_str("Ctrl+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => text.push_str("Space"),
            KeyCode::Char(character) => {
                if self.ctrl || self.alt {
                    text.push(character.to_ascii_uppercase());
                } else {
                    text.push(character);
                }
            }
            KeyCode::Esc => text.push_str("Esc"),
            KeyCode::Enter => text.push_str("Enter"),
            KeyCode::Tab => text.push_str("Tab"),
            KeyCode::BackTab => text.push_str("Shift+Tab"),
            KeyCode::Backspace => text.push_str("Backspace"),
            KeyCode::Delete => text.push_str("Del"),
            KeyCode::Insert => text.push_str("Ins"),
            KeyCode::Home => text.push_str("Home"),
            KeyCode::End => text.push_str("End"),
            KeyCode::PageUp => text.push_str("PgUp"),
            KeyCode::PageDown => text.push_str("PgDn"),
            KeyCode::Up => text.push('↑'),
            KeyCode::Down => text.push('↓'),
            KeyCode::Left => text.push('←'),
            KeyCode::Right => text.push('→'),
            KeyCode::F(number) => text.push_str(&format!("F{number}")),
            other => text.push_str(&format!("{other:?}")),
        }
        text
    }
}

fn single_char(token: &str) -> Result<KeyCode, String> {
    let mut characters = token.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Ok(KeyCode::Char(character)),
        _ => Err(format!("unknown key “{token}”")),
    }
}

// ----------------------------------------------------------------- bindings

/// Config value for one action: a single binding or a list of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    One(String),
    Many(Vec<String>),
}

impl KeySpec {
    pub fn list(&self) -> Vec<String> {
        match self {
            KeySpec::One(value) => vec![value.clone()],
            KeySpec::Many(values) => values.clone(),
        }
    }
}

pub const DEFAULT_PREFIX: &str = "ctrl+r";

/// The default `[keys]` table, written to `config.toml` so it can be edited.
pub fn default_specs() -> BTreeMap<String, KeySpec> {
    Action::all()
        .map(|action| {
            let defaults = action.defaults();
            let spec = if defaults.len() == 1 {
                KeySpec::One(defaults[0].to_string())
            } else {
                KeySpec::Many(defaults.iter().map(|value| value.to_string()).collect())
            };
            (action.name().to_string(), spec)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    chords: Vec<Chord>,
    action: Action,
}

/// What a key press means given the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Run this action.
    Action(Action),
    /// First key of a sequence: wait for the next one.
    Pending(Chord),
    /// The prefix was pressed twice: send that key to the focused pane.
    Literal(Chord),
    /// A sequence was started but the next key is not part of one.
    Cancel,
    /// Not a shortcut: let the pane handle the key.
    Pass,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    prefix: Chord,
    bindings: Vec<Binding>,
    /// Problems found in the configuration (the defaults were used instead).
    pub warnings: Vec<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_config(DEFAULT_PREFIX, &BTreeMap::new())
    }
}

impl Keymap {
    /// Build the key map from the configuration.  Missing actions and invalid
    /// bindings fall back to their defaults and leave a warning behind.
    pub fn from_config(prefix: &str, specs: &BTreeMap<String, KeySpec>) -> Self {
        let mut warnings = Vec::new();
        let prefix_chord = match Chord::parse(prefix) {
            Ok(chord) => chord,
            Err(error) => {
                warnings.push(format!("Invalid prefix “{prefix}”: {error}; using {DEFAULT_PREFIX}"));
                Chord::parse(DEFAULT_PREFIX).expect("the default prefix parses")
            }
        };

        for name in specs.keys() {
            if Action::from_name(name).is_none() {
                warnings.push(format!("Unknown shortcut “{name}” in [keys] was ignored"));
            }
        }

        let mut bindings = Vec::new();
        for action in Action::all() {
            let configured = specs.get(action.name()).map(KeySpec::list);
            let parsed = configured
                .as_ref()
                .map(|list| parse_list(list, prefix_chord))
                .filter(|result| match result {
                    Ok(_) => true,
                    Err(error) => {
                        warnings.push(format!("Shortcut {}: {error}; using the defaults", action.name()));
                        false
                    }
                });
            let chords = match parsed {
                Some(Ok(list)) => list,
                _ => {
                    let defaults: Vec<String> =
                        action.defaults().iter().map(|value| value.to_string()).collect();
                    parse_list(&defaults, prefix_chord).expect("default bindings parse")
                }
            };
            for sequence in chords {
                bindings.push(Binding {
                    chords: sequence,
                    action,
                });
            }
        }

        Self {
            prefix: prefix_chord,
            bindings,
            warnings,
        }
    }

    #[allow(dead_code)]
    pub fn prefix(&self) -> Chord {
        self.prefix
    }

    pub fn prefix_label(&self) -> String {
        self.prefix.label()
    }

    /// Can `chord` become the prefix?  Explains why not otherwise.  The prefix
    /// must survive being typed in a terminal and must not steal a shortcut.
    pub fn check_prefix(&self, chord: Chord) -> Result<(), String> {
        if chord == self.prefix {
            return Ok(());
        }
        let function_key = matches!(chord.code, KeyCode::F(_));
        if !(chord.ctrl || chord.alt || function_key) {
            return Err(format!(
                "{} needs Ctrl or Alt (or use an F key): a bare key is typed text",
                chord.label()
            ));
        }
        if chord.ctrl && !chord.alt {
            let twin = match chord.code {
                KeyCode::Char('i') => Some("Tab"),
                KeyCode::Char('m') | KeyCode::Char('j') => Some("Enter"),
                KeyCode::Char('h') => Some("Backspace"),
                KeyCode::Char('[') => Some("Esc"),
                _ => None,
            };
            if let Some(twin) = twin {
                return Err(format!(
                    "{} cannot be told apart from {twin} in most terminals",
                    chord.label()
                ));
            }
        }
        for binding in &self.bindings {
            let first = binding.chords[0];
            if first == chord && first != self.prefix {
                return Err(format!(
                    "{} is already used by “{}”",
                    chord.label(),
                    binding.action.short()
                ));
            }
        }
        Ok(())
    }

    /// Decide what `key` means.  While `captured` (the terminal or an embedded
    /// editor owns the keyboard) only the prefix key is intercepted, so every
    /// other key reaches the program running there.
    pub fn resolve(&self, pending: Option<Chord>, key: &KeyEvent, captured: bool) -> Resolution {
        if let Some(first) = pending {
            if first.matches(key) {
                return Resolution::Literal(first);
            }
            for binding in &self.bindings {
                if binding.chords.len() == 2
                    && binding.chords[0] == first
                    && binding.chords[1].matches_second(key)
                {
                    return Resolution::Action(binding.action);
                }
            }
            return Resolution::Cancel;
        }

        if captured {
            return if self.prefix.matches(key) {
                Resolution::Pending(self.prefix)
            } else {
                Resolution::Pass
            };
        }

        for binding in &self.bindings {
            if binding.chords.len() == 1 && binding.chords[0].matches(key) {
                return Resolution::Action(binding.action);
            }
        }
        for binding in &self.bindings {
            if binding.chords.len() == 2 && binding.chords[0].matches(key) {
                return Resolution::Pending(binding.chords[0]);
            }
        }
        Resolution::Pass
    }

    /// Labels of the bindings of `action`, e.g. `["Ctrl+B ←", "Ctrl+←"]`.
    pub fn labels(&self, action: Action) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|binding| binding.action == action)
            .map(|binding| {
                binding
                    .chords
                    .iter()
                    .map(Chord::label)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    /// Up to `limit` bindings of `action` joined with ` / `.
    pub fn describe(&self, action: Action, limit: usize) -> String {
        let labels = self.labels(action);
        if labels.is_empty() {
            return "unbound".to_string();
        }
        labels.into_iter().take(limit).collect::<Vec<_>>().join(" / ")
    }

    /// First single-key binding of `action`, for compact hints.
    pub fn primary(&self, action: Action) -> Option<Chord> {
        self.bindings
            .iter()
            .find(|binding| binding.action == action && binding.chords.len() == 1)
            .map(|binding| binding.chords[0])
    }

    /// What can follow `first`: `(label of the second key, action)`.
    pub fn follow_ups(&self, first: Chord) -> Vec<(String, Action)> {
        self.bindings
            .iter()
            .filter(|binding| binding.chords.len() == 2 && binding.chords[0] == first)
            .map(|binding| (binding.chords[1].label(), binding.action))
            .collect()
    }
}

fn parse_list(list: &[String], prefix: Chord) -> Result<Vec<Vec<Chord>>, String> {
    let mut parsed = Vec::new();
    for entry in list {
        let parts: Vec<&str> = entry.split_whitespace().collect();
        if parts.is_empty() || parts.len() > 2 {
            return Err(format!("“{entry}” must be one key or two keys separated by a space"));
        }
        let mut sequence = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            if index == 0 && part.eq_ignore_ascii_case("prefix") {
                if parts.len() == 1 {
                    return Err("“prefix” needs a second key".into());
                }
                sequence.push(prefix);
            } else {
                sequence.push(Chord::parse(part).map_err(|error| format!("“{entry}”: {error}"))?);
            }
        }
        parsed.push(sequence);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn plain(character: char) -> KeyEvent {
        key(KeyCode::Char(character), KeyModifiers::NONE)
    }

    #[test]
    fn chords_parse_in_many_spellings() {
        let ctrl_b = Chord::parse("ctrl+b").unwrap();
        assert_eq!(ctrl_b, Chord::parse("Ctrl+B").unwrap());
        assert!(ctrl_b.ctrl && ctrl_b.code == KeyCode::Char('b'));

        assert_eq!(Chord::parse("T").unwrap().code, KeyCode::Char('T'));
        assert_eq!(Chord::parse("shift+t").unwrap().code, KeyCode::Char('T'));
        assert_eq!(Chord::parse("f5").unwrap().code, KeyCode::F(5));
        assert_eq!(Chord::parse("esc").unwrap().code, KeyCode::Esc);
        assert_eq!(Chord::parse("space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(Chord::parse("+").unwrap().code, KeyCode::Char('+'));
        assert_eq!(Chord::parse("ctrl++").unwrap().code, KeyCode::Char('+'));
        let arrow = Chord::parse("ctrl+left").unwrap();
        assert!(arrow.ctrl && arrow.code == KeyCode::Left);
    }

    #[test]
    fn bad_chords_are_rejected() {
        assert!(Chord::parse("").is_err());
        assert!(Chord::parse("hyper+x").is_err());
        assert!(Chord::parse("ctrl+banana").is_err());
        assert!(Chord::parse("f99").is_err());
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(Chord::parse("ctrl+b").unwrap().label(), "Ctrl+B");
        assert_eq!(Chord::parse("left").unwrap().label(), "←");
        assert_eq!(Chord::parse("tab").unwrap().label(), "Tab");
        assert_eq!(Chord::parse("T").unwrap().label(), "T");
        assert_eq!(Chord::parse("f12").unwrap().label(), "F12");
    }

    #[test]
    fn shift_is_folded_into_letter_case() {
        let map = Keymap::default();
        let shifted = key(KeyCode::Char('t'), KeyModifiers::SHIFT);
        assert_eq!(
            map.resolve(None, &shifted, false),
            Resolution::Action(Action::CycleTheme)
        );
        assert_eq!(
            map.resolve(None, &plain('T'), false),
            Resolution::Action(Action::CycleTheme)
        );
    }

    #[test]
    fn default_bindings_do_not_collide() {
        let map = Keymap::default();
        assert!(map.warnings.is_empty(), "{:?}", map.warnings);
        let mut seen = std::collections::HashSet::new();
        for action in Action::all() {
            for label in map.labels(action) {
                assert!(seen.insert(label.clone()), "{label} is bound twice");
            }
        }
    }

    #[test]
    fn every_action_has_a_binding_and_a_name_that_roundtrips() {
        let map = Keymap::default();
        for action in Action::all() {
            assert!(!map.labels(action).is_empty(), "{} is unbound", action.name());
            assert_eq!(Action::from_name(action.name()), Some(action));
        }
    }

    #[test]
    fn prefix_then_arrow_resolves_even_with_ctrl_still_held() {
        let map = Keymap::default();
        let prefix = key(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let Resolution::Pending(first) = map.resolve(None, &prefix, false) else {
            panic!("the prefix starts a sequence");
        };
        for modifiers in [KeyModifiers::NONE, KeyModifiers::CONTROL] {
            assert_eq!(
                map.resolve(Some(first), &key(KeyCode::Left, modifiers), false),
                Resolution::Action(Action::ExplorerShrink)
            );
            assert_eq!(
                map.resolve(Some(first), &key(KeyCode::Down, modifiers), false),
                Resolution::Action(Action::TerminalShrink)
            );
        }
        assert_eq!(
            map.resolve(Some(first), &plain('t'), false),
            Resolution::Action(Action::ToggleTerminal)
        );
    }

    #[test]
    fn prefix_twice_is_literal_and_unknown_followups_cancel() {
        let map = Keymap::default();
        let prefix = key(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let first = map.prefix();
        assert_eq!(
            map.resolve(Some(first), &prefix, true),
            Resolution::Literal(first)
        );
        assert_eq!(map.resolve(Some(first), &plain('z'), true), Resolution::Cancel);
    }

    #[test]
    fn captured_panes_only_lose_the_prefix() {
        let map = Keymap::default();
        // `q`, Tab, Esc and Ctrl+C belong to the shell or editor.
        for pressed in [
            plain('q'),
            key(KeyCode::Tab, KeyModifiers::NONE),
            key(KeyCode::Esc, KeyModifiers::NONE),
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            key(KeyCode::Left, KeyModifiers::CONTROL),
        ] {
            assert_eq!(map.resolve(None, &pressed, true), Resolution::Pass);
        }
        assert!(matches!(
            map.resolve(None, &key(KeyCode::Char('r'), KeyModifiers::CONTROL), true),
            Resolution::Pending(_)
        ));
    }

    #[test]
    fn config_overrides_defaults_and_bad_entries_fall_back() {
        let mut specs = BTreeMap::new();
        specs.insert("quit".to_string(), KeySpec::One("x".into()));
        specs.insert(
            "toggle_terminal".to_string(),
            KeySpec::Many(vec!["f4".into(), "prefix `".into()]),
        );
        specs.insert("help".to_string(), KeySpec::One("hyper+h".into()));
        specs.insert("nonsense".to_string(), KeySpec::One("z".into()));
        let map = Keymap::from_config("ctrl+a", &specs);

        assert_eq!(map.resolve(None, &plain('x'), false), Resolution::Action(Action::Quit));
        assert_eq!(map.resolve(None, &plain('q'), false), Resolution::Pass, "old binding is gone");
        assert_eq!(
            map.resolve(None, &key(KeyCode::F(4), KeyModifiers::NONE), false),
            Resolution::Action(Action::ToggleTerminal)
        );
        // The invalid help binding falls back to `?`.
        assert_eq!(map.resolve(None, &plain('?'), false), Resolution::Action(Action::Help));
        assert!(map.warnings.iter().any(|text| text.contains("help")));
        assert!(map.warnings.iter().any(|text| text.contains("nonsense")));

        // The prefix is configurable and `prefix` in bindings follows it.
        let new_prefix = key(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let Resolution::Pending(first) = map.resolve(None, &new_prefix, false) else {
            panic!("ctrl+a is the prefix now");
        };
        assert_eq!(
            map.resolve(Some(first), &key(KeyCode::Right, KeyModifiers::NONE), false),
            Resolution::Action(Action::ExplorerGrow)
        );
        assert_eq!(
            map.resolve(None, &key(KeyCode::Char('b'), KeyModifiers::CONTROL), false),
            Resolution::Pass
        );
    }

    #[test]
    fn invalid_prefix_falls_back_with_a_warning() {
        let map = Keymap::from_config("wat+wat", &BTreeMap::new());
        assert_eq!(map.prefix_label(), "Ctrl+R");
        assert!(!map.warnings.is_empty());
    }

    #[test]
    fn a_single_prefix_binding_without_second_key_is_invalid() {
        let mut specs = BTreeMap::new();
        specs.insert("quit".to_string(), KeySpec::One("prefix".into()));
        let map = Keymap::from_config("ctrl+b", &specs);
        assert!(map.warnings.iter().any(|text| text.contains("quit")));
        assert_eq!(map.resolve(None, &plain('q'), false), Resolution::Action(Action::Quit));
    }

    #[test]
    fn follow_ups_list_what_the_prefix_offers() {
        let map = Keymap::default();
        let follow_ups = map.follow_ups(map.prefix());
        assert!(follow_ups.iter().any(|(label, action)| label == "←" && *action == Action::ExplorerShrink));
        assert!(follow_ups.iter().any(|(label, action)| label == "t" && *action == Action::ToggleTerminal));
    }

    #[test]
    fn default_specs_roundtrip_through_toml() {
        let specs = default_specs();
        assert_eq!(specs.len(), Action::all().count());
        #[derive(Serialize, Deserialize)]
        struct Holder {
            keys: BTreeMap<String, KeySpec>,
        }
        let text = toml::to_string_pretty(&Holder { keys: specs.clone() }).unwrap();
        let back: Holder = toml::from_str(&text).unwrap();
        assert_eq!(back.keys, specs);
    }

    #[test]
    fn spec_is_the_inverse_of_parse() {
        for spec in [
            "ctrl+r", "alt+x", "f5", "ctrl+alt+z", "T", "esc", "space", "ctrl+left", "pageup",
            "ctrl+]", "alt+X", "shift+tab", "ctrl++", "f12", "backspace", "delete",
        ] {
            let chord = Chord::parse(spec).unwrap();
            assert_eq!(Chord::parse(&chord.spec()).unwrap(), chord, "{spec}");
        }
        assert_eq!(Chord::parse("Ctrl+R").unwrap().spec(), "ctrl+r");
        assert_eq!(Chord::parse("alt+space").unwrap().spec(), "alt+space");
    }

    #[test]
    fn prefix_candidates_are_validated() {
        let keymap = Keymap::default();
        let ok = |spec: &str| keymap.check_prefix(Chord::parse(spec).unwrap());
        assert!(ok("ctrl+a").is_ok());
        assert!(ok("alt+x").is_ok());
        assert!(ok("f9").is_ok());
        assert!(ok("ctrl+r").is_ok(), "the current prefix is accepted");
        assert!(ok("x").unwrap_err().contains("Ctrl or Alt"));
        assert!(ok("ctrl+i").unwrap_err().contains("Tab"));
        assert!(ok("ctrl+m").unwrap_err().contains("Enter"));
        assert!(ok("ctrl+h").unwrap_err().contains("Backspace"));
        assert!(ok("ctrl+j").unwrap_err().contains("Enter"));
        assert!(ok("ctrl+[").unwrap_err().contains("Esc"));
        // Ctrl+T toggles the terminal and Ctrl+C quits by default.
        assert!(ok("ctrl+t").unwrap_err().contains("already used"));
        assert!(ok("ctrl+c").unwrap_err().contains("already used"));
    }
}

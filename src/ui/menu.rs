//! Esc menu, theme chooser and settings screen, in the style of btop and mctui.
//!
//! The menu is drawn *over* the running interface: the screen behind it is
//! only dimmed, never cleared, so the explorer, document and terminal stay
//! visible (and a theme change is previewed live behind the picker).  The
//! background is darkened by blending its colours towards black, like btop
//! does, so it stays readable.  The menu shows
//! a block-letter banner, the version under its right edge and three
//! double-stroke words (OPTIONS, HELP, QUIT) with the selected one
//! highlighted.  The banner uses fixed brand colours that do not follow the
//! theme, and plays a diagonal light sweep followed by a shimmering wave when
//! the menu is opened with Esc.  On small terminals the menu collapses to a compact framed list.
//! The options screen has tabs on top, the settings on the left and a
//! description of the highlighted one on the right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, MENU_ITEMS, OPTION_TABS, OptionItem};
use crate::editor::{editor_command, shell_command};
use crate::keymap::Action;
use crate::ui::themes::{Theme, ThemeName};

type Rgb = (u8, u8, u8);

/// Brand gradient of the banner, top-left to bottom-right.  Deliberately
/// independent of the theme: the logo looks the same whichever palette is
/// active.
pub const BANNER_TOP: Rgb = (0x4c, 0x8d, 0xff);
pub const BANNER_BOTTOM: Rgb = (0x9b, 0x6b, 0xff);
/// Colour of the letters the sweep has not reached yet.
const UNLIT: Rgb = (70, 70, 84);
/// Colour of the bright leading edge of the sweep.
const SWEEP_LIGHT: Rgb = (255, 255, 255);

/// Duration of the light sweep and of the shimmering wave that follows it.
pub const SWEEP_MS: u64 = 900;
pub const WAVE_MS: u64 = 2600;
/// Total length of the animation; afterwards the banner is static.
pub const ANIMATION_MS: u64 = SWEEP_MS + WAVE_MS;
/// Width of the bright edge of the sweep (fraction of the diagonal).
const SWEEP_EDGE: f64 = 0.25;
/// A cell is about twice as tall as wide: count each row as two columns so the
/// diagonal comes out at 45 degrees.
const ROW_WEIGHT: f64 = 2.0;
/// Full turns of the wave, bands visible at once and brightness of its troughs.
const WAVE_CYCLES: f64 = 2.0;
const WAVE_BANDS: f64 = 1.5;
const WAVE_TROUGH: f32 = 0.3;

fn lerp(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// Where the animation is at `elapsed`: how far the sweep got (0.0..=1.0) and
/// the wave as `(phase, amplitude)`.  `None` means the static banner.
fn animation_state(elapsed: Option<Duration>) -> (f64, (f64, f64)) {
    let Some(elapsed) = elapsed else {
        return (1.0, (0.0, 0.0));
    };
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms >= ANIMATION_MS as f64 {
        (1.0, (0.0, 0.0))
    } else if ms <= SWEEP_MS as f64 {
        (ms / SWEEP_MS as f64, (0.0, 0.0))
    } else {
        let t = (ms - SWEEP_MS as f64) / WAVE_MS as f64;
        // Fades in and out so the last frame is exactly the final gradient.
        let amplitude = ((t / 0.15).min(1.0) * ((1.0 - t) / 0.25).min(1.0)).max(0.0);
        (1.0, (t * WAVE_CYCLES, amplitude))
    }
}

/// Fixed brand colour of one banner cell at the given animation time.
pub fn banner_cell_color(row: usize, col: usize, elapsed: Option<Duration>) -> Color {
    let rows = BANNER.len();
    let width = banner_width() as usize;
    let span = width as f64 + rows.saturating_sub(1) as f64 * ROW_WEIGHT;
    let d = ((col as f64 + row as f64 * ROW_WEIGHT) / span.max(1.0)).clamp(0.0, 1.0);
    let finale = lerp(BANNER_TOP, BANNER_BOTTOM, d as f32);

    let (reveal, (phase, amplitude)) = animation_state(elapsed);
    let lead = reveal * (1.0 + SWEEP_EDGE);
    let k = lead - d;
    let mut color = if k >= SWEEP_EDGE {
        finale
    } else if k >= 0.0 {
        lerp(SWEEP_LIGHT, finale, (k / SWEEP_EDGE) as f32) // bright edge of the sweep
    } else {
        UNLIT
    };
    if amplitude > 0.0 {
        // The crest (the gradient colour) travels towards the bottom-right.
        let w = 0.5 + 0.5 * (std::f64::consts::TAU * (d * WAVE_BANDS - phase)).sin();
        let w = (w * w) as f32;
        let dark = lerp((0, 0, 0), color, WAVE_TROUGH);
        color = lerp(color, lerp(dark, color, w), amplitude as f32);
    }
    Color::Rgb(color.0, color.1, color.2)
}

const BANNER: [&str; 8] = [
    " ███████████                                         ███████████ █████  █████ █████ ",
    "░░███░░░░░███                                        ░█░░░███░░░█░░███  ░░███ ░░███ ",
    " ░███    ░███  █████ ████   ████████      ██████     ░   ░███  ░  ░███   ░███  ░███ ",
    " ░██████████  ░░███ ░███   ░░███░░███    ███░░███        ░███     ░███   ░███  ░███ ",
    " ░███░░░░░███  ░███ ░███    ░███ ░███   ░███████         ░███     ░███   ░███  ░███ ",
    " ░███    ░███  ░███ ░███    ░███ ░███   ░███░░░          ░███     ░███   ░███  ░███ ",
    " █████   █████ ░░████████   ████ █████  ░░██████         █████    ░░████████   █████",
    "░░░░░   ░░░░░   ░░░░░░░░   ░░░░ ░░░░░    ░░░░░░         ░░░░░      ░░░░░░░░   ░░░░░ ",
];

/// Double-stroke 3-row glyphs used for the menu words.
fn glyph(character: char) -> [&'static str; 3] {
    match character {
        'O' => ["╔═╗", "║ ║", "╚═╝"],
        'P' => ["╔═╗", "╠═╝", "╩  "],
        'T' => ["╔╦╗", " ║ ", " ╩ "],
        'I' => ["╦", "║", "╩"],
        'N' => ["╔╗╔", "║║║", "╝╚╝"],
        'S' => ["╔═╗", "╚═╗", "╚═╝"],
        'H' => ["╦ ╦", "╠═╣", "╩ ╩"],
        'E' => ["╔═╗", "║╣ ", "╚═╝"],
        'L' => ["╦  ", "║  ", "╩═╝"],
        'Q' => ["╔═╗ ", "║═╬╗", "╚═╝╚"],
        'U' => ["╦ ╦", "║ ║", "╚═╝"],
        _ => [" ", " ", " "],
    }
}

fn word_rows(word: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for character in word.chars() {
        let parts = glyph(character);
        for (row, part) in rows.iter_mut().zip(parts) {
            row.push_str(part);
        }
    }
    rows
}

fn banner_width() -> u16 {
    BANNER
        .iter()
        .map(|line| UnicodeWidthStr::width(*line))
        .max()
        .unwrap_or(0) as u16
}

/// How much of its colour the background keeps while a menu is open.
const DIM_KEEP: f32 = 0.42;

/// Approximate RGB of a terminal colour so it can be darkened.  `Reset` (the
/// terminal default) has no known value and takes `fallback`.
fn color_to_rgb(color: Color, fallback: Rgb) -> Rgb {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => fallback,
        Color::Indexed(index) => indexed_to_rgb(index),
        named => match named {
            Color::Black => (0, 0, 0),
            Color::Red => (205, 49, 49),
            Color::Green => (13, 188, 121),
            Color::Yellow => (229, 229, 16),
            Color::Blue => (36, 114, 200),
            Color::Magenta => (188, 63, 188),
            Color::Cyan => (17, 168, 205),
            Color::Gray => (204, 204, 204),
            Color::DarkGray => (102, 102, 102),
            Color::LightRed => (241, 76, 76),
            Color::LightGreen => (35, 209, 139),
            Color::LightYellow => (245, 245, 67),
            Color::LightBlue => (59, 142, 234),
            Color::LightMagenta => (214, 112, 214),
            Color::LightCyan => (41, 184, 219),
            Color::White => (255, 255, 255),
            _ => fallback,
        },
    }
}

/// xterm 256-colour palette.
fn indexed_to_rgb(index: u8) -> Rgb {
    match index {
        0..=15 => {
            const BASE: [Rgb; 16] = [
                (0, 0, 0),
                (205, 49, 49),
                (13, 188, 121),
                (229, 229, 16),
                (36, 114, 200),
                (188, 63, 188),
                (17, 168, 205),
                (204, 204, 204),
                (102, 102, 102),
                (241, 76, 76),
                (35, 209, 139),
                (245, 245, 67),
                (59, 142, 234),
                (214, 112, 214),
                (41, 184, 219),
                (255, 255, 255),
            ];
            BASE[usize::from(index)]
        }
        16..=231 => {
            let cube = index - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
            (level(cube / 36), level(cube / 6 % 6), level(cube % 6))
        }
        232..=255 => {
            let gray = 8 + 10 * (index - 232);
            (gray, gray, gray)
        }
    }
}

fn darken(color: Color, fallback: Rgb) -> Color {
    let (r, g, b) = color_to_rgb(color, fallback);
    let scale = |value: u8| (f32::from(value) * DIM_KEEP) as u8;
    Color::Rgb(scale(r), scale(g), scale(b))
}

fn rgb_of(color: Color) -> Rgb {
    color_to_rgb(color, (128, 128, 128))
}

/// Darken everything already drawn in `area`, like btop does behind its menu:
/// foreground and background colours are blended towards black, so the
/// interface behind stays readable but clearly recedes.  (`Modifier::DIM` is
/// not used: many terminals ignore it.)
fn dim_background(frame: &mut Frame, area: Rect, theme: &Theme) {
    let (fallback_fg, fallback_bg) = (rgb_of(theme.fg), rgb_of(theme.bg));
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let (fg, bg) = (cell.fg, cell.bg);
                cell.set_fg(darken(fg, fallback_fg));
                cell.set_bg(darken(bg, fallback_bg));
            }
        }
    }
}

fn centered_x(area: Rect, width: u16) -> u16 {
    area.x + area.width.saturating_sub(width) / 2
}

fn put(frame: &mut Frame, x: u16, y: u16, width: u16, text: String, style: Style) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))),
        Rect::new(x, y, width, 1),
    );
}

/// Draw `runs` starting at `(x, y)` without touching the blanks: spaces leave
/// whatever is behind them visible, so the text floats over the interface.
fn put_sparse(frame: &mut Frame, x: u16, y: u16, runs: &[(String, Style)]) {
    let area = frame.area();
    if y >= area.bottom() {
        return;
    }
    let buffer = frame.buffer_mut();
    let mut column = x;
    for (text, style) in runs {
        for character in text.chars() {
            let width = UnicodeWidthStr::width(character.to_string().as_str()) as u16;
            if column >= area.right() {
                return;
            }
            if character != ' ' {
                if let Some(cell) = buffer.cell_mut((column, y)) {
                    cell.set_symbol(&character.to_string());
                    // Keep the darkened background colour unless the style sets one.
                    cell.modifier = style.add_modifier;
                    if let Some(foreground) = style.fg {
                        cell.set_fg(foreground);
                    }
                    if let Some(background) = style.bg {
                        cell.set_bg(background);
                    }
                }
            }
            column = column.saturating_add(width.max(1));
        }
    }
}

fn put_text(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    put_sparse(frame, x, y, &[(text.to_string(), style)]);
}

// ------------------------------------------------------------------- menu

pub fn render_menu(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();
    dim_background(frame, area, &theme);

    let banner_w = banner_width();
    let needed_height = 11 + 3 * 4;
    if area.width >= banner_w + 4 && area.height >= needed_height {
        render_big_menu(frame, area, &theme, selected, banner_w, app.menu_animation());
    } else {
        render_compact_menu(frame, area, &theme, selected);
    }
    draw_corners(frame, area, app, &theme);
}

/// Colour of the star count.
pub const STAR_COLOR: Color = Color::Rgb(0xf5, 0xc2, 0x4b);

/// Star count of the repository at the top right and the credit at the
/// bottom right of the screen.
fn draw_corners(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    if let Some(text) = crate::stars::label(app.stars.state()) {
        let width = UnicodeWidthStr::width(text.as_str()) as u16;
        if width + 3 <= area.width {
            // A leading blank keeps it apart from the header text.
            put_corner(
                frame,
                area.right() - width - 2,
                area.y,
                &format!(" {text}"),
                Style::default().fg(STAR_COLOR).add_modifier(Modifier::BOLD),
            );
        }
    }
    // The key hint is centred on the same row; leave it alone when they meet.
    let credit = CREDIT;
    let width = UnicodeWidthStr::width(credit) as u16;
    let hint_width = UnicodeWidthStr::width(MENU_HINT) as u16;
    let hint_end = centered_x(area, hint_width) + hint_width;
    let x = area.right().saturating_sub(width + 1);
    if width + 2 <= area.width && x > hint_end {
        put_corner(
            frame,
            x - 1,
            area.bottom() - 1,
            &format!(" {credit}"),
            Style::default().fg(theme.fg_muted),
        );
    }
}

/// Draw `text` at `(x, y)` over whatever is there, spaces included, keeping the
/// (darkened) background.  A word of the interface that the text would cut in
/// half is blanked completely, so no stray letters stay next to it.
fn put_corner(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    let area = frame.area();
    if y >= area.bottom() || x >= area.right() {
        return;
    }
    let width = UnicodeWidthStr::width(text) as u16;
    let buffer = frame.buffer_mut();
    let mut start = x;
    while start > area.x
        && buffer
            .cell((start - 1, y))
            .is_some_and(|cell| cell.symbol() != " ")
        && buffer.cell((x, y)).is_some_and(|cell| cell.symbol() != " ")
    {
        start -= 1;
    }
    for column in start..x {
        if let Some(cell) = buffer.cell_mut((column, y)) {
            cell.set_symbol(" ");
        }
    }
    let mut column = x;
    for character in text.chars() {
        if column >= area.right() || column >= x + width {
            break;
        }
        if let Some(cell) = buffer.cell_mut((column, y)) {
            cell.set_symbol(&character.to_string());
            cell.modifier = style.add_modifier;
            if let Some(foreground) = style.fg {
                cell.set_fg(foreground);
            }
        }
        column += UnicodeWidthStr::width(character.to_string().as_str()).max(1) as u16;
    }
    // Blank the rest of a word that continues past the end of the text.
    let mut tail = x.saturating_add(width);
    while tail < area.right()
        && buffer
            .cell((tail, y))
            .is_some_and(|cell| cell.symbol() != " ")
    {
        if let Some(cell) = buffer.cell_mut((tail, y)) {
            cell.set_symbol(" ");
        }
        tail += 1;
    }
}

/// Credit drawn at the bottom right of the Esc menu.
pub const CREDIT: &str = "Made By - fir3";
/// Key hint drawn centred on the menu's last row.
const MENU_HINT: &str = " ↑ / ↓ select · Enter open · Esc close ";

/// The banner plus the version under its right edge, in fixed brand colours.
/// With `elapsed` the opening animation is drawn at that point in time; `None`
/// draws the finished banner.
fn draw_banner(
    frame: &mut Frame,
    x: u16,
    top: u16,
    banner_w: u16,
    theme: &Theme,
    elapsed: Option<Duration>,
) {
    for (row, line) in BANNER.iter().enumerate() {
        let runs: Vec<(String, Style)> = line
            .chars()
            .enumerate()
            .map(|(col, character)| {
                let style = Style::default()
                    .fg(banner_cell_color(row, col, elapsed))
                    .add_modifier(Modifier::BOLD);
                (character.to_string(), style)
            })
            .collect();
        put_sparse(frame, x, top + row as u16, &runs);
    }
    // The version appears once the sweep has lit the whole logo.
    let sweep_done = elapsed.is_none_or(|time| time.as_millis() as u64 >= SWEEP_MS);
    if sweep_done {
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let version_w = UnicodeWidthStr::width(version.as_str()) as u16;
        put_text(
            frame,
            x + banner_w.saturating_sub(version_w),
            top + BANNER.len() as u16,
            &version,
            Style::default().fg(theme.fg_muted),
        );
    }
}

fn render_big_menu(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    selected: usize,
    banner_w: u16,
    elapsed: Option<Duration>,
) {
    // Banner, version, gap, then 3 words of 3 rows with 1 row between them.
    let total = 8 + 1 + 2 + 3 * 4;
    let top = area.y + area.height.saturating_sub(total) / 2;
    let banner_x = centered_x(area, banner_w);

    draw_banner(frame, banner_x, top, banner_w, theme, elapsed);

    let mut y = top + 11;
    for (index, word) in MENU_ITEMS.iter().enumerate() {
        let rows = word_rows(word);
        let width = UnicodeWidthStr::width(rows[0].as_str()) as u16;
        let x = centered_x(area, width);
        let style = if index == selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        let _ = width;
        for (offset, row) in rows.into_iter().enumerate() {
            put_text(frame, x, y + offset as u16, &row, style);
        }
        y += 4;
    }

    hint(frame, area, theme, MENU_HINT);
}

fn render_compact_menu(frame: &mut Frame, area: Rect, theme: &Theme, selected: usize) {
    let width = 26u16.min(area.width);
    let height = 7u16.min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed(theme, "RUNETUI");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let labels = ["OPTIONS", "HELP", "QUIT"];
    for (index, label) in labels.iter().enumerate() {
        let is_selected = index == selected;
        let row = inner.y + 1 + index as u16;
        if row >= inner.y + inner.height {
            break;
        }
        let style = if is_selected {
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg_sel)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        let text = format!("{} {label:<width$}", if is_selected { "❯" } else { " " }, width = inner.width as usize - 2);
        put(frame, inner.x, row, inner.width, text, style);
    }
}

fn framed<'a>(theme: &Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

fn hint(frame: &mut Frame, area: Rect, theme: &Theme, text: &str) {
    if area.height < 2 {
        return;
    }
    let width = UnicodeWidthStr::width(text) as u16;
    if width > area.width {
        return;
    }
    put_text(
        frame,
        centered_x(area, width),
        area.y + area.height - 1,
        text,
        Style::default().fg(theme.fg_muted),
    );
}

// ---------------------------------------------------------------- options

struct OptionText {
    label: &'static str,
    value: String,
    description: String,
}

fn on_off(value: bool) -> String {
    if value { "ON".into() } else { "OFF".into() }
}

fn option_text(app: &App, item: OptionItem) -> OptionText {
    match item {
        OptionItem::Theme => OptionText {
            label: "Theme",
            value: format!("«{}»", app.theme.label()),
            description: "Colour palette. Enter opens the theme picker with a live preview; \
                          Esc there restores the previous theme."
                .into(),
        },
        OptionItem::Icons => OptionText {
            label: "Icons",
            value: app.config.icon_style().slug().to_string(),
            description: "File and folder icons in the explorer. `nerd` needs a Nerd Font, \
                          `unicode` works everywhere and `ascii` uses plain letters. \
                          Enter cycles through them."
                .into(),
        },
        OptionItem::ShowExplorer => OptionText {
            label: "Show explorer",
            value: on_off(app.show_explorer),
            description: format!(
                "Show or hide the file tree on the left ({}).",
                app.keymap.describe(Action::ToggleExplorer, 1)
            ),
        },
        OptionItem::ShowTerminal => OptionText {
            label: "Show terminal",
            value: on_off(app.show_term),
            description: format!(
                "Show or hide the terminal panel at the bottom ({}). The shell keeps \
                 running while it is hidden.",
                app.keymap.describe(Action::ToggleTerminal, 2)
            ),
        },
        OptionItem::Mouse => OptionText {
            label: "Mouse",
            value: on_off(app.config.mouse),
            description: "ON: click focuses a panel, the wheel scrolls it and dragging selects \
                          text from that panel only and copies it (OSC 52). OFF: the terminal's \
                          own selection returns (it selects across panels)."
                .into(),
        },
        OptionItem::Keys => {
            let place = crate::config::Config::path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "config.toml".to_string());
            if app.capturing_prefix {
                OptionText {
                    label: "Key bindings",
                    value: "press a key…".to_string(),
                    description: "Press the key that should become the prefix: Ctrl+<letter>, \
                                  Alt+<key> or an F key. Backspace restores Ctrl+R, Esc cancels."
                        .into(),
                }
            } else {
                OptionText {
                    label: "Key bindings",
                    value: format!("prefix {}", app.keymap.prefix_label()),
                    description: format!(
                        "Enter, then press a key to make it the prefix (saved in config.toml). \
                         The other shortcuts are the [keys] table of {place} (e.g. quit = \
                         [\"q\", \"ctrl+c\"]); two-key shortcuts are written \"prefix left\"."
                    ),
                }
            }
        }
        OptionItem::Animation => OptionText {
            label: "Banner animation",
            value: on_off(app.config.banner_animation),
            description: "Light sweep and shimmer of the RuneTUI banner when the Esc menu \
                          opens. OFF shows the finished banner right away."
                .into(),
        },
        OptionItem::GithubStars => OptionText {
            label: "GitHub stars",
            value: on_off(app.config.show_stars),
            description: format!(
                "Show the star count of github.com/{} in the corner of the Esc menu. It is \
                 fetched once an hour in the background with curl and is the only network \
                 request runetui makes; OFF keeps it fully offline.",
                crate::stars::REPO
            ),
        },
        OptionItem::ShowHidden => OptionText {
            label: "Hidden files",
            value: on_off(app.explorer.show_hidden()),
            description: "List dotfiles and dot-directories in the explorer (`.`).".into(),
        },
        OptionItem::Gitignore => OptionText {
            label: "Respect .gitignore",
            value: on_off(app.explorer.respect_gitignore()),
            description: "Hide files ignored by git, such as `target/` or `node_modules/` (`I`)."
                .into(),
        },
        OptionItem::ExplorerWidth => OptionText {
            label: "Explorer width",
            value: format!("{}%", app.explorer.width_pct()),
            description: format!(
                "Share of the screen used by the explorer (left panel). Press Enter, then + / − \
                 (or ← →) to change it, Enter or Esc to finish. Also {}.",
                app.keymap.describe(Action::ExplorerShrink, 1)
            ),
        },
        OptionItem::EmbeddedEditor => OptionText {
            label: "Edit inside runetui",
            value: on_off(app.config.embedded_editor),
            description: "ON: `e` opens the editor in the document pane and returns here when \
                          you quit it (:wq). OFF: the terminal is handed to the editor, as a \
                          separate screen."
                .into(),
        },
        OptionItem::TerminalHeight => OptionText {
            label: "Terminal height",
            value: format!("{}%", app.term_pct),
            description: format!(
                "Share of the body used by the terminal (bottom panel). Press Enter, then + / − \
                 (or ↑ ↓) to change it, Enter or Esc to finish. Also {}.",
                app.keymap.describe(Action::TerminalGrow, 1)
            ),
        },
        OptionItem::TerminalShell => {
            let (program, _) = shell_command();
            OptionText {
                label: "Shell",
                value: program,
                description: "Program run in the terminal panel. It comes from $SHELL; change it \
                              in your environment."
                    .into(),
            }
        }
        OptionItem::EditorCommand => {
            let (program, args) = editor_command();
            let mut command = program;
            for argument in args {
                command.push(' ');
                command.push_str(&argument);
            }
            OptionText {
                label: "Editor",
                value: command,
                description: "Program used by `e`. It comes from $VISUAL, then $EDITOR, and \
                              defaults to vim. Change it in your shell profile."
                    .into(),
            }
        }
    }
}

pub fn render_options(frame: &mut Frame, area: Rect, app: &App, tab: usize, selected: usize) {
    if area.width < 30 || area.height < 8 {
        render_menu(frame, area, app, 0);
        return;
    }
    let theme = app.theme();
    dim_background(frame, area, &theme);

    let width = area.width.saturating_sub(4).min(84);
    let height = area.height.saturating_sub(2).min(16);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed(&theme, "Options");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    // Tabs.
    let mut spans = vec![Span::raw(" ")];
    for (index, entry) in OPTION_TABS.iter().enumerate() {
        let style = if index == tab {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        spans.push(Span::styled(format!(" {} ", entry.name), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(4),
    );
    let list_width = (body.width * 55 / 100).max(20).min(body.width);
    let list = Rect::new(body.x, body.y, list_width, body.height);
    let detail = Rect::new(
        body.x + list_width,
        body.y,
        body.width - list_width,
        body.height,
    );

    // Settings of the active tab.
    let items = OPTION_TABS[tab].items;
    for (index, item) in items.iter().enumerate() {
        let row = list.y + index as u16;
        if row >= list.y + list.height {
            break;
        }
        let mut text = option_text(app, *item);
        let is_selected = index == selected;
        let adjusting = is_selected && app.adjusting && item.is_adjustable();
        if adjusting {
            text.value = format!("◂ {} ▸", text.value);
        }
        let style = if is_selected {
            Style::default()
                .fg(theme.fg)
                .bg(theme.bg_sel)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        let marker = if is_selected { "❯ " } else { "  " };
        let value_style = if is_selected {
            style.fg(theme.accent)
        } else {
            Style::default().fg(theme.accent2)
        };
        let label_w = list.width as usize;
        let value_w = UnicodeWidthStr::width(text.value.as_str());
        let used = 2 + UnicodeWidthStr::width(text.label) + value_w;
        let gap = label_w.saturating_sub(used + 1).max(1);
        let line = Line::from(vec![
            Span::styled(format!("{marker}{}", text.label), style),
            Span::styled(" ".repeat(gap), style),
            Span::styled(text.value, value_style),
            Span::styled(" ", style),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(list.x, row, list.width, 1),
        );
    }

    // Description of the highlighted setting.
    if let Some(item) = items.get(selected) {
        let text = option_text(app, *item);
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.border));
        let area = block.inner(detail);
        frame.render_widget(block, detail);
        let lines = vec![
            Line::from(Span::styled(
                text.label,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::default(),
            Line::from(Span::styled(text.description, Style::default().fg(theme.fg_dim))),
        ];
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }),
            Rect::new(area.x + 1, area.y, area.width.saturating_sub(1), area.height),
        );
    }

    let hint = if app.adjusting {
        "+ / − adjust · Enter or Esc done"
    } else {
        "↑↓ move · ←→ tab · Enter change · Esc back"
    };
    put(
        frame,
        inner.x + 1,
        inner.y + inner.height - 1,
        inner.width.saturating_sub(2),
        hint.to_string(),
        Style::default().fg(if app.adjusting { theme.yellow } else { theme.fg_muted }),
    );
}

// ----------------------------------------------------------------- themes

/// Theme chooser.  It looks like the Esc menu: the same banner over the
/// dimmed interface, with the theme names listed under it.  Because the
/// interface behind it is live, moving the selection previews the theme.
pub fn render_themes(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();
    dim_background(frame, area, &theme);

    let count = ThemeName::ALL.len() as u16;
    let banner_w = banner_width();
    // Banner (8) + version row + gap + list + gap + title.
    let needed = 8 + 1 + 2 + count + 2;
    if area.width < banner_w + 4 || area.height < needed + 1 {
        render_themes_compact(frame, area, &theme, selected);
        return;
    }

    let top = area.y + area.height.saturating_sub(needed) / 2;
    draw_banner(frame, centered_x(area, banner_w), top, banner_w, &theme, None);

    let title = "Choose a theme — live preview";
    let title_w = UnicodeWidthStr::width(title) as u16;
    put_text(
        frame,
        centered_x(area, title_w),
        top + 10,
        title,
        Style::default().fg(theme.accent2).add_modifier(Modifier::BOLD),
    );

    let row_w = 34u16;
    let x = centered_x(area, row_w);
    for (index, name) in ThemeName::ALL.iter().enumerate() {
        let palette = name.theme();
        let is_selected = index == selected;
        let label_style = if is_selected {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        let runs = vec![
            (if is_selected { "❯ " } else { "  " }.to_string(), label_style),
            (format!("{:<17}", name.label()), label_style),
            ("■ ".to_string(), Style::default().fg(palette.accent)),
            ("■ ".to_string(), Style::default().fg(palette.green)),
            ("■ ".to_string(), Style::default().fg(palette.yellow)),
            ("■".to_string(), Style::default().fg(palette.red)),
        ];
        put_sparse(frame, x, top + 12 + index as u16, &runs);
    }
    hint(frame, area, &theme, " ↑ / ↓ preview · Enter apply · Esc cancel ");
}

fn render_themes_compact(frame: &mut Frame, area: Rect, theme: &Theme, selected: usize) {
    if area.width < 24 || area.height < 6 {
        return;
    }
    let count = ThemeName::ALL.len() as u16;
    let width = area.width.saturating_sub(6).clamp(24, 40);
    let height = (count + 2).min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed(theme, "Theme");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let visible = inner.height as usize;
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    for (row, (index, name)) in ThemeName::ALL.iter().enumerate().skip(offset).take(visible).enumerate() {
        let is_selected = index == selected;
        let style = if is_selected {
            Style::default().fg(theme.accent).bg(theme.bg_sel).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        let text = format!("{} {:<w$}", if is_selected { "❯" } else { " " }, name.label(), w = inner.width as usize - 2);
        put(frame, inner.x, inner.y + row as u16, inner.width, text, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(color: Color) -> Rgb {
        match color {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected an RGB colour, got {other:?}"),
        }
    }

    fn ms(value: u64) -> Option<Duration> {
        Some(Duration::from_millis(value))
    }

    #[test]
    fn banner_starts_unlit_and_ends_on_the_final_gradient() {
        assert_eq!(rgb(banner_cell_color(0, 1, ms(0))), UNLIT);
        assert_eq!(
            banner_cell_color(0, 1, ms(ANIMATION_MS)),
            banner_cell_color(0, 1, None),
            "the animation ends exactly on the static banner"
        );
        // The static gradient runs from the top-left brand colour to the bottom-right one.
        let first = rgb(banner_cell_color(0, 1, None));
        let last = rgb(banner_cell_color(BANNER.len() - 1, banner_width() as usize - 1, None));
        assert!(first.0.abs_diff(BANNER_TOP.0) < 8 && first.2.abs_diff(BANNER_TOP.2) < 8);
        assert!(last.0.abs_diff(BANNER_BOTTOM.0) < 8 && last.2.abs_diff(BANNER_BOTTOM.2) < 8);
    }

    #[test]
    fn the_sweep_lights_the_top_left_first() {
        let half = ms(SWEEP_MS / 2);
        let near = rgb(banner_cell_color(0, 1, half));
        let far = rgb(banner_cell_color(BANNER.len() - 1, banner_width() as usize - 1, half));
        assert_ne!(near, UNLIT, "the start of the diagonal is already lit");
        assert_eq!(far, UNLIT, "the end of the diagonal is not reached yet");
    }

    #[test]
    fn the_wave_changes_brightness_without_changing_the_hue_source() {
        let still = rgb(banner_cell_color(3, 40, None));
        let mut seen_different = false;
        for step in 0..40 {
            let at = ms(SWEEP_MS + WAVE_MS * 4 / 10 + step * 20);
            let wave = rgb(banner_cell_color(3, 40, at));
            assert!(wave.0 <= still.0 && wave.1 <= still.1 && wave.2 <= still.2, "troughs only darken");
            seen_different |= wave != still;
        }
        assert!(seen_different, "the wave is visible");
    }

    #[test]
    fn darken_scales_colours_and_resolves_the_terminal_default() {
        let Color::Rgb(r, g, b) = darken(Color::Rgb(200, 100, 50), (0, 0, 0)) else {
            panic!("darken returns RGB");
        };
        assert!((83..=84).contains(&r) && (41..=42).contains(&g) && (20..=21).contains(&b));
        assert_eq!(darken(Color::Black, (9, 9, 9)), Color::Rgb(0, 0, 0));
        // `Reset` has no value of its own: the theme fallback is used.
        assert_eq!(darken(Color::Reset, (100, 100, 100)), darken(Color::Rgb(100, 100, 100), (0, 0, 0)));
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        assert_eq!(indexed_to_rgb(231), (255, 255, 255));
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
    }
}

//! Terminal user interface.
//!
//! Rendering is a pure function of [`App`] state plus the frame area.  The
//! draw pass also reflows the document when the viewer width changes.

pub mod bar;
pub mod explorer;
pub mod header;
pub mod help;
pub mod icons;
pub mod layout;
pub mod markdown;
pub mod menu;
pub mod overlays;
pub mod statusbar;
pub mod terminal;
pub mod themes;

use ratatui::Frame;
use ratatui::widgets::{Block, Borders};

use crate::app::{App, Focus};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.set_terminal_area(area);
    app.star_rect.set(None);
    app.hits.borrow_mut().clear();
    let theme = app.theme();

    frame.render_widget(Block::default().style(theme.base()), area);

    header::render(frame, app.layout.header, app);

    if let Some(rect) = app.layout.explorer {
        explorer::render(frame, rect, app);
    }
    if let Some(rect) = app.layout.divider {
        frame.render_widget(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(theme.border()),
            rect,
        );
    }

    if app.editor.is_some() {
        let pane = app.layout.viewer;
        if let Some(editor) = app.editor.as_mut() {
            editor.resize(pane.height, pane.width);
        }
        if let Some(editor) = &app.editor {
            let focused = app.focus == Focus::Viewer && !app.overlay.is_open();
            editor.render(frame, pane, &theme, focused);
        }
    } else {
        markdown::render(frame, app.layout.viewer, app);
    }
    terminal::render(frame, app);

    // A mouse selection is painted over its own pane only, and its text is
    // read back from the screen once the button is released.
    if let Some(selection) = &app.selection {
        selection.highlight(frame.buffer_mut(), theme.accent, theme.bg);
    }
    app.capture_selection(frame.buffer_mut());

    statusbar::render(frame, app.layout.status, app);
    overlays::render(frame, area, app);
}

/// Draw one frame to text, for tests in other modules.
#[cfg(test)]
pub fn render_to_text_for_test(app: &mut App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Focus, Overlay};
    use crate::config::Config;
    use crate::ui::themes::ThemeName;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("README.md"),
            b"# Home\n\nWelcome to the project.\n\n## Section\n\nSome text.\n",
        )
        .unwrap();
        fs::write(dir.path().join("other.rs"), b"fn main() {}\n").unwrap();
        dir
    }

    fn app(dir: &tempfile::TempDir) -> App {
        let mut app = App::with_config(Some(dir.path()), Config::default());
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area();
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_to_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn renders_header_explorer_document_and_status() {
        let dir = project();
        let mut app = app(&dir);
        let text = render_to_text(&mut app, 100, 30);
        assert!(text.contains("runetui"));
        assert!(text.contains("README.md"));
        assert!(text.contains("Home"));
        assert!(text.contains("Welcome to the project."));
        assert!(text.contains("EXPLORER"));
        // The theme name is not in the header any more.
        assert!(!text.contains("Catppuccin Mocha"));
        // Bottom-right hints: `edit`, `Esc` and `?` on their own.
        let status = text.lines().last().unwrap();
        assert!(status.contains("edit"), "{status}");
        assert!(status.contains("Esc"), "{status}");
        assert!(status.contains(" ? "), "{status}");
        assert!(!status.contains("? help"));
        assert!(!status.contains("Esc menu"));
    }

    #[test]
    fn header_does_not_repeat_the_project_name_or_path() {
        let dir = project();
        let mut app = app(&dir);
        let text = render_to_text(&mut app, 120, 30);
        let header = text.lines().next().unwrap();
        let path = dir.path().display().to_string();
        assert!(header.contains("runetui"));
        assert!(!header.contains(&path), "no project path in the header: {header}");
        let name = dir.path().file_name().unwrap().to_string_lossy().to_string();
        assert!(!header.contains(&name), "no project name in the header: {header}");
    }

    #[test]
    fn the_e_of_edit_is_bold() {
        let dir = project();
        let mut app = app(&dir);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let y = 29;
        let row: Vec<&str> = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
        let position = (0..97)
            .find(|&x| row[x..x + 4].concat() == "edit")
            .expect("edit hint in the status bar");
        let e = &buffer[(position as u16, y)];
        let d = &buffer[(position as u16 + 1, y)];
        assert!(e.modifier.contains(ratatui::style::Modifier::BOLD));
        assert!(!d.modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn explorer_draws_nerd_icons_and_indent_guides() {
        let dir = project();
        fs::create_dir_all(dir.path().join("src").join("deep")).unwrap();
        fs::write(
            dir.path().join("src").join("deep").join("a.rs"),
            b"fn a() {}\n",
        )
        .unwrap();
        let mut config = Config::default();
        config.set_icon_style(crate::ui::icons::IconStyle::Nerd);
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        app.explorer.refresh();
        for name in ["src", "src/deep"] {
            assert!(app.explorer.select_visible_path(&dir.path().join(name)));
            app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        }
        let text = render_to_text(&mut app, 100, 30);
        assert!(text.contains('\u{f07c}'), "open folder glyph");
        assert!(text.contains('\u{e7a8}'), "rust file glyph");
        assert!(text.contains('│'), "indent guide");
    }

    #[test]
    fn explorer_falls_back_to_ascii_icons() {
        let dir = project();
        let mut config = Config::default();
        config.set_icon_style(crate::ui::icons::IconStyle::Ascii);
        let mut app = App::with_config(Some(dir.path()), config);
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        let text = render_to_text(&mut app, 100, 30);
        assert!(!text.contains('\u{f07b}'));
        assert!(text.contains("other.rs"));
    }

    #[test]
    fn help_overlay_draws_bindings() {
        let dir = project();
        let mut app = app(&dir);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::Help { .. }));
        let text = render_to_text(&mut app, 110, 40);
        assert!(text.contains("Help"));
        assert!(text.contains("Switch focus"));
        assert!(text.contains("Ctrl+R"));
    }

    #[test]
    fn theme_menu_draws_all_themes() {
        let dir = project();
        let mut app = app(&dir);
        app.overlay = Overlay::Themes {
            selected: 0,
            original: app.theme,
        };
        let text = render_to_text(&mut app, 110, 40);
        assert!(text.contains("Choose a theme"));
        for name in ThemeName::ALL {
            assert!(text.contains(name.label()), "{}", name.label());
        }
        assert!(text.contains("live preview"));
    }

    #[test]
    fn esc_menu_shows_banner_version_and_the_three_words() {
        let dir = project();
        let mut app = app(&dir);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::Menu { .. }));
        // The version appears once the opening sweep has finished.
        app.finish_menu_animation();
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("███████████"), "block-letter banner");
        assert!(text.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        // OPTIONS, HELP and QUIT are drawn with the double-stroke font.
        assert!(text.contains("╔═╗"));
        assert!(text.contains("╠═╣"), "the H of HELP");
        assert!(text.contains("╚═╝╚"), "the Q of QUIT");
        // The interface behind the menu stays visible (dimmed, not cleared).
        assert!(text.contains("README.md"), "explorer visible behind the menu");
        assert!(text.contains("EXPLORER") || text.contains("Home"));
    }

    fn banner_cell_color(app: &mut App) -> ratatui::style::Color {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        for y in 0..40 {
            for x in 0..120 {
                if buffer[(x, y)].symbol() == "█" {
                    return buffer[(x, y)].fg;
                }
            }
        }
        panic!("banner not drawn");
    }

    #[test]
    fn banner_colours_do_not_follow_the_theme() {
        let dir = project();
        let mut app = app(&dir);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let first = banner_cell_color(&mut app);
        app.theme = ThemeName::White;
        let second = banner_cell_color(&mut app);
        app.theme = ThemeName::Dracula;
        let third = banner_cell_color(&mut app);
        assert_eq!(first, second);
        assert_eq!(first, third);
        assert!(matches!(first, ratatui::style::Color::Rgb(..)));
    }

    fn render_buffer(app: &mut App) -> Buffer {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn brightness(color: ratatui::style::Color) -> Option<u32> {
        match color {
            ratatui::style::Color::Rgb(r, g, b) => Some(u32::from(r) + u32::from(g) + u32::from(b)),
            _ => None,
        }
    }

    #[test]
    fn menu_darkens_the_background_but_keeps_it_readable() {
        let dir = project();
        let mut app = app(&dir);
        let before = render_buffer(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let after = render_buffer(&mut app);
        let (mut darker, mut brighter, mut kept) = (0, 0, 0);
        for y in 0..40 {
            for x in 0..120 {
                let (a, b) = (&before[(x, y)], &after[(x, y)]);
                if a.symbol() != b.symbol() {
                    continue; // the banner and the menu words
                }
                kept += 1;
                if let (Some(old), Some(new)) = (brightness(a.bg), brightness(b.bg)) {
                    if new < old {
                        darker += 1;
                    } else if new > old {
                        brighter += 1;
                    }
                }
            }
        }
        assert!(kept > 500, "most of the interface is still drawn behind the menu");
        assert!(darker > 500, "the background is darkened");
        assert!(brighter * 20 < darker, "almost nothing gets brighter (only the hint line)");
        let dim_cells = |buffer: &Buffer| {
            buffer
                .content()
                .iter()
                .filter(|cell| cell.modifier.contains(ratatui::style::Modifier::DIM))
                .count()
        };
        assert!(
            dim_cells(&after) <= dim_cells(&before),
            "darkening is done with colours, not by adding the DIM attribute"
        );
    }

    #[test]
    fn esc_menu_banner_animates_only_when_opened_with_esc() {
        let dir = project();
        let mut app = app(&dir);
        assert!(app.menu_animation().is_none(), "no animation without the menu");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.menu_animation().is_some(), "Esc starts the animation");
        // Opening a sub-screen and coming back shows the static banner.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.menu_animation().is_none());
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::Menu { .. }));
        assert!(app.menu_animation().is_none(), "going back does not replay it");
        // Closing and opening again with Esc plays it again.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.menu_animation().is_some());
    }

    #[test]
    fn menu_blanks_leave_the_background_visible() {
        let dir = project();
        let mut app = app(&dir);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let text = render_to_text(&mut app, 120, 40);
        // The banner rows span the document pane: its blank cells must not
        // erase the document text that sits under them.
        let banner_rows: Vec<&str> = text.lines().filter(|l| l.contains("███")).collect();
        assert!(!banner_rows.is_empty());
        assert!(text.contains("TERMINAL"), "terminal title bar still visible behind the menu");
    }

    #[test]
    fn theme_picker_looks_like_the_esc_menu_and_previews_behind_it() {
        let dir = project();
        let mut app = app(&dir);
        app.overlay = Overlay::Themes { selected: 1, original: app.theme };
        let text = render_to_text(&mut app, 120, 42);
        assert!(text.contains("███████████"), "same banner as the Esc menu");
        assert!(text.contains("❯ Tokyo Night"));
        assert!(text.contains("README.md"), "interface visible behind it");
        assert!(!text.contains('┌') || !text.contains("Theme menu"), "no popup frame");
    }

    #[test]
    fn terminal_panel_has_a_title_bar_and_a_focus_gutter() {
        let dir = project();
        let mut app = app(&dir);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let area = app.layout.terminal.unwrap();
        let theme = app.theme();
        let idle = terminal.backend().buffer()[(area.x, area.y + 1)].clone();
        // With the explorer visible its divider is the line; the gutter stays blank.
        assert_eq!(idle.symbol(), " ");
        app.show_explorer = false;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let area = app.layout.terminal.unwrap();
        let alone = terminal.backend().buffer()[(area.x, area.y + 1)].clone();
        assert_eq!(alone.symbol(), "│");
        assert_eq!(alone.fg, theme.border);
        app.show_explorer = true;
        app.focus = Focus::Explorer;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let area = app.layout.terminal.unwrap();

        // Tab twice: explorer → document → terminal.
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Terminal);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let focused = terminal.backend().buffer()[(area.x, area.y + 1)].clone();
        assert_eq!(focused.symbol(), "▍", "same bar as the explorer selection");
        assert_eq!(focused.fg, theme.accent, "the bar is coloured");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("TERMINAL"));
        assert!(text.lines().last().unwrap().contains("TERMINAL"), "status badge");
    }

    #[test]
    fn hidden_terminal_gives_the_space_back() {
        let dir = project();
        let mut app = app(&dir);
        let with = app.layout.viewer.height;
        app.show_term = false;
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        assert!(app.layout.terminal.is_none());
        assert!(app.layout.viewer.height > with);
        let text = render_to_text(&mut app, 100, 30);
        assert!(!text.contains("TERMINAL"));
    }

    #[test]
    fn mouse_selection_highlights_only_its_own_pane() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let dir = project();
        let mut app = app(&dir);
        let _ = render_to_text(&mut app, 100, 30);
        let viewer = app.layout.viewer;
        let mouse = |kind, x, y| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), viewer.x, viewer.y));
        // Drag far into the explorer and past the bottom of the screen.
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 1, viewer.y + 3));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1, viewer.y + 3));

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let theme = app.theme();
        let buffer = terminal.backend().buffer();
        // Nothing in the explorer column was painted.
        for y in 1..29 {
            for x in 0..viewer.x {
                assert_ne!(buffer[(x, y)].bg, theme.accent, "explorer cell {x},{y} selected");
            }
        }
        let copied = app.take_clipboard().expect("text queued for the clipboard");
        assert!(copied.contains("Home"), "{copied}");
        assert!(!copied.contains("EXPLORER"));
        assert!(!copied.contains("README.md"), "explorer text is never copied: {copied}");
    }

    #[test]
    fn esc_menu_collapses_to_a_framed_list_on_small_terminals() {
        let dir = project();
        let mut app = app(&dir);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let text = render_to_text(&mut app, 60, 14);
        assert!(text.contains("RUNETUI"));
        assert!(text.contains("❯ OPTIONS"));
        assert!(text.contains("HELP"));
        assert!(text.contains("QUIT"));
    }

    #[test]
    fn options_screen_draws_tabs_settings_and_description() {
        let dir = project();
        let mut app = app(&dir);
        app.overlay = Overlay::Options {
            tab: 0,
            selected: 0,
        };
        let text = render_to_text(&mut app, 110, 40);
        assert!(text.contains("Options"));
        for tab in ["General", "Explorer", "Editor"] {
            assert!(text.contains(tab), "tab {tab}");
        }
        assert!(text.contains("Theme"));
        assert!(text.contains("Icons"));
        assert!(text.contains("live preview"), "description of Theme");

        app.overlay = Overlay::Options {
            tab: 2,
            selected: 0,
        };
        let text = render_to_text(&mut app, 110, 40);
        assert!(text.contains("Edit inside runetui"));
        assert!(text.contains("ON"));
    }

    #[test]
    fn embedded_editor_is_drawn_in_the_document_pane() {
        use crate::editor::EmbeddedEditor;
        let dir = project();
        let mut app = app(&dir);
        let file = dir.path().join("other.rs");
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        let Ok(editor) = EmbeddedEditor::spawn_with("sh", &args, &file, 10, 40) else {
            return; // no pty in this environment
        };
        app.editor = Some(editor);
        app.focus = Focus::Viewer;
        let text = render_to_text(&mut app, 110, 30);
        assert!(text.contains(" EDIT "), "status badge");
        assert!(text.contains(":wq"), "how to leave");
        app.editor = None;
    }

    #[test]
    fn outline_overlay_draws_headings() {
        let dir = project();
        let mut app = app(&dir);
        app.focus = Focus::Viewer;
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::Outline { .. }));
        let text = render_to_text(&mut app, 100, 30);
        assert!(text.contains("Outline"));
        assert!(text.contains("Section"));
    }

    #[test]
    fn explorer_can_be_hidden() {
        let dir = project();
        let mut app = app(&dir);
        app.show_explorer = false;
        app.set_terminal_area(Rect::new(0, 0, 100, 30));
        let text = render_to_text(&mut app, 100, 30);
        assert!(!text.contains("EXPLORER"));
        // The document still renders across the full width.
        assert!(text.contains("Welcome to the project."));
    }

    #[test]
    fn theme_changes_are_reflected() {
        let dir = project();
        let mut app = app(&dir);
        assert_eq!(app.theme, ThemeName::CatppuccinMocha);
        app.handle_key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE));
        let text = render_to_text(&mut app, 100, 30);
        assert!(text.contains("Tokyo Night"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let dir = project();
        let mut app = app(&dir);
        for width in 1..30 {
            for height in 1..12 {
                let _ = render_to_text(&mut app, width, height);
            }
        }
    }

    #[test]
    fn esc_menu_shows_the_stars_and_the_credit_in_the_corners() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(1234));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..120u16).map(|x| buffer[(x, y)].symbol()).collect() };
        // Stars: first row, flush right.
        assert!(row(0).trim_end().ends_with("★ 1.2k"), "{:?}", row(0));
        // Credit: last row, flush right.
        assert!(row(39).trim_end().ends_with("Made By - fir3"), "{:?}", row(39));
    }

    #[test]
    fn the_star_count_is_hidden_when_switched_off_but_the_credit_stays() {
        let dir = project();
        let mut app = app(&dir);
        app.config.show_stars = false;
        app.stars.set_enabled(false);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let text = render_to_text(&mut app, 120, 40);
        assert!(!text.contains('★'));
        assert!(text.contains("Made By - fir3"));
    }

    #[test]
    fn unavailable_stars_show_a_dash() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Unavailable);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("★ —"));
    }

    #[test]
    fn small_terminals_do_not_break_the_corner_texts() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(5));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        for (width, height) in [(10, 4), (20, 6), (40, 10), (60, 14)] {
            let _ = render_to_text(&mut app, width, height);
        }
    }

    #[test]
    fn the_key_bindings_option_explains_the_capture() {
        let dir = project();
        let mut app = app(&dir);
        let selected = crate::app::OPTION_TABS[0]
            .items
            .iter()
            .position(|item| *item == crate::app::OptionItem::Keys)
            .unwrap();
        app.overlay = Overlay::Options { tab: 0, selected };
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("prefix Ctrl+R"));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("press a key"), "the value shows it is waiting");
    }

    #[test]
    fn corner_texts_leave_no_stray_letters_of_the_interface() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        for width in [70u16, 80, 100, 120, 140] {
            let backend = TestBackend::new(width, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let top: String = (0..width).map(|x| buffer[(x, 0)].symbol()).collect();
            let bottom: String = (0..width).map(|x| buffer[(x, 39)].symbol()).collect();
            let star_at = top.find('★').expect("stars drawn");
            assert!(top[star_at..].trim_end().ends_with("★ 42"), "{top:?}");
            // Nothing but spaces between the label and the border of the screen.
            let after = &top[star_at + "★ 42".len()..];
            assert!(after.trim().is_empty(), "stray text after the stars: {top:?}");
            assert!(bottom.trim_end().ends_with("Made By - fir3"), "{bottom:?}");
            // Never glued to the status text before it.
            assert!(bottom.contains(" Made By - fir3"), "{bottom:?}");
            assert!(top.contains(" ★ 42"), "{top:?}");
        }
    }

    fn header_row(app: &mut App, width: u16) -> String {
        let backend = TestBackend::new(width, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..width).map(|x| buffer[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_header_shows_kind_then_progress_then_the_stars_at_the_far_right() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(1234));
        for width in [80u16, 120, 209] {
            let top = header_row(&mut app, width);
            let kind = app.viewer.kind_label.clone();
            let kind_at = top.find(kind.as_str()).expect("kind badge");
            let star = top.find('★').expect("stars in the header");
            assert!(kind_at < star, "kind before stars: {top:?}");
            let between = &top[kind_at + kind.len()..star];
            assert!(between.contains('%') || between.contains('—'), "progress between: {top:?}");
            assert!(top.trim_end().ends_with("★ 1.2k"), "stars last: {top:?}");
            // One blank column of margin after the stars, as before.
            assert!(top.ends_with("★ 1.2k "), "{top:?}");
        }
    }

    #[test]
    fn the_header_records_where_the_stars_are_for_clicks() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rect = app.star_rect.get().expect("rect recorded");
        let buffer = terminal.backend().buffer();
        let drawn: String = (rect.x..rect.x + rect.width).map(|x| buffer[(x, rect.y)].symbol()).collect();
        assert_eq!(drawn, "★ 42");
        assert_eq!(rect.y, 0);

        // No stars, no rectangle.
        app.config.show_stars = false;
        app.stars.set_enabled(false);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(app.star_rect.get().is_none());
    }

    #[test]
    fn the_menu_records_its_own_star_rectangle() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rect = app.star_rect.get().expect("menu rect");
        let buffer = terminal.backend().buffer();
        let drawn: String = (rect.x..rect.x + rect.width).map(|x| buffer[(x, rect.y)].symbol()).collect();
        assert_eq!(drawn, "★ 42");
    }

    thread_local! {
        static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn opener_ok(url: &str) -> Result<(), String> {
        OPENED.with(|log| log.borrow_mut().push(url.to_string()));
        Ok(())
    }
    fn opener_missing(url: &str) -> Result<(), String> {
        OPENED.with(|log| log.borrow_mut().push(url.to_string()));
        Err("xdg-open exited with 3".to_string())
    }
    fn click(app: &mut App, x: u16, y: u16) {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }
    fn draw_once(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
    }

    #[test]
    fn clicking_the_header_stars_opens_the_repository() {
        let dir = project();
        let mut app = app(&dir);
        app.opener = opener_ok;
        OPENED.with(|log| log.borrow_mut().clear());
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        draw_once(&mut app);
        let rect = app.star_rect.get().unwrap();
        click(&mut app, rect.x + 1, rect.y);
        OPENED.with(|log| assert_eq!(*log.borrow(), vec![crate::stars::REPO_URL.to_string()]));
        assert!(app.status_text().is_some_and(|text| text.contains("Opened")));

        // Elsewhere on the header, or just beside the label: nothing.
        OPENED.with(|log| log.borrow_mut().clear());
        click(&mut app, 2, 0);
        click(&mut app, rect.x - 1, 0);
        click(&mut app, rect.x + rect.width, 0);
        OPENED.with(|log| assert!(log.borrow().is_empty()));
    }

    #[test]
    fn clicking_the_menu_stars_opens_the_repository_and_keeps_the_menu() {
        let dir = project();
        let mut app = app(&dir);
        app.opener = opener_ok;
        OPENED.with(|log| log.borrow_mut().clear());
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        draw_once(&mut app);
        let rect = app.star_rect.get().unwrap();
        click(&mut app, rect.x, rect.y);
        OPENED.with(|log| assert_eq!(log.borrow().len(), 1));
        assert!(matches!(app.overlay, crate::app::Overlay::Menu { .. }));
    }

    #[test]
    fn without_a_browser_the_address_is_copied_and_the_status_says_so() {
        let dir = project();
        let mut app = app(&dir);
        app.opener = opener_missing;
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        draw_once(&mut app);
        let rect = app.star_rect.get().unwrap();
        click(&mut app, rect.x, rect.y);
        let text = app.take_clipboard().expect("address copied");
        assert_eq!(text, crate::stars::REPO_URL);
        let message = app.copy_status(&text);
        assert!(message.contains("No browser") && message.contains(crate::stars::REPO_URL), "{message}");
        // The next ordinary copy reports itself again.
        assert_eq!(app.copy_status("abc"), "Copied 3 characters");
    }

    #[test]
    fn with_mouse_off_the_stars_are_not_clickable() {
        let dir = project();
        let mut app = app(&dir);
        app.opener = opener_ok;
        OPENED.with(|log| log.borrow_mut().clear());
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        app.config.mouse = false;
        draw_once(&mut app);
        let rect = app.star_rect.get().unwrap();
        click(&mut app, rect.x, rect.y);
        OPENED.with(|log| assert!(log.borrow().is_empty()));
    }

    #[test]
    fn the_header_stars_follow_the_state_and_the_option() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Loading);
        assert!(header_row(&mut app, 120).contains("★ …"));
        app.stars.set_for_test(crate::stars::StarsState::Unavailable);
        assert!(header_row(&mut app, 120).contains("★ —"));
        app.config.show_stars = false;
        app.stars.set_enabled(false);
        assert!(!header_row(&mut app, 120).contains('★'));
    }

    #[test]
    fn narrow_headers_drop_the_stars_first_and_keep_the_rest() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        let mut dropped = false;
        for width in (10u16..=60).rev() {
            let top = header_row(&mut app, width);
            assert_eq!(top.chars().count(), width as usize);
            if !top.contains('★') {
                dropped = true;
            } else {
                assert!(!dropped, "stars never come back at {width}: {top:?}");
            }
        }
        assert!(dropped, "the stars give way when there is no room");
    }

    #[test]
    fn the_esc_menu_draws_the_stars_once() {
        let dir = project();
        let mut app = app(&dir);
        app.stars.set_for_test(crate::stars::StarsState::Count(42));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.finish_menu_animation();
        let top = header_row(&mut app, 120);
        assert_eq!(top.matches('★').count(), 1, "{top:?}");
    }
}

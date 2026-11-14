# runetui

A Markdown and source-code viewer and editor for the terminal, built in Rust.
The package is called `runetui`; its command-line binary is `rtui`.
It pairs a Neovim/NvimTree-style project explorer with an Obsidian-flavoured
document pane: headings, tables, task lists, callouts, `[[wikilinks]]`, a
document outline, live reload, optional editing with your `$EDITOR`, and syntax
highlighting in nine colour themes.

```
┌ runetui ─────────────────────────────────────────────────────────────────┐
│ PROJECT                    │ notes/architecture.md             markdown   │
│ ▾ notes                    │                                              │
│   ▸ daily                  │ # Architecture                               │
│   • architecture.md        │                                              │
│   • reading-list.md        │ The viewer parses Markdown into an owned AST │
│ ▸ src                      │ and reflows it for the current width.        │
│   • main.rs                │                                              │
│   • config.rs              │ ╭── rust ──────────────────────────────────╮ │
│ README.md                  │ │ fn main() -> Result<()> { ... }          │ │
│                            │ ╰──────────────────────────────────────────╯ │
├────────────────────────────┴──────────────────────────────────────────────┤
│ notes/architecture.md  42/180  [search: render]           catppuccin-mocha│
└───────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Two-panel interface** — resizable, hideable project explorer beside the
  document viewer, with a header, divider, and status bar. Automatically hides
  the explorer on narrow terminals and reflows the document on resize.
- **Project explorer** — lazy directory tree, expand/collapse, per-extension
  icons, hidden-file toggle, optional `.gitignore` filtering, remembered last
  directory, a clear selection indicator, and automatic file preview while you
  move through the tree with the arrow keys.
- **Markdown rendering** — headings H1–H6, bold/italic/strikethrough, ordered
  and unordered lists (nested), task lists, tables with alignment, blockquotes
  and callouts, links, horizontal rules, inline code, footnotes, definition
  lists, and fenced code blocks.
- **Real syntax highlighting** — [syntect](https://github.com/trishume/syntect)
  with a palette-derived theme, per-block language labels, automatic language
  detection, and token highlighting for code blocks and standalone source files.
- **Obsidian-style links** — `[[wikilinks]]`, relative Markdown links, and
  `#anchors` all open inside the app. The outline (`o`) gives an automatic
  table of contents.
- **Arrow-key navigation** — `↑`/`↓`, `←`/`→`, `Home`/`End`, `PageUp`/`PageDown`,
  plus `Ctrl+D`/`Ctrl+U`, `/` search with `n`/`N`, `Tab`, and `q` to quit.
- **Edit in place** — press `e` and your editor (`$VISUAL`, `$EDITOR` or `vim`)
  opens *inside* the document pane, in a pseudo-terminal, with the explorer still
  visible. Quit it (`:wq`) and runetui reloads the file. It can be switched off
  in the options to get the old full-screen editor.
- **Integrated terminal** — a shell panel under the document (`Ctrl+T` to show or
  hide it) with scrollback, resizable together with the explorer through a
  tmux-style `Ctrl+R` prefix.
- **Configurable shortcuts** — every global key lives in the `[keys]` table of
  `config.toml`, including the prefix key.
- **Pane-limited mouse selection** — drag to select and copy text from the
  document, editor or terminal without grabbing the explorer or the borders.
- **btop-style Esc menu** — `Esc` opens a menu over the live interface (it is
  dimmed, not cleared) with a block-letter banner and OPTIONS / HELP / QUIT, like
  btop and mctui. OPTIONS has tabs (General, Explorer, Editor, Terminal) with a
  description of each setting.
- **Nine themes** — Catppuccin Mocha (default), Tokyo Night, Nord, Dracula,
  Gruvbox, Black, White, Snow, and Default Dark. Code highlighting follows the
  active palette. The Black, White, and Snow palettes are inspired by btop.
- **Safe by construction** — documents are opened with `fs::read`, parsed in
  memory, and never written, reformatted, or executed by runetui itself. Files
  are only saved when you explicitly edit them with `e`.
- **Live reload** — a filesystem watcher refreshes the tree and reloads the
  open document when it changes on disk. `.git` traffic is ignored and updates
  are debounced.

## Installation

Requires a Rust toolchain (edition 2024, Rust 1.85 or newer) and a terminal
with 256-colour or true-colour support.

```sh
cargo build --release
install -m 755 target/release/rtui ~/.local/bin/rtui
```

The package name is `runetui`, but the installed command is the shorter
`rtui`.

### Prebuilt binary (Linux x86_64)

The distribution ZIP includes a stripped `rtui` binary for 64-bit Linux next to
the source tree. It needs a glibc-based system:

```sh
chmod +x rtui
./rtui
```

The binary does not need the Rust toolchain. Build from source as shown above on
other platforms.

Run the tests with:

```sh
cargo test
```

## Icons

The explorer draws per-folder and per-file icons with indent guides. Three
styles are available:

| Style | Needs | Notes |
| --- | --- | --- |
| `unicode` (default) | Any font | Geometric symbols |
| `nerd` | A [Nerd Font](https://www.nerdfonts.com/) in the terminal | Open/closed folder glyphs, special folders (`src`, `docs`, `tests`, `.git`, `node_modules`…) and language icons |
| `ascii` | Nothing | Letters and punctuation only |

Press `F` to cycle styles, pass `-i nerd` on the command line, or set
`icons = "nerd"` in `config.toml`. The choice is remembered. The default is
`unicode` because Nerd Font glyphs appear as vertical boxes in terminals
without a patched font.

## Usage

```sh
rtui                     # reopen the last project, then the current directory
rtui ~/notes             # open a project directory
rtui ~/notes/today.md    # open a project and select the file
rtui -t nord ~/notes     # start with a specific theme
```

When no path is given, runetui restores the last project directory from its
configuration. In a fresh directory it opens the current working directory and
automatically previews `README.md` if one exists; otherwise it shows a welcome
screen.

### Command-line options

| Option | Description |
| --- | --- |
| `PATH` | Project directory or document to open. A file opens its parent project and selects the file. |
| `-t`, `--theme <THEME>` | Start with `catppuccin-mocha`, `tokyo-night`, `nord`, `dracula`, `gruvbox`, `black`, `white`, `snow`, or `default-dark`. |
| `-i`, `--icons <STYLE>` | Start with `unicode` (default), `nerd`, or `ascii` icons. |
| `-h`, `--help` | Show help. |
| `-V`, `--version` | Show the version. |

Invalid theme names produce an error listing the available themes. Settings
changed during a session (theme, explorer width, hidden-file and `.gitignore`
toggles, last directory) are saved on a clean exit.

## Key bindings

Press `?` inside the app for the same reference, or `o` for the document
outline.

### Panels

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Switch focus: explorer → document → terminal |
| `Ctrl+T` (or `Ctrl+R` `t`) | Show or hide the terminal panel |
| `Ctrl+R` `b` | Show or hide the explorer |
| `Ctrl+R` `←` / `→` | Shrink / grow the explorer (left panel) |
| `Ctrl+R` `↑` / `↓` | Grow / shrink the terminal (bottom panel) |
| `Ctrl+R` `o` / `p` | Next / previous panel (works from the terminal and the editor) |
| `Ctrl+R` `m` | Open the Esc menu from anywhere |
| `Ctrl+R` `Ctrl+R` | Send a literal `Ctrl+R` to the terminal or editor |
| `Ctrl+←` `→` `↑` `↓`, `<` `>` | Direct resize shortcuts (not while the terminal or editor has focus) |
| `←` / `→` | In the tree: go to the parent / expand or descend · In the document: `←` returns to the explorer |

`Ctrl+R` is a prefix, like in tmux: press it, then the second key (you can keep
`Ctrl` held for the arrow). While the terminal or the embedded editor has the
keyboard, **every** key goes to it (`Esc`, `Tab`, `q`, `Ctrl+C`...) except the
prefix. The status bar shows a `CTRL+R` badge and the possible next keys while
the prefix is pending.

The default prefix is `Ctrl+R`. Inside the terminal or the editor a few programs
use that key themselves (bash reverse search, vim redo): press the prefix twice to
send it through, or pick another one with `prefix = "ctrl+a"` in `config.toml`.

The `Esc` menu darkens the interface behind it (colours are blended towards black,
as in btop, so it stays readable) and its logo plays a short light-sweep and
shimmer animation each time it is opened with `Esc`.

The terminal panel runs `$SHELL` in the project directory. It keeps running
while hidden, closes when the shell exits, and its title bar and left gutter
turn to the accent colour while it has focus. The gutter uses the same `▍` bar as
the explorer selection.

### Mouse

| Action | Effect |
| --- | --- |
| Click | Focus the panel under the pointer (in the explorer it also selects the row) |
| Drag | Select text **from one panel only** (document, editor or terminal); the explorer, borders and other panels are never included. The text is copied on release |
| Wheel | Scroll the panel under the pointer (terminal scrollback included) |

Copying uses the terminal's OSC 52 clipboard sequence, so it also works over
SSH in terminals that support it (kitty, WezTerm, Alacritty, foot, iTerm2,
Windows Terminal, tmux with `set-clipboard on`...). Terminals without OSC 52
support will select but not copy. *Options → General → Mouse* turns the capture off
to get the terminal's own selection back.

### Explorer

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move selection; the highlighted file is previewed automatically |
| `Space` / `Enter` | Open the file and focus the document, or expand/collapse a folder |
| `←` / `→` | Collapse a folder or jump to its parent / expand a folder or descend into it |
| `Home` / `End` | Jump to the first / last entry |
| `PageUp` / `PageDown` | Move a page up / down |
| `e` | Edit the selected file in the pane with `$VISUAL`, `$EDITOR`, or `vim` |
| `H` | Toggle hidden files |
| `I` | Toggle `.gitignore` filtering |
| `r` | Refresh the project tree |
| `F` | Cycle the icon style (`unicode` → `ascii` → `nerd`); `unicode` shows letter badges such as a blue `M` for Markdown, an orange `R` for Rust or a red `D` for `.d` files |

### Document

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move the cursor line down / up |
| `Home` / `End` | Jump to the start / end |
| `Ctrl+D` / `Ctrl+U` | Scroll half a page |
| `PageUp` / `PageDown` | Scroll a full page up / down |
| `←` | Return focus to the explorer |
| `Enter` | Follow a link, anchor, or `[[wikilink]]` |
| `/` | Search the document (case-insensitive) |
| `n` / `N` | Next / previous search match |
| `o` | Open the document outline |
| `Ctrl+O` / `Ctrl+I` | History back / forward |
| `e` | Edit the current file inside runetui (vim in the pane) |
| `r` | Reload the document and refresh the tree |

External `http(s)` links are reported in the status bar but never launched from
inside the app. The editor is only started when you press `e`; while it runs, every key (Esc
included) goes to it, and the pane closes when you quit it.

### General

| Key | Action |
| --- | --- |
| `Esc` | Open the Esc menu (OPTIONS / HELP / QUIT) over the live interface, or go back / close an overlay, prompt or search |
| `T` | Cycle the colour theme |
| `?` | Show or hide help |
| `q` | Quit (also `Ctrl+C`) |

Pressing `Esc` with no overlay open brings up the menu. `↑`/`↓` (or `j`/`k`)
select OPTIONS, HELP or QUIT and `Enter` opens it; `Esc` goes back one level.
In OPTIONS, `←`/`→` change tab, `Enter` changes the highlighted setting, and
*Theme* opens the picker where `↑`/`↓` preview themes live, `Enter` applies and
`Esc` restores the previous one. Choices are saved in `config.toml`.

The menu and the theme picker are drawn over the running interface, which is
only dimmed, so you can see the explorer, document and terminal behind them (and
the theme change live). The `RuneTUI` banner keeps fixed brand colours whatever
the theme. *Explorer width* and *Terminal height* are adjusted from the Options
screen with `Enter`, then `+` / `-`, then `Enter` or `Esc` when done.

The search prompt accepts text and supports `Backspace`; `Ctrl+U` clears the
input. `Enter` runs the search and `Esc` cancels it.

## Supported files

| Content | Behaviour |
| --- | --- |
| `.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn` | Full Markdown rendering, outline, links |
| Source and config files | Syntax-highlighted with line numbers, language auto-detected |
| Other UTF-8 text | Plain text with line numbers |
| Binary files | Clear "unsupported" message |
| Files larger than 4 MiB | Reported as too large to keep scrolling responsive |
| Empty files | Empty-file message |

YAML front matter is hidden from the rendered document. Markdown features are
parsed with [pulldown-cmark](https://github.com/raphlinus/pulldown-cmark),
including GFM tables, task lists, strikethrough, footnotes, definition lists,
metadata blocks, and wikilinks.

## Themes

| Theme | CLI value |
| --- | --- |
| Catppuccin Mocha (default) | `catppuccin-mocha` |
| Tokyo Night | `tokyo-night` |
| Nord | `nord` |
| Dracula | `dracula` |
| Gruvbox | `gruvbox` |
| Black | `black` |
| White | `white` |
| Snow | `snow` |
| Default Dark | `default-dark` |

Each theme defines semantic colours for the interface and generates a matching
syntect theme, so code tokens, headings, quotes, tables, and the status bar stay
coherent. Black, White, and Snow follow btop's high-contrast dark, white, and
greyish-white looks.

Press `Esc`, then OPTIONS → Theme, use `↑`/`↓` to preview a theme, and `Enter`
to apply it. You can also cycle with `T` or choose a theme at startup
with `-t/--theme`.

## Configuration

Settings live at:

- Linux: `~/.config/runetui/config.toml`
- macOS: `~/Library/Application Support/runetui/config.toml`
- Windows: `%APPDATA%\runetui\config.toml`

```toml
last_directory = "/home/you/notes"
theme = "catppuccin-mocha"
icons = "unicode"
show_hidden = false
respect_gitignore = true
explorer_width = 28
show_explorer = true
embedded_editor = true
show_terminal = true
terminal_height = 30
mouse = true
prefix = "ctrl+r"

[keys]
quit = ["q", "ctrl+c", "prefix q"]
toggle_terminal = ["ctrl+t", "prefix t"]
explorer_shrink = ["prefix left", "ctrl+left", "<"]
# ...one entry per action; the file lists all of them
```

`explorer_width` is a percentage of the terminal width, clamped to 14–60;
`terminal_height` is a percentage of the body height, clamped to 10–70.

### Shortcuts

Every global shortcut is listed in the `[keys]` table and can be rebound with a
single key or a list of them. A binding is one key (`"ctrl+t"`, `"T"`, `"f5"`,
`"alt+x"`, `"esc"`, `"shift+tab"`) or two keys separated by a space
(`"ctrl+x b"`); the word `prefix` stands for the `prefix` key. Missing or invalid
entries fall back to their defaults, with a message in the status bar. Actions:
`quit`, `menu`, `help`, `outline`, `search`, `search_next`, `search_prev`,
`focus_next`, `focus_prev`, `edit`, `toggle_explorer`, `toggle_terminal`,
`cycle_theme`, `toggle_hidden`, `toggle_gitignore`, `cycle_icons`, `refresh`,
`history_back`, `history_forward`, `half_page_down`, `half_page_up`, `page_down`,
`explorer_shrink`, `explorer_grow`, `terminal_grow`, `terminal_shrink`. Arrow
navigation, `Enter`, `Space`, `Home`/`End` and `PageUp`/`PageDown` are fixed.
Missing fields fall back to defaults, and a malformed configuration never
prevents documents from being read. There are no other files on disk owned by
runetui.

## Architecture

```
src/
├── main.rs                     CLI, terminal lifecycle, event loop
├── app.rs                      Application state, key handling, navigation, persistence
├── config.rs                   Settings and startup path resolution
├── editor.rs                   Embedded editor: vim in a pseudo-terminal, key encoding
├── events.rs                   Crossterm input with a 100 ms tick (12 ms while editing)
├── filesystem/
│   ├── tree.rs                 Lazy arena tree, hidden/.gitignore filtering
│   ├── explorer.rs             Selection, viewport, expansion, width
│   └── watcher.rs              Debounced notify-based file watching
├── markdown/
│   ├── parser.rs               pulldown-cmark events → owned document AST
│   ├── renderer.rs             Width-aware layout into styled lines
│   ├── code_highlighter.rs     syntect integration and language labels
│   ├── toc.rs                  Outline and anchor resolution
│   └── mod.rs                  Rendered document types and slugification
├── viewer/
│   ├── mod.rs                  Loading, caching, scrolling, search, history
│   ├── text.rs                 Plain-text and source-file views
│   └── syntax.rs               Shared highlighting helpers
└── ui/
    ├── mod.rs                  Top-level draw and UI tests
    ├── layout.rs               Responsive pane geometry
    ├── themes.rs               Palettes and semantic styles
    ├── header.rs               Header bar
    ├── explorer.rs             Tree pane with indent guides
    ├── menu.rs                 Esc menu (banner, OPTIONS/HELP/QUIT) and options tabs
    ├── icons.rs                Nerd / Unicode / ASCII file and folder icons
    ├── markdown.rs             Document lines with cursor/search overlays
    ├── statusbar.rs            Status and search prompt
    ├── bar.rs                  Shared bar layout
    ├── overlays.rs             Outline popup
    └── help.rs                 Help overlay content
```

The rendering pipeline keeps parsing and layout separate:

1. `pulldown-cmark` events are converted into an owned, width-independent AST.
2. The renderer converts the AST into `RenderedDoc` — styled lines, headings,
   and link ranges — for the current width and theme.
3. The UI draws those lines, adding the cursor-line highlight, search matches,
   line numbers, and code-block frames.

Because the AST is width-independent, resizing and changing themes only re-run
layout, not parsing. The active theme also drives the generated syntect theme,
so code blocks recolour together with the rest of the interface.

## Editing and safety

runetui reads documents into memory and never modifies them by itself:

- Files are opened read-only (`fs::read`) and never written back automatically.
- No formatting, renaming, or shell commands are performed.
- Pressing `e` is the only way to modify a file: it runs `$VISUAL`, then
  `$EDITOR`, or `vim` as a fallback, inside the document pane (a real editor in a
  pseudo-terminal) and reloads the file when the editor exits. If the pane
  cannot start, or `embedded_editor = false`, the TUI is suspended instead.
- Scripts and code are displayed, never executed.
- External links are not opened automatically.
- runetui itself only writes `config.toml` under the platform configuration
  directory.

## Limitations

- Terminal graphics are text-only; images and embedded HTML are shown as plain
  fallbacks rather than rendered.
- Files over 4 MiB are reported instead of previewed.
- Syntax detection is heuristic for extension-less files; syntect's bundled
  syntax set covers common languages only.
- Live reload watches the opened project recursively; extremely large trees may
  take a moment to index on first open.
- Windows and macOS are supported through crossterm and `dirs`, but day-to-day
  development targets Linux. The terminal panel and the embedded editor rely on
  ConPTY on Windows, which has not been verified.
- Mouse copy needs a terminal with OSC 52 support (see Mouse).

## License

MIT.

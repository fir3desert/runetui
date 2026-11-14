# runetui

A terminal-native Markdown and source-code viewer/editor built in Rust.

`runetui` pairs a lazy project explorer with an Obsidian-inspired document pane:
Markdown rendering, outline navigation, in-app links, syntax highlighting, a
live terminal panel, and an embedded editor workflow without leaving the TUI.
The CLI binary is `rtui`.

```rs
┌ runetui ──────────────────────────────────────────────────────────────────┐
│ PROJECT                    │ README.md                         markdown   │
│ ▾ notes                    │                                              │
│   ▸ daily                  │ # runetui                                    │
│   • architecture.md        │                                              │
│   • reading-list.md        │ A terminal-native Markdown viewer.           │
│ ▸ src                      │                                              │
│   • main.rs                │ ╭── rust ──────────────────────────────────╮ │
│   • app.rs                 │ │ fn main() -> Result<()> { ... }          │ │
│   • config.rs              │ ╰──────────────────────────────────────────╯ │
│   • editor.rs              │                                              │
├────────────────────────────┴──────────────────────────────────────────────┤
│ README.md  1/42  [search: runetui]  catppuccin-mocha                      │
└───────────────────────────────────────────────────────────────────────────┘
```

## Highlights

- Neovim/NvimTree-style project explorer with lazy loading and per-folder expansion
- Markdown rendering for headings, lists, tables, blockquotes, callouts, HRs, task lists and wikilinks
- Inline `[[wikilinks]]`, relative document links, and `#anchors` opening in-app
- Real code syntax highlighting using `syntect` with a theme-aware palette
- Built-in terminal panel with tmux-style prefix resizing and focus management
- Embedded editor inside the document pane (`$VISUAL`, `$EDITOR`, or `vim` fallback)
- Live file watching and debounced refreshes for the tree and opened document
- btop-style Esc menu with animated banner, live theme preview, and options tabs
- Mouse text selection limited to one pane with OSC 52 clipboard support
- Configurable keys via `config.toml` and a `prefix` key like tmux
- GitHub star count in the header/menu, cached and fetched only when enabled

## Features

### Project explorer

- Lazy directory tree loaded on demand
- Expand/collapse folders with keyboard or mouse
- Hidden-file toggle and `.gitignore` filtering
- Icon styles: `unicode`, `ascii`, and `nerd`
- Auto-preview while moving through the tree
- Remembers the last opened project directory

### Markdown viewer

- H1–H6 headings and anchor generation
- Ordered and unordered lists, nested blocks, task lists
- Tables, blockquotes, callouts, footnotes, definition lists
- Code fences with language labels and syntax highlighting
- `[[wikilinks]]`, relative Markdown links and `#anchors`
- Outline (`o`) and search through the document
- History back/forward via `Ctrl+O` / `Ctrl+I`

### Terminal and editor

- Shell panel under the document area
- `Ctrl+T` to show/hide terminal, plus tmux-like prefix resizing (`Ctrl+R`)
- Embedded editor inside the TUI using a pseudo-terminal
- Focus-aware panel switching and “force close” recovery for hung terminal/editor
- Option to disable embedded editing and fall back to a full-screen external editor

### Interface and UX

- Resizable explorer and terminal panes
- Responsive layout with automatic explorer hiding on narrow terminals
- Nine themes: Catppuccin Mocha, Tokyo Night, Nord, Dracula, Gruvbox, Black, White, Snow, Default Dark
- Theme previews live while browsing the options menu
- Mouse selection limited to the focused pane, avoiding explorer/border capture
- OSC 52 clipboard support when the active terminal supports it
- Safe-by-default file handling: files are read into memory and only written by user-driven editor actions

## Installation

Requires a Rust toolchain with edition 2024 and Rust 1.85 or newer.

```sh
cargo build --release
install -m 755 target/release/rtui ~/.local/bin/rtui
```

The crate is named `runetui`, but the binary installed is `rtui`.

### Prebuilt binary

A Linux x86_64 binary can be used directly from the project directory when included in the release bundle:

```sh
chmod +x rtui
./rtui
```

On other platforms, build from source using the same Cargo command above.

Run the tests with:

```sh
cargo test
```

## Usage

```sh
rtui                     # reopen the last project, or the current directory
rtui ~/notes            # open a project directory
rtui ~/notes/today.md   # open a project and select a file
rtui -t nord ~/notes    # start with a specific theme
rtui -i nerd ~/notes    # start with Nerd Font explorer icons
```

If no path is provided, `runetui` restores the last project directory from its
configuration. In a fresh directory it opens the current working directory and
shows a README preview when available; otherwise it displays a welcome screen.

### Command line options

- `PATH`: project directory or document to open
- `-t, --theme <THEME>`: choose a palette (`catppuccin-mocha`, `tokyo-night`, `nord`, `dracula`, `gruvbox`, `black`, `white`, `snow`, `default-dark`)
- `-i, --icons <STYLE>`: choose icon style (`unicode`, `nerd`, `ascii`)
- `-h, --help`: show help
- `-V, --version`: show version

## Configuration

Settings are stored in the platform config directory:

- Linux: `~/.config/runetui/config.toml`
- macOS: `~/Library/Application Support/runetui/config.toml`
- Windows: `%APPDATA%\runetui\config.toml`

Example:

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
banner_animation = true
show_stars = true

[keys]
quit = ["q", "ctrl+c", "prefix q"]
toggle_terminal = ["ctrl+t", "prefix t"]
explorer_shrink = ["prefix left", "ctrl+left", "<"]
```

`explorer_width` is clamped to 14–60% of the terminal width, and
`terminal_height` is clamped to 10–70% of the body height.

### GitHub stars

The header and Esc menu can show the repository star count. This is the only
network call made by `runetui`: it fetches the GitHub API once per hour, caches
that result locally, and falls back to `★ —` when unavailable.

You can disable it with `show_stars = false` or from Options → General.

## Key bindings

Press `?` inside the app for the live help overlay, or `o` to open the document
outline.

### Global / panel controls

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Switch focus between explorer, document, terminal |
| `Ctrl+T` | Show or hide the terminal |
| `Ctrl+R` `t` | Show or hide the terminal |
| `Ctrl+R` `b` | Show or hide the explorer |
| `Ctrl+R` `←` / `→` | Resize the explorer |
| `Ctrl+R` `↑` / `↓` | Resize the terminal |
| `Ctrl+R` `o` / `p` | Next / previous panel |
| `Ctrl+R` `m` | Open the Esc menu |
| `Ctrl+R` `x` | Force-close a hung editor or terminal |
| `Esc` | Menu / back / close overlays |
| `T` | Cycle theme |
| `?` | Toggle help |
| `q` / `Ctrl+C` | Quit |

`Ctrl+R` is a tmux-like prefix: press it, then the second key. The prefix can be
rebound in `config.toml` through `prefix`, and `keys` can override any visible
shortcut.

### Explorer

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move selection |
| `Space` / `Enter` | Open file or expand/collapse folder |
| `←` / `→` | Collapse / parent or expand / descend |
| `Home` / `End` | Jump to first / last entry |
| `PageUp` / `PageDown` | Move a page |
| `e` | Edit the selected file in-place |
| `H` | Toggle hidden files |
| `I` | Toggle `.gitignore` filtering |
| `r` | Refresh tree |
| `F` | Cycle icon style |

### Document

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move cursor |
| `Home` / `End` | Jump to beginning / end |
| `Ctrl+D` / `Ctrl+U` | Scroll half a page |
| `PageUp` / `PageDown` | Scroll a full page |
| `←` | Return focus to explorer |
| `Enter` | Follow link, anchor or `[[wikilink]]` |
| `/` | Search |
| `n` / `N` | Next / previous match |
| `o` | Open outline |
| `Ctrl+O` / `Ctrl+I` | History back / forward |
| `e` | Edit current file |
| `r` | Reload document and refresh tree |

### Mouse

| Action | Effect |
| --- | --- |
| Click | Focus the panel under the pointer |
| Drag | Select text in a single pane and copy it on release |
| Wheel | Scroll the panel under the pointer |

Mouse copy uses the terminal OSC 52 sequence, so it works over SSH in terminals
that support it (for example kitty, WezTerm, Alacritty, foot, iTerm2, Windows
Terminal, and tmux with clipboard enabled).

## Supported file types

| Content | Behavior |
| --- | --- |
| `.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn` | Full Markdown parsing and document rendering |
| Source and config files | Syntax-highlighted with line numbers |
| Other UTF-8 text | Plain text rendering with numbers |
| Binary files | Clear unsupported/binary notice |
| Empty files | Empty-file message |
| Files over 4 MiB | Reported as too large for responsive preview |

Markdown support includes GFM tables, task lists, task markers, strikethrough,
footnotes, definition lists, callouts, metadata blocks, and `[[wikilinks]]`.

## Architecture

The project is organized into a small set of core modules:

```text
src/
├── main.rs                  CLI entrypoint, event loop, editor launch
├── app.rs                   App state, navigation, overlays, key handling
├── config.rs                Persistent config, defaults, platform path handling
├── editor.rs                Embedded pseudo-terminal editor support
├── events.rs                Crossterm event loop and timing
├── keymap.rs                Shortcut model and `[keys]` binding parsing
├── stars.rs                 GitHub star polling and cached counts
├── selection.rs             Single-pane mouse selection logic
├── clipboard.rs             OSC 52 clipboard sequence support
├── filesystem/
│   ├── mod.rs
│   ├── tree.rs              Lazy arena tree and .gitignore handling
│   ├── explorer.rs          Explorer selection, viewport, expansion, width
│   └── watcher.rs           Debounced filesystem watch support
├── markdown/
│   ├── mod.rs               Rendered document types and helper functions
│   ├── parser.rs            pulldown-cmark AST conversion
│   ├── renderer.rs          Width-aware layout and styled render output
│   ├── code_highlighter.rs   syntect highlight session and language labels
│   └── toc.rs               Outline and anchor resolution
├── viewer/
│   ├── mod.rs               Document loading, caching, scrolling and search
│   ├── text.rs              Plain-text rendering
│   └── syntax.rs            Shared code-view helpers
└── ui/
    ├── mod.rs               Top-level drawing and tests
    ├── layout.rs            Layout geometry for panes
    ├── themes.rs            Theme palettes and semantic colors
    ├── header.rs            Header bar rendering
    ├── explorer.rs          Explorer pane rendering
    ├── menu.rs              Esc menu, options tabs, theme picker
    ├── help.rs              Help overlay
    ├── icons.rs             Icon styles and glyph selection
    ├── markdown.rs          Markdown pane rendering and overlays
    ├── terminal.rs          Terminal panel renderer
    ├── overlays.rs          Outline popup and overlays
    ├── statusbar.rs         Status line and search prompt
    ├── bar.rs               Shared bar layout primitives
    └── themes.rs            Theme definitions
```

The rendering pipeline keeps parsing and layout separate:

1. Markdown and text content is read from disk once and parsed into an in-memory representation.
2. The renderer turns that model into styled rows for the current viewport width and theme.
3. The UI layers cursor highlighting, selection, links, search matches, and line numbers on top.

## Safety and behavior

`runetui` is intentionally conservative:

- Files are opened read-only and never rewritten by the app itself.
- It does not execute Markdown or source files.
- The shell and editor run only when the user explicitly interacts with them.
- The app only writes `config.toml` under the OS config directory.
- External links are surfaced but not launched automatically.

## Limitations

- Terminal graphics are text-based; images and embedded HTML are not rendered as rich UI.
- Files larger than 4 MiB are reported instead of fully previewed.
- Syntax detection is heuristic for extensionless files and common languages only.
- Very large trees may take time to index on first open.
- Linux is the primary development target; macOS/Windows are supported but the terminal editor and shell flow depend on terminal capabilities.
- OSC 52 clipboard support requires a terminal that supports it.

## License

MIT.

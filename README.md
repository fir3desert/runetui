# runetui

A Markdown and source-code viewer and editor for the terminal, built in Rust.
The package is called `runetui`; its command-line binary is `rtui`.
It pairs a Neovim/NvimTree-style project explorer with an Obsidian-flavoured
document pane: headings, tables, task lists, callouts, `[[wikilinks]]`, a
document outline, live reload, optional editing with your `$EDITOR`, and syntax
highlighting in nine colour themes.

```
┌ runetui ──────────────────────────────────────────────────────────────────┐
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
│ VIEWER  notes/architecture.md      1 backlink  4.2 KiB  42/180  e  Esc  ? │
└───────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Two-panel interface** — resizable, hideable project explorer beside the
  document viewer, with a header, divider, and status bar. Automatically hides
  the explorer on narrow terminals and reflows the document on resize.
- **Project explorer** — lazy directory tree, expand/collapse, per-extension
  icons, hidden-file toggle, optional `.gitignore` filtering, optional last
  directory memory, a clear selection indicator, and automatic file preview while you
  move through the tree with the arrow keys.
- **Markdown rendering** — headings H1–H6, bold/italic/strikethrough, ordered
  and unordered lists (nested), task lists, tables with alignment, blockquotes
  and callouts, links, horizontal rules, inline code, footnotes, definition
  lists, and fenced code blocks.
- **Real syntax highlighting** — [syntect](https://github.com/trishume/syntect)
  with a palette-derived theme, per-block language labels, automatic language
  detection, and token highlighting for code blocks and standalone source files.
  Long source files are highlighted in chunks as you scroll, so opening them
  stays quick (see [Supported files](#supported-files)).
- **Fuzzy file finder** — `Ctrl+P` opens a search box over every file of the
  project (it honours `.gitignore` and the hidden-files toggle, like the
  explorer). Typing a few letters of a path ranks the files by consecutive
  letters, word starts and file name over folders, and highlights the letters
  that matched. `Enter` opens the file and reveals its row in the explorer. The
  index is built on a background thread, so it never blocks the interface.
- **Bookmarks and recent files** — `m` marks the line you are reading (and
  unmarks it); `'` lists the bookmarks, with the text of each line, and the last
  20 files you opened on a second tab. They are saved per project in
  `state.toml`, with paths relative to the project root.
- **Backlinks** — `B` lists the other files of the project that link to the one
  you are reading (Markdown links and `[[wikilinks]]`), with the line of each
  link; `Enter` opens that file at the link. Once the index is ready, the status
  bar shows `1 backlink` or `N backlinks`. The link index is built in the
  background and kept up to date.
- **Export to HTML and PDF, and copy as Markdown** — `X` (or `Enter` on a file →
  `Export...`) writes the document as a self-contained HTML page in the active
  theme, or as a PDF through a program found on your system. With a mouse
  selection in the viewer, `Y` copies the **original Markdown** of those lines,
  not the rendered text.
- **Obsidian-style links** — `[[wikilinks]]`, relative Markdown links, and
  `#anchors` all open inside the app. The outline (`o`) gives an automatic
  table of contents.
- **Arrow-key navigation** — `↑`/`↓`, `←`/`→`, `Home`/`End`, `PageUp`/`PageDown`
  (`Ctrl+F` for the next page), plus `Ctrl+D`/`Ctrl+U`, `/` search with `n`/`N`,
  `Tab`, and `q` to quit.
- **Edit in place** — press `e` and your editor (`$VISUAL`, `$EDITOR` or `vim`)
  opens *inside* the document pane, in a pseudo-terminal, with the explorer still
  visible. Quit it (`:wq`) and runetui reloads the file. It can be switched off
  in the options to get the old full-screen editor.
- **Integrated terminal** — a shell panel under the document (`Ctrl+T` to show or
  hide it) with scrollback, resizable together with the explorer through a
  tmux-style `Ctrl+R` prefix.
- **Configurable shortcuts** — every global key except the fixed `q` to quit
  lives in the `[keys]` table of `config.toml`; the prefix can also be changed
  from OPTIONS → General → Key bindings.
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
- **Live reload** — a background filesystem watcher refreshes the folders you
  have open and reloads the open document when it changes on disk. It never
  walks the whole project, so a home directory opens as fast as a scratch
  folder. `.git` traffic is ignored and updates are debounced.
- **Instant start on huge trees** — nothing proportional to the size of the
  tree runs before the first frame. Directories are listed on background
  threads and, if one is slow (a cold disk, a network mount), the interface
  stays usable and the status bar shows `⠋ indexing…` (or `| indexing…` with
  ASCII icons) until it arrives. The indicator waits 150 ms before appearing so
  quick listings do not flash.

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
rtui                     # open the current directory (or the last one with reopen_last_directory = true)
rtui ~/notes             # open a project directory
rtui ~/notes/today.md    # open a project and select the file
rtui -t nord ~/notes     # start with a specific theme
```

When no path is given, runetui restores the last project directory from its
configuration. In a fresh directory it opens the current working directory and
automatically previews `README.md` if one exists; otherwise it shows a welcome
screen with the project name and path and a short table of the main keys.

### Command-line options

| Option | Description |
| --- | --- |
| `PATH` | Project directory or document to open. A file opens its parent project and selects the file. |
| `-t`, `--theme <THEME>` | Start with `catppuccin-mocha`, `tokyo-night`, `nord`, `dracula`, `gruvbox`, `black`, `white`, `snow`, or `default-dark`. |
| `-i`, `--icons <STYLE>` | Start with `unicode` (default), `nerd`, or `ascii` icons. |
| `-h`, `--help` | Show help. |
| `-V`, `--version` | Show the version. |

Invalid theme names produce an error listing the available themes; invalid icon
styles produce `unknown icon style … Available styles: nerd, unicode, ascii`.
Settings changed during a session (theme, explorer width, hidden-file and
`.gitignore` toggles, last directory) are saved on a clean exit.

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
| `Ctrl+R` `x` | Emergency exit: kill the focused editor or terminal, even if it is hung (unsaved editor changes are lost) |
| `Ctrl+R` `Ctrl+R` | Send a literal `Ctrl+R` to the terminal or editor |
| `Ctrl+←` `→` `↑` `↓`, `<` `>` | Direct resize shortcuts (not while the terminal or editor has focus) |
| `←` / `→` | In the tree: go to the parent / expand or descend · In the document: `←` returns to the explorer |

`Ctrl+R` is a prefix, like in tmux: press it, then the second key (you can keep
`Ctrl` held for the arrow). While the terminal or the embedded editor has the
keyboard, **every** key goes to it (`Esc`, `Tab`, `q`, `Ctrl+C`...) except the
prefix. The second key ignores Ctrl/Shift on named keys, so `prefix` + `Ctrl+←`
works like `prefix` + `←`. A pending prefix is cancelled by any other key, or
automatically after three seconds. Until then the status bar shows the `CTRL+R`
badge, the possible second keys and `· any other key cancels`; while a pane has
the keyboard it shows a `CTRL+R prefix` reminder instead of the usual hints.

The default prefix is `Ctrl+R`. Inside the terminal or the editor a few programs
use that key themselves (bash reverse search, vim redo): press the prefix twice to
send it through, change it in OPTIONS → General → Key bindings, or set
`prefix = "ctrl+a"` in `config.toml`.

The `Esc` menu darkens the interface behind it (colours are blended towards black,
as in btop, so it stays readable) and its logo plays a short light-sweep and
shimmer animation each time it is opened with `Esc`.

The terminal panel runs `$SHELL` in the project directory. It keeps running
while hidden, closes when the shell exits, and its title bar and left gutter
turn to the accent colour while it has focus. The gutter uses the same `▍` bar as
the explorer selection.

The status bar's left side carries a focus badge (`EXPLORER`, `VIEWER`,
`TERMINAL`, a green `EDIT` while the embedded editor runs, or `CTRL+R` while a
prefix is pending), the open file or the latest message, and the live search
prompt. Its right side can show `no live refresh`, `hidden`, `no-gitignore`, the
current search match (`3/12`), the backlink count, the file size, `line/total`,
and the configured `edit`/`menu`/`help` keys (`e  Esc  ?` by default).

### Mouse

| Action | Effect |
| --- | --- |
| Click | Focus the panel under the pointer (in the explorer it also selects the row and previews a file) |
| Click on a folder's `▸`/`▾` | Open or close the folder without touching the open document |
| Double click (explorer) | Open the file for reading, or open/close the folder |
| Click on the `★ 42` star count (header or Esc menu) | Open the repository in the browser (or copy its address with OSC 52 when there is no browser) |
| Click in the Esc menu | Activate OPTIONS / HELP / QUIT; the wheel moves the selection |
| Click in Options | Tabs switch tab; a row selects it; a double click on a row, or a click on its value, changes it (like `Enter`); the wheel moves the selection |
| Click in Themes / Outline / Links chooser | Pick the row (a theme is applied, a heading jumped to, a link followed); the wheel moves the selection |
| Click a result of the file finder | Open that file; the wheel moves the selection |
| Click a tab or a row of the bookmarks popup | Switch list / jump to the bookmark or open the file |
| Click a row of the backlinks popup | Open that file at the link |
| Click a choice of the file menu's `Export...` submenu | Export to HTML / PDF |
| Click outside a popup (Options, Help, Outline, Links, Find file, Bookmarks, Backlinks) | Close it, like `Esc` |
| Drag | Select text **from one panel only** (document, editor or terminal); the explorer, borders and other panels are never included. The text is copied on release |
| Wheel | Scroll the panel under the pointer (terminal scrollback included), or the list of the open menu / popup |

Copying uses the terminal's OSC 52 clipboard sequence, so it also works over
SSH in terminals that support it (kitty, WezTerm, Alacritty, foot, iTerm2,
Windows Terminal, tmux with `set-clipboard on`...). Terminals without OSC 52
support will select but not copy. *Options → General → Mouse* turns the capture off
to get the terminal's own selection back; with it off none of the clicks above
exist (every action has a keyboard equivalent).

### Find file

| Key | Action |
| --- | --- |
| `Ctrl+P` (or `Ctrl+R` `f`) | Open the finder. `Ctrl+R` `f` also works from the terminal and the editor, which keep every other key |
| *typing* | Filter the files: a few letters of the path, in order (`uimod` finds `src/ui/mod.rs`); spaces are ignored and case does not matter |
| `↑` / `↓` (`Ctrl+P` / `Ctrl+N`), `PageUp` / `PageDown`, `Home` / `End` | Move through the results (up to 200, the list scrolls) |
| `Enter` | Open the highlighted file in the viewer and select its row in the explorer, unfolding the folders on the way |
| `Backspace`, `Ctrl+U`, `Ctrl+W` | Delete a character / clear the query / delete a word |
| `Esc` | Close the finder |

Inside the finder `q` is just a letter (press `Esc` first, then `q` to quit) and
`Ctrl+C` does nothing, like everywhere else. Pasting text fills the query. The
first time the finder is opened the project is indexed in the background:
the popup shows `⠋ indexing…` (or `| indexing…` with ASCII icons) with the
number of files found so far, and
results appear as soon as they are known (what you type meanwhile is applied
when they arrive). The index is kept for the session and rebuilt when files
change on disk, on `r` (refresh) and when you toggle hidden files or
`.gitignore` filtering. Projects are indexed up to 200 000 files (the counter
then reads `200000+`). Symlinks are not followed and `.git` is never listed.

### Bookmarks and recent files

| Key | Action |
| --- | --- |
| `m` | Bookmark the current line of the open file; press it again on the same line to remove the bookmark |
| `'` (or `Ctrl+R` `'`) | Open the bookmarks popup |
| `Tab` / `Shift+Tab`, `←` / `→` (`h` / `l`) | Switch between **Bookmarks** and **Recent** |
| `↑` / `↓` (`j` / `k`), `PageUp` / `PageDown`, `Home` / `End` | Move through the list |
| `Enter` / `Space` | Jump to the bookmarked line (opening the file if needed), or open the recent file |
| `d` / `Delete` | Delete the bookmark, or forget the recent file |
| `Esc` / `q` | Close the popup |

A bookmark remembers a **source line**, not a screen row, so it stays correct
when the window is resized or the text re-flows. In a Markdown file whose block
spans several lines (a wrapped paragraph, a table) the line is the first line of
that block. Jumping is a navigation, so `Ctrl+O` comes back.

*Recent* lists the files you opened on purpose, most recent first, at most 20:
through the explorer (`Space`, `Enter` on a file's action menu or a double click),
the file finder, a link, a bookmark or the command line. Merely arrowing through
the explorer to preview files does not count, and neither does the `README.md`
that opens by default.

`state.toml` lives next to `config.toml` and holds one table per project root.
Paths are stored relative to the project root. The file is read forgivingly: a
corrupt file is ignored (and moved to `state.toml.corrupt` the next time something
is saved), and entries that are malformed, point outside the project or name a
file that no longer exists are dropped without any error. Bookmarks are limited
to 500 per project.

### Backlinks

| Key | Action |
| --- | --- |
| `B` | List the files that link to the open file; once indexed, the status bar shows `1 backlink` or `N backlinks` |
| `↑` / `↓` (`j` / `k`), `PageUp` / `PageDown`, `Home` / `End` | Move through the list |
| `Enter` / `Space` | Open that file at the link (`Ctrl+O` comes back) |
| `Esc` / `q` | Close the popup |

Each entry shows `file:line` and the text of the line that holds the link. Which
links count follows what happens when you open them in the viewer:

- relative Markdown links (`../notes/a.md`, `a.md#section`, `/from/the/root.md`)
  are resolved from the folder of the file that contains them; `#section` and
  `?query` are ignored, `%20` is decoded and a link without extension gets `.md`;
- `[[wikilinks]]` (`[[file]]`, `[[file|alias]]`, `[[file#section]]`, `[[folder/file]]`)
  are looked up next to the file, then from the project root, then by name
  anywhere in the project, ignoring case. If several files match, the shallowest
  one wins, then alphabetical order;
- broken links, `#anchor`-only links, web addresses, links that leave the project
  and links inside code blocks are ignored; two links to the same file on one
  line count once, and a file never counts as a backlink of itself.

Links to any file count (`[code](src/main.rs)` is a backlink of `main.rs`), but only
Markdown files (`.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn`) are searched for links.
The index uses the same hidden-files and `.gitignore` settings as the explorer.

The index is built on a background thread shortly after start-up (so opening a huge
project stays instant) and reuses what it already read from files that did not change.
It is rebuilt about two seconds after files change on disk (watcher events, `r`,
saving from the editor). Until the first build finishes the popup shows
`indexing…` and the status bar shows no count. At most 20 000 Markdown files and
256 MB are read per build; a bigger project is indexed partially (the popup then
says `partial`). Inside a paragraph or list, which can only be mapped to the source as
a whole block, the jump lands on the row that shows the link.

### Export to HTML and PDF

| Key | Action |
| --- | --- |
| `X` | Export the open document: choose **HTML** or **PDF** (`h` / `p` pick one directly) |
| `Enter` on a file in the explorer → `Export...` | The same two choices for that file; `Esc` goes back to the file's actions |

The export runs on a background thread; the status bar says `Exporting HTML…` and
then where the file went (`Exported HTML: /path/guide.html`).

- **HTML** is one self-contained file: the CSS is embedded, the colours come from
  the active theme (a dark page for dark themes, a light one for `white` / `snow`),
  code blocks are coloured with the same highlighter as the viewer, every heading
  has an `id` so `#section` links work, and local images of the project (up to 2 MB
  each) are embedded as `data:` URIs. Tables, task lists, footnotes, callouts and
  `[[wikilinks]]` are kept; front matter is not shown. A Content-Security-Policy
  forbids scripts, so raw HTML in the Markdown can show but cannot run code. A file
  that is not Markdown (`main.rs`) becomes a page with its highlighted source,
  exported as `main.rs.html`.
- **PDF**: runetui has no PDF engine, so it converts the HTML with the first of
  `wkhtmltopdf`, a Chromium-family browser (`chromium`, `chromium-browser`,
  `google-chrome`, `google-chrome-stable`, `chrome`, `microsoft-edge`, started with
  `--headless`) or `weasyprint` that it finds on `PATH`. The programs are started
  without a shell and each path is a single argument, so names with spaces are fine.
  The conversion is stopped after 90 seconds. If none is installed the HTML is
  written instead and the status bar says what is missing (`PDF export needs
  wkhtmltopdf, chromium (--headless) or weasyprint, and none was found in PATH.
  Wrote HTML instead: …`); if the converter fails, the status bar gives its error
  and the HTML is kept too. A browser that refuses to start as root is retried once
  with `--no-sandbox` (the page forbids scripts), and the status bar says so.
- The file is written **next to the original**, or in `export_dir`
  (see [Configuration](#configuration)), which is created when missing. An existing
  `.html` is replaced only if runetui wrote it, and an existing `.pdf` only if it
  really is a PDF; any other file is left alone and the export gets a numbered name
  (`guide (2).html`).

### Copy the selection as Markdown

| Key | Action |
| --- | --- |
| `Y` | With text selected with the mouse in the viewer, copy the original Markdown of those lines |

Select with the mouse (drag) and press `Y`. Rendered rows are mapped back to the
source lines they came from, and the status bar tells you what was copied
(`Copied Markdown: lines 9-10`). The mapping is exact where the rendered rows are
the source lines: list items, headings, one-line paragraphs, and code blocks (the
fences are included when you select the frame rows). Where it cannot be exact
(a paragraph that flows over several lines, a table, a line with a link that is
shown without its address, a long line that wraps) runetui copies the **whole block**
or line instead and says so: `Copied lines 13-13 as whole blocks: the selection
could not be mapped exactly`. For plain text and source files the original lines
are copied. Like any key, `Y` ends the selection.

### Explorer

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move selection; the highlighted file is previewed automatically |
| `Space` | Open the file and focus the document, or expand/collapse a folder |
| `Enter` | Actions menu for the selected file or folder: open, **open in VS Code**, **open in a new terminal** (in the folder, or the file's folder), open with the default app, **Edit here**, **Export...**, copy path / name. Pick with `↑`/`↓` (`j`/`k`) + `Enter`, a number, or a click; `Esc` closes it |
| `←` / `→` | Collapse a folder or jump to its parent / expand a folder or descend into it |
| `Home` / `End` | Jump to the first / last entry |
| `PageUp` / `PageDown` | Move a page up / down |
| `e` | Edit the selected file in the pane with `$VISUAL`, `$EDITOR`, or `vim` |
| `H` | Toggle hidden files |
| `I` | Toggle `.gitignore` filtering |
| `r` | Refresh the project tree |
| `F` | Cycle the icon style (`unicode` → `ascii` → `nerd` → `unicode` from the default); `unicode` shows letter badges such as a blue `M` for Markdown, an orange `R` for Rust or a red `D` for `.d` files |

### Document

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move the cursor line down / up |
| `Home` / `End` | Jump to the start / end |
| `Ctrl+D` / `Ctrl+U` | Scroll half a page |
| `PageUp` / `PageDown`, `Ctrl+F` | Scroll a full page up / down |
| `←` | Return focus to the explorer |
| `Space` (`follow_link`) | Follow the link on the cursor line (`Enter` does the same). On a code block, both copy the whole block. Several links on the line open a small chooser (`↑`/`↓` or `j`/`k`, `Enter` or `1`–`9`, `Esc` to cancel) |
| `Enter` | Follow a link, anchor, or `[[wikilink]]`; on a code block, copy it |
| `/` | Search the document (case-insensitive) |
| `n` / `N` | Next / previous search match |
| `o` | Open the document outline |
| `Ctrl+O` / `Ctrl+I` | History back / forward |
| `e` / `Ctrl+E` | Edit the current file inside runetui (vim in the pane) |
| `r` | Reload the document and refresh the tree (no effect in the terminal) |

Following a link never runs anything. A relative link (`notes/guide.md`,
`my%20notes/Deep%20Doc.md#section-two`) opens that document in the viewer, is
recorded in the history and moves the explorer selection; `#anchor` links jump
to the heading; `http(s)` and `mailto:` links go to the browser (`xdg-open`,
`open`, or `rundll32 url.dll,FileProtocolHandler`, passed as one argument and
never through a shell; without a browser the address is copied with OSC 52 and
the status bar says so). Any other scheme (`file:`, `javascript:`, `ftp:`…) is
refused, and so is a path that resolves, through `..` or a symlink, to
somewhere outside the project root. A missing target is reported in the status
bar. In the explorer `Space` still opens files and folds folders, in the
search prompt, the editor and the terminal it is an ordinary space, and on a
line without a link it does nothing. Rebind it with `follow_link` in `[keys]`.
The editor is only started when you press `e`; while it runs, every key (Esc
included) goes to it, and the pane closes when you quit it. Navigation history
keeps the last 100 documents.

### General

| Key | Action |
| --- | --- |
| `Esc` | Open the Esc menu (OPTIONS / HELP / QUIT) over the live interface, or go back / close an overlay, prompt or search |
| `T` | Cycle the colour theme |
| `?` (or `Ctrl+R` `?`) | Show or hide help |
| `q` | Quit (the only way out: `Ctrl+C` does nothing) |
| `Enter` / `Space` | On a link: follow it. On a code block: copy the whole block to the clipboard |

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
| `.txt`, `.text`, `.log`, `.rst`, `.adoc`, `.org` (and other UTF-8 text) | Plain text with line numbers |
| Binary files | Clear "unsupported" message |
| Files larger than 4 MiB | Reported as too large to keep scrolling responsive |
| Empty files | Empty-file message |

Source files with 400 lines or more are highlighted lazily: the first 200 lines
are ready when the file opens, and 300 more are coloured whenever you scroll
close to the end of what has been highlighted. `End`, search and jumps to a
bookmark load the rest. While a file is partial the status bar's `line/total` is
an estimate based on the highlighted part.

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
last_directory = "/home/you/notes"   # only used when reopen_last_directory = true
reopen_last_directory = false          # true: a bare `rtui` reopens last_directory
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
# export_dir = "exports"             # where HTML/PDF exports go (default: next to the original)
prefix = "ctrl+r"
banner_animation = true
show_stars = true

[keys]
toggle_terminal = ["ctrl+t", "prefix t"]
explorer_shrink = ["prefix left", "ctrl+left", "<"]
# ...one entry per action; the file lists all of them
```

`explorer_width` is a percentage of the terminal width, clamped to 14–60;
`terminal_height` is a percentage of the body height, clamped to 10–70.
`export_dir` is where [exports](#export-to-html-and-pdf) are written: a relative
path is relative to the project root, `~/` is your home folder, and unset (or
blank) means next to the original file.

### GitHub stars

The header shows the star count of the repository at the far right
(`markdown  0%  ★ 42`: kind, progress, stars). Clicking it opens
<https://github.com/fir3desert/runetui>. The count is the only network request
runetui makes itself: the
system `curl` asks the GitHub API once an hour in the background (the count is
cached in `stars.txt` under the platform cache directory for an hour and shown
compactly, e.g. `★ 1.2k`; it reads `★ …` until the first answer). The header
shows `★ —` when it cannot be fetched. Set `show_stars = false` (or turn it off
in Options → General) to disable the request and hide the count. On narrow
terminals it is the first item dropped.

### Document type badge

The badge in the header says what the viewer is showing: `welcome` before any
document is open, `markdown`, a language (`rust`, `makefile`…), `binary`, and for
plain text `text` (`.txt`) or `text · <extension or name>` (`text · .rst`,
`text · .gitignore`, `text · LICENSE`). Long names are shortened with `…`.
A name is only treated as code when syntect's bundled syntaxes really have one
for it (`Makefile` does; `.gitignore`, `Dockerfile`, `.env`, `LICENSE` and
`toml` do not). The header also shows the heading under the cursor and the
reading percentage, plus a centred `HH:MM:SS` clock when there is room.

### Diagnosing a slow start

`RTUI_TRACE=1 rtui ~/` writes the time of every start-up phase (and every
directory listing) to `rtui-trace.log` in the system temporary directory;
`RTUI_TRACE=/some/file` picks the file and `RTUI_TRACE=stderr` writes to stderr
(`rtui 2>trace.log`). The path is printed when the program exits. Off by default.

### Shortcuts

Every global shortcut except `q` is listed in the `[keys]` table and can be
rebound with a single key or a list of them; quitting is always the plain `q`.
A binding is one key (`"ctrl+t"`, `"T"`, `"f5"`, `"alt+x"`, `"esc"`,
`"shift+tab"`) or two keys separated by a space (`"ctrl+x b"`); the word
`prefix` stands for the `prefix` key. Modifiers also accept `control`/`ctl`
and `meta`/`opt`/`option`, `esc` accepts `escape`, and named function keys go
up to `F24`. The second key of a two-key binding ignores Ctrl and Shift when it
is a named key, so `prefix left` also matches `prefix Ctrl+Left`. Missing or
invalid entries fall back to their defaults, with the first warning shown in
the status bar. Actions:
`quit` (fixed to `q`), `menu`, `help`, `outline`, `find_file`, `bookmark`, `bookmarks`, `backlinks`, `export`, `copy_markdown`, `follow_link`, `search`, `search_next`, `search_prev`,
`focus_next`, `focus_prev`, `edit`, `toggle_explorer`, `toggle_terminal`,
`cycle_theme`, `toggle_hidden`, `toggle_gitignore`, `cycle_icons`, `refresh`,
`history_back`, `history_forward`, `half_page_down`, `half_page_up`, `page_down`,
`explorer_shrink`, `explorer_grow`, `terminal_grow`, `terminal_shrink`,
`force_close`. Arrow navigation, `Enter`, `Home`/`End` and `PageUp`/`PageDown`
are fixed; `Space` is `follow_link` in the document and always opens/folds in
the explorer.

The prefix can also be changed without editing a file: OPTIONS → General →
Key bindings, press `Enter`, then type the new key. It must use Ctrl, Alt or an
F key; `Ctrl+I`, `Ctrl+M`, `Ctrl+J`, `Ctrl+H` and `Ctrl+[` are rejected because
terminals confuse them with Tab, Enter, Backspace and Esc, and a key already
used by another shortcut is rejected too. `Backspace` restores the default,
`Esc` cancels, and the result is saved to `config.toml`.

Missing fields fall back to defaults, and a malformed configuration never
prevents documents from being read. Besides `config.toml`, runetui keeps one
more file, `state.toml` (see [Bookmarks and recent files](#bookmarks-and-recent-files)).

## Architecture

```
src/
├── main.rs                     CLI, terminal lifecycle, event loop
├── app.rs                      Application state, key handling, navigation, persistence
├── config.rs                   Settings and startup path resolution
├── editor.rs                   Embedded editor: vim in a pseudo-terminal, key encoding
├── events.rs                   Crossterm input with a 100 ms tick (12 ms while editing)
├── backlinks.rs                Link index: who links to which file, built in the background (B)
├── export.rs                   HTML export (theme CSS, highlighted code, embedded images) and PDF through wkhtmltopdf / Chromium / weasyprint
├── finder.rs                   Fuzzy matcher and the background project file index (Ctrl+P)
├── state.rs                    Per-project bookmarks and recent files (state.toml)
├── filesystem/
│   ├── tree.rs                 Lazy arena tree, background listings, hidden/.gitignore filtering
│   ├── explorer.rs             Selection, viewport, expansion, width
│   └── watcher.rs              Background non-recursive notify watcher
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
    ├── overlays.rs             Outline, links, file menu and file finder popups
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
- Links never execute anything: only documents inside the project and
  `http(s)`/`mailto:` addresses (handed to the system opener as a single
  argument) are followed.
- runetui itself only writes `config.toml` and `state.toml` (bookmarks and recent
  files) under the platform configuration directory.

## Limitations

- Terminal graphics are text-only; images and embedded HTML are shown as plain
  fallbacks rather than rendered.
- Files over 4 MiB are reported instead of previewed.
- Syntax detection is heuristic for extension-less files; syntect's bundled
  syntax set covers common languages only.
- Live reload watches only the folders that are open in the explorer (plus the
  folder of the open document), up to 2048 directories, not the whole tree. A
  folder with more than 10 000 entries is shown truncated. If the system refuses
  to watch (inotify limit), the status bar says `no live refresh` and reports
  `Live refresh limited (…); press r to refresh by hand`; `r` still refreshes by
  hand.
- Windows and macOS are supported through crossterm and `dirs`, but day-to-day
  development targets Linux. The terminal panel and the embedded editor rely on
  ConPTY on Windows, which has not been verified.
- Mouse copy needs a terminal with OSC 52 support (see Mouse).

## License

MIT.

//! Export a document to a self-contained HTML page, or to PDF.
//!
//! * **HTML**: the Markdown source is converted with the same parser settings
//!   the viewer uses ([`crate::markdown::markdown_options`]).  The page has its
//!   CSS embedded, takes its colours from the active theme (dark or light, as the
//!   theme is), colours code blocks with the viewer's highlighter, gives every
//!   heading an `id` so `#section` links work and embeds the project's local
//!   images as `data:` URIs.  A Content-Security-Policy forbids scripts, so raw
//!   HTML inside the Markdown cannot run anything.
//! * **PDF**: runetui has no PDF engine.  The HTML is converted by
//!   `wkhtmltopdf`, a Chromium-family browser (`--headless`) or `weasyprint`,
//!   whichever is found first on `PATH`.  Programs are started without a shell,
//!   and every path is one argument.  When no converter exists, or it fails, the
//!   HTML is written instead and the message says what was missing.
//!
//! Everything here runs on a background thread started by the app; [`run`]
//! never touches the interface.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag, TagEnd, html};
use ratatui::style::{Color, Modifier};

use crate::backlinks::is_markdown;
use crate::markdown::code_highlighter::CodeHighlighter;
use crate::markdown::{markdown_options, slugify};
use crate::ui::themes::Theme;
use crate::viewer::MAX_FILE_SIZE;

/// Local images bigger than this stay links instead of being embedded.
pub const IMAGE_LIMIT: u64 = 2 * 1024 * 1024;
/// A converter that runs longer than this is stopped.
pub const CONVERTER_TIMEOUT: Duration = Duration::from_secs(90);
/// Marks the pages this module writes, so a later export may replace them.
const GENERATOR_MARK: &str = "<meta name=\"generator\" content=\"runetui\">";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Pdf,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Html => "html",
            Format::Pdf => "pdf",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Html => "HTML",
            Format::Pdf => "PDF",
        }
    }
}

// ------------------------------------------------------------------- HTML

fn hex(color: Color) -> Option<String> {
    match color {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

fn css_color(color: Color, fallback: &str) -> String {
    hex(color).unwrap_or_else(|| fallback.to_string())
}

/// Is the theme dark?  Judged by the luminance of its background.
pub fn is_dark(theme: &Theme) -> bool {
    match theme.bg {
        Color::Rgb(r, g, b) => {
            let luminance = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
            luminance < 128.0
        }
        _ => true,
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// The stylesheet of an exported page, with the colours of `theme`.
pub fn stylesheet(theme: &Theme) -> String {
    let dark = is_dark(theme);
    let c = |color: Color| css_color(color, if dark { "#cccccc" } else { "#222222" });
    format!(
        r#":root {{
  color-scheme: {scheme};
  --bg: {bg}; --bg-alt: {bg_alt}; --bg-sel: {bg_sel};
  --fg: {fg}; --fg-dim: {fg_dim}; --fg-muted: {fg_muted};
  --accent: {accent}; --accent2: {accent2}; --border: {border};
  --green: {green}; --yellow: {yellow}; --orange: {orange}; --red: {red};
  --purple: {purple}; --cyan: {cyan};
}}
* {{ box-sizing: border-box; }}
html {{ background: var(--bg); }}
body {{
  max-width: 52rem; margin: 2.5rem auto; padding: 0 1.25rem 3rem;
  background: var(--bg); color: var(--fg);
  font: 16px/1.65 system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  overflow-wrap: anywhere;
}}
h1, h2, h3, h4, h5, h6 {{ line-height: 1.25; margin: 1.8em 0 0.6em; color: var(--accent); }}
h1 {{ font-size: 2em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }}
h2 {{ font-size: 1.5em; padding-bottom: 0.25em; border-bottom: 1px solid var(--border); }}
h3 {{ font-size: 1.25em; color: var(--accent2); }}
h4, h5, h6 {{ font-size: 1em; color: var(--accent2); }}
p, ul, ol, dl, table, blockquote, figure {{ margin: 0 0 1em; }}
a {{ color: var(--accent2); text-decoration: underline; text-underline-offset: 2px; }}
code, kbd {{
  font: 0.9em ui-monospace, SFMono-Regular, Menlo, Consolas, "DejaVu Sans Mono", monospace;
}}
:not(pre) > code {{ background: var(--bg-alt); color: var(--orange); padding: 0.15em 0.4em; border-radius: 4px; }}
figure.code {{ margin: 0 0 1.2em; border: 1px solid var(--border); border-radius: 6px; overflow: hidden; background: var(--bg-alt); }}
figure.code figcaption {{ padding: 0.2em 0.8em; font-size: 0.75em; color: var(--fg-muted); border-bottom: 1px solid var(--border); text-transform: lowercase; }}
figure.code pre {{ margin: 0; padding: 0.8em 1em; overflow-x: auto; line-height: 1.45; color: var(--fg); }}
figure.code code {{ background: none; padding: 0; color: inherit; }}
blockquote {{ margin-left: 0; padding: 0.1em 1em; border-left: 4px solid var(--accent); color: var(--fg-dim); background: var(--bg-alt); border-radius: 0 6px 6px 0; }}
blockquote > :last-child {{ margin-bottom: 0; }}
.markdown-alert {{ border-left-width: 4px; }}
.markdown-alert-note {{ border-left-color: var(--cyan); }}
.markdown-alert-tip {{ border-left-color: var(--green); }}
.markdown-alert-important {{ border-left-color: var(--purple); }}
.markdown-alert-warning {{ border-left-color: var(--yellow); }}
.markdown-alert-caution {{ border-left-color: var(--red); }}
table {{ border-collapse: collapse; display: block; max-width: 100%; overflow-x: auto; }}
th, td {{ border: 1px solid var(--border); padding: 0.4em 0.8em; }}
th {{ background: var(--bg-alt); color: var(--accent); text-align: left; }}
hr {{ border: 0; border-top: 1px solid var(--border); margin: 2em 0; }}
img {{ max-width: 100%; height: auto; }}
ul.contains-task-list, li.task-list-item {{ list-style: none; }}
li > input[type="checkbox"] {{ margin: 0 0.5em 0 -1.3em; }}
del {{ color: var(--fg-muted); }}
mark {{ background: var(--yellow); color: var(--bg); }}
.footnote-definition {{ font-size: 0.9em; color: var(--fg-dim); }}
.math {{ font-family: "Latin Modern Math", "STIX Two Math", serif; font-style: italic; }}
::selection {{ background: var(--bg-sel); }}
@page {{ margin: 14mm; }}
@media print {{
  body {{ max-width: none; margin: 0; padding: 0; }}
  html, body {{ -webkit-print-color-adjust: exact; print-color-adjust: exact; }}
  figure.code, blockquote, table, img {{ break-inside: avoid; }}
  figure.code pre {{ white-space: pre-wrap; overflow: visible; }}
  h1, h2, h3, h4 {{ break-after: avoid; }}
}}
"#,
        scheme = if dark { "dark" } else { "light" },
        bg = c(theme.bg),
        bg_alt = c(theme.bg_alt),
        bg_sel = c(theme.bg_sel),
        fg = c(theme.fg),
        fg_dim = c(theme.fg_dim),
        fg_muted = c(theme.fg_muted),
        accent = c(theme.accent),
        accent2 = c(theme.accent2),
        border = c(theme.border),
        green = c(theme.green),
        yellow = c(theme.yellow),
        orange = c(theme.orange),
        red = c(theme.red),
        purple = c(theme.purple),
        cyan = c(theme.cyan),
    )
}

fn page(title: &str, theme: &Theme, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"{slug}\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         {GENERATOR_MARK}\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: https: http:; style-src 'unsafe-inline'\">\n\
         <title>{title}</title>\n<style>\n{css}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        slug = theme.name.slug(),
        title = escape(title),
        css = stylesheet(theme),
    )
}

/// One code block as HTML, coloured with the viewer's highlighter.
fn code_block_html(code: &str, lang: &str, theme: &Theme, highlighter: &mut CodeHighlighter) -> String {
    let highlighted = highlighter.highlight(code, lang, theme);
    let mut out = String::from("<figure class=\"code\">");
    if !highlighted.label.is_empty() {
        out.push_str(&format!("<figcaption>{}</figcaption>", escape(&highlighted.label)));
    }
    out.push_str("<pre><code>");
    for (index, line) in highlighted.lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        for span in line {
            let mut style = String::new();
            if let Some(color) = span.style.fg.and_then(hex) {
                style.push_str(&format!("color:{color};"));
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                style.push_str("font-weight:bold;");
            }
            if span.style.add_modifier.contains(Modifier::ITALIC) {
                style.push_str("font-style:italic;");
            }
            if span.style.add_modifier.contains(Modifier::UNDERLINED) {
                style.push_str("text-decoration:underline;");
            }
            if style.is_empty() {
                out.push_str(&escape(&span.text));
            } else {
                out.push_str(&format!("<span style=\"{style}\">{}</span>", escape(&span.text)));
            }
        }
    }
    out.push_str("</code></pre></figure>\n");
    out
}

// ------------------------------------------------------------------ images

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(char::from(BASE64[(triple >> 18) as usize & 63]));
        out.push(char::from(BASE64[(triple >> 12) as usize & 63]));
        out.push(if chunk.len() > 1 { char::from(BASE64[(triple >> 6) as usize & 63]) } else { '=' });
        out.push(if chunk.len() > 2 { char::from(BASE64[triple as usize & 63]) } else { '=' });
    }
    out
}

fn image_mime(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// `data:` URI for a local image of the project, or `None` to leave the link
/// as it is (remote, missing, too big, outside the project, not an image).
fn embed_image(destination: &str, base_dir: &Path, root: &Path) -> Option<String> {
    let destination = destination.trim();
    if destination.is_empty() || destination.contains("://") || destination.starts_with("data:") {
        return None;
    }
    let path_part = destination.split(['#', '?']).next().unwrap_or(destination);
    let decoded = crate::markdown::toc::decode_fragment(path_part);
    let candidate = match decoded.strip_prefix('/') {
        Some(from_root) => root.join(from_root),
        None => base_dir.join(&decoded),
    };
    let mime = image_mime(&candidate)?;
    let canonical = candidate.canonicalize().ok()?;
    if !canonical.starts_with(root.canonicalize().ok()?) {
        return None;
    }
    let metadata = fs::metadata(&canonical).ok()?;
    if !metadata.is_file() || metadata.len() > IMAGE_LIMIT {
        return None;
    }
    let bytes = fs::read(&canonical).ok()?;
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

// ---------------------------------------------------------------- documents

/// Where a document lives, to embed its images.
#[derive(Debug, Clone, Copy)]
pub struct Location<'a> {
    pub base_dir: &'a Path,
    pub root: &'a Path,
}

/// Slugs for the headings that do not carry an explicit id, in order.
fn heading_slugs(source: &str) -> Vec<String> {
    let mut slugs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut current: Option<(bool, String)> = None;
    for event in Parser::new_ext(source, markdown_options()) {
        match event {
            Event::Start(Tag::Heading { id, .. }) => {
                if let Some(id) = &id {
                    seen.push(id.to_string());
                }
                current = Some((id.is_none(), String::new()));
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, buffer)) = &mut current {
                    buffer.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((needs_slug, text)) = current.take() {
                    if needs_slug {
                        let base = slugify(&text);
                        let mut slug = base.clone();
                        let mut copy = 1;
                        while seen.contains(&slug) {
                            slug = format!("{base}-{copy}");
                            copy += 1;
                        }
                        seen.push(slug.clone());
                        slugs.push(slug);
                    }
                }
            }
            _ => {}
        }
    }
    slugs
}

/// Text of the first level-1 heading, for the page title.
fn first_title(source: &str) -> Option<String> {
    let mut inside = false;
    let mut title = String::new();
    for event in Parser::new_ext(source, markdown_options()) {
        match event {
            Event::Start(Tag::Heading { level: pulldown_cmark::HeadingLevel::H1, .. }) => inside = true,
            Event::Text(text) | Event::Code(text) if inside => title.push_str(&text),
            Event::End(TagEnd::Heading(_)) if inside => break,
            _ => {}
        }
    }
    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// A Markdown document as a complete, self-contained HTML page.
pub fn markdown_to_html(
    source: &str,
    file_name: &str,
    location: Option<Location<'_>>,
    theme: &Theme,
    highlighter: &mut CodeHighlighter,
) -> String {
    let mut slugs = heading_slugs(source).into_iter();
    let mut events: Vec<Event<'_>> = Vec::new();
    let mut code: Option<(String, String)> = None;
    for event in Parser::new_ext(source, markdown_options()) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, text)) = code.take() {
                    events.push(Event::Html(CowStr::from(code_block_html(&text, &lang, theme, highlighter))));
                }
            }
            Event::Text(text) if code.is_some() => {
                if let Some((_, buffer)) = &mut code {
                    buffer.push_str(&text);
                }
            }
            Event::Start(Tag::Heading { level, id: None, classes, attrs }) => {
                let id = slugs.next().filter(|slug| !slug.is_empty()).map(CowStr::from);
                events.push(Event::Start(Tag::Heading { level, id, classes, attrs }));
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                let dest_url = match location.and_then(|place| embed_image(&dest_url, place.base_dir, place.root)) {
                    Some(uri) => CowStr::from(uri),
                    None => dest_url,
                };
                events.push(Event::Start(Tag::Image { link_type, dest_url, title, id }));
            }
            other => events.push(other),
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    let title = first_title(source).unwrap_or_else(|| file_name.to_string());
    page(&title, theme, &body)
}

/// A non-Markdown text file as a page holding its (highlighted) source.
pub fn text_to_html(
    source: &str,
    file_name: &str,
    theme: &Theme,
    highlighter: &mut CodeHighlighter,
) -> String {
    let lang = Path::new(file_name)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_string)
        .unwrap_or_else(|| file_name.to_string());
    let body = format!(
        "<h1>{}</h1>\n{}",
        escape(file_name),
        code_block_html(source, &lang, theme, highlighter)
    );
    page(file_name, theme, &body)
}

// -------------------------------------------------------------- converters

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConverterKind {
    Wkhtmltopdf,
    Chromium,
    Weasyprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converter {
    pub name: &'static str,
    pub program: PathBuf,
    pub kind: ConverterKind,
}

/// In order of preference.
const CANDIDATES: [(&str, ConverterKind); 8] = [
    ("wkhtmltopdf", ConverterKind::Wkhtmltopdf),
    ("chromium", ConverterKind::Chromium),
    ("chromium-browser", ConverterKind::Chromium),
    ("google-chrome", ConverterKind::Chromium),
    ("google-chrome-stable", ConverterKind::Chromium),
    ("chrome", ConverterKind::Chromium),
    ("microsoft-edge", ConverterKind::Chromium),
    ("weasyprint", ConverterKind::Weasyprint),
];

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The first converter found in the directories of `path_var`.
pub fn find_converter(path_var: &OsStr) -> Option<Converter> {
    let directories: Vec<PathBuf> = std::env::split_paths(path_var).collect();
    for (name, kind) in CANDIDATES {
        for directory in &directories {
            let mut candidates = vec![directory.join(name)];
            if cfg!(windows) {
                candidates.insert(0, directory.join(format!("{name}.exe")));
            }
            if let Some(program) = candidates.into_iter().find(|path| is_executable(path)) {
                return Some(Converter { name, program, kind });
            }
        }
    }
    None
}

fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !text.starts_with('/') {
        url.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                url.push(char::from(byte));
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

/// Arguments for `converter` (the program itself is not included).  Each path
/// is exactly one argument, so spaces in names need no quoting.
pub fn converter_args(
    converter: &Converter,
    html: &Path,
    pdf: &Path,
    profile: &Path,
    no_sandbox: bool,
) -> Vec<OsString> {
    match converter.kind {
        ConverterKind::Wkhtmltopdf => vec![
            "--quiet".into(),
            "--enable-local-file-access".into(),
            "--print-media-type".into(),
            html.as_os_str().to_os_string(),
            pdf.as_os_str().to_os_string(),
        ],
        ConverterKind::Weasyprint => vec![html.as_os_str().to_os_string(), pdf.as_os_str().to_os_string()],
        ConverterKind::Chromium => {
            let mut args: Vec<OsString> = vec![
                "--headless".into(),
                "--disable-gpu".into(),
                "--no-pdf-header-footer".into(),
            ];
            if no_sandbox {
                args.push("--no-sandbox".into());
            }
            let mut profile_arg = OsString::from("--user-data-dir=");
            profile_arg.push(profile);
            args.push(profile_arg);
            let mut output = OsString::from("--print-to-pdf=");
            output.push(pdf);
            args.push(output);
            args.push(file_url(html).into());
            args
        }
    }
}

/// Convert `html` to `pdf`.  `Ok` carries a note when something unusual was
/// needed (a browser that refuses to run as root).
pub fn convert(
    converter: &Converter,
    html: &Path,
    pdf: &Path,
    scratch: &Path,
    timeout: Duration,
) -> Result<Option<&'static str>, String> {
    let attempt = |no_sandbox: bool| -> Result<(), String> {
        let _ = fs::remove_file(pdf);
        let log_path = scratch.join("converter.log");
        let log = fs::File::create(&log_path).map_err(|error| error.to_string())?;
        let profile = scratch.join("profile");
        let args = converter_args(converter, html, pdf, &profile, no_sandbox);
        let mut child = Command::new(&converter.program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", converter.name))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} took longer than {} s", converter.name, timeout.as_secs()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => return Err(error.to_string()),
            }
        };
        let mut output = String::new();
        if let Ok(mut file) = fs::File::open(&log_path) {
            let _ = file.by_ref().take(16 * 1024).read_to_string(&mut output);
        }
        let reason = output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(140).collect::<String>());
        if !status.success() {
            return Err(match reason {
                Some(reason) => format!("{}: {reason}", converter.name),
                None => format!("{} failed ({status})", converter.name),
            });
        }
        match fs::metadata(pdf) {
            Ok(meta) if meta.len() > 0 => Ok(()),
            _ => Err(format!("{} produced no file", converter.name)),
        }
    };

    match attempt(false) {
        Ok(()) => Ok(None),
        // A browser run as root refuses to start its sandbox; the page was
        // written by runetui (and forbids scripts), so try once without it.
        Err(message) if converter.kind == ConverterKind::Chromium && message.contains("no-sandbox") => {
            attempt(true).map(|()| Some("sandbox off: running as root"))
        }
        Err(message) => Err(message),
    }
}

// ------------------------------------------------------------------ export

/// What to export and how.
#[derive(Debug, Clone)]
pub struct Request {
    pub source: PathBuf,
    /// Project root (images outside it are never embedded).
    pub root: PathBuf,
    pub format: Format,
    /// Where the result goes.
    pub out_dir: PathBuf,
    pub theme: Theme,
    /// `PATH` used to look for a PDF converter.
    pub path_var: OsString,
    pub timeout: Duration,
}

/// What an export did, for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub message: String,
    /// The file that was written, if any.
    pub written: Option<PathBuf>,
    pub ok: bool,
}

fn failure(message: impl Into<String>) -> Outcome {
    Outcome {
        message: message.into(),
        written: None,
        ok: false,
    }
}

/// An existing file that runetui may replace: a page it wrote, or a PDF.
fn replaceable(path: &Path, format: Format) -> bool {
    let mut head = [0u8; 4096];
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let read = file.read(&mut head).unwrap_or(0);
    let head = &head[..read];
    match format {
        Format::Html => String::from_utf8_lossy(head).contains(GENERATOR_MARK),
        Format::Pdf => head.starts_with(b"%PDF"),
    }
}

/// The path to write: `name.ext`, unless a file that is not ours is there, in
/// which case `name (2).ext`, `name (3).ext`...
pub fn output_path(out_dir: &Path, name: &str, format: Format) -> PathBuf {
    let extension = format.extension();
    let first = out_dir.join(format!("{name}.{extension}"));
    if !first.exists() || replaceable(&first, format) {
        return first;
    }
    for number in 2..1000 {
        let candidate = out_dir.join(format!("{name} ({number}).{extension}"));
        if !candidate.exists() || replaceable(&candidate, format) {
            return candidate;
        }
    }
    first
}

/// `README.md` is exported as `README.*`; other files keep their extension in
/// the name (`main.rs.html`) so they cannot clash with a Markdown file.
fn export_name(file_name: &str) -> String {
    if is_markdown(file_name) {
        Path::new(file_name)
            .file_stem()
            .map_or_else(|| file_name.to_string(), |stem| stem.to_string_lossy().into_owned())
    } else {
        file_name.to_string()
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".part");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn scratch_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let path = std::env::temp_dir().join(format!("rtui-export-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Do the export.  Blocking: the app calls it from a thread.
pub fn run(request: &Request) -> Outcome {
    let file_name = request
        .source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = match fs::metadata(&request.source) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return failure(format!("Cannot export {file_name}: not a file")),
        Err(error) => return failure(format!("Cannot export {file_name}: {error}")),
    };
    if metadata.len() > MAX_FILE_SIZE {
        return failure(format!("Cannot export {file_name}: the file is too big"));
    }
    let source = match fs::read(&request.source).map(String::from_utf8) {
        Ok(Ok(text)) => text,
        Ok(Err(_)) => return failure(format!("Cannot export {file_name}: not a text file")),
        Err(error) => return failure(format!("Cannot export {file_name}: {error}")),
    };

    let mut highlighter = CodeHighlighter::new();
    let html = if is_markdown(&file_name) {
        let base_dir = request.source.parent().unwrap_or_else(|| Path::new("."));
        let location = Location { base_dir, root: &request.root };
        markdown_to_html(&source, &file_name, Some(location), &request.theme, &mut highlighter)
    } else {
        text_to_html(&source, &file_name, &request.theme, &mut highlighter)
    };

    if let Err(error) = fs::create_dir_all(&request.out_dir) {
        return failure(format!("Cannot create {}: {error}", request.out_dir.display()));
    }
    let name = export_name(&file_name);

    let write_html = |html: &str| -> Result<PathBuf, String> {
        let target = output_path(&request.out_dir, &name, Format::Html);
        write_atomically(&target, html.as_bytes())
            .map(|()| target.clone())
            .map_err(|error| format!("Cannot write {}: {error}", target.display()))
    };

    match request.format {
        Format::Html => match write_html(&html) {
            Ok(path) => Outcome {
                message: format!("Exported HTML: {}", path.display()),
                written: Some(path),
                ok: true,
            },
            Err(message) => failure(message),
        },
        Format::Pdf => {
            let Some(converter) = find_converter(&request.path_var) else {
                return match write_html(&html) {
                    Ok(path) => Outcome {
                        message: format!(
                            "PDF export needs wkhtmltopdf, chromium (--headless) or weasyprint, and none was found in PATH. Wrote HTML instead: {}",
                            path.display()
                        ),
                        written: Some(path),
                        ok: false,
                    },
                    Err(message) => failure(message),
                };
            };
            let scratch = match scratch_dir() {
                Ok(path) => path,
                Err(error) => return failure(format!("Cannot export PDF: {error}")),
            };
            let result = (|| -> Result<(PathBuf, Option<&'static str>), String> {
                let html_path = scratch.join("document.html");
                fs::write(&html_path, html.as_bytes()).map_err(|error| error.to_string())?;
                let pdf_path = scratch.join("document.pdf");
                let note = convert(&converter, &html_path, &pdf_path, &scratch, request.timeout)?;
                let target = output_path(&request.out_dir, &name, Format::Pdf);
                let bytes = fs::read(&pdf_path).map_err(|error| error.to_string())?;
                write_atomically(&target, &bytes)
                    .map_err(|error| format!("Cannot write {}: {error}", target.display()))?;
                Ok((target, note))
            })();
            let _ = fs::remove_dir_all(&scratch);
            match result {
                Ok((path, note)) => Outcome {
                    message: format!(
                        "Exported PDF ({}{}): {}",
                        converter.name,
                        note.map(|note| format!(", {note}")).unwrap_or_default(),
                        path.display()
                    ),
                    written: Some(path),
                    ok: true,
                },
                Err(reason) => match write_html(&html) {
                    Ok(path) => Outcome {
                        message: format!("PDF conversion failed ({reason}). Wrote HTML instead: {}", path.display()),
                        written: Some(path),
                        ok: false,
                    },
                    Err(message) => failure(format!("PDF conversion failed ({reason}); {message}")),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::ThemeName;

    fn render(source: &str, theme: ThemeName) -> String {
        let mut highlighter = CodeHighlighter::new();
        markdown_to_html(source, "doc.md", None, &theme.theme(), &mut highlighter)
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"Man is distinguished"), "TWFuIGlzIGRpc3Rpbmd1aXNoZWQ=");
    }

    #[test]
    fn the_page_is_self_contained_and_follows_the_theme() {
        let dark = render("# Title\n\ntext\n", ThemeName::CatppuccinMocha);
        assert!(dark.starts_with("<!DOCTYPE html>"));
        assert!(dark.contains("<style>") && dark.contains("</style>"), "the CSS is embedded");
        assert!(!dark.contains("<link") && !dark.contains("<script"), "nothing external");
        assert!(dark.contains("color-scheme: dark;"));
        assert!(dark.contains("data-theme=\"catppuccin-mocha\""));
        assert!(dark.contains(GENERATOR_MARK));
        assert!(dark.contains("<title>Title</title>"), "the first heading names the page");
        assert!(dark.contains("Content-Security-Policy") && dark.contains("default-src 'none'"));
        let Color::Rgb(r, g, b) = ThemeName::CatppuccinMocha.theme().bg else { panic!() };
        assert!(dark.contains(&format!("--bg: #{r:02x}{g:02x}{b:02x};")));

        let light = render("# Title\n", ThemeName::White);
        assert!(light.contains("color-scheme: light;"));
        assert!(!is_dark(&ThemeName::Snow.theme()) && is_dark(&ThemeName::Black.theme()));
    }

    #[test]
    fn every_bundled_theme_gives_a_valid_stylesheet() {
        for name in ThemeName::ALL {
            let css = stylesheet(&name.theme());
            assert!(css.contains("--accent: #"), "{} has RGB colours", name.slug());
            assert!(!css.contains("{{") && !css.contains("}}"), "{} has unescaped braces", name.slug());
        }
    }

    #[test]
    fn a_document_without_a_heading_is_titled_by_its_file_name() {
        let mut highlighter = CodeHighlighter::new();
        let page = markdown_to_html("just text\n", "notes.md", None, &ThemeName::Nord.theme(), &mut highlighter);
        assert!(page.contains("<title>notes.md</title>"));
        let tricky = markdown_to_html("# 1 < 2 & \"x\"\n", "a.md", None, &ThemeName::Nord.theme(), &mut highlighter);
        assert!(tricky.contains("<title>1 &lt; 2 &amp; &quot;x&quot;</title>"), "the title is escaped");
    }

    #[test]
    fn code_blocks_are_coloured_with_the_viewers_highlighter() {
        let page = render("```rust\nfn main() { let x = \"<hi>\"; }\n```\n", ThemeName::CatppuccinMocha);
        assert!(page.contains("<figure class=\"code\">"));
        assert!(page.contains("<figcaption>"), "the language label");
        assert!(page.contains("<span style=\"color:#"), "{page}");
        assert!(page.contains("&lt;hi&gt;"), "code is escaped");
        assert!(!page.contains("<hi>"));
        // Unknown and missing languages still render.
        assert!(render("```\nplain\n```\n", ThemeName::Nord).contains("plain"));
        assert!(render("```nosuchlang\nx\n```\n", ThemeName::Nord).contains("<pre><code>"));
        assert!(render("    indented\n", ThemeName::Nord).contains("indented"));
        assert!(render("```rust\n```\n", ThemeName::Nord).contains("<figure"));
    }

    #[test]
    fn headings_get_ids_so_anchor_links_work() {
        let page = render("# Hello World\n\n## Hello World\n\n## Other {#custom}\n\n[go](#hello-world-1)\n", ThemeName::Nord);
        assert!(page.contains("<h1 id=\"hello-world\">"), "{page}");
        assert!(page.contains("<h2 id=\"hello-world-1\">"), "repeated headings get a number\n{page}");
        assert!(page.contains("id=\"custom\""), "explicit ids are kept\n{page}");
    }

    #[test]
    fn markdown_features_come_through() {
        let page = render(
            "| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n\n~~gone~~\n\n[[wiki|alias]]\n\n> [!NOTE]\n> careful\n\nfootnote[^1]\n\n[^1]: the note\n",
            ThemeName::Nord,
        );
        assert!(page.contains("<table>") && page.contains("<th>a</th>"));
        assert!(page.contains("type=\"checkbox\""));
        assert!(page.contains("<del>gone</del>"));
        assert!(page.contains(">alias</a>"), "{page}");
        assert!(page.contains("markdown-alert-note"), "{page}");
        assert!(page.contains("footnote-definition"));
    }

    #[test]
    fn front_matter_is_not_shown() {
        let page = render("---\ntitle: secret\n---\n\n# Visible\n", ThemeName::Nord);
        assert!(!page.contains("secret"), "{page}");
        assert!(page.contains("Visible"));
    }

    #[test]
    fn local_images_are_embedded_and_remote_or_unsafe_ones_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(root.join("img")).unwrap();
        fs::write(root.join("img/a b.png"), [0x89, b'P', b'N', b'G']).unwrap();
        fs::write(root.join("big.png"), vec![0u8; (IMAGE_LIMIT + 1) as usize]).unwrap();
        fs::write(root.join("notes.txt"), "x").unwrap();
        fs::write(dir.path().join("secret.png"), [1, 2, 3]).unwrap();
        let source = "![a](img/a%20b.png)\n\n![r](/img/a%20b.png)\n\n![web](https://e.com/x.png)\n\n![big](big.png)\n\n![out](../secret.png)\n\n![txt](notes.txt)\n\n![gone](missing.png)\n";
        let mut highlighter = CodeHighlighter::new();
        let place = Location { base_dir: &root, root: &root };
        let page = markdown_to_html(source, "d.md", Some(place), &ThemeName::Nord.theme(), &mut highlighter);
        assert_eq!(page.matches("src=\"data:image/png;base64,iVBORw==\"").count(), 2, "{page}");
        assert!(page.contains("src=\"https://e.com/x.png\""));
        assert!(page.contains("src=\"big.png\""), "too big to embed");
        assert!(page.contains("src=\"../secret.png\""), "outside the project");
        assert!(page.contains("src=\"notes.txt\"") && page.contains("src=\"missing.png\""));
    }

    #[test]
    fn raw_html_is_kept_but_the_policy_forbids_scripts() {
        let page = render("<script>alert(1)</script>\n\n<b>bold</b>\n", ThemeName::Nord);
        assert!(page.contains("<b>bold</b>"));
        assert!(page.contains("default-src 'none'"), "no script-src means no scripts run");
        assert!(!page.contains("script-src") && !page.contains("unsafe-eval"));
    }

    #[test]
    fn other_text_files_become_a_highlighted_page() {
        let mut highlighter = CodeHighlighter::new();
        let page = text_to_html("fn main() {}\n", "main.rs", &ThemeName::Dracula.theme(), &mut highlighter);
        assert!(page.contains("<h1>main.rs</h1>") && page.contains("<span style=\"color:#"));
        let empty = text_to_html("", "empty.txt", &ThemeName::Dracula.theme(), &mut highlighter);
        assert!(empty.contains("<pre><code>"));
    }

    #[test]
    fn file_urls_escape_what_a_browser_needs_escaped() {
        assert_eq!(file_url(Path::new("/tmp/a b/c#d.html")), "file:///tmp/a%20b/c%23d.html");
        assert_eq!(file_url(Path::new("/tmp/é.html")), "file:///tmp/%C3%A9.html");
    }

    // -------------------------------------------------------- converters

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn converters_are_found_in_order_and_must_be_executable() {
        use std::os::unix::fs::PermissionsExt;
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        assert!(find_converter(&path(&[one.path()])).is_none());

        script(two.path(), "weasyprint", "true");
        assert_eq!(find_converter(&path(&[one.path(), two.path()])).unwrap().name, "weasyprint");
        script(one.path(), "chromium", "true");
        let found = find_converter(&path(&[one.path(), two.path()])).unwrap();
        assert_eq!((found.name, found.kind), ("chromium", ConverterKind::Chromium), "chromium comes before weasyprint");
        script(two.path(), "wkhtmltopdf", "true");
        assert_eq!(find_converter(&path(&[one.path(), two.path()])).unwrap().name, "wkhtmltopdf");

        // A file without the execute bit is not a program.
        let plain = tempfile::tempdir().unwrap();
        fs::write(plain.path().join("wkhtmltopdf"), "x").unwrap();
        fs::set_permissions(plain.path().join("wkhtmltopdf"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(find_converter(&path(&[plain.path()])).is_none());
        // Directories that do not exist are fine.
        assert!(find_converter(&path(&[Path::new("/no/such/dir")])).is_none());
    }

    #[test]
    fn every_path_is_one_argument_even_with_spaces() {
        let html = Path::new("/tmp/my docs/in put.html");
        let pdf = Path::new("/tmp/my docs/out put.pdf");
        let profile = Path::new("/tmp/my docs/profile");
        let make = |kind, name| Converter { name, program: PathBuf::from(name), kind };

        let args = converter_args(&make(ConverterKind::Wkhtmltopdf, "wkhtmltopdf"), html, pdf, profile, false);
        assert_eq!(args[args.len() - 2], html.as_os_str());
        assert_eq!(args[args.len() - 1], pdf.as_os_str());

        let args = converter_args(&make(ConverterKind::Weasyprint, "weasyprint"), html, pdf, profile, false);
        assert_eq!(args, vec![html.as_os_str().to_os_string(), pdf.as_os_str().to_os_string()]);

        let args = converter_args(&make(ConverterKind::Chromium, "chromium"), html, pdf, profile, false);
        assert!(args.contains(&OsString::from("--headless")));
        assert!(args.contains(&OsString::from("--print-to-pdf=/tmp/my docs/out put.pdf")));
        assert!(args.contains(&OsString::from("--user-data-dir=/tmp/my docs/profile")));
        assert_eq!(args.last().unwrap(), "file:///tmp/my%20docs/in%20put.html");
        assert!(!args.contains(&OsString::from("--no-sandbox")));
        let args = converter_args(&make(ConverterKind::Chromium, "chromium"), html, pdf, profile, true);
        assert!(args.contains(&OsString::from("--no-sandbox")));
    }

    #[cfg(unix)]
    fn fake_pdf_script(dir: &Path, name: &str) {
        // The output path is the last argument of wkhtmltopdf and weasyprint.
        script(dir, name, "for last; do :; done\nprintf '%%PDF-1.4 fake' > \"$last\"");
    }

    #[cfg(unix)]
    #[test]
    fn a_converter_that_works_produces_the_pdf() {
        let bin = tempfile::tempdir().unwrap();
        fake_pdf_script(bin.path(), "wkhtmltopdf");
        let work = tempfile::tempdir().unwrap();
        fs::write(work.path().join("in.html"), "<p>x</p>").unwrap();
        let converter = find_converter(bin.path().as_os_str()).unwrap();
        let note = convert(&converter, &work.path().join("in.html"), &work.path().join("out.pdf"), work.path(), Duration::from_secs(20)).unwrap();
        assert_eq!(note, None);
        assert!(fs::read(work.path().join("out.pdf")).unwrap().starts_with(b"%PDF"));
    }

    #[cfg(unix)]
    #[test]
    fn converter_failures_are_explained() {
        let work = tempfile::tempdir().unwrap();
        fs::write(work.path().join("in.html"), "<p>x</p>").unwrap();
        let try_with = |body: &str, timeout: Duration| {
            let bin = tempfile::tempdir().unwrap();
            script(bin.path(), "weasyprint", body);
            let converter = find_converter(bin.path().as_os_str()).unwrap();
            convert(&converter, &work.path().join("in.html"), &work.path().join("out.pdf"), work.path(), timeout)
        };
        let message = try_with("echo 'boom: cannot render' >&2\nexit 3", Duration::from_secs(20)).unwrap_err();
        assert_eq!(message, "weasyprint: boom: cannot render");
        let message = try_with("exit 1", Duration::from_secs(20)).unwrap_err();
        assert!(message.starts_with("weasyprint failed"), "{message}");
        let message = try_with("exit 0", Duration::from_secs(20)).unwrap_err();
        assert_eq!(message, "weasyprint produced no file");
        let message = try_with("sleep 30", Duration::from_millis(300)).unwrap_err();
        assert!(message.contains("took longer"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_that_refuses_to_run_as_root_is_retried_without_its_sandbox() {
        let bin = tempfile::tempdir().unwrap();
        script(
            bin.path(),
            "chromium",
            "case \"$*\" in\n  *--no-sandbox*) for a; do case \"$a\" in --print-to-pdf=*) printf '%%PDF-ok' > \"${a#--print-to-pdf=}\";; esac; done;;\n  *) echo 'Running as root without --no-sandbox is not supported' >&2; exit 1;;\nesac",
        );
        let work = tempfile::tempdir().unwrap();
        fs::write(work.path().join("in.html"), "<p>x</p>").unwrap();
        let converter = find_converter(bin.path().as_os_str()).unwrap();
        let note = convert(&converter, &work.path().join("in.html"), &work.path().join("out.pdf"), work.path(), Duration::from_secs(20)).unwrap();
        assert_eq!(note, Some("sandbox off: running as root"));
        assert!(work.path().join("out.pdf").exists());
    }

    // ------------------------------------------------------------- run

    fn request(dir: &Path, file: &str, format: Format, path_var: &Path) -> Request {
        Request {
            source: dir.join(file),
            root: dir.to_path_buf(),
            format,
            out_dir: dir.to_path_buf(),
            theme: ThemeName::Nord.theme(),
            path_var: path_var.as_os_str().to_os_string(),
            timeout: Duration::from_secs(20),
        }
    }

    #[test]
    fn html_is_written_next_to_the_original_and_the_message_says_where() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("guide.md"), "# Guide\n\ntext\n").unwrap();
        let none = tempfile::tempdir().unwrap();
        let outcome = run(&request(dir.path(), "guide.md", Format::Html, none.path()));
        assert!(outcome.ok, "{outcome:?}");
        let written = outcome.written.clone().unwrap();
        assert_eq!(written, dir.path().join("guide.html"));
        assert!(outcome.message.contains(&written.display().to_string()), "{}", outcome.message);
        let html = fs::read_to_string(&written).unwrap();
        assert!(html.contains("<h1 id=\"guide\">Guide</h1>"));
        assert!(!dir.path().join("guide.html.part").exists(), "no leftovers");
    }

    #[test]
    fn exporting_again_replaces_our_page_but_never_a_foreign_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# One\n").unwrap();
        let none = tempfile::tempdir().unwrap();
        let first = run(&request(dir.path(), "a.md", Format::Html, none.path()));
        fs::write(dir.path().join("a.md"), "# Two\n").unwrap();
        let second = run(&request(dir.path(), "a.md", Format::Html, none.path()));
        assert_eq!(first.written, second.written, "our own page is replaced");
        assert!(fs::read_to_string(dir.path().join("a.html")).unwrap().contains("Two"));

        fs::write(dir.path().join("b.md"), "# B\n").unwrap();
        fs::write(dir.path().join("b.html"), "<h1>my own hand-written page</h1>").unwrap();
        let outcome = run(&request(dir.path(), "b.md", Format::Html, none.path()));
        assert_eq!(outcome.written, Some(dir.path().join("b (2).html")));
        assert_eq!(fs::read_to_string(dir.path().join("b.html")).unwrap(), "<h1>my own hand-written page</h1>");
    }

    #[test]
    fn a_configured_folder_is_created_and_used() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# A\n").unwrap();
        let none = tempfile::tempdir().unwrap();
        let mut asked = request(dir.path(), "a.md", Format::Html, none.path());
        asked.out_dir = dir.path().join("exports/html");
        let outcome = run(&asked);
        assert_eq!(outcome.written, Some(dir.path().join("exports/html/a.html")));
        assert!(outcome.written.unwrap().is_file());
    }

    #[test]
    fn source_files_keep_their_extension_in_the_exported_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("main.md"), "# m\n").unwrap();
        let none = tempfile::tempdir().unwrap();
        assert_eq!(run(&request(dir.path(), "main.rs", Format::Html, none.path())).written, Some(dir.path().join("main.rs.html")));
        assert_eq!(run(&request(dir.path(), "main.md", Format::Html, none.path())).written, Some(dir.path().join("main.html")));
        assert_eq!(export_name("NOTES.MARKDOWN"), "NOTES");
        assert_eq!(export_name("Makefile"), "Makefile");
    }

    #[test]
    fn unreadable_inputs_fail_with_a_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        let none = tempfile::tempdir().unwrap();
        let missing = run(&request(dir.path(), "nope.md", Format::Html, none.path()));
        assert!(!missing.ok && missing.written.is_none() && missing.message.contains("nope.md"), "{missing:?}");
        fs::create_dir(dir.path().join("folder.md")).unwrap();
        assert!(run(&request(dir.path(), "folder.md", Format::Html, none.path())).message.contains("not a file"));
        fs::write(dir.path().join("bin.md"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(run(&request(dir.path(), "bin.md", Format::Html, none.path())).message.contains("not a text file"));
        fs::write(dir.path().join("huge.md"), vec![b'a'; (MAX_FILE_SIZE + 1) as usize]).unwrap();
        assert!(run(&request(dir.path(), "huge.md", Format::Html, none.path())).message.contains("too big"));
    }

    #[test]
    fn without_a_converter_pdf_falls_back_to_html_and_names_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# A\n").unwrap();
        let none = tempfile::tempdir().unwrap();
        let outcome = run(&request(dir.path(), "a.md", Format::Pdf, none.path()));
        assert!(!outcome.ok);
        for program in ["wkhtmltopdf", "chromium", "weasyprint"] {
            assert!(outcome.message.contains(program), "{}", outcome.message);
        }
        assert!(outcome.message.contains("Wrote HTML instead"), "{}", outcome.message);
        assert_eq!(outcome.written, Some(dir.path().join("a.html")));
        assert!(!dir.path().join("a.pdf").exists());
    }

    #[cfg(unix)]
    #[test]
    fn pdf_goes_through_the_converter_and_leaves_no_html_behind() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("my notes.md"), "# Notes\n").unwrap();
        let bin = tempfile::tempdir().unwrap();
        fake_pdf_script(bin.path(), "wkhtmltopdf");
        let outcome = run(&request(dir.path(), "my notes.md", Format::Pdf, bin.path()));
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(outcome.written, Some(dir.path().join("my notes.pdf")));
        assert!(outcome.message.starts_with("Exported PDF (wkhtmltopdf)"), "{}", outcome.message);
        assert!(fs::read(dir.path().join("my notes.pdf")).unwrap().starts_with(b"%PDF"));
        assert!(!dir.path().join("my notes.html").exists(), "the page was only a step");
        // A PDF of the same name is replaced; the scratch folder is gone.
        let again = run(&request(dir.path(), "my notes.md", Format::Pdf, bin.path()));
        assert_eq!(again.written, outcome.written);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_converter_leaves_the_html_and_reports_why() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# A\n").unwrap();
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "wkhtmltopdf", "echo 'Exit with code 1 due to network error' >&2\nexit 1");
        let outcome = run(&request(dir.path(), "a.md", Format::Pdf, bin.path()));
        assert!(!outcome.ok);
        assert!(outcome.message.contains("PDF conversion failed (wkhtmltopdf: Exit with code 1"), "{}", outcome.message);
        assert_eq!(outcome.written, Some(dir.path().join("a.html")));
    }

    #[cfg(unix)]
    #[test]
    fn a_foreign_pdf_is_not_overwritten_unless_it_is_a_pdf() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# A\n").unwrap();
        fs::write(dir.path().join("a.pdf"), "not really a pdf").unwrap();
        let bin = tempfile::tempdir().unwrap();
        fake_pdf_script(bin.path(), "weasyprint");
        let outcome = run(&request(dir.path(), "a.md", Format::Pdf, bin.path()));
        assert_eq!(outcome.written, Some(dir.path().join("a (2).pdf")));
        assert_eq!(fs::read_to_string(dir.path().join("a.pdf")).unwrap(), "not really a pdf");
    }
}

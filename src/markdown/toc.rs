//! Table of contents / document outline.
//!
//! The outline is derived from the headings found by the renderer, so it is
//! always in sync with the rendered document and requires no separate pass
//! over the Markdown source.

use crate::markdown::{Heading, slugify};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: u8,
    pub title: String,
    pub slug: String,
    pub line: usize,
}

/// Build the outline entries from rendered headings.
pub fn outline(headings: &[Heading]) -> Vec<OutlineItem> {
    headings
        .iter()
        .map(|heading| OutlineItem {
            level: heading.level.clamp(1, 6),
            title: heading.text.clone(),
            slug: heading.slug.clone(),
            line: heading.line,
        })
        .collect()
}

/// Resolve an anchor (`#some-heading`, `Some Heading`, percent-encoded…) to a
/// rendered line.  Matching is intentionally forgiving, like Obsidian.
pub fn resolve_anchor(headings: &[Heading], anchor: &str) -> Option<usize> {
    let decoded = decode_fragment(anchor);
    let wanted = decoded.trim().trim_start_matches('#').trim();
    if wanted.is_empty() {
        return Some(0);
    }
    let wanted_lower = wanted.to_lowercase();
    let wanted_slug = slugify(wanted);

    headings
        .iter()
        .find(|heading| {
            heading.slug == wanted_slug
                || heading.text.to_lowercase() == wanted_lower
                || heading.slug == wanted_lower
        })
        .map(|heading| heading.line)
}

/// Decode the small subset of percent-encoding used in Markdown anchors.
pub fn decode_fragment(fragment: &str) -> String {
    let mut bytes = Vec::with_capacity(fragment.len());
    let raw = fragment.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                bytes.push(value);
                index += 3;
                continue;
            }
        }
        let rest = &fragment[index..];
        if let Some(character) = rest.chars().next() {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            index += character.len_utf8();
        } else {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Split a link destination into `(path, fragment)`.
pub fn split_fragment(dest: &str) -> (&str, Option<&str>) {
    match dest.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (dest, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headings() -> Vec<Heading> {
        vec![
            Heading {
                level: 1,
                text: "Introduction".into(),
                slug: "introduction".into(),
                line: 0,
            },
            Heading {
                level: 2,
                text: "Getting Started".into(),
                slug: "getting-started".into(),
                line: 12,
            },
            Heading {
                level: 2,
                text: "¿Qué es esto?".into(),
                slug: "qué-es-esto".into(),
                line: 30,
            },
        ]
    }

    #[test]
    fn resolves_anchors_in_several_forms() {
        let headings = headings();
        assert_eq!(resolve_anchor(&headings, "#getting-started"), Some(12));
        assert_eq!(resolve_anchor(&headings, "Getting Started"), Some(12));
        assert_eq!(resolve_anchor(&headings, "#Getting%20Started"), Some(12));
        assert_eq!(resolve_anchor(&headings, "#qu%C3%A9-es-esto"), Some(30));
        assert_eq!(resolve_anchor(&headings, "#missing"), None);
        assert_eq!(resolve_anchor(&headings, ""), Some(0));
    }

    #[test]
    fn splits_link_fragments() {
        assert_eq!(
            split_fragment("docs/a.md#intro"),
            ("docs/a.md", Some("intro"))
        );
        assert_eq!(split_fragment("#intro"), ("", Some("intro")));
        assert_eq!(split_fragment("docs/a.md"), ("docs/a.md", None));
    }

    #[test]
    fn builds_outline() {
        let outline = outline(&headings());
        assert_eq!(outline.len(), 3);
        assert_eq!(outline[1].title, "Getting Started");
        assert_eq!(outline[1].level, 2);
    }
}

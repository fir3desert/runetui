//! Copying to the system clipboard through the terminal (OSC 52).
//!
//! Terminals that support OSC 52 (kitty, WezTerm, Alacritty, foot, iTerm2,
//! Windows Terminal, tmux with `set-clipboard on`...) put the text on the
//! clipboard of the machine the terminal runs on, which also works over SSH.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Texts longer than this are cut: many terminals refuse huge OSC 52 payloads.
pub const MAX_COPY_BYTES: usize = 100_000;

/// Standard Base64 with padding.
pub fn base64(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

/// The escape sequence that copies `text`.
pub fn osc52(text: &str) -> String {
    let mut end = text.len().min(MAX_COPY_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("\x1b]52;c;{}\x07", base64(&text.as_bytes()[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_wraps_the_payload() {
        assert_eq!(osc52("foo"), "\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let text = "ñ".repeat(MAX_COPY_BYTES);
        let sequence = osc52(&text);
        assert!(sequence.len() < MAX_COPY_BYTES * 2);
        assert!(sequence.starts_with("\x1b]52;c;"));
    }
}

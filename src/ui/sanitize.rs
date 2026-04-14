//! Terminal output sanitization for user-controlled strings.
//!
//! Strip ASCII control characters AND Unicode bidirectional
//! overrides (the subset of format-control characters that reorder
//! terminal output, including explicit bidi codes, isolates, directional
//! marks, and interlinear annotations). Other invisible / format
//! characters like zero-width joiners are NOT stripped to avoid
//! over-blocking legitimate text in CJK / Emoji contexts.
//!
//! Use whenever printing values that originate from user-editable
//! config, CLI args, or any other untrusted source.

/// Sanitize `s` for single-line terminal output. Strips ASCII control
/// characters and Unicode bidirectional overrides (explicit bidi codes,
/// isolates, directional marks, and interlinear annotations — replacing
/// with spaces). Other invisible / format characters (e.g. zero-width
/// joiners) are intentionally NOT stripped. Trims leading/trailing
/// whitespace and truncates to `max_len` chars.
pub fn sanitize_single_line(s: &str, max_len: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || is_bidi_override(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    cleaned.chars().take(max_len).collect()
}

/// Unicode bidirectional / interlinear annotation format controls that
/// can visually reorder or spoof text in terminals. These are distinct
/// from the ASCII control-character range and must be filtered separately
/// when displaying untrusted input.
fn is_bidi_override(c: char) -> bool {
    matches!(
        c,
        // Explicit bidi overrides
        '\u{202A}' | '\u{202B}' | '\u{202C}' | '\u{202D}' | '\u{202E}'
        // Isolates
        | '\u{2066}' | '\u{2067}' | '\u{2068}' | '\u{2069}'
        // Arabic letter mark, left-to-right mark, right-to-left mark
        | '\u{200E}' | '\u{200F}' | '\u{061C}'
        // Interlinear annotation
        | '\u{FFF9}' | '\u{FFFA}' | '\u{FFFB}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bidi_override_chars() {
        let evil = "safe\u{202E}text"; // Right-to-left override
        let out = sanitize_single_line(evil, 100);
        assert!(
            !out.chars().any(|c| c == '\u{202E}'),
            "bidi override should be stripped; got: {:?}",
            out
        );
    }

    #[test]
    fn strips_ascii_control_chars() {
        let s = "hello\x07world\x1bbell";
        let out = sanitize_single_line(s, 100);
        assert!(!out.chars().any(|c| c.is_control()));
    }

    #[test]
    fn preserves_normal_text() {
        let s = "normal ASCII and 日本語 text";
        let out = sanitize_single_line(s, 100);
        assert_eq!(out, "normal ASCII and 日本語 text");
    }
}

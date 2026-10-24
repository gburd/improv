//! Pure syntax highlighting for the formula editor: `&str -> LayoutJob`.
//!
//! A small independent char-class scanner (identifiers / numbers / strings /
//! date literals / operators), good enough for visually useful highlighting —
//! it does not need to match `core_model::parser`'s exact grammar. Kept free
//! of any `egui::Ui` access so it's unit-testable without a window.

use crate::theme::{BLACK, DARK_GRAY};
use egui::text::LayoutJob;
use egui::{Color32, FontId, Stroke, TextFormat};
use std::ops::Range;

/// A known scalar/aggregation function name, highlighted distinctly from a
/// plain measure/category identifier.
const FUNCTIONS: &[&str] = &[
    "SUM", "AVG", "MIN", "MAX", "ABS", "ROUND", "FLOOR", "CEIL", "SQRT", "NEG", "MIN2", "MAX2",
    "CALL", "SQL", "OVER", "TRUE", "FALSE", "AND", "OR", "NOT",
];

/// The lexical class of one highlighted token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A measure/category name, or a keyword-like bare word not in
    /// [`FUNCTIONS`].
    Ident,
    /// A recognized function/aggregation/keyword name (case-insensitive).
    Function,
    Number,
    String,
    /// A `#...#` date/time literal.
    Date,
    Operator,
    Whitespace,
}

/// One scanned token: its kind and byte range in the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

/// The character at byte offset `i` of `text`. `i` must be a char boundary
/// (every offset [`scan`] advances to is one, by construction).
///
/// This exists because `text.as_bytes()[i] as char` — the shape this scanner
/// used to have, and the same bug once present in
/// `core_model::parser::tokenize` — casts a single BYTE, which is wrong for
/// any multi-byte UTF-8 sequence and walks the cursor onto a non-boundary
/// offset, panicking on the eventual `&text[start..i]` slice.
fn char_at(text: &str, i: usize) -> char {
    text[i..]
        .chars()
        .next()
        .expect("i < len implies a char here")
}

/// Scan `text` into a flat sequence of [`Token`]s covering every byte (no
/// gaps), classifying identifiers/numbers/strings/date-literals/operators by
/// simple char class. Never fails — unrecognized characters become single-char
/// `Operator` tokens so the whole string is always covered.
///
/// Every `start`/`end` is a char boundary and the tokens tile `text` exactly
/// (no gaps, no overlap): [`highlight_formula`] slices `text` by those ranges.
pub fn scan(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = char_at(text, i);
        let kind = match c {
            c if c.is_whitespace() => {
                i += c.len_utf8();
                while i < bytes.len() {
                    let w = char_at(text, i);
                    if w.is_whitespace() {
                        i += w.len_utf8();
                    } else {
                        break;
                    }
                }
                TokenKind::Whitespace
            }
            '"' => {
                // Byte scanning is safe here: ASCII bytes never occur inside a
                // multi-byte UTF-8 sequence, so `i` stays on a char boundary.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1; // closing quote
                }
                TokenKind::String
            }
            '#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'#' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1; // closing '#'
                }
                TokenKind::Date
            }
            c if c.is_ascii_digit() => {
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                // An exponent (`1e-10`): `e`, a sign, digits.
                let sign = usize::from(matches!(bytes.get(i + 1), Some(b'+' | b'-')));
                if matches!(bytes.get(i), Some(b'e' | b'E'))
                    && bytes.get(i + 1 + sign).is_some_and(u8::is_ascii_digit)
                {
                    i += 1 + sign;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                TokenKind::Number
            }
            c if c.is_alphabetic() || c == '_' => {
                i += c.len_utf8();
                while i < bytes.len() {
                    let w = char_at(text, i);
                    if w.is_alphanumeric() || w == '_' {
                        i += w.len_utf8();
                    } else {
                        break;
                    }
                }
                let word = &text[start..i];
                if FUNCTIONS.iter().any(|f| word.eq_ignore_ascii_case(f)) {
                    TokenKind::Function
                } else {
                    TokenKind::Ident
                }
            }
            _ => {
                // Two-char operators first, then a single CHAR. `get` yields
                // None on a non-boundary/out-of-range end, so this can never
                // split a multi-byte sequence.
                let two = text.get(i..i + 2);
                if matches!(
                    two,
                    Some("<=") | Some(">=") | Some("<>") | Some("==") | Some("!=")
                ) {
                    i += 2;
                } else {
                    i += c.len_utf8();
                }
                TokenKind::Operator
            }
        };
        out.push(Token {
            kind,
            start,
            end: i,
        });
    }
    out
}

/// Token colors, within the NeXTSTEP grayscale palette (`theme.rs`): names,
/// functions and numbers in black on the white field, operators and literals
/// in dark gray (7.5:1 on white).
fn color_for(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Function | TokenKind::Number | TokenKind::Ident | TokenKind::Whitespace => BLACK,
        TokenKind::String | TokenKind::Date | TokenKind::Operator => DARK_GRAY,
    }
}

/// The color of formula error text and of the error-position underline. The
/// palette has no red, so an error is marked by the underline, not by hue.
pub const ERROR_COLOR: Color32 = BLACK;

/// The error-position underline: thicker than a text underline so it reads.
const ERROR_UNDERLINE: Stroke = Stroke {
    width: 2.0,
    color: ERROR_COLOR,
};

/// Build a highlighted [`LayoutJob`] for `text`, coloring tokens per
/// [`scan`]/[`color_for`]. If `error` is `Some(start..end)`, those bytes
/// (clamped to the text and to char boundaries; a span may cover part of a
/// token or several) are rendered in [`ERROR_COLOR`] with a 2 px underline so
/// the error's location is visible inline. `crate::diagnostics` places it:
/// the token at a parse error's position, or the name a compile error names.
pub fn highlight_formula(text: &str, font: FontId, error: Option<Range<usize>>) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.is_empty() {
        // One empty section, as egui's own layouter makes: a job with none
        // lays out zero rows high, and the edit shrinks by a pixel.
        job.append("", 0.0, TextFormat::simple(font, BLACK));
        return job;
    }
    let (lo, hi) = error.map_or((0, 0), |r| {
        let fix = |mut i: usize| {
            i = i.min(text.len());
            while !text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        (fix(r.start), fix(r.end))
    });
    for t in scan(text) {
        let fmt = TextFormat {
            font_id: font.clone(),
            color: color_for(t.kind),
            ..Default::default()
        };
        let err = TextFormat {
            underline: ERROR_UNDERLINE,
            color: ERROR_COLOR,
            ..fmt.clone()
        };
        // Up to three pieces: before the span, inside it, after it.
        let a = lo.clamp(t.start, t.end);
        let b = hi.clamp(a, t.end);
        for (s, e, f) in [(t.start, a, &fmt), (a, b, &err), (b, t.end, &fmt)] {
            if s < e {
                job.append(&text[s..e], 0.0, f.clone());
            }
        }
    }
    job
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(TokenKind, &str)> {
        scan(text)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, &text[t.start..t.end]))
            .collect()
    }

    #[test]
    fn identifies_idents_and_operator() {
        let toks = kinds("Price * Quantity");
        assert_eq!(
            toks,
            vec![
                (TokenKind::Ident, "Price"),
                (TokenKind::Operator, "*"),
                (TokenKind::Ident, "Quantity"),
            ]
        );
    }

    #[test]
    fn known_function_names_are_distinct_from_idents() {
        let toks = kinds("SUM(Revenue OVER Time)");
        assert_eq!(toks[0], (TokenKind::Function, "SUM"));
        assert_eq!(toks[1], (TokenKind::Operator, "("));
        assert_eq!(toks[2], (TokenKind::Ident, "Revenue"));
        assert_eq!(toks[3], (TokenKind::Function, "OVER"));
        assert_eq!(toks[4], (TokenKind::Ident, "Time"));
        assert_eq!(toks[5], (TokenKind::Operator, ")"));

        let abs = kinds("ABS(x)");
        assert_eq!(abs[0], (TokenKind::Function, "ABS"));
        assert_eq!(abs[1].0, TokenKind::Operator);
        assert_eq!(abs[2], (TokenKind::Ident, "x"));
    }

    #[test]
    fn string_and_number_and_date_literals() {
        let toks = kinds(r#"Name == "Widget A""#);
        assert_eq!(toks[0], (TokenKind::Ident, "Name"));
        assert_eq!(toks[1], (TokenKind::Operator, "=="));
        assert_eq!(toks[2], (TokenKind::String, "\"Widget A\""));
        // `""` inside: two adjacent string tokens, both drawn as text.
        let toks = kinds(r#""a""b" & 1"#);
        assert_eq!(
            toks[..2],
            [(TokenKind::String, "\"a\""), (TokenKind::String, "\"b\"")]
        );

        let toks = kinds("3.14 + 2");
        assert_eq!(toks[0], (TokenKind::Number, "3.14"));
        assert_eq!(toks[1], (TokenKind::Operator, "+"));
        assert_eq!(toks[2], (TokenKind::Number, "2"));

        let toks = kinds("1e-10 + 2.5E+3 * 6e2x");
        assert_eq!(toks[0], (TokenKind::Number, "1e-10"));
        assert_eq!(toks[2], (TokenKind::Number, "2.5E+3"));
        assert_eq!(toks[4], (TokenKind::Number, "6e2"));
        assert_eq!(toks[5], (TokenKind::Ident, "x"));
        // No digits after the `e`: the number ends before it.
        let toks = kinds("1e+");
        assert_eq!(toks[0], (TokenKind::Number, "1"));
        assert_eq!(toks[1], (TokenKind::Ident, "e"));

        let toks = kinds("Date > #2025-01-01#");
        assert_eq!(toks[0], (TokenKind::Ident, "Date"));
        assert_eq!(toks[1], (TokenKind::Operator, ">"));
        assert_eq!(toks[2], (TokenKind::Date, "#2025-01-01#"));
    }

    #[test]
    fn scan_covers_every_byte_with_no_gaps() {
        let text = "Price * Quantity - SUM(X OVER Y)";
        let toks = scan(text);
        let mut pos = 0;
        for t in &toks {
            assert_eq!(t.start, pos);
            pos = t.end;
        }
        assert_eq!(pos, text.len());
    }

    /// The load-bearing invariant: tokens tile `text` exactly (start at 0, each
    /// one begins where the last ended, the last ends at `text.len()`) and every
    /// boundary is a char boundary — `highlight_formula` slices by these ranges.
    fn assert_tiles(text: &str) {
        let toks = scan(text);
        let mut pos = 0;
        for t in &toks {
            assert_eq!(t.start, pos, "gap/overlap before {t:?} in {text:?}");
            assert!(t.end > t.start, "empty token {t:?} in {text:?}");
            assert!(
                text.is_char_boundary(t.start),
                "{:?}: start {} not a char boundary",
                text,
                t.start
            );
            assert!(
                text.is_char_boundary(t.end),
                "{:?}: end {} not a char boundary",
                text,
                t.end
            );
            pos = t.end;
        }
        assert_eq!(pos, text.len(), "tokens do not cover all of {text:?}");
    }

    #[test]
    fn unicode_input_never_panics_and_tokens_stay_on_char_boundaries() {
        // Each of these panicked before the byte-cast fix (the reproduced case
        // on EC2 was `scan("é")`).
        for text in [
            "é",
            "café",
            "年度売上",          // CJK identifier
            "年度 * 売上",       // CJK with ASCII operator
            "🚀",                // emoji (4-byte, non-alphabetic)
            "Price 🚀 Quantity", // emoji as an "operator"
            "Prîx * Qté",        // mixed ASCII + accented identifiers
            "SUM(Recéttes OVER Année)",
            "\u{0301}",   // lone combining mark
            "e\u{0301}x", // combining mark inside an identifier
            "ééé",
            "\"café\" == Nom", // non-ASCII inside a string literal
            "#2025-01-é1#",    // non-ASCII inside a date literal
            "3.14 ≤ é",        // multi-byte operator-ish char
            "é<=é",            // two-char op adjacent to multi-byte chars
            "é\"unterminated", // unterminated string after a multi-byte char
            "é#unterminated",
            " é", // non-ASCII whitespace (NBSP)
        ] {
            assert_tiles(text);
            // The renderer slices by the same ranges; must not panic either.
            let job = highlight_formula(text, FontId::default(), Some(1..2));
            assert_eq!(job.text, text, "rendered text differs for {text:?}");
        }
    }

    #[test]
    fn multibyte_identifier_is_one_token() {
        // "café" is 5 bytes / 4 chars: one Ident token spanning all of it.
        let toks = kinds("café");
        assert_eq!(toks, vec![(TokenKind::Ident, "café")]);
        // A non-alphanumeric multi-byte char is a single-CHAR operator token,
        // not a single-byte one.
        let toks = scan("🚀");
        assert_eq!(
            toks,
            vec![Token {
                kind: TokenKind::Operator,
                start: 0,
                end: 4
            }]
        );
    }

    #[test]
    fn error_span_inside_a_multibyte_char_does_not_panic() {
        // byte 1 is a continuation byte of "é": not a char boundary.
        let job = highlight_formula("é + 1", FontId::default(), Some(1..2));
        assert_eq!(job.text, "é + 1");
    }

    #[test]
    fn error_span_marks_the_token() {
        let text = "Price * Widgets";
        // "Widgets" is bytes 8..15.
        let job = highlight_formula(text, FontId::default(), Some(8..15));
        let bad = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == "Widgets")
            .expect("Widgets token present");
        assert_eq!(bad.format.color, ERROR_COLOR);
        assert_eq!(bad.format.underline, ERROR_UNDERLINE);
        // Other tokens are unaffected. (The palette has no error hue, so the
        // underline, not the color, is what marks the error.) egui merges
        // neighbours that look alike into one section, so look up by byte.
        let price = job
            .sections
            .iter()
            .find(|s| s.byte_range.start.0 == 0)
            .expect("a section at Price");
        assert!(job.text[price.byte_range.start.0..price.byte_range.end.0].starts_with("Price"));
        assert_eq!(price.format.underline, Stroke::NONE);
    }

    /// A span underlines exactly its bytes, across or inside tokens, and
    /// clamps an out-of-range or mid-char end.
    #[test]
    fn error_span_marks_exactly_its_bytes() {
        let marked = |text: &str, r| {
            let job = highlight_formula(text, FontId::default(), Some(r));
            assert_eq!(job.text, text);
            job.sections
                .iter()
                .filter(|s| s.format.underline == ERROR_UNDERLINE)
                .map(|s| &text[s.byte_range.start.0..s.byte_range.end.0])
                .collect::<String>()
        };
        assert_eq!(marked("Price * Widgets", 0..7), "Price *");
        assert_eq!(marked("Price * Widgets", 9..11), "id");
        assert_eq!(marked("Price", 3..99), "ce");
        assert_eq!(marked("é + 1", 0..1), "");
        assert_eq!(marked("é + 1", 0..2), "é");
        assert_eq!(marked("é + 1", 1..2), "é", "a mid-char start moves back");
    }

    #[test]
    fn no_error_span_leaves_all_tokens_unmarked() {
        let job = highlight_formula("Price * Quantity", FontId::default(), None);
        assert!(job
            .sections
            .iter()
            .all(|s| s.format.underline == Stroke::NONE));
    }

    #[test]
    fn empty_text_keeps_one_section_for_the_row_height() {
        let job = highlight_formula("", FontId::default(), Some(0..3));
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.underline, Stroke::NONE);
    }

    #[test]
    fn token_colors_are_grayscale() {
        let job = highlight_formula(
            r#"SUM(x) + 1 == "s" #2025-01-01#"#,
            FontId::default(),
            Some(0..3),
        );
        for s in &job.sections {
            let c = s.format.color;
            assert!(c == BLACK || c == DARK_GRAY, "{c:?} is not a palette gray");
        }
    }

    #[test]
    fn whitespace_runs_and_underscored_idents_are_single_tokens() {
        let toks: Vec<_> = scan("_a_b  \t x  ")
            .into_iter()
            .map(|t| (t.kind, t.start, t.end))
            .collect();
        assert_eq!(
            toks,
            vec![
                (TokenKind::Ident, 0, 4),
                (TokenKind::Whitespace, 4, 8),
                (TokenKind::Ident, 8, 9),
                (TokenKind::Whitespace, 9, 11),
            ]
        );
    }
}

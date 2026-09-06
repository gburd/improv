//! Pure syntax highlighting for the formula bar: `&str -> LayoutJob`.
//!
//! A small independent char-class scanner (identifiers / numbers / strings /
//! date literals / operators), good enough for visually useful highlighting —
//! it does not need to match `core_model::parser`'s exact grammar. Kept free
//! of any `egui::Ui` access so it's unit-testable without a window.

use egui::text::LayoutJob;
use egui::{Color32, FontId, Stroke, TextFormat};

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

/// Scan `text` into a flat sequence of [`Token`]s covering every byte (no
/// gaps), classifying identifiers/numbers/strings/date-literals/operators by
/// simple char class. Never fails — unrecognized bytes become single-byte
/// `Operator` tokens so the whole string is always covered.
pub fn scan(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;
        let kind = match c {
            c if c.is_whitespace() => {
                while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                    i += 1;
                }
                TokenKind::Whitespace
            }
            '"' => {
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
                TokenKind::Number
            }
            c if c.is_alphabetic() || c == '_' => {
                while i < bytes.len() && {
                    let b = bytes[i] as char;
                    b.is_alphanumeric() || b == '_'
                } {
                    i += 1;
                }
                let word = &text[start..i];
                if FUNCTIONS.iter().any(|f| word.eq_ignore_ascii_case(f)) {
                    TokenKind::Function
                } else {
                    TokenKind::Ident
                }
            }
            _ => {
                // Two-char operators first, then a single byte.
                let two = text.get(i..(i + 2).min(bytes.len()));
                if matches!(
                    two,
                    Some("<=") | Some(">=") | Some("<>") | Some("==") | Some("!=")
                ) {
                    i += 2;
                } else {
                    i += 1;
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

/// Token colors, chosen to sit within the NeXTSTEP palette (`theme.rs`): dark
/// text on the light `PAPER` field, a muted blue for functions (echoing
/// `NEXT_BLUE`), a dark-red for strings/dates, and plain near-black for
/// identifiers/operators.
fn color_for(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Function => crate::theme::NEXT_BLUE,
        TokenKind::Number => Color32::from_rgb(0x60, 0x30, 0x90),
        TokenKind::String | TokenKind::Date => Color32::from_rgb(0x90, 0x30, 0x20),
        TokenKind::Ident => Color32::from_gray(0x10),
        TokenKind::Operator => Color32::from_gray(0x40),
        TokenKind::Whitespace => Color32::from_gray(0x10),
    }
}

/// The color used to mark the byte offset of a formula parse error (a
/// red-underlined span), applied over whatever token color is at that offset.
pub const ERROR_COLOR: Color32 = Color32::from_rgb(0xC0, 0x30, 0x30);

/// Build a highlighted [`LayoutJob`] for `text`, coloring tokens per
/// [`scan`]/[`color_for`]. If `error_pos` is `Some(byte offset)` within the
/// text, the token at that offset (or, if it falls on a boundary/whitespace,
/// the nearest token) is rendered with a red underline and red text so the
/// parse error location is visible inline.
pub fn highlight_formula(text: &str, font: FontId, error_pos: Option<usize>) -> LayoutJob {
    let mut job = LayoutJob::default();
    let tokens = scan(text);
    // The token (if any) covering `error_pos`, preferring a non-whitespace
    // token; falls back to the last token starting at or before `error_pos`.
    let error_token = error_pos.and_then(|pos| {
        tokens
            .iter()
            .find(|t| t.start <= pos && pos < t.end && t.kind != TokenKind::Whitespace)
            .or_else(|| tokens.iter().rfind(|t| t.start <= pos))
    });
    for t in &tokens {
        let mut fmt = TextFormat {
            font_id: font.clone(),
            color: color_for(t.kind),
            ..Default::default()
        };
        if error_token == Some(t) {
            fmt.underline = Stroke::new(1.5_f32, ERROR_COLOR);
            fmt.color = ERROR_COLOR;
        }
        job.append(&text[t.start..t.end], 0.0, fmt);
    }
    job
}

#[cfg(test)]
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

        let toks = kinds("3.14 + 2");
        assert_eq!(toks[0], (TokenKind::Number, "3.14"));
        assert_eq!(toks[1], (TokenKind::Operator, "+"));
        assert_eq!(toks[2], (TokenKind::Number, "2"));

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

    #[test]
    fn error_position_marks_the_covering_token() {
        let text = "Price * Widgets";
        // "Widgets" starts at byte 9; mark a position inside it.
        let job = highlight_formula(text, FontId::default(), Some(10));
        let bad = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "Widgets")
            .expect("Widgets token present");
        assert_eq!(bad.format.color, ERROR_COLOR);
        assert!(bad.format.underline.width > 0.0);
        // Other tokens are unaffected.
        let price = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "Price")
            .expect("Price token present");
        assert_ne!(price.format.color, ERROR_COLOR);
    }

    #[test]
    fn no_error_position_leaves_all_tokens_unmarked() {
        let job = highlight_formula("Price * Quantity", FontId::default(), None);
        assert!(job.sections.iter().all(|s| s.format.color != ERROR_COLOR));
    }
}

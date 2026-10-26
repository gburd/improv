//! Textual formula parser: `"Revenue = Price * Quantity"` -> [`Formula`].
//!
//! Hand-rolled tokenizer + recursive-descent parser (same shape as
//! `improv_nl_formula`), for the *symbolic* v1 grammar in
//! `AGENT_FORMULA_LANGUAGE.md` §4. Measure/category names resolve against a
//! [`Model`] (like `NlContext`); unknown names are a clean [`ParseError`],
//! never a panic.
//!
//! # Grammar (`AGENT_FORMULA_LANGUAGE.md` §4)
//!
//! ```ebnf
//! Formula      = Identifier "=" Expression ;
//! Expression   = OrExpr ;
//! OrExpr       = AndExpr { "OR" AndExpr } ;
//! AndExpr      = NotExpr { "AND" NotExpr } ;
//! NotExpr      = "NOT" NotExpr | Comparison ;        (* NOT a > b == NOT (a > b) *)
//! Comparison   = Concat [ CmpOp Concat ] ;          (* == <> != < <= > >= *)
//! Concat       = Additive { "&" Additive } ;
//! Additive     = Term { ("+" | "-") Term } ;
//! Term         = Unary { ("*" | "/") Unary } ;
//! Unary        = "-" Unary | Power ;
//! Power        = Postfix [ "^" Unary ] ;            (* right-assoc; -2^2 = -4 *)
//! Postfix      = Primary { "%" } ;                  (* x% = x / 100 *)
//! Primary      = Literal | Call | Special | MeasureRef | Local
//!              | "(" Expression ")" ;
//! Literal      = Number | Text | "TRUE" | "FALSE" | DateLiteral ;  (* #2025-01-31# *)
//! Text         = '"' { Char | '""' } '"' ;          (* "" inside is one ", as in Excel *)
//! Number       = Digits [ "." Digits ] [ ( "e" | "E" ) [ "+" | "-" ] Digits ] ;
//!                                                   (* 42, 1.07, 1e-10, 6.02E23 *)
//! MeasureRef   = Name [ "[" Name { "," Name } "]" ] ;
//! Local        = Identifier ;                       (* bound by LET / LAMBDA *)
//!
//! Call         = FuncName "(" [ Args ] ")" ;           (* Sep: "," or ";" (de) *)
//! Args         = Arg { Sep Arg } { Sep Named } | Named { Sep Named } ;
//! Arg          = Expression [ OverClause ] [ IntoClause ] ;
//! Named        = Identifier "=>" Arg ;              (* a parameter by name: type => 1 *)
//! OverClause   = "OVER" ( Cats [ Window ] | "ALL" [ "EXCEPT" Cats ] )
//!                [ "WHERE" Expression ] ;
//! Window       = "[" Bound ".." Bound "]" ;          (* FIRST | LAST | THIS[+-n] *)
//! IntoClause   = "INTO" Cats ;                      (* a list result, row-major *)
//! Cats         = Name { "," Name } ;
//!
//! Special      = "LET" "(" { Identifier "," Expression "," } Expression ")"
//!              | Lambda "(" Expression { "," Expression } ")"   (* called on the spot *)
//!              | "MAP" "(" Expression { "," Expression } "," Lambda ")"
//!              | ("SCAN" | "REDUCE") "(" Expression "," MeasureRef "OVER" Name "," Lambda ")"
//!              | ("BYROW" | "BYCOL") "(" MeasureRef "OVER" Name "," Lambda ")"
//!              | "FIXPOINT" "(" Expression "," Lambda { "," ( Expression | Named ) } ")"
//!              | "MAKEARRAY" "(" Name "," Name "," Lambda ")"
//!              | "SEQUENCE" "(" Name { "," ( Expression | Named ) } ")"
//!              | NamedLambda "(" [ Expression { "," Expression } ] ")"
//!                (* an argument for a category parameter is a category Name *) ;
//! Lambda       = "LAMBDA" "(" Identifier { "," Identifier } "," Expression ")" ;
//!
//! Definition   = Formula
//!              | Identifier "=" "SQL" "(" Text ")"          (* SQL-sourced input *)
//!              | Identifier "=" "CALL" "(" Name { "," Name } ")"  (* external fn *)
//!              | Identifier "=" Lambda ;                   (* a named LAMBDA *)
//! ```
//!
//! ## Named arguments (`PMT(0.05, 12, 1000, type => 1)`)
//!
//! After the positional arguments, `name => value` sets a parameter by the
//! name [`crate::functions::FnDef::param_names`] gives it (case-insensitive).
//! Each name once, never one a positional argument already set, and no
//! positional argument after a named one. A function with a repeating
//! parameter (`SUM`, `LINEST`) takes none. A skipped optional parameter gets
//! its written default (`[type=0]`); one without a default cannot be
//! skipped. The parser puts each value in its parameter's place, so the
//! stored formula is the positional call and prints as one.
//!
//! ## Quoted identifiers (`'Unit Price'`)
//!
//! Anywhere a measure or category name may appear, it may instead be written
//! inside **single** quotes — the Quantrix spelling. This is the only way to
//! name a measure a CSV header produced (`import_csv` takes the header
//! verbatim, so `Unit Price`, `Cost/Unit` and `2024 Total` are all real
//! measure names), and the only way to name one that collides with a grammar
//! keyword (`'Over'`, `'SUM'`, `'NOT'`).
//!
//! * Double quotes are untouched: `"..."` is still a *text literal*, `'...'` is
//!   a *name*. They never overlap.
//! * A literal `'` inside a quoted name is written **doubled**: `'Bob''s Rate'`
//!   names the measure `Bob's Rate`. (Quantrix/SQL convention; chosen over
//!   backslashes so the quoted form needs no escape character at all.)
//! * A quoted name is never a keyword, a function, `TRUE`/`FALSE` or `OVER`: it
//!   resolves against the model as a measure/category name, full stop.
//! * `''` (empty) and an unterminated `'` are clean parse errors.
//!
//! ## Dotted qualification (`Matrix.Measure`) — parsed, not yet resolvable
//!
//! Quantrix qualifies a name by its owning matrix
//! (`'Defined Input & Outputs'.'Income tax rate (Corporate)'`). Improv has no
//! matrix concept yet: a measure belongs to the *model*, not to a matrix, so
//! there is nothing a qualifier could name. The tokenizer and parser therefore
//! accept the form and reject it with an error that names the unknown
//! qualifier, instead of the old `unexpected character: '.'`. When multiple
//! matrices per view land (GUI plan Step 3), `Parser::parse_name` is the one
//! place resolution has to learn about.
//!
//! ## The `=` ambiguity (assignment vs. equality)
//!
//! `=` is *only* the top-level assignment separator: [`parse_formula`] splits
//! `Identifier "=" ...` first, then parses the RHS as an [`Expr`]. Inside an
//! expression, equality comparison is spelled `==` and not-equal is `<>`
//! (or `!=`). This keeps `Revenue = Price * Quantity` unambiguous while still
//! allowing `flag = Price == 10`.
//!
//! ## Aggregation `DimensionSpec` convention
//!
//! `SUM(Revenue OVER Time)` -> `Call(FuncId(1), [Ref(revenue, DimensionSpec {
//! over: [Time], by: [], except: [] })])`. The compiler (`engine::compiler`)
//! reads the arg ref's `over` to pick the collapsed category, so only `over`
//! is set. Function ids come from `crate::functions` (SUM is 1).
//!
//! ## Not implemented (deferred, per §4/§11.3)
//!
//! * (Date literals `#2025-01-01#` and `#...T...Z#` ARE supported — see
//!   `parse_date_literal`.)
//! * Nothing else material: named scalar calls (`ABS`, `SQRT`, `MIN2`, …) and
//!   the whole-RHS source forms `CALL(...)` / `SQL("...")` (via
//!   `parse_definition`) are supported.

use crate::formula::{
    BinaryOp, DimensionSpec, Expr, Formula, FuncId, UnaryOp, MAX_FORMULA_DEPTH, MAX_FORMULA_NODES,
    MAX_STORED_FORMULA_DEPTH,
};
use crate::functions;
use crate::ids::{CategoryId, MeasureId, Name};
use crate::value::Value;
use crate::Model;

pub const FUNC_SUM: FuncId = FuncId(1);
pub const FUNC_AVG: FuncId = FuncId(2);
pub const FUNC_MIN: FuncId = FuncId(3);
pub const FUNC_MAX: FuncId = FuncId(4);

/// The built-in a name calls as `NAME(args...)` (aggregations excluded):
/// its id and parameter count, from [`crate::functions::all`]
/// (case-insensitive, aliases included).
pub fn scalar_func(name: &str) -> Option<(FuncId, usize)> {
    functions::callable(name).map(|d| (d.id, d.params.len()))
}

/// A formula parse failure. `position` is a byte offset into the source token
/// stream's originating text when known.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub position: Option<usize>,
}

impl ParseError {
    fn new(message: impl Into<String>, position: Option<usize>) -> Self {
        ParseError {
            message: message.into(),
            position,
        }
    }

    /// Whether this is [`MAX_FORMULA_DEPTH`] or [`MAX_FORMULA_NODES`]
    /// refusing a formula, not a syntax or name error.
    pub fn is_limit(&self) -> bool {
        self.message.starts_with(crate::formula::TOO_DEEP)
            || self.message.starts_with(crate::formula::TOO_LARGE)
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.position {
            Some(p) => write!(f, "parse error at {p}: {}", self.message),
            None => write!(f, "parse error: {}", self.message),
        }
    }
}

impl std::error::Error for ParseError {}

/// A parsed formula: the target measure name (LHS of `=`) plus its expression.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaText {
    pub target: Name,
    pub formula: Formula,
}

/// A parsed measure *definition* — either an ordinary formula or a source form
/// (`SQL("...")` / `CALL(fn, m1, m2, ...)`) that describes a host-side source
/// rather than a differential-dataflow expression. Source forms carry the
/// metadata a refresh consumes; they never enter the engine's expression graph,
/// preserving the deterministic core.
#[derive(Debug, Clone, PartialEq)]
pub enum Definition {
    /// `Target = <expr>` — an ordinary engine formula.
    Formula(FormulaText),
    /// `Target = SQL("<query>")` — a SQL-sourced input measure. The query is the
    /// raw string; column→dimension mapping is the caller's concern.
    Sql { target: Name, query: String },
    /// `Target = CALL(func, arg_measure, ...)` — an external-function measure.
    /// `func` is the registered function name; `args` are the argument measure
    /// names (resolved to ids by the caller against the model).
    Call {
        target: Name,
        func: String,
        args: Vec<Name>,
    },
    /// `Name = LAMBDA(p, …, body)` — a named LAMBDA (Excel's Name Manager),
    /// callable as `Name(args)` in formulas once added to the model.
    Lambda(LambdaText),
}

/// A parsed named-LAMBDA definition.
#[derive(Debug, Clone, PartialEq)]
pub struct LambdaText {
    pub name: Name,
    pub lambda: crate::NamedLambda,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    /// A single-quoted name (`'Unit Price'`), with `''` already un-doubled to a
    /// single `'`. Distinct from [`Tok::Ident`] because a quoted name is *never*
    /// a keyword or function: it can only be a measure/category name.
    QIdent(String),
    Number(f64),
    Str(String),
    /// A date/time literal `#YYYY-MM-DD#` or `#YYYY-MM-DDTHH:MM:SSZ#`, stored as
    /// a UTC timestamp.
    Date(chrono::DateTime<chrono::Utc>),
    /// A punctuation/operator lexeme (`+`, `<=`, `==`, `[`, ...).
    Op(String),
}

/// A token plus the byte offset where it began (for error positions).
#[derive(Debug, Clone)]
struct Spanned {
    tok: Tok,
    pos: usize,
}

/// Parse the inside of a `#...#` date literal. Accepts a bare date
/// `YYYY-MM-DD` (midnight UTC) or an RFC3339 timestamp `YYYY-MM-DDTHH:MM:SSZ`.
fn parse_date_literal(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
    }
    None
}

/// `n` as a number literal that parses back to the same `f64`: the
/// shortest round-trip digits, in exponent form (`1e-10`, `6.02e23`) below
/// 1e-6 or from 1e21 up, as JavaScript prints numbers. `None` for a
/// negative, infinite or NaN `n`, which no literal spells (`-5` is negation
/// applied to `5`).
pub fn number_literal(n: f64) -> Option<String> {
    if !n.is_finite() || n.is_sign_negative() {
        return None;
    }
    Some(if n != 0.0 && !(1e-6..1e21).contains(&n) {
        format!("{n:e}")
    } else {
        format!("{n}")
    })
}

fn tokenize(text: &str) -> Result<Vec<Spanned>, ParseError> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        // Decode the real character at this byte offset. `bytes[i] as char`
        // (the previous approach) casts a single BYTE, which is wrong for any
        // multi-byte UTF-8 sequence and can walk `i` to a non-boundary offset,
        // panicking on the eventual `text[..i]` slice. `text` is valid UTF-8
        // (it's a `&str`), so decoding at a boundary we maintain never fails.
        let c = text[i..]
            .chars()
            .next()
            .expect("i < len implies a char here");
        match c {
            c if c.is_whitespace() => i += c.len_utf8(),
            '"' => {
                // String literal: to the closing quote; `""` inside is one
                // `"` (Excel's escape, the only one).
                i += 1;
                let mut s = String::new();
                loop {
                    let s0 = i;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return Err(ParseError::new("unterminated string literal", Some(start)));
                    }
                    s.push_str(&text[s0..i]);
                    i += 1; // a quote: closing, or the first of `""`
                    if bytes.get(i) != Some(&b'"') {
                        break;
                    }
                    s.push('"');
                    i += 1;
                }
                out.push(Spanned {
                    tok: Tok::Str(s),
                    pos: start,
                });
            }
            '#' => {
                // Date literal: #YYYY-MM-DD# or #YYYY-MM-DDTHH:MM:SSZ#.
                i += 1;
                let s0 = i;
                while i < bytes.len() && bytes[i] != b'#' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(ParseError::new("unterminated date literal", Some(start)));
                }
                let raw = &text[s0..i];
                i += 1; // closing '#'
                let dt = parse_date_literal(raw).ok_or_else(|| {
                    ParseError::new(format!("invalid date: {raw:?}"), Some(start))
                })?;
                out.push(Spanned {
                    tok: Tok::Date(dt),
                    pos: start,
                });
            }
            c if c.is_ascii_digit() => {
                // Digits, then `.digits` (a `.` not followed by a digit ends
                // the number: the `..` of a window, `THIS-1..THIS`). A second
                // `.` stays in the number so `1.2.3` is an invalid number.
                while i < bytes.len()
                    && (bytes[i].is_ascii_digit()
                        || (bytes[i] == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)))
                {
                    i += 1;
                }
                // An exponent: `e` or `E`, an optional sign, digits. A name
                // never starts with a digit, so `2e3` is always 2000 and
                // `2 * e3` multiplies by the name `e3`.
                if matches!(bytes.get(i), Some(b'e' | b'E')) {
                    i += 1;
                    if matches!(bytes.get(i), Some(b'+' | b'-')) {
                        i += 1;
                    }
                    let digits = i;
                    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                    if i == digits {
                        return Err(ParseError::new(
                            format!("exponent without digits: {}", &text[start..i]),
                            Some(start),
                        ));
                    }
                }
                let raw = &text[start..i];
                let n = raw
                    .parse::<f64>()
                    .map_err(|_| ParseError::new(format!("invalid number: {raw}"), Some(start)))?;
                // `1e999` is a located error, not infinity: no cell holds
                // infinity. Underflow (`1e-999`) reads as 0, as in Excel.
                if n.is_infinite() {
                    return Err(ParseError::new(
                        format!("number out of range: {raw} (the largest is about 1.8e308)"),
                        Some(start),
                    ));
                }
                out.push(Spanned {
                    tok: Tok::Number(n),
                    pos: start,
                });
            }
            '\'' => {
                // Quoted identifier `'Unit Price'`; `''` is one literal `'`.
                // Advance by `len_utf8()` only (never by raw bytes) so a
                // multi-byte name cannot walk `i` off a char boundary.
                i += 1; // opening quote
                let mut name = String::new();
                let mut closed = false;
                while i < bytes.len() {
                    let c = text[i..].chars().next().expect("i < len");
                    i += c.len_utf8();
                    if c == '\'' {
                        // `i` is on a char boundary here, so this is safe.
                        if text[i..].starts_with('\'') {
                            name.push('\'');
                            i += 1;
                            continue;
                        }
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed {
                    return Err(ParseError::new("unterminated quoted name", Some(start)));
                }
                if name.is_empty() {
                    return Err(ParseError::new(
                        "empty quoted name: '' names nothing",
                        Some(start),
                    ));
                }
                out.push(Spanned {
                    tok: Tok::QIdent(name),
                    pos: start,
                });
            }
            c if c.is_alphabetic()
                || c == '_'
                || (c == '@' && text[i + 1..].starts_with(char::is_alphabetic)) =>
            {
                i += c.len_utf8();
                let word_end = |mut i: usize| {
                    while i < bytes.len() {
                        let b = text[i..].chars().next().expect("i < len");
                        if b.is_alphanumeric() || b == '_' {
                            i += b.len_utf8();
                        } else {
                            break;
                        }
                    }
                    i
                };
                i = word_end(i);
                // A dotted function name (`FLOOR.MATH(`, `NORM.S.DIST(`,
                // `T.DIST.2T(`): the longest run of `.part`s that names a
                // function and is followed by `(` is one identifier.
                let mut j = i;
                let mut best = None;
                // No function name has more than three dots (`T.DIST.2T`),
                // so a long `a.a.a…` costs O(n), not O(n²).
                let mut parts = 0;
                while parts < 3
                    && text[j..].starts_with('.')
                    && text[j + 1..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                {
                    parts += 1;
                    j = word_end(j + 1);
                    let rest = text[j..].trim_start();
                    if rest.starts_with('(') && functions::by_name(&text[start..j]).next().is_some()
                    {
                        best = Some(j);
                    }
                }
                if let Some(end) = best {
                    i = end;
                }
                out.push(Spanned {
                    tok: Tok::Ident(text[start..i].to_string()),
                    pos: start,
                });
            }
            // Two-char operators first, then single-char.
            _ => {
                let two = text.get(i..i + 2);
                let op = match two {
                    Some("<=") | Some(">=") | Some("<>") | Some("==") | Some("!=") | Some("=>") => {
                        i += 2;
                        two.unwrap().to_string()
                    }
                    _ => {
                        let single = "+-*/^=<>()[],.&%".find(c);
                        if single.is_none() {
                            return Err(ParseError::new(
                                format!("unexpected character: {c:?}"),
                                Some(start),
                            ));
                        }
                        i += c.len_utf8();
                        c.to_string()
                    }
                };
                out.push(Spanned {
                    tok: Tok::Op(op),
                    pos: start,
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// An aggregation's `OVER …` clause as parsed.
#[derive(Default)]
struct OverClause {
    cats: Vec<CategoryId>,
    /// `OVER ALL [EXCEPT …]`.
    all: bool,
    except: Vec<CategoryId>,
    /// `OVER T[from..to]`: bound literals (see `functions::WINDOW`).
    window: Option<(Expr, Expr)>,
    /// `INTO K`: where an array function's results land.
    into: Vec<CategoryId>,
    pred: Option<Expr>,
}

struct Parser<'a> {
    toks: &'a [Spanned],
    pos: usize,
    model: &'a Model,
    /// Current recursive-descent nesting depth (parens / unary chains).
    /// Bounded so adversarial input (e.g. thousands of nested `(`) errors
    /// cleanly instead of overflowing the stack.
    depth: usize,
    /// Names bound by `LET` and `LAMBDA`, innermost last; they shadow
    /// measure names and are replaced by what they are bound to.
    scope: Vec<(String, Expr)>,
    /// How many `REDUCE`/`SCAN` lambdas this formula has (each gets its own
    /// placeholder ids).
    lambdas: u32,
    /// Nodes copied in so far by `LET`/`LAMBDA` names, bounded by
    /// [`MAX_FORMULA_NODES`] so a binding chain that doubles fails fast.
    expanded: usize,
    /// Reading a formula a store kept as text: held to the load bounds
    /// ([`MAX_STORED_FORMULA_DEPTH`], [`MAX_STORED_PARSE_DEPTH`]) instead of
    /// a new formula's.
    stored: bool,
}

/// Maximum recursive-descent nesting depth for a single formula: nested
/// parentheses, calls and unary operators. Each level costs one frame per
/// grammar level (24.5 KB in a debug build, 5.7 KB in a release one,
/// measured 2026-10-25) on a stack that grows ([`crate::grow_stack`]). Past
/// [`MAX_FORMULA_DEPTH`] a tree is refused anyway; the extra levels are for
/// redundant parentheses.
const MAX_PARSE_DEPTH: usize = MAX_FORMULA_DEPTH + 64;

/// [`MAX_PARSE_DEPTH`] for a formula a store kept as text: the same, now
/// that the two depth bounds are one (0.11 loaded to 100, past its new 64).
const MAX_STORED_PARSE_DEPTH: usize = MAX_PARSE_DEPTH;

/// A named argument as read: the name, where it starts, and its value.
type Named = (String, Option<usize>, Expr);

const POSITIONAL_AFTER_NAMED: &str =
    "a positional argument cannot follow a named one (name => value)";

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    fn peek_pos(&self) -> Option<usize> {
        self.toks.get(self.pos).map(|s| s.pos).or_else(|| {
            // Point past the last token at EOF.
            self.toks.last().map(|s| s.pos)
        })
    }

    fn bump(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.pos).map(|s| &s.tok);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// Consume a specific operator lexeme if present.
    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if o == op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Case-insensitive keyword match on a *bare* identifier token (without
    /// consuming). A quoted name ([`Tok::QIdent`]) is never a keyword, so
    /// `'Over'` stays a measure name.
    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(kw))
    }

    /// Consume one identifier — bare (`Price`) or quoted (`'Unit Price'`) — and
    /// return its text. `what` names the expectation for the error message.
    fn take_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek() {
            Some(Tok::Ident(w) | Tok::QIdent(w)) => {
                let w = w.clone();
                self.pos += 1;
                Ok(w)
            }
            _ => Err(self.err(format!("expected {what}"))),
        }
    }

    /// `Name = [ Identifier "." ] Identifier` — a measure/category name, bare or
    /// quoted, optionally qualified (`Matrix.Measure`). Returns the name plus
    /// the byte offset it started at, so a failed lookup points at the *name*
    /// rather than at whatever follows it.
    ///
    /// Qualification *parses* but never resolves: Improv has no namespace a
    /// qualifier could name (a measure belongs to the model, not to a matrix),
    /// so a qualified name is a clear error naming the unknown qualifier rather
    /// than a stray-character complaint. Exactly one dot is consumed, so no
    /// input can make this allocate or recurse without bound.
    fn parse_name(&mut self, what: &str) -> Result<(String, Option<usize>), ParseError> {
        let pos = self.peek_pos();
        let first = self.take_ident(what)?;
        if !self.eat_op(".") {
            return Ok((first, pos));
        }
        let leaf = self.take_ident(what)?;
        Err(ParseError::new(
            format!(
                "unknown qualifier: {first} (no such matrix; `{first}.{leaf}` cannot be \
                 resolved — write the name alone)"
            ),
            pos,
        ))
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError::new(msg, self.peek_pos())
    }

    // --- resolution ---

    /// Resolve a measure name consumed by [`Self::parse_name`], reporting the
    /// failure at the name's own offset (not at whatever follows it).
    fn measure_id(&self, name: &str, pos: Option<usize>) -> Result<MeasureId, ParseError> {
        self.model
            .measure_by_name(name)
            .map(|m| m.id)
            .ok_or_else(|| ParseError::new(format!("unknown measure: {name}"), pos))
    }

    fn category_id(&self, name: &str, pos: Option<usize>) -> Result<CategoryId, ParseError> {
        // A named LAMBDA's parameter where a category goes (`OVER c`).
        if let Some(Expr::Ref(id, _)) = self.local(name) {
            if let Some(i) = functions::lambda_param_of(id) {
                return Ok(functions::lambda_category(i));
            }
        }
        self.model
            .category_by_name(name)
            .map(|c| c.id)
            .ok_or_else(|| ParseError::new(format!("unknown category: {name}"), pos))
    }

    // --- grammar ---

    /// The deepest tree this parse accepts.
    fn max_depth(&self) -> usize {
        if self.stored {
            MAX_STORED_FORMULA_DEPTH
        } else {
            MAX_FORMULA_DEPTH
        }
    }

    /// The deepest recursion this parse allows.
    fn max_nest(&self) -> usize {
        if self.stored {
            MAX_STORED_PARSE_DEPTH
        } else {
            MAX_PARSE_DEPTH
        }
    }

    /// Refuse a tree past [`Parser::max_depth`] or [`MAX_FORMULA_NODES`];
    /// `pos` is where it starts.
    fn check(&self, expr: &Expr, pos: Option<usize>) -> Result<(), ParseError> {
        expr.check_depth(self.max_depth())
            .map_err(|m| ParseError::new(m, pos))
    }

    /// [`Parser::check`] a whole formula. A named-LAMBDA call is kept as a
    /// call and expanded when compiled, so a new formula's expansion is
    /// held to the same limit.
    fn check_formula(&self, expr: &Expr, pos: Option<usize>) -> Result<(), ParseError> {
        self.check(expr, pos)?;
        if self.stored || self.model.lambdas.is_empty() {
            return Ok(());
        }
        match self.model.expand_lambdas(expr) {
            Ok(full) => self.check(&full, pos),
            Err(m) => {
                // Expansion stops at the load bound, naming the lambda
                // (`F: formula nests too deeply …`): report the limit.
                let limit = [crate::formula::TOO_DEEP, crate::formula::TOO_LARGE]
                    .iter()
                    .find_map(|t| m.find(t));
                match limit {
                    Some(i) => Err(ParseError::new(&m[i..], pos)),
                    // Anything else the compiler reports, with its own context.
                    None => Ok(()),
                }
            }
        }
    }

    /// `lhs op rhs`: one more link of an operator chain, at `at`. A chain
    /// grows its tree without recursing, so its depth is checked as it grows
    /// (`depth` is lhs's, 0 before the first link): a long chain stops at the
    /// operator that passes [`MAX_FORMULA_DEPTH`], before it builds a tree too
    /// deep even to drop.
    fn link(
        &self,
        at: Option<usize>,
        depth: &mut usize,
        op: BinaryOp,
        lhs: Expr,
        rhs: Expr,
    ) -> Result<Expr, ParseError> {
        if *depth == 0 {
            *depth = lhs.depth_and_nodes().0;
        }
        *depth = 1 + (*depth).max(rhs.depth_and_nodes().0);
        if *depth > self.max_depth() {
            return Err(ParseError::new(
                crate::formula::too_deep(*depth, self.max_depth()),
                at,
            ));
        }
        Ok(Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs)))
    }

    /// Expression = OrExpr
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    /// OrExpr = AndExpr { "OR" AndExpr }
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let (mut lhs, mut depth) = (self.parse_and()?, 0);
        while self.peek_kw("or") {
            let at = self.peek_pos();
            self.bump();
            let rhs = self.parse_and()?;
            lhs = self.link(at, &mut depth, BinaryOp::Or, lhs, rhs)?;
        }
        Ok(lhs)
    }

    /// AndExpr = NotExpr { "AND" NotExpr }
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let (mut lhs, mut depth) = (self.parse_not()?, 0);
        while self.peek_kw("and") {
            let at = self.peek_pos();
            self.bump();
            let rhs = self.parse_not()?;
            lhs = self.link(at, &mut depth, BinaryOp::And, lhs, rhs)?;
        }
        Ok(lhs)
    }

    /// NotExpr = { "NOT" } Comparison
    ///
    /// NOT binds looser than a comparison (as in SQL and spreadsheets), so
    /// `NOT Price > 15` is `NOT (Price > 15)`. A chain of NOTs is read in a
    /// loop (no parser stack), but each NOT still counts toward
    /// `MAX_PARSE_DEPTH`: the compiler and printers walk the tree recursively.
    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        let mut nots = 0usize;
        while self.peek_kw("not") {
            if self.depth + nots >= self.max_nest() {
                return Err(self.err("expression nested too deeply"));
            }
            self.bump();
            nots += 1;
        }
        self.depth += nots;
        let e = self.parse_comparison();
        self.depth -= nots;
        let mut e = e?;
        for _ in 0..nots {
            e = Expr::UnaryOp(UnaryOp::Not, Box::new(e));
        }
        Ok(e)
    }

    /// Comparison = Additive [ cmp-op Additive ]  (non-associative)
    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_concat()?;
        let op = match self.peek() {
            Some(Tok::Op(o)) => match o.as_str() {
                "==" => Some(BinaryOp::Eq),
                "<>" | "!=" => Some(BinaryOp::Ne),
                "<" => Some(BinaryOp::Lt),
                "<=" => Some(BinaryOp::Le),
                ">" => Some(BinaryOp::Gt),
                ">=" => Some(BinaryOp::Ge),
                _ => None,
            },
            _ => None,
        };
        match op {
            Some(op) => {
                self.bump();
                let rhs = self.parse_concat()?;
                Ok(Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs)))
            }
            None => Ok(lhs),
        }
    }

    /// Concat = Additive { "&" Additive }
    fn parse_concat(&mut self) -> Result<Expr, ParseError> {
        let (mut lhs, mut depth) = (self.parse_additive()?, 0);
        loop {
            let at = self.peek_pos();
            if !self.eat_op("&") {
                break;
            }
            let rhs = self.parse_additive()?;
            lhs = self.link(at, &mut depth, BinaryOp::Concat, lhs, rhs)?;
        }
        Ok(lhs)
    }

    /// Additive = Term { ("+"|"-") Term }
    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let (mut lhs, mut depth) = (self.parse_term()?, 0);
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" => BinaryOp::Add,
                Some(Tok::Op(o)) if o == "-" => BinaryOp::Sub,
                _ => break,
            };
            let at = self.peek_pos();
            self.bump();
            let rhs = self.parse_term()?;
            lhs = self.link(at, &mut depth, op, lhs, rhs)?;
        }
        Ok(lhs)
    }

    /// Term = Unary { ("*"|"/") Unary }
    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let (mut lhs, mut depth) = (self.parse_unary()?, 0);
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "*" => BinaryOp::Mul,
                Some(Tok::Op(o)) if o == "/" => BinaryOp::Div,
                _ => break,
            };
            let at = self.peek_pos();
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = self.link(at, &mut depth, op, lhs, rhs)?;
        }
        Ok(lhs)
    }

    /// Unary = "-" Unary | Power. A chain of minuses is read in a loop (each
    /// counts toward `MAX_PARSE_DEPTH`, as the compiler recurses over it).
    /// Unary minus binds looser than `^`: `-2^2` is `-(2^2)`.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let mut negs = 0usize;
        while matches!(self.peek(), Some(Tok::Op(o)) if o == "-") {
            if self.depth + negs >= self.max_nest() {
                return Err(self.err("expression nested too deeply"));
            }
            self.bump();
            negs += 1;
        }
        self.depth += negs;
        let e = self.parse_power();
        self.depth -= negs;
        let mut e = e?;
        for _ in 0..negs {
            e = Expr::UnaryOp(UnaryOp::Neg, Box::new(e));
        }
        Ok(e)
    }

    /// Power = Postfix [ "^" Unary ]: right-associative (`2^3^2` is `2^9`),
    /// and the exponent may be negative (`2^-1`).
    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_postfix()?;
        if !self.eat_op("^") {
            return Ok(base);
        }
        self.depth += 1;
        if self.depth > self.max_nest() {
            self.depth -= 1;
            return Err(self.err("expression nested too deeply"));
        }
        let exp = crate::grow_stack(|| self.parse_unary());
        self.depth -= 1;
        Ok(Expr::BinaryOp(
            BinaryOp::Pow,
            Box::new(base),
            Box::new(exp?),
        ))
    }

    /// Postfix = Primary { "%" }: `x%` is `x / 100` (`50%` folds to `0.5`).
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let (mut e, mut depth) = (self.parse_primary()?, 0);
        loop {
            let at = self.peek_pos();
            if !self.eat_op("%") {
                break;
            }
            e = match e {
                Expr::Literal(Value::Number(n)) => Expr::Literal(Value::Number(n / 100.0)),
                other => self.link(
                    at,
                    &mut depth,
                    BinaryOp::Div,
                    other,
                    Expr::Literal(Value::Number(100.0)),
                )?,
            };
        }
        Ok(e)
    }

    /// Primary = Literal | Call | MeasureRef | "(" Expression ")"
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_nest() {
            self.depth -= 1;
            return Err(self.err("expression nested too deeply"));
        }
        let result = crate::grow_stack(|| self.parse_primary_inner());
        self.depth -= 1;
        result
    }

    fn parse_primary_inner(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(Tok::Op(o)) if o == "(" => {
                self.bump();
                let e = self.parse_expr()?;
                if !self.eat_op(")") {
                    return Err(self.err("expected ')'"));
                }
                Ok(e)
            }
            Some(Tok::Number(n)) => {
                let n = *n;
                self.bump();
                Ok(Expr::Literal(Value::Number(n)))
            }
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.bump();
                Ok(Expr::Literal(Value::Text(s)))
            }
            Some(Tok::Date(dt)) => {
                let dt = *dt;
                self.bump();
                Ok(Expr::Literal(Value::DateTime(dt)))
            }
            Some(Tok::Ident(_)) => self.parse_ident_primary(),
            // A quoted name is only ever a measure reference.
            Some(Tok::QIdent(_)) => self.parse_measure_ref(),
            Some(Tok::Op(o)) => Err(self.err(format!("unexpected operator: {o}"))),
        }
    }

    /// Identifier-headed primary: TRUE/FALSE, aggregation, or a measure ref.
    fn parse_ident_primary(&mut self) -> Result<Expr, ParseError> {
        // Peek the identifier text without holding the borrow.
        let word = match self.peek() {
            Some(Tok::Ident(w)) => w.clone(),
            _ => return Err(self.err("expected identifier")),
        };
        // `name =>` that no call of a built-in took (parse_call and the
        // special forms consume their own).
        if matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "=>") {
            return Err(self.err(format!(
                "unexpected {word} =>: a named argument goes in a call of a built-in \
                 function with a fixed parameter list, after its positional arguments"
            )));
        }
        // `TRUE()` / `FALSE()` are the functions; bare `TRUE` the literal.
        let is_call =
            matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "(");
        // A name bound by LET or LAMBDA.
        if !is_call {
            if let Some(e) = self.local(&word) {
                self.expanded += e.depth_and_nodes().1;
                if self.expanded > MAX_FORMULA_NODES {
                    return Err(self.err(crate::formula::too_large()));
                }
                self.bump();
                return Ok(e);
            }
        }
        if is_call && functions::callable(&word).is_none() {
            if let Some((id, l)) = self.model.lambda_by_name(&word) {
                let kinds = l.param_kinds().map_err(|e| self.err(e))?;
                return self.parse_named_lambda_call(id, &kinds, &word);
            }
        }
        if is_call {
            match word.to_ascii_uppercase().as_str() {
                "LET" => return self.parse_let(),
                "LAMBDA" => return self.parse_lambda_call(),
                "MAP" => return self.parse_map(),
                "SEQUENCE" => return self.parse_sequence(),
                "MAKEARRAY" => return self.parse_makearray(),
                "BYROW" | "BYCOL" => return self.parse_byrow(&word),
                "REDUCE" | "SCAN" => return self.parse_fold(&word),
                "FIXPOINT" => return self.parse_fixpoint(),
                _ => {}
            }
        }
        if is_call && (word.eq_ignore_ascii_case("true") || word.eq_ignore_ascii_case("false")) {
            return self.parse_call(&word);
        }

        if word.eq_ignore_ascii_case("true") {
            self.bump();
            return Ok(Expr::Literal(Value::Boolean(true)));
        }
        if word.eq_ignore_ascii_case("false") {
            self.bump();
            return Ok(Expr::Literal(Value::Boolean(false)));
        }
        // A function name only calls when directly followed by "(";
        // otherwise the word is an ordinary measure name.
        if is_call
            && (functions::aggregation(&word).is_some() || functions::callable(&word).is_some())
        {
            return self.parse_call(&word);
        }
        // `NAME(` that is neither a function nor a measure: say so, and name
        // the closest functions (a typo like PMTT).
        if is_call && self.model.measure_by_name(&word).is_none() {
            let pos = self.toks.get(self.pos).map(|s| s.pos);
            let near = functions::closest(&word, 3);
            let hint = if near.is_empty() {
                String::new()
            } else {
                format!("; did you mean {}?", near.join(", "))
            };
            return Err(ParseError::new(
                format!("unknown function: {word}{hint}"),
                pos,
            ));
        }
        self.parse_measure_ref()
    }

    /// `NAME(args…)`: an aggregation `NAME(MeasureRef OVER Category)` when
    /// the name has one and `OVER` follows the first argument, otherwise a
    /// call of the non-aggregating function the name means. Arity is checked
    /// against the registry so a bad call is a clear parse error.
    fn parse_call(&mut self, name: &str) -> Result<Expr, ParseError> {
        self.bump(); // function name
        if !self.eat_op("(") {
            return Err(self.err("expected '(' after function name"));
        }
        // The callable's parameter kinds, to read category arguments
        // (`POSITION(Time)`) as categories rather than measures.
        let callable = functions::callable(name);
        let mut args = Vec::new();
        let mut named: Vec<Named> = Vec::new();
        let mut over: Option<OverClause> = None;
        let mut into: Vec<CategoryId> = Vec::new();
        if !matches!(self.toks.get(self.pos).map(|s| &s.tok), Some(Tok::Op(o)) if o == ")") {
            loop {
                let label = self.named_arg()?;
                if label.is_none() && !named.is_empty() {
                    return Err(self.err(POSITIONAL_AFTER_NAMED));
                }
                let i = match &label {
                    Some((n, _)) => callable.and_then(|d| d.param_index(n)),
                    None => Some(args.len()),
                };
                let is_cat = over.is_none()
                    && callable
                        .zip(i)
                        .is_some_and(|(d, i)| d.param(i) == functions::Arg::Category)
                    && functions::aggregation(name).is_none();
                let arg = if is_cat {
                    Expr::Category(self.parse_category_name()?)
                } else {
                    self.parse_expr()?
                };
                match label {
                    Some((n, at)) => named.push((n, at, arg)),
                    None => args.push(arg),
                }
                if self.peek_kw("over") {
                    if over.is_some() {
                        return Err(self.err("only one OVER clause per aggregation"));
                    }
                    over = Some(self.parse_over_clause()?);
                }
                if self.peek_kw("into") {
                    self.bump();
                    if !into.is_empty() {
                        return Err(self.err("only one INTO clause per call"));
                    }
                    // `INTO R, C`: a list laid over several categories.
                    into.push(self.parse_category_name()?);
                    while self.comma_then_category() {
                        self.bump(); // ,
                        into.push(self.parse_category_name()?);
                    }
                }
                if self.eat_op(",") {
                    continue;
                }
                break;
            }
        }
        if !self.eat_op(")") {
            return Err(self.err("expected ')' to close function call"));
        }
        // A BYROW parameter (`SUM(row)`) carries its row's category: the
        // call reduces over it.
        if over.is_none() && functions::aggregation(name).is_some() {
            over = args.iter().find_map(|a| match a {
                Expr::Ref(_, s) if !s.over.is_empty() => Some(OverClause {
                    cats: s.over.clone(),
                    ..OverClause::default()
                }),
                _ => None,
            });
        }
        if over.is_some() || !into.is_empty() {
            let mut clause = over.unwrap_or_default();
            clause.into = into;
            return self.finish_aggregation(name, args, named, clause);
        }
        let def = callable.ok_or_else(|| {
            self.err(format!(
                "{name} aggregates: write {name}(Measure OVER Category)"
            ))
        })?;
        let args = Self::place_named(def, args, named)?;
        if !def.accepts(args.len()) {
            return Err(self.err(format!(
                "{} takes {}, got {} argument(s)",
                def.name,
                def.hint(),
                args.len()
            )));
        }
        Ok(Expr::Call(def.id, args))
    }

    /// `OVER Cat {, Cat} [ "[" Bound ".." Bound "]" ] [WHERE pred]` or
    /// `OVER ALL [EXCEPT Cat {, Cat}] [WHERE pred]`. A `,` continues the
    /// category list only when a category name follows and is not itself
    /// followed by `(`; otherwise it starts the next argument.
    fn parse_over_clause(&mut self) -> Result<OverClause, ParseError> {
        self.bump(); // OVER
        let mut clause = OverClause::default();
        if self.peek_kw("all") && !self.next_is_open_paren() {
            self.bump();
            clause.all = true;
            if self.peek_kw("except") {
                self.bump();
                clause.except.push(self.parse_category_name()?);
                while self.comma_then_category() {
                    self.bump(); // ,
                    clause.except.push(self.parse_category_name()?);
                }
            }
        } else {
            clause.cats.push(self.parse_category_name()?);
            if self.eat_op("[") {
                let from = self.parse_bound()?;
                if !self.eat_op(".") || !self.eat_op(".") {
                    return Err(self.err("expected '..' in a window: OVER T[FIRST..THIS]"));
                }
                let to = self.parse_bound()?;
                if !self.eat_op("]") {
                    return Err(self.err("expected ']' to close the window"));
                }
                clause.window = Some((from, to));
            } else {
                while self.comma_then_category() {
                    self.bump(); // ,
                    clause.cats.push(self.parse_category_name()?);
                }
            }
        }
        if self.peek_kw("where") {
            self.bump();
            clause.pred = Some(self.parse_expr()?);
        }
        Ok(clause)
    }

    /// A window bound: `FIRST`, `LAST`, `THIS`, `THIS-n`, `THIS+n`, as the
    /// literal the `WINDOW` marker stores (text, or an offset number).
    fn parse_bound(&mut self) -> Result<Expr, ParseError> {
        let word = match self.peek() {
            Some(Tok::Ident(w)) => w.to_ascii_uppercase(),
            _ => return Err(self.err("expected FIRST, LAST or THIS in a window")),
        };
        self.bump();
        let lit = |v| Ok(Expr::Literal(v));
        match word.as_str() {
            "FIRST" | "LAST" => lit(Value::Text(word)),
            "THIS" => {
                let sign = match self.peek() {
                    Some(Tok::Op(o)) if o == "+" => 1.0,
                    Some(Tok::Op(o)) if o == "-" => -1.0,
                    _ => return lit(Value::Number(0.0)),
                };
                self.bump();
                match self.peek() {
                    Some(Tok::Number(n)) if n.fract() == 0.0 => {
                        let n = *n;
                        self.bump();
                        lit(Value::Number(sign * n))
                    }
                    _ => Err(self.err("expected a whole number after THIS+ / THIS-")),
                }
            }
            _ => Err(self.err("expected FIRST, LAST or THIS in a window")),
        }
    }

    fn next_is_open_paren(&self) -> bool {
        matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "(")
    }

    /// Whether the next tokens are `, Category` (and not `, Name(` or a
    /// measure): the OVER category list continues.
    fn comma_then_category(&self) -> bool {
        let at = |k: usize| self.toks.get(self.pos + k).map(|s| &s.tok);
        let name = match (at(0), at(1)) {
            (Some(Tok::Op(c)), Some(Tok::Ident(n) | Tok::QIdent(n))) if c == "," => n,
            _ => return false,
        };
        let followed_ok = !matches!(at(2), Some(Tok::Op(o)) if o == "(" || o == ".");
        followed_ok
            && self.model.category_by_name(name).is_some()
            && self.model.measure_by_name(name).is_none()
    }

    /// Build `AGG(args…)` once its OVER clause is read: every argument the
    /// registry marks as reduced must be a measure reference and gets the
    /// OVER categories; a `WHERE` and a window become trailing markers.
    fn finish_aggregation(
        &mut self,
        name: &str,
        args: Vec<Expr>,
        named: Vec<Named>,
        clause: OverClause,
    ) -> Result<Expr, ParseError> {
        let def = functions::aggregation(name)
            .ok_or_else(|| self.err(format!("{name} does not aggregate OVER a category")))?;
        let mut args = Self::place_named(def, args, named)?;
        if !def.accepts(args.len()) {
            return Err(self.err(format!(
                "{} takes {}, got {} argument(s)",
                def.name,
                def.hint(),
                args.len()
            )));
        }
        if clause.window.is_some() && def.kind == functions::Kind::PerItem {
            return Err(self.err(format!("{} cannot take a window", def.name)));
        }
        // Over::Leading: every leading measure reference is reduced.
        let leading = match def.over {
            functions::Over::Leading(min) => {
                let n = args
                    .iter()
                    .take_while(|a| matches!(a, Expr::Ref(..)))
                    .count();
                if n < min {
                    return Err(self.err(format!(
                        "{} needs at least {min} measure(s) before its options",
                        def.name
                    )));
                }
                Some(n)
            }
            _ => None,
        };
        for (i, a) in args.iter_mut().enumerate() {
            if !leading.map_or_else(|| def.over.contains(i), |n| i < n) {
                continue;
            }
            let Expr::Ref(id, spec) = a else {
                return Err(self.err(format!(
                    "{} argument {} must be a measure reference (it is reduced OVER the \
                     categories)",
                    def.name,
                    i + 1
                )));
            };
            let mut s = spec.clone();
            if clause.all {
                // OVER ALL [EXCEPT …]: every category of the measure but the
                // excepted ones (resolved now, from the measure).
                let m = self.model.measures.get(id);
                s.over = m
                    .map(|m| {
                        m.categories
                            .iter()
                            .copied()
                            .filter(|c| !clause.except.contains(c))
                            .collect()
                    })
                    .unwrap_or_default();
            } else {
                s.over = clause.cats.clone();
            }
            *a = Expr::Ref(*id, s);
        }
        if let Some(p) = clause.pred {
            args.push(Expr::Call(functions::WHERE, vec![p]));
        }
        if let Some((from, to)) = clause.window {
            args.push(Expr::Call(functions::WINDOW, vec![from, to]));
        }
        if !clause.into.is_empty() {
            args.push(Expr::Call(
                functions::INTO,
                clause.into.into_iter().map(Expr::Category).collect(),
            ));
        }
        Ok(Expr::Call(def.id, args))
    }

    /// `name =>` ahead, the start of a named argument: consume it and
    /// return the name and where it starts. A positional argument after a
    /// named one is an error, so the caller checks only for `None`.
    fn named_arg(&mut self) -> Result<Option<(String, Option<usize>)>, ParseError> {
        let at = |k: usize| self.toks.get(self.pos + k).map(|s| &s.tok);
        let name = match (at(0), at(1)) {
            (Some(Tok::Ident(n)), Some(Tok::Op(o))) if o == "=>" => n.clone(),
            _ => return Ok(None),
        };
        let pos = self.peek_pos();
        self.pos += 2;
        Ok(Some((name, pos)))
    }

    /// The arguments after a special form's leading ones (`args`, read
    /// already): `, expr` positional ones, then `, name => expr` named
    /// ones, at most `max` in all, placed by [`Self::place_named`]. Stops
    /// before the `)`.
    fn rest_args(
        &mut self,
        def: &functions::FnDef,
        mut args: Vec<Expr>,
        max: usize,
    ) -> Result<Vec<Expr>, ParseError> {
        let mut named: Vec<Named> = Vec::new();
        while args.len() + named.len() < max && self.eat_op(",") {
            match self.named_arg()? {
                Some((n, at)) => named.push((n, at, self.parse_expr()?)),
                None if !named.is_empty() => return Err(self.err(POSITIONAL_AFTER_NAMED)),
                None => args.push(self.parse_expr()?),
            }
        }
        Self::place_named(def, args, named)
    }

    /// `def`'s positional `args` with its `named` ones (`name => expr`) put
    /// in their parameters' places. A name is one of `def`'s parameters
    /// (case-insensitive), given once and not also by position; a function
    /// with a repeating parameter takes none. An optional parameter a named
    /// one skips gets its default (`[match_mode=0]`); a required one, or one
    /// with no default, cannot be skipped.
    fn place_named(
        def: &functions::FnDef,
        args: Vec<Expr>,
        named: Vec<Named>,
    ) -> Result<Vec<Expr>, ParseError> {
        let Some(first) = named.first().map(|n| n.1) else {
            return Ok(args);
        };
        let f = def.name;
        if def.variadic {
            return Err(ParseError::new(
                format!("{f} takes its arguments by position (a parameter repeats), not by name"),
                first,
            ));
        }
        let n = args.len();
        let mut slots: Vec<Option<Expr>> = args.into_iter().map(Some).collect();
        for (name, at, e) in named {
            let Some(i) = def.param_index(&name) else {
                let near = def.closest_param(&name);
                let hint = if near.is_empty() {
                    let all: Vec<String> = (0..def.params.len())
                        .filter_map(|i| def.param_name(i))
                        .collect();
                    format!("its parameters are {}", all.join(", "))
                } else {
                    format!("did you mean {}?", near.join(", "))
                };
                return Err(ParseError::new(
                    format!("{f} has no parameter {name}; {hint}"),
                    at,
                ));
            };
            if slots.len() <= i {
                slots.resize(i + 1, None);
            }
            if slots[i].is_some() {
                let how = if i < n {
                    format!("already given, as argument {}", i + 1)
                } else {
                    "named twice".to_string()
                };
                return Err(ParseError::new(format!("{f}: {name} is {how}"), at));
            }
            slots[i] = Some(e);
        }
        slots
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let name = def.param_name(i).unwrap_or_default();
                s.or_else(|| def.param_default(i)).ok_or_else(|| {
                    let why = if i < def.required {
                        "is required"
                    } else {
                        "has no default"
                    };
                    ParseError::new(
                        format!(
                            "{f}: {name} (argument {}) {why}, so a later named argument \
                             cannot skip it: give it, by position or as {name} => …",
                            i + 1
                        ),
                        first,
                    )
                })
            })
            .collect()
    }

    /// What a `LET`/`LAMBDA` name is bound to, innermost binding first.
    fn local(&self, name: &str) -> Option<Expr> {
        self.scope
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, e)| e.clone())
    }

    /// A bare name for LET/LAMBDA.
    fn binding_name(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(Tok::Ident(w)) => {
                let w = w.clone();
                self.bump();
                Ok(w)
            }
            _ => Err(self.err("expected a name to bind")),
        }
    }

    fn expect(&mut self, op: &str, what: &str) -> Result<(), ParseError> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(self.err(format!("expected {what}")))
        }
    }

    /// `LET(name, value, [name, value, …], body)`: the body with each name
    /// replaced by its value (a later value may use an earlier name). LET
    /// is expanded here; the stored formula is the expanded expression.
    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // LET
        self.expect("(", "'(' after LET")?;
        let base = self.scope.len();
        loop {
            // `name ,` starts another binding; anything else is the body.
            let is_binding = matches!(self.peek(), Some(Tok::Ident(_)))
                && matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == ",")
                && self.scope.len() - base < 126;
            if !is_binding && self.scope.len() > base {
                break;
            }
            let name = self.binding_name()?;
            self.expect(",", "',' after a LET name")?;
            let at = self.peek_pos();
            let value = self.parse_expr()?;
            // Checked here too, so bindings cannot compound into a tree too
            // deep to drop before the whole formula is checked.
            self.check(&value, at)?;
            self.scope.push((name, value));
            self.expect(",", "',' then the LET body")?;
        }
        let body = self.parse_expr();
        self.scope.truncate(base);
        let body = body?;
        self.expect(")", "')' to close LET")?;
        Ok(body)
    }

    /// `LAMBDA(p1, …, body)` as written: its parameter names and the
    /// tokens of its body (parsed when the arguments are known).
    fn parse_lambda_params(&mut self) -> Result<(Vec<String>, usize, usize), ParseError> {
        self.bump(); // LAMBDA
        self.expect("(", "'(' after LAMBDA")?;
        let mut params = Vec::new();
        // Parameters are the names followed by ','; the last part is the body.
        while matches!(self.peek(), Some(Tok::Ident(_)))
            && matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == ",")
        {
            params.push(self.binding_name()?);
            self.bump(); // ,
        }
        let start = self.pos;
        // Skip the body to its closing ')' (balanced).
        let mut level = 0usize;
        while let Some(t) = self.peek() {
            match t {
                Tok::Op(o) if o == "(" => level += 1,
                Tok::Op(o) if o == ")" && level == 0 => break,
                Tok::Op(o) if o == ")" => level -= 1,
                _ => {}
            }
            self.bump();
        }
        let end = self.pos;
        self.expect(")", "')' to close LAMBDA")?;
        if params.is_empty() {
            return Err(self.err("LAMBDA needs at least one parameter"));
        }
        Ok((params, start, end))
    }

    /// Parse the body tokens `start..end` with `params` bound to `args`.
    fn lambda_body(
        &mut self,
        params: &[String],
        args: Vec<Expr>,
        start: usize,
        end: usize,
    ) -> Result<Expr, ParseError> {
        if params.len() != args.len() {
            return Err(self.err(format!(
                "this LAMBDA takes {} argument(s), got {}",
                params.len(),
                args.len()
            )));
        }
        let saved = self.pos;
        let base = self.scope.len();
        self.scope.extend(params.iter().cloned().zip(args));
        self.pos = start;
        let body = self.parse_expr();
        let stopped = self.pos;
        self.scope.truncate(base);
        self.pos = saved;
        let body = body?;
        if stopped != end {
            return Err(ParseError::new(
                "unexpected trailing input in the LAMBDA body",
                self.toks.get(stopped).map(|s| s.pos),
            ));
        }
        self.check(&body, self.toks.get(start).map(|s| s.pos))?;
        Ok(body)
    }

    /// `Name(a, …)` of a named LAMBDA whose parameters take `kinds`: stored
    /// as a call of [`functions::named_lambda`], expanded when compiled. A
    /// category parameter's argument is a category name, a reduced one's a
    /// measure reference.
    fn parse_named_lambda_call(
        &mut self,
        id: u32,
        kinds: &[crate::LambdaParam],
        name: &str,
    ) -> Result<Expr, ParseError> {
        self.bump(); // name
        self.expect("(", "'('")?;
        let n = kinds.len();
        let mut args = Vec::new();
        if !self.eat_op(")") {
            loop {
                let arg = match kinds.get(args.len()) {
                    Some(crate::LambdaParam::Category) => {
                        Expr::Category(self.parse_category_name()?)
                    }
                    kind => {
                        let e = self.parse_expr()?;
                        if kind == Some(&crate::LambdaParam::Reduced) && !matches!(e, Expr::Ref(..))
                        {
                            return Err(self.err(format!(
                                "{name} reduces argument {} over a category: pass a measure, \
                                 not an expression",
                                args.len() + 1
                            )));
                        }
                        e
                    }
                };
                args.push(arg);
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect(")", "')' to close the call")?;
        }
        if args.len() != n {
            return Err(self.err(format!("{name} takes {n} argument(s), got {}", args.len())));
        }
        Ok(Expr::Call(functions::named_lambda(id), args))
    }

    /// `LAMBDA(p, …, body)(a, …)`: called on the spot.
    fn parse_lambda_call(&mut self) -> Result<Expr, ParseError> {
        let (params, start, end) = self.parse_lambda_params()?;
        if !self.eat_op("(") {
            return Err(self.err(
                "a LAMBDA must be called on the spot (LAMBDA(x, x + 1)(Price)) or passed \
                 to MAP, REDUCE or SCAN",
            ));
        }
        let mut args = Vec::new();
        loop {
            args.push(self.parse_expr()?);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect(")", "')' to close the LAMBDA call")?;
        self.lambda_body(&params, args, start, end)
    }

    /// `MAP(a, [b, …], LAMBDA(p, [q, …], body))`: the body with each
    /// parameter replaced by its argument, element by element.
    fn parse_map(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // MAP
        self.expect("(", "'(' after MAP")?;
        let mut args = Vec::new();
        while !self.peek_kw("lambda") {
            args.push(self.parse_expr()?);
            self.expect(",", "',' then a LAMBDA in MAP")?;
        }
        let (params, start, end) = self.parse_lambda_params()?;
        self.expect(")", "')' to close MAP")?;
        self.lambda_body(&params, args, start, end)
    }

    /// `POSITION(cat)`, the item's 1-based position along `cat`.
    fn position(cat: CategoryId) -> Expr {
        Expr::Call(functions::POSITION, vec![Expr::Category(cat)])
    }

    /// `SEQUENCE(T, [start = 1], [step = 1])`: `start`, `start + step`, …
    /// along T's items, i.e. `(POSITION(T) - 1) * step + start`. Excel's
    /// `SEQUENCE(rows, cols, …)` sizes a spill; here the category is the size.
    fn parse_sequence(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // SEQUENCE
        self.expect("(", "'(' after SEQUENCE")?;
        let cat = self.parse_category_name()?;
        let def = functions::callable("SEQUENCE").expect("SEQUENCE is registered");
        let rest = self.rest_args(def, vec![Expr::Category(cat)], usize::MAX)?;
        self.expect(")", "')' to close SEQUENCE")?;
        if rest.len() > 3 {
            return Err(self.err("SEQUENCE takes (Category, [start], [step])"));
        }
        let mut it = rest.into_iter().skip(1);
        let one = || Expr::Literal(Value::Number(1.0));
        let (start, step) = (it.next().unwrap_or_else(one), it.next().unwrap_or_else(one));
        let index = Expr::BinaryOp(
            BinaryOp::Sub,
            Box::new(Self::position(cat)),
            Box::new(one()),
        );
        Ok(Expr::BinaryOp(
            BinaryOp::Add,
            Box::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(index),
                Box::new(step),
            )),
            Box::new(start),
        ))
    }

    /// `MAKEARRAY(R, C, LAMBDA(r, c, body))`: the body over R × C with `r`
    /// and `c` the item positions along R and C.
    fn parse_makearray(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // MAKEARRAY
        self.expect("(", "'(' after MAKEARRAY")?;
        let r = self.parse_category_name()?;
        self.expect(",", "',' after the row category")?;
        let c = self.parse_category_name()?;
        self.expect(",", "',' then LAMBDA(r, c, body)")?;
        if !self.peek_kw("lambda") {
            return Err(self.err("MAKEARRAY needs LAMBDA(r, c, body)"));
        }
        let (params, start, end) = self.parse_lambda_params()?;
        self.expect(")", "')' to close MAKEARRAY")?;
        self.lambda_body(
            &params,
            vec![Self::position(r), Self::position(c)],
            start,
            end,
        )
    }

    /// `BYROW(x OVER C, LAMBDA(row, body))` (and `BYCOL`): the body once
    /// per item of x's other categories, with `row` the values along C,
    /// which the body must reduce (`SUM(row)`, `MAX(row) - MIN(row)`, …).
    /// In a dimensional model a row and a column differ only in which
    /// category is named, so BYCOL is the same form.
    fn parse_byrow(&mut self, name: &str) -> Result<Expr, ParseError> {
        let name = name.to_ascii_uppercase();
        self.bump(); // BYROW / BYCOL
        self.expect("(", "'(' after the function name")?;
        let x = self.parse_measure_ref()?;
        if !self.peek_kw("over") {
            return Err(self.err(format!("{name} needs x OVER Category (the row's category)")));
        }
        self.bump();
        let cat = self.parse_category_name()?;
        let row = match x {
            Expr::Ref(id, mut spec) => {
                spec.over = vec![cat];
                Expr::Ref(id, spec)
            }
            _ => return Err(self.err(format!("{name} takes a measure reference"))),
        };
        self.expect(",", "',' then LAMBDA(row, body)")?;
        if !self.peek_kw("lambda") {
            return Err(self.err(format!("{name} needs LAMBDA(row, body)")));
        }
        let (params, start, end) = self.parse_lambda_params()?;
        self.expect(")", "')' to close the call")?;
        let body = self.lambda_body(&params, vec![row], start, end)?;
        fn bare(e: &Expr) -> bool {
            match e {
                Expr::Ref(_, s) => !s.over.is_empty(),
                Expr::Call(f, _) if functions::by_id(*f).is_some_and(|d| d.kind.reduces()) => false,
                Expr::Call(_, a) => a.iter().any(bare),
                Expr::UnaryOp(_, a) => bare(a),
                Expr::BinaryOp(_, a, b) => bare(a) || bare(b),
                _ => false,
            }
        }
        if bare(&body) {
            return Err(self.err(format!(
                "{name}'s LAMBDA must reduce its row to one value (SUM(row), MAX(row), …)"
            )));
        }
        Ok(body)
    }

    /// `REDUCE(init, x OVER T, LAMBDA(acc, v, body))` and `SCAN(…)`: a fold
    /// along T in item order. Stored as `Call(id, [init, x OVER T, body])`
    /// with `acc` and `v` as placeholder references (see
    /// `functions::lambda_slot`).
    fn parse_fold(&mut self, name: &str) -> Result<Expr, ParseError> {
        let def = functions::callable(name).expect("REDUCE/SCAN are registered");
        self.bump(); // name
        self.expect("(", "'(' after the function name")?;
        let init = self.parse_expr()?;
        self.expect(",", "',' after the initial value")?;
        let x = self.parse_measure_ref()?;
        if !self.peek_kw("over") {
            return Err(self.err(format!(
                "{} folds a measure: write x OVER Category",
                def.name
            )));
        }
        self.bump();
        let cat = self.parse_category_name()?;
        let x = match x {
            Expr::Ref(id, mut spec) => {
                spec.over = vec![cat];
                Expr::Ref(id, spec)
            }
            _ => return Err(self.err(format!("{} folds a measure reference", def.name))),
        };
        self.expect(",", "',' then a LAMBDA(accumulator, value, body)")?;
        if !self.peek_kw("lambda") {
            return Err(self.err(format!(
                "{} needs LAMBDA(accumulator, value, body)",
                def.name
            )));
        }
        let (params, start, end) = self.parse_lambda_params()?;
        self.expect(")", "')' to close the fold")?;
        if params.len() != 2 {
            return Err(self.err(format!(
                "{}'s LAMBDA takes (accumulator, value), got {} parameter(s)",
                def.name,
                params.len()
            )));
        }
        let k = self.lambdas;
        self.lambdas += 1;
        let slot = |i| Expr::Ref(functions::lambda_slot(k, i), DimensionSpec::default());
        let body = self.lambda_body(&params, vec![slot(0), slot(1)], start, end)?;
        Ok(Expr::Call(def.id, vec![init, x, body]))
    }

    /// `FIXPOINT(init, LAMBDA(r, body), [max_iter], [tolerance])`: stored as
    /// `Call(id, [init, body, max_iter?, tolerance?])` with `r` the
    /// placeholder `functions::lambda_slot(k, 0)`. `k` is taken before
    /// `init` is read, so a FIXPOINT that is the whole formula has k = 0.
    fn parse_fixpoint(&mut self) -> Result<Expr, ParseError> {
        let def = functions::callable("FIXPOINT").expect("FIXPOINT is registered");
        self.bump(); // FIXPOINT
        self.expect("(", "'(' after FIXPOINT")?;
        let k = self.lambdas;
        self.lambdas += 1;
        let init = self.parse_expr()?;
        self.expect(",", "',' then LAMBDA(r, body)")?;
        if !self.peek_kw("lambda") {
            return Err(self.err("FIXPOINT needs LAMBDA(r, body)"));
        }
        let (params, start, end) = self.parse_lambda_params()?;
        if params.len() != 1 {
            return Err(self.err(format!(
                "FIXPOINT's LAMBDA takes one parameter (the iterate), got {}",
                params.len()
            )));
        }
        let r = Expr::Ref(functions::lambda_slot(k, 0), DimensionSpec::default());
        let body = self.lambda_body(&params, vec![r], start, end)?;
        let args = self.rest_args(def, vec![init, body], 4)?;
        self.expect(")", "')' to close FIXPOINT")?;
        Ok(Expr::Call(def.id, args))
    }

    /// MeasureRef = Name [ "[" DimList "]" ]
    fn parse_measure_ref(&mut self) -> Result<Expr, ParseError> {
        let (name, pos) = self.parse_name("a measure name")?;
        let id = self.measure_id(&name, pos)?;
        let mut spec = DimensionSpec::default();
        if self.eat_op("[") {
            spec.by.push(self.parse_category_name()?);
            while self.eat_op(",") {
                spec.by.push(self.parse_category_name()?);
            }
            if !self.eat_op("]") {
                return Err(self.err("expected ']' to close dimension list"));
            }
        }
        Ok(Expr::Ref(id, spec))
    }

    fn parse_category_name(&mut self) -> Result<CategoryId, ParseError> {
        let (name, pos) = self.parse_name("a category name")?;
        self.category_id(&name, pos)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a full `Target = Expression` formula. The LHS identifier is the target
/// measure name (returned as-is; it need not exist yet). The RHS is resolved
/// against `model`.
pub fn parse_formula(model: &Model, text: &str) -> Result<FormulaText, ParseError> {
    let toks = tokenize(text)?;
    let (target, rhs_start) = parse_lhs(&toks)?;
    let mut p = Parser::new(&toks[rhs_start..], model, false);
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    p.check_formula(&expr, p.toks.first().map(|s| s.pos))?;
    Ok(FormulaText {
        target,
        formula: Formula::new(expr),
    })
}

impl<'a> Parser<'a> {
    fn new(toks: &'a [Spanned], model: &'a Model, stored: bool) -> Self {
        Parser {
            toks,
            pos: 0,
            model,
            depth: 0,
            scope: Vec::new(),
            lambdas: 0,
            expanded: 0,
            stored,
        }
    }
}

/// Locate the target name (LHS) and the index of the first RHS token (past
/// `=`), skipping an optional bracketed target dimension list. Shared by
/// [`parse_formula`] and [`parse_definition`].
fn parse_lhs(toks: &[Spanned]) -> Result<(Name, usize), ParseError> {
    if toks.is_empty() {
        return Err(ParseError::new("empty formula", None));
    }
    let target = match &toks[0].tok {
        // Bare or quoted: `'Unit Price' = ...` names a CSV-derived measure.
        Tok::Ident(w) | Tok::QIdent(w) => Name(w.clone()),
        _ => {
            return Err(ParseError::new(
                "formula must start with a target measure name",
                Some(toks[0].pos),
            ))
        }
    };
    let mut idx = 1;
    if matches!(toks.get(idx).map(|s| &s.tok), Some(Tok::Op(o)) if o == "[") {
        idx += 1;
        while !matches!(toks.get(idx).map(|s| &s.tok), Some(Tok::Op(o)) if o == "]") {
            if toks.get(idx).is_none() {
                return Err(ParseError::new(
                    "unterminated '[' in target dimension list",
                    Some(toks[0].pos),
                ));
            }
            idx += 1;
        }
        idx += 1; // past ']'
    }
    if !matches!(toks.get(idx).map(|s| &s.tok), Some(Tok::Op(o)) if o == "=") {
        return Err(ParseError::new(
            "expected '=' after target measure name",
            toks.get(idx).map(|s| s.pos),
        ));
    }
    Ok((target, idx + 1))
}

/// Parse a measure definition: an ordinary `Target = <expr>` formula, or a
/// source form `Target = SQL("...")` / `Target = CALL(fn, m1, m2, ...)`. The
/// source forms are recognized only as the *entire* RHS (they are measure
/// sources, not sub-expressions), keeping them out of the engine's expression
/// grammar and thus off the deterministic hot path.
pub fn parse_definition(model: &Model, text: &str) -> Result<Definition, ParseError> {
    definition(model, text, false)
}

/// [`parse_definition`] for a definition a store kept as text, loaded:
/// held to [`MAX_STORED_FORMULA_DEPTH`] rather than [`MAX_FORMULA_DEPTH`], so
/// a formula saved before that limit, which worked, still loads.
pub fn parse_stored_definition(model: &Model, text: &str) -> Result<Definition, ParseError> {
    definition(model, text, true)
}

fn definition(model: &Model, text: &str, stored: bool) -> Result<Definition, ParseError> {
    let toks = tokenize(text)?;
    let (target, rhs) = parse_lhs(&toks)?;
    // Is the RHS exactly `IDENT ( ... )` where IDENT is SQL/CALL?
    if let Some(Tok::Ident(head)) = toks.get(rhs).map(|s| &s.tok) {
        let is_open = matches!(toks.get(rhs + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "(");
        if is_open && head.eq_ignore_ascii_case("sql") {
            return parse_sql_form(&toks, rhs, target);
        }
        if is_open && head.eq_ignore_ascii_case("call") {
            return parse_call_form(&toks, rhs, target);
        }
        if is_open && head.eq_ignore_ascii_case("lambda") {
            return parse_lambda_form(model, &toks, rhs, target, stored);
        }
    }
    // Fall through to an ordinary formula expression.
    let mut p = Parser::new(&toks[rhs..], model, stored);
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    p.check_formula(&expr, p.toks.first().map(|s| s.pos))?;
    Ok(Definition::Formula(FormulaText {
        target,
        formula: Formula::new(expr),
    }))
}

/// `Name = LAMBDA(p, …, body)`: a named LAMBDA. The body may read only its
/// parameters (and literals, functions and earlier named LAMBDAs), so a
/// call depends on nothing but its arguments.
fn parse_lambda_form(
    model: &Model,
    toks: &[Spanned],
    rhs: usize,
    target: Name,
    stored: bool,
) -> Result<Definition, ParseError> {
    if functions::callable(&target.0).is_some() {
        return Err(ParseError::new(
            format!(
                "{} is a built-in function; name the LAMBDA differently",
                target.0
            ),
            None,
        ));
    }
    // The lambda being (re)defined is not visible in its own body.
    let mut m = model.clone();
    let own = m.lambda_by_name(&target.0).map(|(id, _)| id);
    if let Some(id) = own {
        m.lambdas.remove(&id);
    }
    let mut p = Parser::new(&toks[rhs..], &m, stored);
    let (params, start, end) = p.parse_lambda_params()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected input after the LAMBDA (a named LAMBDA is not called here)"));
    }
    let args = (0..params.len())
        .map(|i| Expr::Ref(functions::lambda_param(i), DimensionSpec::default()))
        .collect();
    let body = p.lambda_body(&params, args, start, end)?;
    let refs = body.referenced_measures();
    if refs
        .iter()
        .any(|r| functions::lambda_param_of(*r).is_none())
    {
        return Err(ParseError::new(
            "a named LAMBDA reads only its parameters: pass measures as arguments",
            None,
        ));
    }
    if refs.iter().any(|r| functions::lambda_slot_of(*r).is_some()) {
        return Err(ParseError::new(
            "a named LAMBDA cannot hold SCAN or REDUCE",
            None,
        ));
    }
    let lambda = crate::NamedLambda {
        name: target.clone(),
        params,
        body,
    };
    lambda.param_kinds().map_err(|e| ParseError::new(e, None))?;
    Ok(Definition::Lambda(LambdaText {
        name: target,
        lambda,
    }))
}

/// `SQL("<query>")` — a single string literal argument.
fn parse_sql_form(toks: &[Spanned], rhs: usize, target: Name) -> Result<Definition, ParseError> {
    // toks[rhs] = SQL, [rhs+1] = '(', [rhs+2] = Str, [rhs+3] = ')'
    let query = match toks.get(rhs + 2).map(|s| &s.tok) {
        Some(Tok::Str(s)) => s.clone(),
        _ => {
            return Err(ParseError::new(
                "SQL(...) takes a single quoted query string",
                toks.get(rhs + 2).map(|s| s.pos),
            ))
        }
    };
    match toks.get(rhs + 3).map(|s| &s.tok) {
        Some(Tok::Op(o)) if o == ")" => {}
        _ => {
            return Err(ParseError::new(
                "expected ')' to close SQL(...)",
                toks.get(rhs + 3).map(|s| s.pos),
            ))
        }
    }
    if toks.get(rhs + 4).is_some() {
        return Err(ParseError::new(
            "unexpected trailing input after SQL(...)",
            toks.get(rhs + 4).map(|s| s.pos),
        ));
    }
    Ok(Definition::Sql { target, query })
}

/// `CALL(func, arg_measure, ...)` — a function name then zero or more measure
/// name arguments.
fn parse_call_form(toks: &[Spanned], rhs: usize, target: Name) -> Result<Definition, ParseError> {
    let func = match toks.get(rhs + 2).map(|s| &s.tok) {
        Some(Tok::Ident(w)) => w.clone(),
        _ => {
            return Err(ParseError::new(
                "CALL(...) takes a function name then argument measures",
                toks.get(rhs + 2).map(|s| s.pos),
            ))
        }
    };
    // After the function name: optional `, arg, arg, ...` then `)`.
    let mut i = rhs + 3;
    let mut args = Vec::new();
    loop {
        match toks.get(i).map(|s| &s.tok) {
            Some(Tok::Op(o)) if o == ")" => {
                i += 1;
                break;
            }
            Some(Tok::Op(o)) if o == "," => {
                i += 1;
                match toks.get(i).map(|s| &s.tok) {
                    Some(Tok::Ident(w) | Tok::QIdent(w)) => {
                        args.push(Name(w.clone()));
                        i += 1;
                    }
                    _ => {
                        return Err(ParseError::new(
                            "expected an argument measure name after ',' in CALL(...)",
                            toks.get(i).map(|s| s.pos),
                        ))
                    }
                }
            }
            other => {
                return Err(ParseError::new(
                    if other.is_none() {
                        "unterminated CALL(...)"
                    } else {
                        "expected ',' or ')' in CALL(...)"
                    },
                    toks.get(i).map(|s| s.pos),
                ))
            }
        }
    }
    if toks.get(i).is_some() {
        return Err(ParseError::new(
            "unexpected trailing input after CALL(...)",
            toks.get(i).map(|s| s.pos),
        ));
    }
    Ok(Definition::Call { target, func, args })
}

/// Parse just an expression (the RHS), with no target/assignment.
pub fn parse_expr(model: &Model, text: &str) -> Result<Formula, ParseError> {
    expr(model, text, false)
}

/// [`parse_expr`] for an expression a store kept as text, loaded (see
/// [`parse_stored_definition`]).
pub fn parse_stored_expr(model: &Model, text: &str) -> Result<Formula, ParseError> {
    expr(model, text, true)
}

fn expr(model: &Model, text: &str, stored: bool) -> Result<Formula, ParseError> {
    let toks = tokenize(text)?;
    if toks.is_empty() {
        return Err(ParseError::new("empty expression", None));
    }
    let mut p = Parser::new(&toks, model, stored);
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    p.check_formula(&expr, p.toks.first().map(|s| s.pos))?;
    Ok(Formula::new(expr))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{Measure, MeasureKind, Name, ValueType};

    const TIME: CategoryId = CategoryId(1);
    const PRODUCT: CategoryId = CategoryId(2);
    const PRICE: MeasureId = MeasureId(100);
    const QUANTITY: MeasureId = MeasureId(101);
    const REVENUE: MeasureId = MeasureId(102);
    const COST: MeasureId = MeasureId(103);

    fn fixture() -> Model {
        let mut m = Model::new();
        m.add_category(TIME, "Time");
        m.add_category(PRODUCT, "Product");
        for (id, name, cats, vt) in [
            (PRICE, "Price", vec![PRODUCT], ValueType::Number),
            (QUANTITY, "Quantity", vec![TIME, PRODUCT], ValueType::Number),
            (REVENUE, "Revenue", vec![TIME, PRODUCT], ValueType::Number),
            (COST, "Cost", vec![TIME, PRODUCT], ValueType::Number),
        ] {
            m.add_measure(Measure {
                id,
                name: Name(name.into()),
                value_type: vt,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
        }
        m
    }

    fn refr(id: MeasureId) -> Expr {
        Expr::Ref(id, DimensionSpec::default())
    }

    /// Every form of the OVER clause, category arguments, `@` and dotted
    /// names, and the errors each can raise.
    #[test]
    fn over_clauses_category_args_and_their_errors() {
        let m = fixture();
        let p = |s: &str| parse_expr(&m, s).map(|f| f.expr);
        let call = |f: u32, a: Vec<Expr>| Expr::Call(FuncId(f), a);
        let over = |id, cats: Vec<CategoryId>| {
            Expr::Ref(
                id,
                DimensionSpec {
                    over: cats,
                    ..Default::default()
                },
            )
        };
        // Several categories; a comma then a non-category is the next argument.
        assert_eq!(
            p("SUM(Quantity OVER Time, Product)"),
            Ok(call(1, vec![over(QUANTITY, vec![TIME, PRODUCT])]))
        );
        assert_eq!(
            p("TEXTJOIN(\"-\", TRUE, Quantity OVER Time)"),
            Ok(call(
                909,
                vec![
                    Expr::Literal(Value::Text("-".into())),
                    Expr::Literal(Value::Boolean(true)),
                    over(QUANTITY, vec![TIME])
                ]
            ))
        );
        // ALL, ALL EXCEPT (resolved from the measure's categories).
        assert_eq!(
            p("SUM(Quantity OVER ALL)"),
            Ok(call(1, vec![over(QUANTITY, vec![TIME, PRODUCT])]))
        );
        assert_eq!(
            p("SUM(Quantity OVER ALL EXCEPT Product)"),
            Ok(call(1, vec![over(QUANTITY, vec![TIME])]))
        );
        assert_eq!(
            p("SUM(Quantity OVER ALL EXCEPT Product, Time)"),
            Ok(call(1, vec![over(QUANTITY, vec![])]))
        );
        // WHERE and windows become trailing markers.
        let w = p("SUM(Quantity OVER Time WHERE Quantity > 1)").unwrap();
        assert!(
            matches!(&w, Expr::Call(_, a) if matches!(&a[1], Expr::Call(id, _) if *id == functions::WHERE))
        );
        for (src, from, to) in [
            (
                "SUM(Quantity OVER Time[FIRST..THIS])",
                Value::Text("FIRST".into()),
                Value::Number(0.0),
            ),
            (
                "SUM(Quantity OVER Time[THIS-2..THIS+1])",
                Value::Number(-2.0),
                Value::Number(1.0),
            ),
            (
                "SUM(Quantity OVER Time[THIS..LAST])",
                Value::Number(0.0),
                Value::Text("LAST".into()),
            ),
        ] {
            let e = p(src).unwrap();
            let Expr::Call(_, a) = e else { panic!() };
            assert_eq!(
                a[1],
                Expr::Call(
                    functions::WINDOW,
                    vec![Expr::Literal(from), Expr::Literal(to)]
                ),
                "{src}"
            );
        }
        // OVER after the second reduced argument applies to both (CORREL-like
        // shapes are exercised by the engine; TEXTJOIN reduces arg 2 only).
        // Category arguments, `@` names, dotted names.
        assert_eq!(
            p("POSITION(Time)"),
            Ok(call(900, vec![Expr::Category(TIME)]))
        );
        assert_eq!(
            p("@ITEM(Product)"),
            Ok(call(901, vec![Expr::Category(PRODUCT)]))
        );
        assert_eq!(
            p("@itemcount(Time)"),
            Ok(call(903, vec![Expr::Category(TIME)]))
        );
        assert_eq!(
            p("PREV(Quantity, Time, 2, 0)"),
            Ok(call(
                904,
                vec![
                    refr(QUANTITY),
                    Expr::Category(TIME),
                    Expr::Literal(Value::Number(2.0)),
                    Expr::Literal(Value::Number(0.0))
                ]
            ))
        );
        assert_eq!(p("TRUE()"), Ok(call(39, vec![])));
        assert_eq!(p("ISO.CEILING(Price)"), Ok(call(210, vec![refr(PRICE)])));
        // Dotted names whose prefixes are not functions, and digit parts.
        let joined = |src: &str| tokenize(src).unwrap().first().map(|t| t.tok.clone());
        assert_eq!(
            joined("NORM.S.DIST(1)"),
            Some(Tok::Ident("NORM.S.DIST".into()))
        );
        assert_eq!(
            joined("T.DIST.2T(1, 2)"),
            Some(Tok::Ident("T.DIST.2T".into()))
        );
        assert_eq!(
            joined("FLOOR.MATH (1)"),
            Some(Tok::Ident("FLOOR.MATH".into()))
        );
        assert_eq!(joined("Sheet.Price"), Some(Tok::Ident("Sheet".into())));
        // Errors.
        for (src, want) in [
            ("SUM(Quantity OVER Time OVER Product)", "expected ')'"),
            (
                "SUM(Quantity OVER Time, Quantity OVER Product)",
                "only one OVER",
            ),
            ("SUM(Quantity OVER Time[FIRST.THIS])", "'..'"),
            ("SUM(Quantity OVER Time[FIRST..THIS)", "']'"),
            ("SUM(Quantity OVER Time[NOW..THIS])", "FIRST, LAST or THIS"),
            ("SUM(Quantity OVER Time[1..THIS])", "FIRST, LAST or THIS"),
            ("SUM(Quantity OVER Time[THIS+x..THIS])", "whole number"),
            ("SUM(Quantity OVER Time[THIS-1.5..THIS])", "whole number"),
            ("ABS(Price OVER Time)", "does not aggregate"),
            ("SUM(Price * 2 OVER Time)", "measure reference"),
            ("POSITION(Price)", "unknown category"),
            ("SUM(Quantity OVER Nope)", "unknown category"),
            ("SUM(Quantity OVER ALL EXCEPT Nope)", "unknown category"),
            ("TEXTJOIN(\"-\", Quantity OVER Time)", "takes"),
        ] {
            let e = p(src).unwrap_err();
            assert!(e.message.contains(want), "{src}: {}", e.message);
        }
        // `@` alone, or before a non-letter, is not a name.
        assert!(p("@ + 1").is_err());
        assert!(p("@1").is_err());
        // A dotted measure-like name that is not a function stays an error.
        assert!(p("FLOOR.NOPE(Price)").is_err());
        // `ALL(` is a function-looking word, not OVER ALL.
        assert!(p("SUM(Quantity OVER ALL(Time))").is_err());
    }

    /// LET and LAMBDA expand in place; their errors name the problem.
    /// A named LAMBDA's parameter used as a category (`OVER c`,
    /// `POSITION(c)`) takes a category name at the call; on expansion the
    /// reduced argument carries the `OVER` and the category maps through,
    /// also through another lambda.
    #[test]
    fn named_lambda_category_parameters() {
        use crate::LambdaParam;
        let mut m = fixture();
        let def = |m: &Model, t: &str| match parse_definition(m, t) {
            Ok(Definition::Lambda(l)) => l.lambda,
            other => panic!("{t}: {other:?}"),
        };
        let p = |m: &Model, t: &str| parse_expr(m, t).unwrap().expr;
        let l = def(&m, "LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))");
        assert_eq!(
            l.param_kinds(),
            Ok(vec![LambdaParam::Reduced, LambdaParam::Category])
        );
        m.lambdas.insert(1, l);
        let call = p(&m, "LOGNORM(Quantity, Time)");
        assert_eq!(
            call,
            Expr::Call(
                functions::named_lambda(1),
                vec![refr(QUANTITY), Expr::Category(TIME)]
            )
        );
        assert_eq!(
            m.expand_lambdas(&call),
            Ok(p(&m, "LN1P(Quantity) / LN1P(MAX(Quantity OVER Time))"))
        );
        let at = def(&m, "AT = LAMBDA(v, k, LOGNORM(v, k) * POSITION(k))");
        assert_eq!(
            at.param_kinds(),
            Ok(vec![LambdaParam::Value, LambdaParam::Category])
        );
        m.lambdas.insert(2, at);
        assert_eq!(
            m.expand_lambdas(&p(&m, "AT(Quantity, Product)")),
            Ok(p(
                &m,
                "LN1P(Quantity) / LN1P(MAX(Quantity OVER Product)) * POSITION(Product)"
            ))
        );
        for (text, why) in [
            ("LOGNORM(Quantity * 2, Time)", "pass a measure"),
            ("LOGNORM(Quantity, Nope)", "unknown category: Nope"),
            ("LOGNORM(Quantity)", "takes 2"),
        ] {
            let e = parse_expr(&m, text).unwrap_err();
            assert!(e.message.contains(why), "{text}: {}", e.message);
        }
        let e = parse_definition(&m, "Bad = LAMBDA(x, c, c + SUM(x OVER c))").unwrap_err();
        assert!(e.message.contains("both as a value and as a category"));
        // Hand-built calls with the wrong kind of argument fail to expand.
        let call = |args| Expr::Call(functions::named_lambda(1), args);
        let e = m.expand_lambdas(&call(vec![refr(QUANTITY), refr(PRICE)]));
        assert!(e
            .unwrap_err()
            .contains("LOGNORM: argument 2 must be a category"));
        let one = Expr::Literal(Value::Number(1.0));
        let e = m.expand_lambdas(&call(vec![one, Expr::Category(TIME)]));
        assert!(e.unwrap_err().contains("must be a measure"));
    }

    #[test]
    fn let_and_lambda() {
        let m = fixture();
        let p = |s: &str| parse_expr(&m, s).map(|f| f.expr);
        assert_eq!(p("LET(x, Price, x + x)"), p("Price + Price"));
        assert_eq!(p("LET(x, Price, y, x * 2, y - x)"), p("Price * 2 - Price"));
        // A local shadows a measure; it ends with its LET.
        assert_eq!(p("LET(Price, 3, Price) + Price"), p("3 + Price"));
        assert_eq!(
            p("LAMBDA(a, a * a)(Price + 1)"),
            p("(Price + 1) * (Price + 1)")
        );
        assert_eq!(
            p("MAP(Price, Cost, LAMBDA(a, b, a - b))"),
            p("Price - Cost")
        );
        let fold = p("REDUCE(0, Quantity OVER Time, LAMBDA(acc, v, acc + v))").unwrap();
        let Expr::Call(_, args) = fold else { panic!() };
        assert_eq!(
            args[2],
            Expr::BinaryOp(
                BinaryOp::Add,
                Box::new(Expr::Ref(
                    functions::lambda_slot(0, 0),
                    DimensionSpec::default()
                )),
                Box::new(Expr::Ref(
                    functions::lambda_slot(0, 1),
                    DimensionSpec::default()
                )),
            )
        );
        assert_eq!(
            functions::lambda_slot_of(functions::lambda_slot(3, 1)),
            Some((3, 1))
        );
        assert_eq!(functions::lambda_slot_of(MeasureId(5)), None);
        // FIXPOINT: r is the first placeholder, the options stay as written.
        let r = Expr::Ref(functions::lambda_slot(0, 0), DimensionSpec::default());
        let fix = functions::callable("FIXPOINT").unwrap().id;
        assert_eq!(
            p("FIXPOINT(Price, LAMBDA(r, r / 2 + 1), 50)"),
            Ok(Expr::Call(
                fix,
                vec![
                    refr(PRICE),
                    Expr::BinaryOp(
                        BinaryOp::Add,
                        Box::new(Expr::BinaryOp(
                            BinaryOp::Div,
                            Box::new(r.clone()),
                            Box::new(Expr::Literal(Value::Number(2.0)))
                        )),
                        Box::new(Expr::Literal(Value::Number(1.0)))
                    ),
                    Expr::Literal(Value::Number(50.0)),
                ]
            ))
        );
        assert_eq!(
            p("FIXPOINT(1, LAMBDA(x, x))"),
            Ok(Expr::Call(fix, vec![Expr::Literal(Value::Number(1.0)), r]))
        );
        for (src, want) in [
            ("LET(x, Price)", "expected"),
            ("LET(1, Price, 2)", "a name to bind"),
            ("LET(x Price, x)", "',' after a LET name"),
            ("LET(x, Price, x", "')' to close LET"),
            ("LAMBDA(x, x)", "called on the spot"),
            ("LAMBDA(x)(1)", "at least one parameter"),
            ("LAMBDA(x, x + 1)(1, 2)", "takes 1 argument"),
            ("LAMBDA(x, x +)(1)", ""),
            ("LAMBDA(x, x", "')' to close LAMBDA"),
            ("MAP(Price LAMBDA(v, v))", "',' then a LAMBDA"),
            ("MAP(Price, LAMBDA(v, v)", "')' to close MAP"),
            ("SCAN(0, Quantity, LAMBDA(a, v, a))", "OVER"),
            ("SCAN(0, Quantity OVER Time, 1)", "needs LAMBDA"),
            (
                "SCAN(0, Quantity OVER Time LAMBDA(a, v, a))",
                "',' then a LAMBDA",
            ),
            (
                "SCAN(0, Quantity OVER Time, LAMBDA(v, v))",
                "takes (accumulator, value)",
            ),
            (
                "SCAN(0 Quantity OVER Time, LAMBDA(a, v, a))",
                "',' after the initial",
            ),
            (
                "SCAN(0, Quantity OVER Time, LAMBDA(a, v, a)",
                "')' to close the fold",
            ),
            ("SCAN 1", "unknown measure"),
            ("FIXPOINT(0, 1)", "needs LAMBDA"),
            ("FIXPOINT(0 LAMBDA(r, r))", "',' then LAMBDA"),
            ("FIXPOINT(0, LAMBDA(a, b, a))", "one parameter"),
            (
                "FIXPOINT(0, LAMBDA(r, r), 5, 0, 1)",
                "')' to close FIXPOINT",
            ),
            ("LET 1", "unknown measure"),
        ] {
            let e = p(src).unwrap_err();
            assert!(e.message.contains(want), "{src}: {}", e.message);
        }
    }

    /// `NAME(` that is no function or measure: an "unknown function" with
    /// the nearest names, at the name.
    #[test]
    fn unknown_function_suggests() {
        let m = fixture();
        let e = parse_expr(&m, "1 + PMTT(1, 2, 3)").unwrap_err();
        assert!(
            e.message
                .starts_with("unknown function: PMTT; did you mean PMT"),
            "{}",
            e.message
        );
        assert_eq!(e.position, Some(4));
        let e = parse_expr(&m, "Qqqqqqqqqq(1)").unwrap_err();
        assert_eq!(e.message, "unknown function: Qqqqqqqqqq");
    }

    /// `name => value` after the positional arguments: placed in its
    /// parameter's slot, skipped optional ones filled with their defaults,
    /// and every misuse a located error.
    #[test]
    fn named_arguments() {
        let m = fixture();
        let p = |s: &str| parse_expr(&m, s).map(|f| f.expr);
        let same = |a: &str, b: &str| assert_eq!(p(a).unwrap(), p(b).unwrap(), "{a} vs {b}");
        same("ROUND(Price, num_digits => 2)", "ROUND(Price, 2)");
        same("ROUND(Price, NUM_DIGITS => 2)", "ROUND(Price, 2)");
        same("ROUND(number => Price, num_digits => 2)", "ROUND(Price, 2)");
        same("ROUND(num_digits => 2, number => Price)", "ROUND(Price, 2)");
        // A skipped optional parameter gets its written default.
        same("PMT(0.05, 12, 100, type => 1)", "PMT(0.05, 12, 100, 0, 1)");
        same(
            "XLOOKUP(Price, Quantity OVER Time, Cost, search_mode => -1)",
            "XLOOKUP(Price, Quantity OVER Time, Cost, NA(), 0, -1)",
        );
        same(
            "XLOOKUP(Price, Quantity OVER Time, Cost, match_mode => 2)",
            "XLOOKUP(Price, Quantity OVER Time, Cost, NA(), 2)",
        );
        same(
            "FIXPOINT(0, LAMBDA(r, r / 2 + 1), max_iter => 50)",
            "FIXPOINT(0, LAMBDA(r, r / 2 + 1), 50)",
        );
        same(
            "FIXPOINT(0, LAMBDA(r, r / 2 + 1), tolerance => 1e-6)",
            "FIXPOINT(0, LAMBDA(r, r / 2 + 1), 100, 1e-6)",
        );
        same(
            "FIXPOINT(0, LAMBDA(r, r), tolerance => 0, max_iter => 5)",
            "FIXPOINT(0, LAMBDA(r, r), 5, 0)",
        );
        same("SEQUENCE(Time, step => 2)", "SEQUENCE(Time, 1, 2)");
        same("PREV(Price, Time, default => 0)", "PREV(Price, Time, 1, 0)");
        same("PREV(Price, category => Time)", "PREV(Price, Time)");
        same(
            "SUM(Price OVER Time WHERE Price > 1)",
            "SUM(Price OVER Time WHERE Price > 1)",
        );
        same(
            "TEXTSPLIT(Price, \",\", match_mode => 1 INTO Time)",
            "TEXTSPLIT(Price, \",\", \"\", FALSE, 1 INTO Time)",
        );
        same(
            "LOGREG(Price, Quantity OVER Time, stats => TRUE INTO Time)",
            "LOGREG(Price, Quantity OVER Time, TRUE, TRUE INTO Time)",
        );
        for (src, want, at) in [
            (
                "ROUND(num_digits => 2, Price)",
                "positional argument cannot follow",
                23,
            ),
            (
                "ROUND(Price, num_digit => 2)",
                "ROUND has no parameter num_digit; did you mean num_digits",
                13,
            ),
            (
                "ROUND(Price, zzzzzzzz => 2)",
                "its parameters are number, num_digits",
                13,
            ),
            (
                "ROUND(Price, number => 2)",
                "number is already given, as argument 1",
                13,
            ),
            (
                "ROUND(Price, num_digits => 2, num_digits => 3)",
                "num_digits is named twice",
                30,
            ),
            (
                "ROUND(num_digits => 2)",
                "number (argument 1) is required",
                6,
            ),
            (
                "SUM(Price, number1 => 2)",
                "SUM takes its arguments by position",
                11,
            ),
            (
                "LINEST(Price, Quantity OVER Time, stats => TRUE INTO Time)",
                "LINEST takes its arguments by position",
                34,
            ),
            (
                "REGEX(\"a\", \"b\", flags => \"g\")",
                "replacement (argument 3) has no default",
                16,
            ),
            (
                "FIXPOINT(0, LAMBDA(r, r), max_iter => 5, 1)",
                "positional argument cannot follow",
                41,
            ),
            (
                "FIXPOINT(0, LAMBDA(r, r), tol => 1)",
                "did you mean tolerance",
                26,
            ),
            (
                "SEQUENCE(Time, start => 1, 2)",
                "positional argument cannot follow",
                27,
            ),
            ("Price + x => 1", "unexpected x =>", 8),
            ("LAMBDA(a, a)(a => 1)", "unexpected a =>", 13),
        ] {
            let e = p(src).unwrap_err();
            assert!(e.message.contains(want), "{src}: {}", e.message);
            assert_eq!(e.position, Some(at), "{src}: {}", e.message);
        }
        // `=>` is one token, never `=` then `>`.
        assert!(p("Price => 1").is_err());
        assert!(p("Price >= 1").is_ok());
    }

    #[test]
    fn parses_assignment_and_multiplication() {
        let m = fixture();
        let f = parse_formula(&m, "Revenue = Price * Quantity").unwrap();
        assert_eq!(f.target, Name("Revenue".into()));
        assert_eq!(
            f.formula.expr,
            Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(refr(PRICE)),
                Box::new(refr(QUANTITY))
            )
        );
    }

    #[test]
    fn precedence_mul_over_add() {
        // a + b * c == a + (b*c)
        let m = fixture();
        let f = parse_expr(&m, "Price + Quantity * Revenue").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Add,
                Box::new(refr(PRICE)),
                Box::new(Expr::BinaryOp(
                    BinaryOp::Mul,
                    Box::new(refr(QUANTITY)),
                    Box::new(refr(REVENUE)),
                )),
            )
        );
    }

    #[test]
    fn parens_group() {
        // (a + b) * c
        let m = fixture();
        let f = parse_expr(&m, "(Price + Quantity) * Revenue").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::BinaryOp(
                    BinaryOp::Add,
                    Box::new(refr(PRICE)),
                    Box::new(refr(QUANTITY)),
                )),
                Box::new(refr(REVENUE)),
            )
        );
    }

    #[test]
    fn subtraction() {
        let m = fixture();
        let f = parse_formula(&m, "Profit = Revenue - Cost").unwrap();
        assert_eq!(f.target, Name("Profit".into()));
        assert_eq!(
            f.formula.expr,
            Expr::BinaryOp(BinaryOp::Sub, Box::new(refr(REVENUE)), Box::new(refr(COST)))
        );
    }

    #[test]
    fn aggregation_sum_over_time() {
        // TotalRevenue[Product] = SUM(Revenue OVER Time)
        let m = fixture();
        let f = parse_formula(&m, "TotalRevenue[Product] = SUM(Revenue OVER Time)").unwrap();
        assert_eq!(f.target, Name("TotalRevenue".into()));
        let expected = Expr::Call(
            FUNC_SUM,
            vec![Expr::Ref(
                REVENUE,
                DimensionSpec {
                    over: vec![TIME],
                    by: vec![],
                    except: vec![],
                },
            )],
        );
        assert_eq!(f.formula.expr, expected);
    }

    #[test]
    fn dim_list_sets_by() {
        let m = fixture();
        let f = parse_expr(&m, "Quantity[Time, Product]").unwrap();
        assert_eq!(
            f.expr,
            Expr::Ref(
                QUANTITY,
                DimensionSpec {
                    by: vec![TIME, PRODUCT],
                    over: vec![],
                    except: vec![],
                }
            )
        );
    }

    /// The mathematical convention for `^`: right-associative and tighter
    /// than unary minus. `%` binds tightest; `&` sits between `+ -` and the
    /// comparisons.
    #[test]
    fn power_concat_and_percent_precedence() {
        let m = fixture();
        let p = |s: &str| parse_expr(&m, s).unwrap().expr;
        let num = |n: f64| Expr::Literal(Value::Number(n));
        let pow = |a, b| Expr::BinaryOp(BinaryOp::Pow, Box::new(a), Box::new(b));
        let neg = |a| Expr::UnaryOp(UnaryOp::Neg, Box::new(a));
        assert_eq!(p("-2^2"), neg(pow(num(2.0), num(2.0))));
        assert_eq!(p("2^3^2"), pow(num(2.0), pow(num(3.0), num(2.0))));
        assert_eq!(p("2^-1"), pow(num(2.0), neg(num(1.0))));
        assert_eq!(p("(-2)^2"), pow(neg(num(2.0)), num(2.0)));
        assert_eq!(p("2*3^2"), parse_expr(&m, "2*(3^2)").unwrap().expr);
        assert_eq!(p("50%"), num(0.5));
        assert_eq!(p("Price%"), parse_expr(&m, "Price / 100").unwrap().expr);
        assert_eq!(p("2^50%"), pow(num(2.0), num(0.5)));
        assert_eq!(
            p("Price + 1 & \"x\""),
            parse_expr(&m, "(Price + 1) & \"x\"").unwrap().expr
        );
        assert_eq!(
            p("\"a\" & \"b\" == \"ab\""),
            parse_expr(&m, "(\"a\" & \"b\") == \"ab\"").unwrap().expr
        );
        assert_eq!(p("--Price"), neg(neg(refr(PRICE))));
        assert!(parse_expr(&m, &format!("{}1", "-".repeat(10_000))).is_err());
        assert!(parse_expr(&m, "2^").is_err());
    }

    /// NOT binds looser than a comparison and tighter than AND, as in SQL:
    /// `NOT Price > 15` is `NOT (Price > 15)`, and `NOT a AND b` is
    /// `(NOT a) AND b`.
    #[test]
    fn not_binds_looser_than_comparison() {
        let m = fixture();
        assert_eq!(
            parse_expr(&m, "NOT Price > 15").unwrap(),
            parse_expr(&m, "NOT (Price > 15)").unwrap()
        );
        assert_eq!(
            parse_expr(&m, "NOT Price > 15 AND Price < 20").unwrap(),
            parse_expr(&m, "(NOT (Price > 15)) AND Price < 20").unwrap()
        );
        assert_eq!(
            parse_expr(&m, "NOT NOT Price > 15").unwrap(),
            parse_expr(&m, "NOT (NOT (Price > 15))").unwrap()
        );
        // NOT chains count toward the depth limit (the compiler recurses).
        let deep = format!("{}Price > 1", "NOT ".repeat(10_000));
        assert!(parse_expr(&m, &deep).is_err());
        // NOT^n (Price > 1) is n + 2 levels.
        let ok = format!("{}Price > 1", "NOT ".repeat(MAX_FORMULA_DEPTH - 2));
        assert!(parse_expr(&m, &ok).is_ok());
    }

    #[test]
    fn comparison_and_logical() {
        let m = fixture();
        // Price > 10
        let f = parse_expr(&m, "Price > 10").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Gt,
                Box::new(refr(PRICE)),
                Box::new(Expr::Literal(Value::Number(10.0))),
            )
        );

        // NOT (Price > 10)
        let f = parse_expr(&m, "NOT (Price > 10)").unwrap();
        assert!(matches!(f.expr, Expr::UnaryOp(UnaryOp::Not, _)));

        // Price > 10 AND Quantity < 5  -> And(Gt, Lt)
        let f = parse_expr(&m, "Price > 10 AND Quantity < 5").unwrap();
        assert!(matches!(f.expr, Expr::BinaryOp(BinaryOp::And, _, _)));

        // Equality uses '==' inside expressions (not top-level '=').
        let f = parse_expr(&m, "Price == 10").unwrap();
        assert!(matches!(f.expr, Expr::BinaryOp(BinaryOp::Eq, _, _)));
        // '<>' and '!=' both map to Ne.
        assert!(matches!(
            parse_expr(&m, "Price <> 10").unwrap().expr,
            Expr::BinaryOp(BinaryOp::Ne, _, _)
        ));
        assert!(matches!(
            parse_expr(&m, "Price != 10").unwrap().expr,
            Expr::BinaryOp(BinaryOp::Ne, _, _)
        ));
    }

    #[test]
    fn literals() {
        let m = fixture();
        assert_eq!(
            parse_expr(&m, "TRUE").unwrap().expr,
            Expr::Literal(Value::Boolean(true))
        );
        assert_eq!(
            parse_expr(&m, "false").unwrap().expr,
            Expr::Literal(Value::Boolean(false))
        );
        assert_eq!(
            parse_expr(&m, "2.5").unwrap().expr,
            Expr::Literal(Value::Number(2.5))
        );
        assert_eq!(
            parse_expr(&m, "\"hello\"").unwrap().expr,
            Expr::Literal(Value::Text("hello".into()))
        );
    }

    #[test]
    fn unary_neg() {
        let m = fixture();
        let f = parse_expr(&m, "-Price").unwrap();
        assert_eq!(f.expr, Expr::UnaryOp(UnaryOp::Neg, Box::new(refr(PRICE))));
    }

    #[test]
    fn errors_never_panic() {
        let m = fixture();
        // Unknown measure.
        assert!(parse_expr(&m, "Widgets * Price").is_err());
        // Unknown category in dim list.
        assert!(parse_expr(&m, "Price[Region]").is_err());
        // Trailing garbage.
        assert!(parse_expr(&m, "Price Quantity").is_err());
        // Unbalanced paren.
        assert!(parse_expr(&m, "(Price + Quantity").is_err());
        // Missing '=' at top level.
        assert!(parse_formula(&m, "Revenue Price").is_err());
        // Empty.
        assert!(parse_expr(&m, "").is_err());
        // Unterminated string.
        assert!(parse_expr(&m, "\"oops").is_err());
        // A dangling operator.
        assert!(parse_expr(&m, "Price ^").is_err());
        assert!(parse_expr(&m, "Price &").is_err());
    }

    #[test]
    fn error_has_position_and_display() {
        let m = fixture();
        let e = parse_expr(&m, "Price + Widgets").unwrap_err();
        assert!(e.position.is_some());
        assert!(e.to_string().contains("unknown measure"));
    }

    #[test]
    fn parses_named_scalar_call() {
        let m = fixture();
        // ABS(Price) -> Call(FuncId(10), [Ref(Price)]).
        let f = parse_expr(&m, "ABS(Price)").unwrap();
        assert_eq!(f.expr, Expr::Call(FuncId(10), vec![refr(PRICE)]));

        // Two-arg MIN2, case-insensitive, args are full expressions.
        let f = parse_expr(&m, "min2(Price, Cost - Revenue)").unwrap();
        match f.expr {
            Expr::Call(FuncId(20), args) => {
                assert_eq!(args.len(), 2);
                assert_eq!(args[0], refr(PRICE));
            }
            other => panic!("expected MIN2 call, got {other:?}"),
        }
    }

    #[test]
    fn scalar_call_wrong_arity_errors() {
        let m = fixture();
        assert!(parse_expr(&m, "ABS(Price, Cost)").is_err()); // ABS is arity 1
        assert!(parse_expr(&m, "MID(Price)").is_err()); // MID takes 3
        assert!(parse_expr(&m, "ROUND()").is_err()); // ROUND needs at least 1
        let e = parse_expr(&m, "ROUND(Price, 1, 2)").unwrap_err();
        assert!(
            e.message.contains("ROUND(number, [num_digits=0])"),
            "{}",
            e.message
        );
        // MIN2 is MIN's old name: variadic now, so one argument is fine.
        assert_eq!(
            parse_expr(&m, "MIN2(Price)").unwrap().expr,
            Expr::Call(FuncId(20), vec![refr(PRICE)])
        );
        // A scalar-only name with OVER, and an aggregation name without it.
        assert!(parse_expr(&m, "ABS(Price OVER Time)").is_err());
        assert_eq!(
            parse_expr(&m, "SUM(Price, Cost)").unwrap().expr,
            Expr::Call(FuncId(22), vec![refr(PRICE), refr(COST)])
        );
        // Optional arguments, aliases, zero-arg calls.
        assert_eq!(
            parse_expr(&m, "round(Price, 2)").unwrap().expr,
            Expr::Call(
                FuncId(11),
                vec![refr(PRICE), Expr::Literal(Value::Number(2.0))]
            )
        );
        assert_eq!(
            parse_expr(&m, "PI()").unwrap().expr,
            Expr::Call(FuncId(112), vec![])
        );
        assert_eq!(
            parse_expr(&m, "AVG(Price OVER Time)").unwrap().expr,
            parse_expr(&m, "AVERAGE(Price OVER Time)").unwrap().expr
        );
    }

    #[test]
    fn scalar_name_without_paren_is_a_measure_ref() {
        // A bare name matching a scalar func but not followed by "(" is parsed
        // as a measure ref (and errors if unknown) — no false function parse.
        let m = fixture();
        assert!(parse_expr(&m, "ABS").is_err()); // unknown measure "ABS"
    }

    #[test]
    fn definition_falls_through_to_ordinary_formula() {
        let m = fixture();
        match parse_definition(&m, "Revenue = Price * Quantity").unwrap() {
            Definition::Formula(ft) => {
                assert_eq!(ft.target, Name("Revenue".into()));
            }
            other => panic!("expected Formula, got {other:?}"),
        }
    }

    #[test]
    fn parses_date_literal() {
        let m = fixture();
        // Bare date -> midnight UTC.
        let f = parse_expr(&m, "#2025-01-15#").unwrap();
        match f.expr {
            Expr::Literal(Value::DateTime(dt)) => {
                assert_eq!(dt.to_rfc3339(), "2025-01-15T00:00:00+00:00");
            }
            other => panic!("expected date literal, got {other:?}"),
        }
        // Full RFC3339.
        let f = parse_expr(&m, "#2025-01-15T09:30:00Z#").unwrap();
        assert!(matches!(f.expr, Expr::Literal(Value::DateTime(_))));
        // Bad date errors, not panics.
        assert!(parse_expr(&m, "#not-a-date#").is_err());
        assert!(parse_expr(&m, "#2025-01-15").is_err()); // unterminated
    }

    #[test]
    fn definition_parses_sql_form() {
        let m = fixture();
        match parse_definition(&m, r#"Sales = SQL("select region, amount from sales")"#).unwrap() {
            Definition::Sql { target, query } => {
                assert_eq!(target, Name("Sales".into()));
                assert_eq!(query, "select region, amount from sales");
            }
            other => panic!("expected Sql, got {other:?}"),
        }
        // SQL(...) needs exactly one string literal.
        assert!(parse_definition(&m, "X = SQL(Price)").is_err());
        assert!(parse_definition(&m, r#"X = SQL("a", "b")"#).is_err());
    }

    #[test]
    fn definition_parses_call_form() {
        let m = fixture();
        // Zero-arg call.
        match parse_definition(&m, "Now = CALL(now)").unwrap() {
            Definition::Call { target, func, args } => {
                assert_eq!(target, Name("Now".into()));
                assert_eq!(func, "now");
                assert!(args.is_empty());
            }
            other => panic!("expected Call, got {other:?}"),
        }
        // Multi-arg call, argument measures by name (resolved by the caller).
        match parse_definition(&m, "H = CALL(hypot, Price, Quantity)").unwrap() {
            Definition::Call { func, args, .. } => {
                assert_eq!(func, "hypot");
                assert_eq!(args, vec![Name("Price".into()), Name("Quantity".into())]);
            }
            other => panic!("expected Call, got {other:?}"),
        }
        // Malformed arg lists error, not panic.
        assert!(parse_definition(&m, "X = CALL(f,)").is_err());
        assert!(parse_definition(&m, "X = CALL(f, Price").is_err()); // unterminated
        assert!(parse_definition(&m, "X = CALL()").is_err()); // no function name
    }

    #[test]
    fn call_and_sql_are_only_recognized_as_the_whole_rhs() {
        // `CALL`/`SQL` as a sub-expression is NOT a source form; it falls into
        // the expression parser, where an unknown measure named CALL/SQL errors
        // (they are not builtins). This keeps source forms off the engine path.
        let m = fixture();
        assert!(parse_definition(&m, "X = Price + SQL(\"q\")").is_err());
    }

    #[test]
    fn adversarial_inputs_never_panic() {
        // Manual fallback for fuzz/fuzz_targets/fuzz_formula_parser.rs (which
        // needs a nightly toolchain + cargo-fuzz to actually run under
        // libFuzzer): feed a batch of hand-picked adversarial strings through
        // all three parser entry points and assert none of them panics. An
        // Ok or an Err are both fine outcomes; a panic is the only failure.
        let m = fixture();
        let mut inputs: Vec<String> = [
            "",
            "   \t\n  ",
            "((((((((((((((((((((((((((((((((",
            "\"unterminated",
            "'unterminated", // unterminated quoted name
            "''",            // empty quoted name
            "'a''",          // doubled quote then EOF
            "Sheet1.Price",  // dotted qualification (unresolvable today)
            ".",
            "#2025-01-01", // unterminated date literal
            "#not-a-date#",
            "999999999999999999999999999999999999999999999999",
            "1e999999999999999999999999999999",
            "1e",
            "1e+",
            "1E-99999999999999999999",
            "9.9.9e9e9",
            "+++++---***///",
            "CALL(",
            "CALL(f,)",
            "CALL(f, ,)",
            "SQL(",
            "SQL(\"\")",
            "SQL(1)",
            "\u{0}\u{0}\u{0}",
            "\u{1F4A9}\u{1F4A9}\u{1F4A9}", // multi-byte UTF-8 (emoji)
            "Price[Product",
            "Price[,,,]",
            "SUM(Price OVER)",
            "SUM(Price OVER Product OVER Time)",
            "=",
            "X = = =",
            "X=",
            "X = CALL(CALL(CALL(CALL(f))))",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        inputs.push("a".repeat(10_000));
        inputs.push("(".repeat(5_000));
        for s in &inputs {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = parse_expr(&m, s);
                let _ = parse_formula(&m, s);
                let _ = parse_definition(&m, s);
            }))
            .unwrap_or_else(|e| panic!("parser panicked on input {s:?}: {e:?}"));
        }
    }

    #[test]
    fn multi_byte_utf8_does_not_panic_the_tokenizer() {
        // Regression: the tokenizer used to cast a raw BYTE to `char` and step
        // one byte at a time, slicing off a UTF-8 character boundary on any
        // multi-byte input (found via the adversarial-input sweep above with
        // an emoji). Multi-byte identifiers/whitespace/unknown chars must all
        // either tokenize or error cleanly — never panic.
        let m = fixture();
        assert!(parse_expr(&m, "\u{1F4A9}\u{1F4A9}\u{1F4A9}").is_err()); // unknown char, not a crash
                                                                         // A multi-byte char inside what would otherwise be an identifier.
        let _ = parse_expr(&m, "Price\u{00e9}"); // must not panic either way
    }

    // --- quoted identifiers (`'Unit Price'`) ---

    /// A model whose measure/category names are the kind `import_csv` produces
    /// verbatim from a CSV header: spaces, punctuation, leading digits,
    /// non-ASCII, and names that collide with grammar keywords.
    fn odd_names() -> Model {
        let mut m = fixture();
        m.add_category(CategoryId(3), "Fiscal Year");
        for (id, name) in [
            (MeasureId(200), "Unit Price"),
            (MeasureId(201), "Cost-Per-Unit"),
            (MeasureId(202), "Revenue/Unit"),
            (MeasureId(203), "Margin (net)"),
            (MeasureId(204), "2024 Total"),
            (MeasureId(205), "Bob's Rate"),
            (MeasureId(206), "Umsatz \u{20AC} \u{5E74}\u{5EA6}"),
            (MeasureId(207), "Over"),
            (MeasureId(208), "SUM"),
            (MeasureId(209), "NOT"),
        ] {
            m.add_measure(Measure {
                id,
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![TIME, PRODUCT],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        m
    }

    #[test]
    fn quoted_name_resolves_a_measure_with_spaces() {
        // The defect this closes: a CSV header `Unit Price` had no spelling.
        let m = odd_names();
        let f = parse_expr(&m, "'Unit Price' * Quantity").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(refr(MeasureId(200))),
                Box::new(refr(QUANTITY)),
            )
        );
    }

    #[test]
    fn quoted_names_cover_punctuation_digits_utf8_and_keywords() {
        let m = odd_names();
        for (src, id) in [
            ("'Unit Price'", 200),
            ("'Cost-Per-Unit'", 201),
            ("'Revenue/Unit'", 202),
            ("'Margin (net)'", 203),
            ("'2024 Total'", 204),
            ("'Umsatz \u{20AC} \u{5E74}\u{5EA6}'", 206),
            // Keyword collisions: quoting makes them names, not syntax.
            ("'Over'", 207),
            ("'SUM'", 208),
            ("'NOT'", 209),
        ] {
            assert_eq!(
                parse_expr(&m, src).unwrap().expr,
                refr(MeasureId(id)),
                "{src}"
            );
        }
        // And they compose: a quoted keyword-named measure inside an expression
        // is not read as an operator.
        assert!(matches!(
            parse_expr(&m, "'NOT' + 'SUM'").unwrap().expr,
            Expr::BinaryOp(BinaryOp::Add, _, _)
        ));
        // `SUM` quoted is a ref even directly before '(' — quoting wins.
        assert!(parse_expr(&m, "'SUM'(Price)").is_err()); // trailing '(', not a call
    }

    #[test]
    fn doubled_quote_escapes_an_embedded_quote() {
        // Chosen escape: `''` inside a quoted name is one literal `'`
        // (Quantrix/SQL convention).
        let m = odd_names();
        let f = parse_expr(&m, "'Bob''s Rate' + Price").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Add,
                Box::new(refr(MeasureId(205))),
                Box::new(refr(PRICE)),
            )
        );
        // Without the doubling it is a *different* (unknown) name, and the
        // trailing `s Rate'` opens an unterminated quote.
        assert!(parse_expr(&m, "'Bob's Rate'").is_err());
    }

    #[test]
    fn quoted_names_work_everywhere_a_name_does() {
        let m = odd_names();
        // Target of an assignment.
        let f = parse_formula(&m, "'Gross Margin' = 'Unit Price' - 'Cost-Per-Unit'").unwrap();
        assert_eq!(f.target, Name("Gross Margin".into()));
        // Dimension list and aggregation category.
        let f = parse_expr(&m, "'Unit Price'[Time, Product]").unwrap();
        assert_eq!(
            f.expr,
            Expr::Ref(
                MeasureId(200),
                DimensionSpec {
                    by: vec![TIME, PRODUCT],
                    over: vec![],
                    except: vec![],
                }
            )
        );
        let f = parse_expr(&m, "SUM('Unit Price' OVER 'Fiscal Year')").unwrap();
        assert_eq!(
            f.expr,
            Expr::Call(
                FUNC_SUM,
                vec![Expr::Ref(
                    MeasureId(200),
                    DimensionSpec {
                        over: vec![CategoryId(3)],
                        by: vec![],
                        except: vec![],
                    }
                )]
            )
        );
        // Scalar call argument, and CALL(...) argument measures.
        assert!(parse_expr(&m, "ABS('Unit Price')").is_ok());
        match parse_definition(&m, "X = CALL(f, 'Unit Price')").unwrap() {
            Definition::Call { args, .. } => {
                assert_eq!(args, vec![Name("Unit Price".into())]);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn quoted_name_errors_are_clear_not_panics() {
        let m = odd_names();
        // Unknown quoted measure / category name.
        let e = parse_expr(&m, "'No Such Measure' * Price").unwrap_err();
        assert!(
            e.to_string().contains("unknown measure: No Such Measure"),
            "{e}"
        );
        assert_eq!(e.position, Some(0));
        let e = parse_expr(&m, "SUM(Price OVER 'No Such Category')").unwrap_err();
        assert!(
            e.to_string().contains("unknown category: No Such Category"),
            "{e}"
        );
        // Unterminated and empty quoted names.
        let e = parse_expr(&m, "'Unit Price").unwrap_err();
        assert!(e.to_string().contains("unterminated quoted name"), "{e}");
        let e = parse_expr(&m, "'' + Price").unwrap_err();
        assert!(e.to_string().contains("empty quoted name"), "{e}");
    }

    /// `""` inside a string is one `"`, as in Excel.
    #[test]
    fn doubled_double_quote_in_text() {
        let m = fixture();
        let t = |s: &str| parse_expr(&m, s).map(|f| f.expr);
        let lit = |s: &str| Ok(Expr::Literal(Value::Text(s.into())));
        assert_eq!(t(r#""a""b""#), lit(r#"a"b"#));
        assert_eq!(t(r#""""""#), lit(r#"""#));
        assert_eq!(t(r#""""#), lit(""));
        assert_eq!(t(r#""say ""hi"", é""#), lit(r#"say "hi", é"#));
        assert!(matches!(
            t(r#""a" & "b""#),
            Ok(Expr::BinaryOp(BinaryOp::Concat, ..))
        ));
        assert!(t(r#""a"""#).is_err(), "unterminated after a doubled quote");
        assert!(t(r#""a" "b""#).is_err(), "a space is no escape");
    }

    #[test]
    fn double_quotes_are_still_text_literals() {
        // `'` introduces a NAME, `"` a text literal. Adding one must not have
        // disturbed the other.
        let m = odd_names();
        assert_eq!(
            parse_expr(&m, "\"Unit Price\"").unwrap().expr,
            Expr::Literal(Value::Text("Unit Price".into()))
        );
        // A single quote inside a text literal is just a character.
        assert_eq!(
            parse_expr(&m, "\"it's\"").unwrap().expr,
            Expr::Literal(Value::Text("it's".into()))
        );
    }

    #[test]
    fn dotted_qualification_parses_but_names_the_unknown_qualifier() {
        // Improv has no matrix namespace yet (GUI plan Step 3), so the only
        // honest resolution is a clear error naming the qualifier — not the old
        // `unexpected character: '.'`.
        let m = odd_names();
        for src in [
            "Sheet1.Price",
            "'Defined Input & Outputs'.'Income tax rate'",
            "Price + Sheet1.Quantity",
            "SUM(Price OVER Sheet1.Time)",
        ] {
            let e = parse_expr(&m, src).unwrap_err();
            assert!(e.to_string().contains("unknown qualifier"), "{src}: {e}");
        }
        // The qualifier is named verbatim, quoted or not.
        let e = parse_expr(&m, "'Defined Input & Outputs'.'Income tax rate'").unwrap_err();
        assert!(e.to_string().contains("Defined Input & Outputs"), "{e}");
        // A dot with no name after it is also an error, never a panic.
        assert!(parse_expr(&m, "Price.").is_err());
        assert!(parse_expr(&m, ".Price").is_err());
        // Chains do not recurse: exactly one dot is consumed, then it errors.
        assert!(parse_expr(&m, &"a.".repeat(5_000)).is_err());
    }

    #[test]
    fn quoted_and_dotted_adversarial_inputs_never_panic() {
        // Companion to `adversarial_inputs_never_panic`, for the quoting and
        // qualification additions. Ok or Err are both fine; a panic is not.
        let m = odd_names();
        let mut inputs: Vec<String> = [
            "'",
            "''",
            "'''",
            "''''",
            "'''''",
            "'unterminated",
            "'a\u{20AC}",  // unterminated, multi-byte
            "'\u{1F4A9}'", // quoted emoji name (unknown measure)
            "'\u{1F4A9}",  // unterminated after multi-byte
            "'a''",        // doubling then EOF
            "'\u{0}'",
            "'Unit Price'[",
            "'Unit Price'[']",
            ".",
            "..",
            "a..b",
            "'a'.'b'.'c'",
            "X = 'a'.'b'",
            "X = ''",
            "SUM('a' OVER 'b')",
            "CALL(f, 'a'",
            "SQL('q')",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        inputs.push(format!("'{}", "a".repeat(10_000))); // unterminated, long
        inputs.push("'a''".repeat(5_000)); // doubling storm
        inputs.push(".".repeat(5_000));
        for s in &inputs {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = parse_expr(&m, s);
                let _ = parse_formula(&m, s);
                let _ = parse_definition(&m, s);
            }))
            .unwrap_or_else(|e| panic!("parser panicked on input {s:?}: {e:?}"));
        }
    }

    /// Exponent literals, their errors, and names that start with `e`.
    #[test]
    fn exponent_literals() {
        let mut m = fixture();
        for (id, name) in [(120, "E"), (121, "e1")] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let p = |s: &str| parse_expr(&m, s).map(|f| f.expr);
        let num = |n: f64| Expr::Literal(Value::Number(n));
        let mul = |a, b| Expr::BinaryOp(BinaryOp::Mul, Box::new(a), Box::new(b));
        for (text, n) in [
            ("1e-10", 1e-10),
            ("2.5E+3", 2500.0),
            ("6.02e23", 6.02e23),
            ("1E3", 1000.0),
            ("0e0", 0.0),
            ("1e-999", 0.0), // underflow reads as 0
        ] {
            assert_eq!(p(text), Ok(num(n)), "{text}");
        }
        // A name never starts with a digit: `2e1` is 20, `2 * e1` a name.
        let (e, e1) = (refr(MeasureId(120)), refr(MeasureId(121)));
        assert_eq!(p("2e1"), Ok(num(20.0)));
        assert_eq!(p("2 * e1"), Ok(mul(num(2.0), e1.clone())));
        assert_eq!(p("E * e1"), Ok(mul(e, e1)));
        assert!(p("e").unwrap_err().message.contains("unknown measure: e"));
        for (text, msg, pos) in [
            ("1e", "exponent without digits: 1e", 0),
            ("1e+", "exponent without digits: 1e+", 0),
            ("Price * 2.5E-", "exponent without digits: 2.5E-", 8),
            ("2else", "exponent without digits: 2e", 0),
            ("1e999", "number out of range: 1e999", 0),
            ("1 + 2e308", "number out of range: 2e308", 4),
        ] {
            let err = p(text).unwrap_err();
            assert!(err.message.starts_with(msg), "{text}: {}", err.message);
            assert_eq!(err.position, Some(pos), "{text}");
        }
    }

    #[test]
    fn number_literals_print_and_read_back() {
        for (n, text) in [
            (0.0, "0"),
            (42.0, "42"),
            (1.07, "1.07"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (1e-10, "1e-10"),
            (6.02e23, "6.02e23"),
            (1e21, "1e21"),
            (123456789012345680000.0, "123456789012345680000"),
            (f64::MAX, "1.7976931348623157e308"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(number_literal(n).as_deref(), Some(text));
            let f = parse_expr(&Model::new(), text).unwrap();
            assert_eq!(f.expr, Expr::Literal(Value::Number(n)), "{text}");
        }
        for n in [-1.0, -0.0, f64::INFINITY, f64::NAN] {
            assert_eq!(number_literal(n), None, "{n}");
        }
    }

    #[test]
    fn deeply_nested_parens_error_instead_of_overflowing_the_stack() {
        // Regression: recursive-descent `parse_primary` recursed once per '(',
        // so thousands of nested opens overflowed the stack (found via the
        // adversarial-input sweep). A bounded depth now errors cleanly.
        let m = fixture();
        let deep = "(".repeat(5_000);
        assert!(parse_expr(&m, &deep).is_err());
        // A reasonable nesting depth still works fine.
        let reasonable = format!("{}Price{}", "(".repeat(50), ")".repeat(50));
        assert!(parse_expr(&m, &reasonable).is_ok());
    }

    /// A formula over Price whose tree is `depth` levels deep, in each shape
    /// the depth limit has to catch.
    fn deep(shape: &str, depth: usize) -> String {
        match shape {
            "+" | "*" => vec!["Price"; depth].join(&format!(" {shape} ")),
            // NOT^n (Price > 1) is n + 2 levels; Price%^n is n + 1.
            "NOT" => format!("{}Price > 1", "NOT ".repeat(depth - 2)),
            "%" => format!("Price{}", "%".repeat(depth - 1)),
            "ABS" => format!("{}Price{}", "ABS(".repeat(depth - 1), ")".repeat(depth - 1)),
            "-" => format!("{}Price", "- ".repeat(depth - 1)),
            // Each IF's test (Price > 1) is two levels below it.
            "IF" => format!(
                "{}Price{}",
                "IF(Price > 1, ".repeat(depth - 2),
                ", Price)".repeat(depth - 2)
            ),
            // IF(Price == 1, 1, IF(…, 0)): one level per case, plus the test.
            "SWITCH" => {
                let cases: String = (1..=depth - 2).map(|k| format!(", {k}, {k}")).collect();
                format!("SWITCH(Price{cases}, 0)")
            }
            _ => unreachable!("{shape}"),
        }
    }

    #[test]
    fn formulas_deeper_than_the_limit_are_refused() {
        let m = fixture();
        let want = format!(
            "formula nests too deeply ({} levels; the limit is {MAX_FORMULA_DEPTH})",
            MAX_FORMULA_DEPTH + 1
        );
        for shape in ["+", "*", "ABS", "-", "IF", "SWITCH", "NOT", "%"] {
            let ok = deep(shape, MAX_FORMULA_DEPTH);
            let f = parse_expr(&m, &ok).unwrap_or_else(|e| panic!("{shape}: {e}"));
            assert_eq!(f.expr.depth_and_nodes().0, MAX_FORMULA_DEPTH, "{shape}");
            let def = format!("Q = {ok}");
            assert!(parse_formula(&m, &def).is_ok(), "{shape}");
            assert!(parse_definition(&m, &def).is_ok(), "{shape}");

            let over = deep(shape, MAX_FORMULA_DEPTH + 1);
            let def = format!("Q = {over}");
            // A chain stops at the operator that passes the limit; any other
            // shape is refused once parsed, at the start of the expression.
            let at = match shape {
                "+" | "*" | "%" => over.rfind(shape).unwrap(),
                _ => 0,
            };
            for (e, at) in [
                (parse_expr(&m, &over).map(drop), at),
                (parse_formula(&m, &def).map(drop), at + 4),
                (parse_definition(&m, &def).map(drop), at + 4),
            ] {
                let e = e.unwrap_err();
                assert!(e.is_limit(), "{shape}");
                assert_eq!(e.message, want, "{shape}");
                assert_eq!(e.position, Some(at), "{shape}");
            }
        }
        // Each chain operator, `%`, LET values and inline LAMBDA bodies.
        for (op, text) in [
            ("or", vec!["TRUE"; MAX_FORMULA_DEPTH + 1].join(" OR ")),
            ("and", vec!["TRUE"; MAX_FORMULA_DEPTH + 1].join(" AND ")),
            ("&", vec!["\"a\""; MAX_FORMULA_DEPTH + 1].join(" & ")),
            ("/", vec!["Price"; MAX_FORMULA_DEPTH + 1].join(" / ")),
            ("%", format!("Price{}", "%".repeat(MAX_FORMULA_DEPTH))),
            (
                "LET",
                format!("LET(x, {}, 1)", deep("ABS", MAX_FORMULA_DEPTH + 1)),
            ),
            (
                "LAMBDA",
                format!("LAMBDA(x, {})(1)", deep("ABS", MAX_FORMULA_DEPTH + 1)),
            ),
        ] {
            let e = parse_expr(&m, &text).unwrap_err();
            assert_eq!(e.message, want, "{op}");
            assert!(e.position.is_some(), "{op}");
        }
        // A named LAMBDA's body is checked too.
        let body = deep("ABS", MAX_FORMULA_DEPTH + 1).replace("Price", "x");
        let e = parse_definition(&m, &format!("F = LAMBDA(x, {body})")).unwrap_err();
        assert_eq!(e.message, want);
    }

    #[test]
    fn huge_formulas_error_on_a_small_stack() {
        // The tree is checked as it is built, so none of these builds one too
        // deep to drop (dropping a 200k-term chain overflowed 2 MiB). 1 MiB:
        // a Windows main thread.
        let r = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let m = fixture();
                let group = format!("({})", vec!["Price"; MAX_FORMULA_DEPTH - 1].join(" + "));
                let n = MAX_PARSE_DEPTH + 1;
                let texts = [
                    vec!["Price"; 200_000].join(" + "),
                    vec!["Price"; 200_000].join(" * "),
                    format!("Price{}", "%".repeat(200_000)),
                    format!("{}Price{}", "(Price + ".repeat(n), ")".repeat(n)),
                    vec![group.as_str(); 1_000].join(" + "),
                    format!("{}Price{}", "ABS(".repeat(n), ")".repeat(n)),
                    format!("{}Price{}", "(".repeat(100_000), ")".repeat(100_000)),
                ];
                let chains = &texts[..3];
                texts.iter().all(|t| parse_expr(&m, t).is_err())
                    && chains.iter().all(|t| parse_stored_expr(&m, t).is_err())
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(r);
    }

    #[test]
    fn stored_text_formulas_load_up_to_the_stored_bound() {
        let m = fixture();
        // A 45-term sum (refused by 0.11, which allowed 40) and 0.10's
        // nesting (100) load as stored text, and are fine as new ones too.
        let sum = vec!["Price"; 45].join(" + ");
        let f = parse_stored_expr(&m, &sum).unwrap();
        assert_eq!(f.expr.depth_and_nodes().0, 45);
        assert_eq!(parse_expr(&m, &sum).unwrap(), f);
        let def = format!("Q = {sum}");
        assert!(parse_definition(&m, &def).is_ok());
        assert!(matches!(
            parse_stored_definition(&m, &def),
            Ok(Definition::Formula(_))
        ));
        let nested = format!("{}Price{}", "ABS(".repeat(100), ")".repeat(100));
        assert!(parse_expr(&m, &nested).is_ok());
        assert!(parse_stored_expr(&m, &nested).is_ok());
        // Past the stored bound, it is refused too, naming that bound.
        let over = vec!["Price"; MAX_STORED_FORMULA_DEPTH + 1].join(" + ");
        let e = parse_stored_expr(&m, &over).unwrap_err();
        assert!(e.is_limit());
        assert_eq!(
            e.message,
            format!(
                "formula nests too deeply ({} levels; the limit is {MAX_STORED_FORMULA_DEPTH})",
                MAX_STORED_FORMULA_DEPTH + 1
            )
        );
        // A stored named LAMBDA too.
        let at = |n: usize| {
            let body = format!("{}x{}", "ABS(".repeat(n), ")".repeat(n));
            format!("F = LAMBDA(x, {body})")
        };
        let n = MAX_STORED_FORMULA_DEPTH - 1;
        assert!(matches!(
            parse_stored_definition(&m, &at(n)),
            Ok(Definition::Lambda(_))
        ));
        assert!(parse_stored_definition(&m, &at(n + 1))
            .unwrap_err()
            .is_limit());
    }

    #[test]
    fn a_new_formula_is_limited_after_named_lambdas_expand() {
        let mut m = fixture();
        let body = format!("{}x{}", "ABS(".repeat(129), ")".repeat(129));
        let Ok(Definition::Lambda(l)) = parse_definition(&m, &format!("F = LAMBDA(x, {body})"))
        else {
            panic!()
        };
        m.lambdas.insert(1, l.lambda);
        // F(Price) expands to 130 levels; F(F(Price)) to 259.
        assert!(parse_expr(&m, "F(Price)").is_ok());
        let e = parse_expr(&m, "F(F(Price))").unwrap_err();
        assert_eq!(
            e.message,
            "formula nests too deeply (259 levels; the limit is 256)"
        );
        assert_eq!(e.position, Some(0));
        // A stored one, expanded when compiled, is held to the stored bound.
        assert!(parse_stored_expr(&m, "F(F(Price))").is_ok());
        let deep = format!("{}Price{}", "F(".repeat(3), ")".repeat(3));
        let e = m
            .expand_lambdas(&parse_stored_expr(&m, &deep).unwrap().expr)
            .unwrap_err();
        assert!(e.contains("the limit is 256"), "{e}");
    }

    #[test]
    fn a_let_that_doubles_fails_fast() {
        // a1 = a0 + a0, a2 = a1 + a1, …: 2^24 nodes took 13 s to build.
        let m = fixture();
        let mut text = String::from("LET(a0, Price");
        for k in 1..=24 {
            text += &format!(", a{k}, a{0} + a{0}", k - 1);
        }
        text += ", a24)";
        let t = std::time::Instant::now();
        let e = parse_expr(&m, &text).unwrap_err();
        assert!(
            t.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            t.elapsed()
        );
        assert!(e.message.starts_with("formula is too large"), "{e}");
        assert!(e.is_limit());
        assert!(!parse_expr(&m, "1 +").unwrap_err().is_limit());
        assert!(e.position.is_some());
        // The same chain, short enough, is fine.
        assert!(parse_expr(&m, "LET(a0, Price, a1, a0 + a0, a2, a1 + a1, a2)").is_ok());
    }
}

//! Textual formula parser: `"Revenue = Price * Quantity"` -> [`Formula`].
//!
//! Hand-rolled tokenizer + recursive-descent parser (same shape as
//! `improv_nl_formula`), for the *symbolic* v1 grammar in
//! `AGENT_FORMULA_LANGUAGE.md` §4. Measure/category names resolve against a
//! [`Model`] (like `NlContext`); unknown names are a clean [`ParseError`],
//! never a panic.
//!
//! # Grammar (v1 subset of §4, plus named scalar calls; no `SQL("...")`)
//!
//! ```ebnf
//! Formula      = Identifier "=" Expression ;
//! Expression   = OrExpr ;
//! OrExpr       = AndExpr { "OR" AndExpr } ;
//! AndExpr      = Comparison { "AND" Comparison } ;
//! Comparison   = Additive [ ("==" | "<>" | "!=" | "<" | "<=" | ">" | ">=") Additive ] ;
//! Additive     = Term { ("+" | "-") Term } ;
//! Term         = Factor { ("*" | "/") Factor } ;
//! Factor       = Primary ;                               (* '^' power reserved, not in v1 AST *)
//! Primary      = Literal
//!              | Aggregation
//!              | MeasureRef
//!              | "(" Expression ")"
//!              | ("-" | "NOT") Primary ;
//! MeasureRef   = Name [ "[" DimList "]" ] ;                (* DimList -> DimensionSpec.by *)
//! DimList      = Name { "," Name } ;
//! Aggregation  = AggFunc "(" MeasureRef "OVER" Name ")" ;
//! AggFunc      = "SUM" | "AVG" | "MIN" | "MAX" ;
//! Literal      = Number | "TRUE" | "FALSE" | '"' text '"' ;
//!
//! Name         = [ Identifier "." ] Identifier ;           (* qualifier: see below *)
//! Identifier   = BareIdent | QuotedIdent ;
//! BareIdent    = (alpha | "_") { alnum | "_" } ;
//! QuotedIdent  = "'" ( char-except-quote | "''" )+ "'" ;
//! ```
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
//! is set. Func ids: SUM=1, AVG=2, MIN=3, MAX=4.
//!
//! ## Not implemented (deferred, per §4/§11.3)
//!
//! * (Date literals `#2025-01-01#` and `#...T...Z#` ARE supported — see
//!   `parse_date_literal`.)
//! * Nothing else material: named scalar calls (`ABS`, `SQRT`, `MIN2`, …) and
//!   the whole-RHS source forms `CALL(...)` / `SQL("...")` (via
//!   `parse_definition`) are supported.

use crate::formula::{BinaryOp, DimensionSpec, Expr, Formula, FuncId, UnaryOp};
use crate::ids::{CategoryId, MeasureId, Name};
use crate::value::Value;
use crate::Model;

pub const FUNC_SUM: FuncId = FuncId(1);
pub const FUNC_AVG: FuncId = FuncId(2);
pub const FUNC_MIN: FuncId = FuncId(3);
pub const FUNC_MAX: FuncId = FuncId(4);

/// Named scalar built-in functions callable as `NAME(args...)` in a formula.
///
/// These ids and arities MUST match the engine's scalar registry
/// (`improv_engine::compiler::scalar_arity`). This is the deterministic,
/// in-process function surface; the Phase 6 external-language `CALL(...)` form
/// plugs additional runtimes into the same `Expr::Call` seam.
///
/// Returns `(FuncId, arity)` for a recognized name (case-insensitive).
pub fn scalar_func(name: &str) -> Option<(FuncId, usize)> {
    let (id, arity) = match name.to_ascii_uppercase().as_str() {
        "ABS" => (10, 1),
        "ROUND" => (11, 1),
        "FLOOR" => (12, 1),
        "CEIL" => (13, 1),
        "SQRT" => (14, 1),
        "NEG" => (15, 1),
        "MIN2" => (20, 2),
        "MAX2" => (21, 2),
        _ => return None,
    };
    Some((FuncId(id), arity))
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
                // String literal: consume to the closing quote (no escapes in v1).
                i += 1;
                let s0 = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(ParseError::new("unterminated string literal", Some(start)));
                }
                let s = text[s0..i].to_string();
                i += 1; // closing quote
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
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                let raw = &text[start..i];
                let n = raw
                    .parse::<f64>()
                    .map_err(|_| ParseError::new(format!("invalid number: {raw}"), Some(start)))?;
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
            c if c.is_alphabetic() || c == '_' => {
                i += c.len_utf8();
                while i < bytes.len() {
                    let b = text[i..].chars().next().expect("i < len");
                    if b.is_alphanumeric() || b == '_' {
                        i += b.len_utf8();
                    } else {
                        break;
                    }
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
                    Some("<=") | Some(">=") | Some("<>") | Some("==") | Some("!=") => {
                        i += 2;
                        two.unwrap().to_string()
                    }
                    _ => {
                        let single = "+-*/^=<>()[],.".find(c);
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

struct Parser<'a> {
    toks: &'a [Spanned],
    pos: usize,
    model: &'a Model,
    /// Current recursive-descent nesting depth (parens / unary chains).
    /// Bounded so adversarial input (e.g. thousands of nested `(`) errors
    /// cleanly instead of overflowing the stack.
    depth: usize,
}

/// Maximum recursive-descent nesting depth for a single formula. Generous for
/// any real formula; small enough to never approach the stack limit.
const MAX_PARSE_DEPTH: usize = 200;

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
        self.model
            .category_by_name(name)
            .map(|c| c.id)
            .ok_or_else(|| ParseError::new(format!("unknown category: {name}"), pos))
    }

    // --- grammar ---

    /// Expression = OrExpr
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    /// OrExpr = AndExpr { "OR" AndExpr }
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.peek_kw("or") {
            self.bump();
            let rhs = self.parse_and()?;
            lhs = Expr::BinaryOp(BinaryOp::Or, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// AndExpr = Comparison { "AND" Comparison }
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        while self.peek_kw("and") {
            self.bump();
            let rhs = self.parse_comparison()?;
            lhs = Expr::BinaryOp(BinaryOp::And, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// Comparison = Additive [ cmp-op Additive ]  (non-associative)
    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_additive()?;
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
                let rhs = self.parse_additive()?;
                Ok(Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs)))
            }
            None => Ok(lhs),
        }
    }

    /// Additive = Term { ("+"|"-") Term }
    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" => BinaryOp::Add,
                Some(Tok::Op(o)) if o == "-" => BinaryOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_term()?;
            lhs = Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// Term = Factor { ("*"|"/") Factor }
    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_factor()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "*" => BinaryOp::Mul,
                Some(Tok::Op(o)) if o == "/" => BinaryOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_factor()?;
            lhs = Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// Factor = Primary [ "^" Factor ]   (right-associative power)
    ///
    /// The AST has no `Pow` `BinaryOp` variant, so `^` is rejected with a clear
    /// error rather than mislowered. Grammar slot kept for forward-compat.
    fn parse_factor(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_primary()?;
        if matches!(self.peek(), Some(Tok::Op(o)) if o == "^") {
            return Err(self.err("'^' (power) is not supported by the v1 AST"));
        }
        Ok(base)
    }

    /// Primary = Literal | Aggregation | MeasureRef | "(" Expression ")"
    ///         | ("-"|"NOT") Primary
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth -= 1;
            return Err(self.err("expression nested too deeply"));
        }
        let result = self.parse_primary_inner();
        self.depth -= 1;
        result
    }

    fn parse_primary_inner(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(Tok::Op(o)) if o == "-" => {
                self.bump();
                let inner = self.parse_primary()?;
                Ok(Expr::UnaryOp(UnaryOp::Neg, Box::new(inner)))
            }
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

    /// Identifier-headed primary: NOT, TRUE/FALSE, aggregation, or a measure ref.
    fn parse_ident_primary(&mut self) -> Result<Expr, ParseError> {
        // Peek the identifier text without holding the borrow.
        let word = match self.peek() {
            Some(Tok::Ident(w)) => w.clone(),
            _ => return Err(self.err("expected identifier")),
        };

        if word.eq_ignore_ascii_case("not") {
            self.bump();
            let inner = self.parse_primary()?;
            return Ok(Expr::UnaryOp(UnaryOp::Not, Box::new(inner)));
        }
        if word.eq_ignore_ascii_case("true") {
            self.bump();
            return Ok(Expr::Literal(Value::Boolean(true)));
        }
        if word.eq_ignore_ascii_case("false") {
            self.bump();
            return Ok(Expr::Literal(Value::Boolean(false)));
        }
        if let Some(func) = agg_func(&word) {
            // Aggregation only when directly followed by "(" — otherwise treat
            // "SUM" etc. as an ordinary (if unusual) measure name.
            if matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "(") {
                return self.parse_aggregation(func);
            }
        }
        if let Some((func, arity)) = scalar_func(&word) {
            // Named scalar call `NAME(args...)` only when directly followed by
            // "("; otherwise the word is an ordinary measure name.
            if matches!(self.toks.get(self.pos + 1).map(|s| &s.tok), Some(Tok::Op(o)) if o == "(") {
                return self.parse_scalar_call(&word, func, arity);
            }
        }
        self.parse_measure_ref()
    }

    /// A named scalar function call: `NAME(expr, expr, ...)`. Arity is checked
    /// against the registry so a bad call is a clear parse error.
    fn parse_scalar_call(
        &mut self,
        name: &str,
        func: FuncId,
        arity: usize,
    ) -> Result<Expr, ParseError> {
        self.bump(); // function name
        if !self.eat_op("(") {
            return Err(self.err("expected '(' after function name"));
        }
        let mut args = Vec::new();
        if !matches!(self.toks.get(self.pos).map(|s| &s.tok), Some(Tok::Op(o)) if o == ")") {
            loop {
                args.push(self.parse_expr()?);
                if self.eat_op(",") {
                    continue;
                }
                break;
            }
        }
        if !self.eat_op(")") {
            return Err(self.err("expected ')' to close function call"));
        }
        if args.len() != arity {
            return Err(self.err(format!(
                "{name} takes {arity} argument(s), got {}",
                args.len()
            )));
        }
        Ok(Expr::Call(func, args))
    }

    /// Aggregation = AggFunc "(" MeasureRef "OVER" Identifier ")"
    fn parse_aggregation(&mut self, func: FuncId) -> Result<Expr, ParseError> {
        self.bump(); // AggFunc
        if !self.eat_op("(") {
            return Err(self.err("expected '(' after aggregation function"));
        }
        // Inner measure ref (no dim-list bracket expected here, but allow it).
        let arg = self.parse_measure_ref()?;
        if !self.peek_kw("over") {
            return Err(self.err("expected 'OVER' in aggregation"));
        }
        self.bump(); // OVER
        let cat = self.parse_category_name()?;
        if !self.eat_op(")") {
            return Err(self.err("expected ')' to close aggregation"));
        }
        // Attach the collapsed category to the arg ref's DimensionSpec.over.
        let arg = match arg {
            Expr::Ref(id, mut spec) => {
                spec.over.push(cat);
                Expr::Ref(id, spec)
            }
            _ => return Err(self.err("aggregation argument must be a measure reference")),
        };
        Ok(Expr::Call(func, vec![arg]))
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

fn agg_func(word: &str) -> Option<FuncId> {
    if word.eq_ignore_ascii_case("sum") {
        Some(FUNC_SUM)
    } else if word.eq_ignore_ascii_case("avg") {
        Some(FUNC_AVG)
    } else if word.eq_ignore_ascii_case("min") {
        Some(FUNC_MIN)
    } else if word.eq_ignore_ascii_case("max") {
        Some(FUNC_MAX)
    } else {
        None
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
    let mut p = Parser {
        toks: &toks[rhs_start..],
        pos: 0,
        model,
        depth: 0,
    };
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(FormulaText {
        target,
        formula: Formula::new(expr),
    })
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
    }
    // Fall through to an ordinary formula expression.
    let mut p = Parser {
        toks: &toks[rhs..],
        pos: 0,
        model,
        depth: 0,
    };
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(Definition::Formula(FormulaText {
        target,
        formula: Formula::new(expr),
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
    let toks = tokenize(text)?;
    if toks.is_empty() {
        return Err(ParseError::new("empty expression", None));
    }
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        model,
        depth: 0,
    };
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(Formula::new(expr))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
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
        // Power is rejected (no AST variant).
        assert!(parse_expr(&m, "Price ^ Quantity").is_err());
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
        assert!(parse_expr(&m, "MIN2(Price)").is_err()); // MIN2 is arity 2
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
}

//! Textual formula parser: `"Revenue = Price * Quantity"` -> [`Formula`].
//!
//! Hand-rolled tokenizer + recursive-descent parser (same shape as
//! `improv_nl_formula`), for the *symbolic* v1 grammar in
//! `AGENT_FORMULA_LANGUAGE.md` §4. Measure/category names resolve against a
//! [`Model`] (like `NlContext`); unknown names are a clean [`ParseError`],
//! never a panic.
//!
//! # Grammar (v1 subset of §4 — no `CALL(...)`/`SQL("...")`)
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
//! MeasureRef   = Identifier [ "[" DimList "]" ] ;         (* DimList -> DimensionSpec.by *)
//! DimList      = Identifier { "," Identifier } ;
//! Aggregation  = AggFunc "(" MeasureRef "OVER" Identifier ")" ;
//! AggFunc      = "SUM" | "AVG" | "MIN" | "MAX" ;
//! Literal      = Number | "TRUE" | "FALSE" | '"' text '"' ;
//! ```
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
//! * Date literals `#2025-01-01#` (spec §3.3, optional here).
//! * `CALL(...)` / `SQL("...")` forms (later phases).

use crate::formula::{BinaryOp, DimensionSpec, Expr, Formula, FuncId, UnaryOp};
use crate::ids::{CategoryId, MeasureId, Name};
use crate::value::Value;
use crate::Model;

pub const FUNC_SUM: FuncId = FuncId(1);
pub const FUNC_AVG: FuncId = FuncId(2);
pub const FUNC_MIN: FuncId = FuncId(3);
pub const FUNC_MAX: FuncId = FuncId(4);

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

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Number(f64),
    Str(String),
    /// A punctuation/operator lexeme (`+`, `<=`, `==`, `[`, ...).
    Op(String),
}

/// A token plus the byte offset where it began (for error positions).
#[derive(Debug, Clone)]
struct Spanned {
    tok: Tok,
    pos: usize,
}

fn tokenize(text: &str) -> Result<Vec<Spanned>, ParseError> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;
        match c {
            c if c.is_whitespace() => i += 1,
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
            c if c.is_alphabetic() || c == '_' => {
                while i < bytes.len() && {
                    let b = bytes[i] as char;
                    b.is_alphanumeric() || b == '_'
                } {
                    i += 1;
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
                        let single = "+-*/^=<>()[],".find(c);
                        if single.is_none() {
                            return Err(ParseError::new(
                                format!("unexpected character: {c:?}"),
                                Some(start),
                            ));
                        }
                        i += 1;
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
}

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

    /// Case-insensitive keyword match on an identifier token (without consuming).
    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(kw))
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError::new(msg, self.peek_pos())
    }

    // --- resolution ---

    fn measure_id(&self, name: &str) -> Result<MeasureId, ParseError> {
        self.model
            .measure_by_name(name)
            .map(|m| m.id)
            .ok_or_else(|| self.err(format!("unknown measure: {name}")))
    }

    fn category_id(&self, name: &str) -> Result<CategoryId, ParseError> {
        self.model
            .category_by_name(name)
            .map(|c| c.id)
            .ok_or_else(|| self.err(format!("unknown category: {name}")))
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
            Some(Tok::Ident(_)) => self.parse_ident_primary(),
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
        self.parse_measure_ref()
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

    /// MeasureRef = Identifier [ "[" DimList "]" ]
    fn parse_measure_ref(&mut self) -> Result<Expr, ParseError> {
        let name = match self.peek() {
            Some(Tok::Ident(w)) => w.clone(),
            _ => return Err(self.err("expected a measure name")),
        };
        let id = self.measure_id(&name)?;
        self.bump();

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
        let name = match self.peek() {
            Some(Tok::Ident(w)) => w.clone(),
            _ => return Err(self.err("expected a category name")),
        };
        let id = self.category_id(&name)?;
        self.bump();
        Ok(id)
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
    if toks.is_empty() {
        return Err(ParseError::new("empty formula", None));
    }
    // LHS: Identifier [ "[" DimList "]" ] "=".  The target may carry a
    // dimension list (e.g. `TotalRevenue[Product] = ...`); it declares the
    // target measure's own dims, which live in the measure metadata, so we
    // parse and discard it here and return only the name.
    let target = match &toks[0].tok {
        Tok::Ident(w) => Name(w.clone()),
        _ => {
            return Err(ParseError::new(
                "formula must start with a target measure name",
                Some(toks[0].pos),
            ))
        }
    };
    // Find the assignment `=`, skipping an optional bracketed dim-list.
    let mut idx = 1;
    if matches!(toks.get(idx).map(|s| &s.tok), Some(Tok::Op(o)) if o == "[") {
        // Consume up to and including the matching ']'.
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
    let has_assign = matches!(toks.get(idx).map(|s| &s.tok), Some(Tok::Op(o)) if o == "=");
    if !has_assign {
        return Err(ParseError::new(
            "expected '=' after target measure name",
            toks.get(idx).map(|s| s.pos),
        ));
    }
    let mut p = Parser {
        toks: &toks[idx + 1..],
        pos: 0,
        model,
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
}

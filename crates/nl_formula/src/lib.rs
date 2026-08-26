//! Controlled-natural-language (CNL) <-> formula translation (Phase 4).
//!
//! This is a *controlled* natural language: a small, fixed, deterministic
//! grammar, not open-English NLP. No LLMs, no external services.
//!
//! # Grammar (EBNF-ish)
//!
//! ```text
//! sentence   = [ subject copula ] expr [ "." ]
//! subject    = measure-name          (* ignored; RHS is what we return *)
//! copula     = "equals" | "is"
//!
//! expr       = term { ("plus" | "+" | "minus" | "-") term }
//! term       = factor { ("times" | "*" | "divided" "by" | "/") factor }
//! factor     = number
//!            | agg-call
//!            | measure-ref
//!            | "(" expr ")"
//!
//! agg-call   = ("the")? ("sum"|"average"|"min"|"max") "of" measure-ref
//! measure-ref= measure-name { dim-phrase }
//! dim-phrase = "over" cat-list          (* aggregate over: DimensionSpec.over *)
//!            | ("by" | "for" "each") cat-list  (* keep/group:  DimensionSpec.by  *)
//! cat-list   = category-name { "and" category-name }
//! ```
//!
//! ## Function-id mapping (built-in registry)
//!
//! | phrase       | FuncId |
//! |--------------|--------|
//! | `sum of`     | `1`    |
//! | `average of` | `2`    |
//! | `min of`     | `3`    |
//! | `max of`     | `4`    |
//!
//! ## Design choices
//!
//! * The subject + copula (`Revenue equals`, `Revenue is`) is parsed and
//!   discarded: the returned [`Formula`] is only the right-hand-side [`Expr`].
//!   The target measure is a property of *where* the formula is stored, not of
//!   the expression tree.
//! * A dimension phrase binds to the nearest preceding measure reference,
//!   populating that `Ref`'s [`DimensionSpec`] (`over` -> aggregate over,
//!   `by`/`for each` -> keep). `except` is not surfaced by the grammar.

use improv_core_model::{
    BinaryOp, CategoryId, DimensionSpec, Expr, Formula, FuncId, MeasureId, Model, Value,
};
use thiserror::Error;

pub const FUNC_SUM: FuncId = FuncId(1);
pub const FUNC_AVERAGE: FuncId = FuncId(2);
pub const FUNC_MIN: FuncId = FuncId(3);
pub const FUNC_MAX: FuncId = FuncId(4);

/// Name-resolution context: measures and categories looked up by name.
pub struct NlContext<'m> {
    model: &'m Model,
}

impl<'m> NlContext<'m> {
    pub fn new(model: &'m Model) -> Self {
        NlContext { model }
    }

    // Names arrive lowercased from the tokenizer, so match case-insensitively.
    fn measure_id(&self, name: &str) -> Option<MeasureId> {
        self.model
            .measures
            .values()
            .find(|m| m.name.0.eq_ignore_ascii_case(name))
            .map(|m| m.id)
    }

    fn category_id(&self, name: &str) -> Option<CategoryId> {
        self.model
            .categories
            .values()
            .find(|c| c.name.0.eq_ignore_ascii_case(name))
            .map(|c| c.id)
    }

    fn measure_name(&self, id: MeasureId) -> Option<&str> {
        self.model.measures.get(&id).map(|m| m.name.0.as_str())
    }

    fn category_name(&self, id: CategoryId) -> Option<&str> {
        self.model.categories.get(&id).map(|c| c.name.0.as_str())
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum NlError {
    #[error("unknown measure: {0}")]
    UnknownMeasure(String),
    #[error("unknown category: {0}")]
    UnknownCategory(String),
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unexpected token: {0}")]
    Unexpected(String),
    #[error("empty input")]
    Empty,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// A token is just a lowercased word, or a symbol. Multi-word operators
/// ("divided by", "for each", "the sum of") are recognized by the parser from
/// this flat stream, so the tokenizer stays trivial.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        match ch {
            '+' | '-' | '*' | '/' | '(' | ')' => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                out.push(ch.to_string());
            }
            c if c.is_whitespace() || c == '.' || c == ',' => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c.to_ascii_lowercase()),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

// ---------------------------------------------------------------------------
// Parser (recursive descent over the token stream)
// ---------------------------------------------------------------------------

struct Parser<'a, 'm> {
    toks: &'a [String],
    pos: usize,
    ctx: &'a NlContext<'m>,
}

/// Reserved words that can never be a measure/category name.
fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "plus"
            | "minus"
            | "times"
            | "divided"
            | "by"
            | "over"
            | "for"
            | "each"
            | "and"
            | "of"
            | "the"
            | "sum"
            | "average"
            | "min"
            | "max"
            | "equals"
            | "is"
    )
}

impl<'a, 'm> Parser<'a, 'm> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(String::as_str)
    }

    fn peek2(&self) -> Option<&str> {
        self.toks.get(self.pos + 1).map(String::as_str)
    }

    fn bump(&mut self) -> Option<&str> {
        let t = self.toks.get(self.pos).map(String::as_str);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, w: &str) -> bool {
        if self.peek() == Some(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// sentence = [ subject copula ] expr
    fn parse_sentence(&mut self) -> Result<Expr, NlError> {
        if self.toks.is_empty() {
            return Err(NlError::Empty);
        }
        // Optional "<subject> equals|is". Only strip when a copula actually
        // follows the first word, so "price times quantity" (no copula) parses
        // as a bare expression.
        if self.peek2() == Some("equals") || self.peek2() == Some("is") {
            self.bump(); // subject
            self.bump(); // copula
        }
        let e = self.parse_expr()?;
        match self.peek() {
            None => Ok(e),
            Some(t) => Err(NlError::Unexpected(t.to_string())),
        }
    }

    /// expr = term { (plus|+|minus|-) term }
    fn parse_expr(&mut self) -> Result<Expr, NlError> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = match self.peek() {
                Some("plus") | Some("+") => BinaryOp::Add,
                Some("minus") | Some("-") => BinaryOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_term()?;
            lhs = Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// term = factor { (times|*|divided by|/) factor }
    fn parse_term(&mut self) -> Result<Expr, NlError> {
        let mut lhs = self.parse_factor()?;
        loop {
            let op = match self.peek() {
                Some("times") | Some("*") => BinaryOp::Mul,
                Some("/") => BinaryOp::Div,
                Some("divided") if self.peek2() == Some("by") => {
                    self.bump(); // "divided"
                    BinaryOp::Div // "by" consumed below
                }
                _ => break,
            };
            self.bump(); // operator token ("times"/"*"/"/"/"by")
            let rhs = self.parse_factor()?;
            lhs = Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// factor = number | agg-call | measure-ref | "(" expr ")"
    fn parse_factor(&mut self) -> Result<Expr, NlError> {
        match self.peek() {
            None => Err(NlError::UnexpectedEof),
            Some("(") => {
                self.bump();
                let e = self.parse_expr()?;
                if !self.eat(")") {
                    return Err(NlError::Unexpected(
                        self.peek().unwrap_or("<eof>").to_string(),
                    ));
                }
                Ok(e)
            }
            // "the sum of ...", "sum of ...", etc.
            Some("the") if self.peek2().is_some_and(is_agg_word) => {
                self.bump(); // "the"
                self.parse_agg()
            }
            Some(w) if is_agg_word(w) => self.parse_agg(),
            Some(w) => {
                if let Ok(n) = w.parse::<f64>() {
                    self.bump();
                    return Ok(Expr::Literal(Value::Number(n)));
                }
                self.parse_measure_ref()
            }
        }
    }

    /// agg-call = ("sum"|"average"|"min"|"max") "of" measure-ref
    fn parse_agg(&mut self) -> Result<Expr, NlError> {
        let func = match self.bump() {
            Some("sum") => FUNC_SUM,
            Some("average") => FUNC_AVERAGE,
            Some("min") => FUNC_MIN,
            Some("max") => FUNC_MAX,
            other => return Err(NlError::Unexpected(other.unwrap_or("<eof>").to_string())),
        };
        if !self.eat("of") {
            return Err(NlError::Unexpected(
                self.peek().unwrap_or("<eof>").to_string(),
            ));
        }
        let arg = self.parse_measure_ref()?;
        Ok(Expr::Call(func, vec![arg]))
    }

    /// measure-ref = measure-name { dim-phrase }
    fn parse_measure_ref(&mut self) -> Result<Expr, NlError> {
        let name = match self.peek() {
            None => return Err(NlError::UnexpectedEof),
            Some(w) if is_keyword(w) => return Err(NlError::Unexpected(w.to_string())),
            Some(w) => w.to_string(),
        };
        let id = self
            .ctx
            .measure_id(&name)
            .ok_or_else(|| NlError::UnknownMeasure(name.clone()))?;
        self.bump();

        let mut spec = DimensionSpec::default();
        loop {
            match self.peek() {
                Some("over") => {
                    self.bump();
                    spec.over.extend(self.parse_cat_list()?);
                }
                Some("by") => {
                    self.bump();
                    spec.by.extend(self.parse_cat_list()?);
                }
                Some("for") if self.peek2() == Some("each") => {
                    self.bump(); // "for"
                    self.bump(); // "each"
                    spec.by.extend(self.parse_cat_list()?);
                }
                _ => break,
            }
        }
        Ok(Expr::Ref(id, spec))
    }

    /// cat-list = category-name { "and" category-name }
    fn parse_cat_list(&mut self) -> Result<Vec<CategoryId>, NlError> {
        let mut cats = vec![self.parse_cat()?];
        while self.eat("and") {
            cats.push(self.parse_cat()?);
        }
        Ok(cats)
    }

    fn parse_cat(&mut self) -> Result<CategoryId, NlError> {
        let name = match self.peek() {
            None => return Err(NlError::UnexpectedEof),
            Some(w) if is_keyword(w) => return Err(NlError::Unexpected(w.to_string())),
            Some(w) => w.to_string(),
        };
        let id = self
            .ctx
            .category_id(&name)
            .ok_or_else(|| NlError::UnknownCategory(name.clone()))?;
        self.bump();
        Ok(id)
    }
}

fn is_agg_word(w: &str) -> bool {
    matches!(w, "sum" | "average" | "min" | "max")
}

/// Parse a controlled sentence into a [`Formula`] (its right-hand-side Expr).
pub fn parse_nl_formula(ctx: &NlContext, text: &str) -> Result<Formula, NlError> {
    let toks = tokenize(text);
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        ctx,
    };
    Ok(Formula::new(p.parse_sentence()?))
}

// ---------------------------------------------------------------------------
// Describe (Expr -> English)
// ---------------------------------------------------------------------------

/// Render a [`Formula`] back into a controlled English sentence.
pub fn describe_formula(ctx: &NlContext, formula: &Formula) -> String {
    describe_expr(ctx, &formula.expr)
}

fn describe_expr(ctx: &NlContext, e: &Expr) -> String {
    match e {
        Expr::Literal(Value::Number(n)) => fmt_num(*n),
        Expr::Literal(v) => format!("{v:?}"),
        Expr::Ref(id, spec) => describe_ref(ctx, *id, spec),
        Expr::UnaryOp(_, inner) => format!("negative {}", describe_expr(ctx, inner)),
        Expr::BinaryOp(op, l, r) => {
            let word = match op {
                BinaryOp::Add => "plus",
                BinaryOp::Sub => "minus",
                BinaryOp::Mul => "times",
                BinaryOp::Div => "divided by",
                // Comparison/logical ops are outside the CNL surface; render a
                // best-effort symbol so describe never panics.
                BinaryOp::And => "and",
                BinaryOp::Or => "or",
                BinaryOp::Eq => "equals",
                BinaryOp::Ne => "does not equal",
                BinaryOp::Lt => "is less than",
                BinaryOp::Le => "is at most",
                BinaryOp::Gt => "is greater than",
                BinaryOp::Ge => "is at least",
            };
            format!("{} {word} {}", describe_expr(ctx, l), describe_expr(ctx, r))
        }
        Expr::Call(func, args) => {
            let name = match *func {
                FUNC_SUM => "sum",
                FUNC_AVERAGE => "average",
                FUNC_MIN => "min",
                FUNC_MAX => "max",
                FuncId(n) => return format!("function {n} of {}", join_args(ctx, args)),
            };
            format!("the {name} of {}", join_args(ctx, args))
        }
    }
}

fn join_args(ctx: &NlContext, args: &[Expr]) -> String {
    args.iter()
        .map(|a| describe_expr(ctx, a))
        .collect::<Vec<_>>()
        .join(" and ")
}

fn describe_ref(ctx: &NlContext, id: MeasureId, spec: &DimensionSpec) -> String {
    let mut s = ctx
        .measure_name(id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("measure {}", id.0));
    if !spec.over.is_empty() {
        s.push_str(" over ");
        s.push_str(&cat_list(ctx, &spec.over));
    }
    if !spec.by.is_empty() {
        s.push_str(" for each ");
        s.push_str(&cat_list(ctx, &spec.by));
    }
    s
}

fn cat_list(ctx: &NlContext, cats: &[CategoryId]) -> String {
    cats.iter()
        .map(|c| {
            ctx.category_name(*c)
                .map(str::to_string)
                .unwrap_or_else(|| format!("category {}", c.0))
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{CategoryId, Measure, MeasureId, MeasureKind, Model, Name, ValueType};

    const TIME: CategoryId = CategoryId(1);
    const PRODUCT: CategoryId = CategoryId(2);
    const PRICE: MeasureId = MeasureId(100);
    const QUANTITY: MeasureId = MeasureId(101);
    const REVENUE: MeasureId = MeasureId(102);

    fn fixture() -> Model {
        let mut m = Model::new();
        m.add_category(TIME, "Time");
        m.add_category(PRODUCT, "Product");
        for (id, name, cats) in [
            (PRICE, "Price", vec![PRODUCT]),
            (QUANTITY, "Quantity", vec![TIME, PRODUCT]),
            (REVENUE, "Revenue", vec![TIME, PRODUCT]),
        ] {
            m.add_measure(Measure {
                id,
                name: Name(name.into()),
                value_type: ValueType::Number,
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
    fn parses_multiplication() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let f = parse_nl_formula(&ctx, "price times quantity").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(refr(PRICE)),
                Box::new(refr(QUANTITY))
            )
        );
    }

    #[test]
    fn strips_subject_and_copula() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let a = parse_nl_formula(&ctx, "Revenue equals price times quantity.").unwrap();
        let b = parse_nl_formula(&ctx, "price times quantity").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parses_sum_aggregation() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let f = parse_nl_formula(&ctx, "the sum of revenue over time for each product").unwrap();
        let expected = Expr::Call(
            FUNC_SUM,
            vec![Expr::Ref(
                REVENUE,
                DimensionSpec {
                    over: vec![TIME],
                    by: vec![PRODUCT],
                    except: vec![],
                },
            )],
        );
        assert_eq!(f.expr, expected);
    }

    #[test]
    fn arithmetic_precedence() {
        // price times quantity plus revenue  ==  (price*quantity) + revenue
        let m = fixture();
        let ctx = NlContext::new(&m);
        let f = parse_nl_formula(&ctx, "price times quantity plus revenue").unwrap();
        let expected = Expr::BinaryOp(
            BinaryOp::Add,
            Box::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(refr(PRICE)),
                Box::new(refr(QUANTITY)),
            )),
            Box::new(refr(REVENUE)),
        );
        assert_eq!(f.expr, expected);
    }

    #[test]
    fn divided_by_multiword() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let f = parse_nl_formula(&ctx, "revenue divided by quantity").unwrap();
        assert_eq!(
            f.expr,
            Expr::BinaryOp(
                BinaryOp::Div,
                Box::new(refr(REVENUE)),
                Box::new(refr(QUANTITY))
            )
        );
    }

    #[test]
    fn describes_expr() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let e = Expr::Call(
            FUNC_SUM,
            vec![Expr::Ref(
                REVENUE,
                DimensionSpec {
                    over: vec![TIME],
                    by: vec![PRODUCT],
                    except: vec![],
                },
            )],
        );
        let s = describe_formula(&ctx, &Formula::new(e));
        assert_eq!(s, "the sum of Revenue over Time for each Product");
    }

    #[test]
    fn describes_multiplication() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let e = Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(refr(PRICE)),
            Box::new(refr(QUANTITY)),
        );
        assert_eq!(
            describe_formula(&ctx, &Formula::new(e)),
            "Price times Quantity"
        );
    }

    #[test]
    fn round_trip_stable() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let originals = [
            Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(refr(PRICE)),
                Box::new(refr(QUANTITY)),
            ),
            Expr::Call(
                FUNC_SUM,
                vec![Expr::Ref(
                    REVENUE,
                    DimensionSpec {
                        over: vec![TIME],
                        by: vec![PRODUCT],
                        except: vec![],
                    },
                )],
            ),
            Expr::BinaryOp(
                BinaryOp::Add,
                Box::new(Expr::BinaryOp(
                    BinaryOp::Mul,
                    Box::new(refr(PRICE)),
                    Box::new(refr(QUANTITY)),
                )),
                Box::new(refr(REVENUE)),
            ),
        ];
        for orig in originals {
            let f = Formula::new(orig.clone());
            let text = describe_formula(&ctx, &f);
            let reparsed = parse_nl_formula(&ctx, &text)
                .unwrap_or_else(|e| panic!("reparse of {text:?} failed: {e}"));
            assert_eq!(reparsed.expr, orig, "round trip via {text:?}");
        }
    }

    #[test]
    fn unknown_measure_errors() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let err = parse_nl_formula(&ctx, "widgets times quantity").unwrap_err();
        assert_eq!(err, NlError::UnknownMeasure("widgets".into()));
    }

    #[test]
    fn unknown_category_errors() {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let err = parse_nl_formula(&ctx, "the sum of revenue over region").unwrap_err();
        assert_eq!(err, NlError::UnknownCategory("region".into()));
    }
}

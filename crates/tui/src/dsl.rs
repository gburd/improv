//! Formulas printed as the symbolic DSL the parser reads (H1-17), so the
//! status line and a formula edit show `Left = Budget - Spent` as it was
//! typed, not the controlled English. [`formula`] gives `None` for a shape
//! the grammar has no exact spelling for, and
//! [`crate::app::App::formula_text`] shows only text that parses back to
//! the same formula, else the English. The GUI prints the same way
//! (`formula_dsl` in `crates/gui`).

use improv_core_model::functions::{self, FnDef, Over};
use improv_core_model::{
    parser, BinaryOp, CategoryId, Expr, Formula, FuncId, MeasureId, Model, UnaryOp, Value,
};

/// `f` as DSL text, or `None` when it has no exact spelling.
pub fn formula(model: &Model, f: &Formula) -> Option<String> {
    expr(model, &f.expr)
}

/// How tightly `e` binds, by the parser's grammar levels (higher is
/// tighter), so a child is parenthesized only where it must be.
fn prec(e: &Expr) -> u8 {
    match e {
        Expr::BinaryOp(op, ..) => match op {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => 4,
            BinaryOp::Concat => 5,
            BinaryOp::Add | BinaryOp::Sub => 6,
            BinaryOp::Mul | BinaryOp::Div => 7,
            BinaryOp::Pow => 9,
        },
        // NOT sits between AND and the comparisons; unary minus between
        // `* /` and `^` (`-2^2` is `-(2^2)`).
        Expr::UnaryOp(UnaryOp::Not, _) => 3,
        Expr::UnaryOp(UnaryOp::Neg, _) => 8,
        _ => 10,
    }
}

/// `e`, parenthesized when it binds more loosely than `min`.
fn child(model: &Model, e: &Expr, min: u8) -> Option<String> {
    let s = expr(model, e)?;
    Some(if prec(e) < min { format!("({s})") } else { s })
}

fn expr(model: &Model, e: &Expr) -> Option<String> {
    // A formula may be 256 levels deep: past a small main thread's stack.
    improv_core_model::grow_stack(|| expr_here(model, e))
}

fn expr_here(model: &Model, e: &Expr) -> Option<String> {
    match e {
        Expr::Literal(v) => literal(v),
        // A fold's LAMBDA parameters (see `call`).
        Expr::Ref(id, _) if functions::lambda_slot_of(*id).is_some() => {
            let (k, i) = functions::lambda_slot_of(*id)?;
            Some(format!("{}{k}", if i == 0 { "acc" } else { "val" }))
        }
        // `OVER` is spelled only inside an aggregation, `EXCEPT` never alone.
        Expr::Ref(id, spec) if spec.over.is_empty() && spec.except.is_empty() => {
            reference(model, *id, &spec.by)
        }
        Expr::Ref(..) => None,
        Expr::UnaryOp(op, inner) => {
            let s = child(model, inner, prec(e))?;
            Some(match op {
                UnaryOp::Neg => format!("-{s}"),
                UnaryOp::Not => format!("NOT {s}"),
            })
        }
        Expr::BinaryOp(op, l, r) => {
            let p = prec(e);
            let sym = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::And => "AND",
                BinaryOp::Or => "OR",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "<>",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::Pow => "^",
                BinaryOp::Concat => "&",
            };
            // Left-associative: a same-level right child takes parentheses.
            // Comparisons do not chain, so either side does; `^` is
            // right-associative, its base a primary, its exponent a unary.
            let (lmin, rmin) = match op {
                BinaryOp::Pow => (p + 1, 8),
                _ if p == 4 => (5, 5),
                _ => (p, p + 1),
            };
            Some(format!(
                "{} {sym} {}",
                child(model, l, lmin)?,
                child(model, r, rmin)?
            ))
        }
        Expr::Call(func, args) => call(model, *func, args),
        Expr::Category(c) => category(model, *c),
    }
}

/// `a, b, …`, each argument printed.
fn list(model: &Model, args: &[Expr]) -> Option<String> {
    let parts: Option<Vec<String>> = args.iter().map(|a| expr(model, a)).collect();
    Some(parts?.join(", "))
}

fn call(model: &Model, func: FuncId, args: &[Expr]) -> Option<String> {
    if let Some(id) = functions::named_lambda_of(func) {
        let l = model.lambdas.get(&id)?;
        return Some(format!("{}({})", ident(&l.name.0)?, list(model, args)?));
    }
    let def = functions::by_id(func)?;
    let name = def.name;
    match (name, args) {
        // SCAN/REDUCE(init, x OVER T, body): the body's placeholders print
        // as the LAMBDA's parameters.
        ("SCAN" | "REDUCE", [init, Expr::Ref(x, spec), body]) => {
            let [cat] = spec.over.as_slice() else {
                return None;
            };
            Some(format!(
                "{name}({}, {} OVER {}, LAMBDA(acc0, val0, {}))",
                expr(model, init)?,
                reference(model, *x, &spec.by)?,
                category(model, *cat)?,
                expr(model, body)?
            ))
        }
        ("SCAN" | "REDUCE", _) => None,
        // FIXPOINT(init, body, [max_iter], [tolerance]).
        ("FIXPOINT", [init, body, opts @ ..]) => {
            let opts = opts
                .iter()
                .map(|o| expr(model, o).map(|s| format!(", {s}")));
            Some(format!(
                "FIXPOINT({}, LAMBDA(acc0, {}){})",
                expr(model, init)?,
                expr(model, body)?,
                opts.collect::<Option<String>>()?
            ))
        }
        _ if functions::is_aggregation(func) => aggregation(model, def, args),
        _ => Some(format!("{name}({})", list(model, args)?)),
    }
}

/// `AGG(args…)` with its `OVER` clause, window and `WHERE` after the first
/// reduced argument (after the last leading one for `LINEST`-like
/// functions), and `INTO` at the end. `None` when a reduced argument is not
/// a plain reference, the reduced ones differ in `OVER`, or one has
/// `EXCEPT`.
fn aggregation(model: &Model, def: &FnDef, args: &[Expr]) -> Option<String> {
    let (mut pred, mut window, mut into, mut rest) = (None, None, None, Vec::new());
    for a in args {
        match a {
            Expr::Call(id, k) if *id == functions::INTO => into = Some(k),
            Expr::Call(id, p) if *id == functions::WHERE => pred = p.first(),
            Expr::Call(id, b) if *id == functions::WINDOW => window = Some(b),
            other => rest.push(other),
        }
    }
    let reduced = |i: &usize| def.reduced(*i, rest[*i]);
    let over_at = match def.over {
        Over::Leading(_) => (0..rest.len()).rev().find(reduced),
        _ => (0..rest.len()).find(reduced),
    };
    let mut over: Option<&Vec<CategoryId>> = None;
    let mut parts = Vec::new();
    for (i, a) in rest.iter().enumerate() {
        if !def.reduced(i, a) {
            parts.push(expr(model, a)?);
            continue;
        }
        let Expr::Ref(id, spec) = a else {
            return None;
        };
        if !spec.except.is_empty() || spec.over.is_empty() || over.is_some_and(|o| *o != spec.over)
        {
            return None;
        }
        let mut part = reference(model, *id, &spec.by)?;
        if Some(i) == over_at {
            let cats: Option<Vec<String>> = spec.over.iter().map(|c| category(model, *c)).collect();
            part.push_str(&format!(" OVER {}", cats?.join(", ")));
            if let Some(b) = window {
                let ([lo, hi], [_]) = (b.as_slice(), spec.over.as_slice()) else {
                    return None;
                };
                part.push_str(&format!("[{}..{}]", bound(lo)?, bound(hi)?));
            }
            if let Some(p) = pred {
                part.push_str(&format!(" WHERE {}", expr(model, p)?));
            }
        }
        over = Some(&spec.over);
        parts.push(part);
    }
    over?;
    if let Some(ks) = into {
        let last = parts.last_mut()?;
        last.push_str(&format!(" INTO {}", list(model, ks)?));
    }
    Some(format!("{}({})", def.name, parts.join(", ")))
}

/// A window bound as written: `FIRST`, `LAST`, `THIS`, `THIS-2`, `THIS+1`.
fn bound(e: &Expr) -> Option<String> {
    match e {
        Expr::Literal(Value::Text(t)) if t == "FIRST" || t == "LAST" => Some(t.clone()),
        Expr::Literal(Value::Number(n)) if n.fract() == 0.0 => Some(match *n {
            0.0 => "THIS".into(),
            n if n > 0.0 => format!("THIS+{n}"),
            n => format!("THIS-{}", -n),
        }),
        _ => None,
    }
}

/// A measure, and its `[Cat, …]` when it is read along categories.
fn reference(model: &Model, id: MeasureId, by: &[CategoryId]) -> Option<String> {
    let name = ident(&model.measures.get(&id)?.name.0)?;
    if by.is_empty() {
        return Some(name);
    }
    let cats: Option<Vec<String>> = by.iter().map(|c| category(model, *c)).collect();
    Some(format!("{name}[{}]", cats?.join(", ")))
}

fn category(model: &Model, c: CategoryId) -> Option<String> {
    ident(&model.categories.get(&c)?.name.0)
}

/// `name` bare when the tokenizer reads it back as one identifier that is
/// not a keyword, else quoted (`'Unit Price'`, `'Bob''s'`). `None` for the
/// empty name, which no spelling names.
fn ident(name: &str) -> Option<String> {
    const KEYWORDS: &[&str] = &[
        "NOT", "AND", "OR", "OVER", "TRUE", "FALSE", "WHERE", "INTO", "EXCEPT", "ALL",
    ];
    let mut chars = name.chars();
    let word = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_');
    if word && !KEYWORDS.iter().any(|k| name.eq_ignore_ascii_case(k)) {
        return Some(name.into());
    }
    (!name.is_empty()).then(|| format!("'{}'", name.replace('\'', "''")))
}

fn literal(v: &Value) -> Option<String> {
    match v {
        // No sign (`-5` is a negation) and no infinity in a literal.
        Value::Number(n) => parser::number_literal(*n),
        Value::Boolean(b) => Some(if *b { "TRUE" } else { "FALSE" }.into()),
        // `""` is a string's one escape; a control character has none.
        Value::Text(t) => (!t.contains('\\') && !t.contains(char::is_control))
            .then(|| format!("\"{}\"", t.replace('"', "\"\""))),
        Value::DateTime(dt) => Some(format!("#{}#", dt.to_rfc3339())),
        Value::Enum(_) | Value::Error(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::revenue_model;
    use improv_core_model::{DimensionSpec, ItemId, Measure, MeasureKind, Name, ValueType};

    fn model() -> Model {
        let mut m = revenue_model();
        m.add_category(CategoryId(3), "Region");
        m.add_item(ItemId(30), CategoryId(3), "North");
        m.add_category(CategoryId(4), "Slot");
        for (id, name) in [(110, "Unit Price"), (111, "Over"), (112, "Bob's")] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![CategoryId(2)],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        m
    }

    fn parse(m: &Model, text: &str) -> Formula {
        parser::parse_expr(m, text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    /// Typed formulas print back as typed, and parse to the same formula.
    #[test]
    fn formulas_print_as_typed() {
        let m = model();
        let defined =
            parser::parse_definition(&m, "Margin = LAMBDA(p, c, p - c)").expect("a lambda");
        let parser::Definition::Lambda(lt) = defined else {
            panic!("a lambda")
        };
        let mut m = m;
        m.lambdas.insert(1, lt.lambda);
        for text in [
            "Price * Quantity",
            "Price - Quantity - 1",
            "Price - (Quantity - 1)",
            "(Price + 1) * 2",
            "-Price ^ 2",
            "(-Price) ^ 2",
            "2 ^ 3 ^ 2",
            "(2 ^ 3) ^ 2",
            "2 ^ -1",
            "NOT Price > 1 AND Quantity < 2 OR FALSE",
            "NOT (Price > 1 AND TRUE)",
            "(Price > 1) == TRUE",
            "\"a\" & \"say \"\"hi\"\"\" & Price",
            "Price * 6.02e23 + 1e-10 + 0.5",
            "#2025-01-31T00:00:00+00:00#",
            "'Unit Price' + 'Over' + 'Bob''s'",
            "Price[Product] + Quantity[Time, Product]",
            "ROUND(Price, 2) + ABS(Quantity)",
            "IF(Price > 1, Price, 0)",
            "Quantity - PREV(Quantity, Time, 1, 0)",
            "Quantity * POSITION(Time)",
            "SUM(Quantity OVER Time)",
            "SUM(Quantity OVER Time, Product)",
            "SUM(Quantity OVER Time[FIRST..THIS])",
            "SUM(Quantity OVER Time[THIS-2..THIS+1])",
            "SUM(Quantity OVER Time[THIS..LAST])",
            "SUM(Quantity OVER Time WHERE Quantity > 90)",
            "TEXTJOIN(\"-\", TRUE, Quantity OVER Time)",
            "SORT(Quantity OVER Time INTO Region, Slot)",
            "LINEST(Quantity, Price, Price OVER Time, FALSE, TRUE INTO Region, Slot)",
            "SCAN(0, Quantity OVER Time, LAMBDA(acc0, val0, acc0 + val0 * 2))",
            "REDUCE(0, Quantity OVER Time, LAMBDA(acc0, val0, acc0 + val0))",
            "FIXPOINT(Quantity, LAMBDA(acc0, acc0 / 2 + Price), 50, 1e-10)",
            "FIXPOINT(Price, LAMBDA(acc0, acc0 / 2))",
            "Margin(Price, Quantity * 2)",
        ] {
            let f = parse(&m, text);
            let shown = formula(&m, &f).unwrap_or_else(|| panic!("{text}: no DSL"));
            assert_eq!(shown, text);
            assert_eq!(parse(&m, &shown), f, "{text}");
        }
        // Named arguments print in their places.
        let f = parse(&m, "ROUND(Price, num_digits => 2)");
        assert_eq!(formula(&m, &f).as_deref(), Some("ROUND(Price, 2)"));
    }

    /// Shapes the grammar cannot spell print as nothing, never as text that
    /// reads back as something else.
    #[test]
    fn unspellable_shapes_print_nothing() {
        let m = model();
        let r = |id, over: Vec<u32>, except: Vec<u32>| {
            let cats = |v: Vec<u32>| v.into_iter().map(CategoryId).collect();
            Expr::Ref(
                MeasureId(id),
                DimensionSpec {
                    by: vec![],
                    over: cats(over),
                    except: cats(except),
                },
            )
        };
        let call = |f: FuncId, a: Vec<Expr>| Expr::Call(f, a);
        let n = |n: f64| Expr::Literal(Value::Number(n));
        let sum = parser::FUNC_SUM;
        let scan = functions::by_name("SCAN").next().unwrap().id;
        let where_ = |p| call(functions::WHERE, vec![p]);
        let window = |a, b| call(functions::WINDOW, vec![a, b]);
        for e in [
            r(101, vec![1], vec![]),
            r(101, vec![], vec![1]),
            r(999, vec![], vec![]),
            n(-1.0),
            n(f64::INFINITY),
            Expr::Literal(Value::Text("a\\b".into())),
            Expr::Literal(Value::Enum(3)),
            Expr::Category(CategoryId(99)),
            call(FuncId(987_654), vec![]),
            call(functions::named_lambda(7), vec![]),
            call(sum, vec![n(1.0)]),
            call(sum, vec![r(101, vec![], vec![])]),
            call(sum, vec![r(101, vec![1], vec![2])]),
            call(sum, vec![r(101, vec![1], vec![]), r(100, vec![2], vec![])]),
            call(
                sum,
                vec![r(101, vec![1, 2], vec![]), window(n(0.0), n(0.0))],
            ),
            call(sum, vec![r(101, vec![1], vec![]), window(n(0.5), n(0.0))]),
            call(
                sum,
                vec![r(101, vec![1], vec![]), window(n(0.0), n(f64::NAN))],
            ),
            call(sum, vec![r(101, vec![1], vec![]), where_(n(-1.0))]),
            call(
                sum,
                vec![
                    r(101, vec![1], vec![]),
                    call(functions::INTO, vec![Expr::Category(CategoryId(99))]),
                ],
            ),
            call(scan, vec![n(0.0)]),
            call(scan, vec![n(0.0), r(101, vec![1, 2], vec![]), n(1.0)]),
        ] {
            assert_eq!(formula(&m, &Formula::new(e.clone())), None, "{e:?}");
        }
        assert_eq!(ident(""), None);
    }

    /// The property: whatever prints parses back to the same formula, over
    /// random trees of every node kind (a fixed seed, so it is
    /// reproducible), and most of them print.
    #[test]
    fn printed_formulas_parse_back_to_themselves() {
        let m = model();
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut pick = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        fn gen(pick: &mut dyn FnMut(usize) -> usize, depth: u32) -> Expr {
            let leaf = depth == 0;
            let refd = |id, by: Vec<CategoryId>| {
                Expr::Ref(
                    MeasureId(id),
                    DimensionSpec {
                        by,
                        ..Default::default()
                    },
                )
            };
            match pick(if leaf { 4 } else { 10 }) {
                0 => Expr::Literal(Value::Number(
                    [0.0, 1.0, 2.5, 100.0, 1e-7, 6.02e23][pick(6)],
                )),
                1 => Expr::Literal(match pick(3) {
                    0 => Value::Boolean(pick(2) == 0),
                    1 => Value::Text("say \"hi\"".into()),
                    _ => Value::Text(String::new()),
                }),
                2 => [refd(100, vec![]), refd(101, vec![]), refd(110, vec![])][pick(3)].clone(),
                3 => refd(101, vec![CategoryId(1), CategoryId(2)]),
                4 => {
                    let op = [UnaryOp::Neg, UnaryOp::Not][pick(2)];
                    Expr::UnaryOp(op, Box::new(gen(pick, depth - 1)))
                }
                5..=7 => {
                    let op = [
                        BinaryOp::Add,
                        BinaryOp::Sub,
                        BinaryOp::Mul,
                        BinaryOp::Div,
                        BinaryOp::And,
                        BinaryOp::Or,
                        BinaryOp::Eq,
                        BinaryOp::Ne,
                        BinaryOp::Lt,
                        BinaryOp::Le,
                        BinaryOp::Gt,
                        BinaryOp::Ge,
                        BinaryOp::Pow,
                        BinaryOp::Concat,
                    ][pick(14)];
                    let (l, r) = (gen(pick, depth - 1), gen(pick, depth - 1));
                    Expr::BinaryOp(op, Box::new(l), Box::new(r))
                }
                8 => {
                    let (name, n) = [("ROUND", 2), ("ABS", 1), ("IF", 3)][pick(3)];
                    let id = functions::by_name(name).next().unwrap().id;
                    Expr::Call(id, (0..n).map(|_| gen(pick, depth - 1)).collect())
                }
                _ => Expr::Call(
                    parser::FUNC_SUM,
                    vec![Expr::Ref(
                        MeasureId(101),
                        DimensionSpec {
                            over: vec![CategoryId(1)],
                            ..Default::default()
                        },
                    )],
                ),
            }
        }
        let mut printed = 0;
        for i in 0..3000 {
            let f = Formula::new(gen(&mut pick, 1 + i % 5));
            if let Some(text) = formula(&m, &f) {
                printed += 1;
                let back = parser::parse_expr(&m, &text)
                    .unwrap_or_else(|e| panic!("{f:?} printed as {text:?}: {e}"));
                assert_eq!(back, f, "{text}");
            }
        }
        assert!(printed > 2900, "{printed} of 3000 printed");
    }
}

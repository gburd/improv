//! The crate's tests: unit, the deterministic Time × Product oracle, and
//! property tests (the class cache is exact; a forbid only hides).

use std::collections::BTreeMap;
use std::sync::Arc;

use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, ValueType, ViewId,
};
use proptest::prelude::*;

use crate::templates::{render, Who};
use crate::{Action, Channel, Policies, Principal, Resource, Reveals, Session, SCHEMA};

const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const PRICE: MeasureId = MeasureId(100);
const QUANTITY: MeasureId = MeasureId(101);
const REVENUE: MeasureId = MeasureId(102);
const BY_PRODUCT: MeasureId = MeasureId(103);
const A: ItemId = ItemId(20);
const B: ItemId = ItemId(21);

fn measure(id: MeasureId, name: &str, cats: Vec<CategoryId>, kind: MeasureKind) -> Measure {
    Measure {
        id,
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind,
        description: None,
        format: None,
    }
}

fn r(m: MeasureId) -> Box<Expr> {
    Box::new(Expr::Ref(m, DimensionSpec::default()))
}

/// The canonical Time × Product model, plus a per-product total over Time
/// and a view.
fn model() -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    m.add_item(ItemId(10), TIME, "2025");
    m.add_item(ItemId(11), TIME, "2026");
    m.add_item(A, PRODUCT, "WidgetA");
    m.add_item(B, PRODUCT, "WidgetB");
    m.add_measure(measure(PRICE, "Price", vec![PRODUCT], MeasureKind::Input));
    m.add_measure(measure(
        QUANTITY,
        "Quantity",
        vec![TIME, PRODUCT],
        MeasureKind::Input,
    ));
    let rev = Expr::BinaryOp(BinaryOp::Mul, r(PRICE), r(QUANTITY));
    m.add_measure(measure(
        REVENUE,
        "Revenue",
        vec![TIME, PRODUCT],
        MeasureKind::Derived(Formula::new(rev)),
    ));
    // SUM(Revenue OVER Product), by Time: a total over products.
    let over = DimensionSpec {
        over: vec![PRODUCT],
        ..Default::default()
    };
    let total = Expr::Call(improv_core_model::FuncId(0), vec![Expr::Ref(REVENUE, over)]);
    m.add_measure(measure(
        BY_PRODUCT,
        "Total",
        vec![TIME],
        MeasureKind::Derived(Formula::new(total)),
    ));
    m.add_view(improv_core_model::View {
        id: ViewId(1),
        name: Name("Prices".into()),
        measure: PRICE,
        ..serde_json::from_str(r#"{"id":1,"name":"x","measure":100}"#).unwrap()
    });
    m
}

fn at(pairs: &[(CategoryId, ItemId)]) -> Coordinate {
    Coordinate::from_pairs(pairs.iter().copied())
}

fn session(text: &str, groups: &str, user: &str, roles: &[&str]) -> Session {
    let p = Policies::parse(text, groups).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let who = Principal {
        user: user.into(),
        roles: roles.iter().map(|r| r.to_string()).collect(),
    };
    Session::new(Some(Arc::new(p)), &model(), &who, Channel::Local)
}

fn template(name: &str, who: Who, measures: &[MeasureId], items: &[ItemId]) -> String {
    render(name, &who, measures, items).unwrap()
}

#[test]
fn the_schema_parses_with_no_warnings() {
    let (_, warnings) = cedar_policy::Schema::from_cedarschema_str(SCHEMA).unwrap();
    assert_eq!(warnings.count(), 0);
    let names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
    for n in &names {
        assert!(SCHEMA.contains(&format!("action {n} ")), "{n}");
        assert_eq!(n.parse::<Action>().unwrap().name(), *n);
        assert_eq!(n.to_lowercase().parse::<Action>().unwrap().to_string(), *n);
    }
    let err = "Peek".parse::<Action>().unwrap_err();
    assert!(err.contains("ReadCell"), "{err}");
}

#[test]
fn no_policies_allows_everything_that_exists() {
    let s = Session::new(None, &model(), &Principal::user("eve"), Channel::Local);
    assert!(!s.restricted());
    assert!(s.cell_editable(PRICE, &at(&[(PRODUCT, A)])));
    assert!(s.allowed(Action::Admin, &Resource::Model));
    assert!(!s.allowed(Action::ReadMeasure, &Resource::Measure(MeasureId(9))));
    assert!(!s.cell_visible(PRICE, &at(&[(TIME, ItemId(10))])));
    assert_eq!(s.explain(Action::Admin, &Resource::Model).unwrap().len(), 1);
    assert!(s
        .explain(Action::ReadItem, &Resource::Item(ItemId(99)))
        .is_err());
}

#[test]
fn default_deny_and_forbid_wins() {
    let s = session(
        r#"permit (principal, action == Improv::Action::"ViewModel", resource);"#,
        "",
        "eve",
        &[],
    );
    assert!(s.restricted());
    assert!(s.allowed(Action::ViewModel, &Resource::Model));
    assert!(!s.measure_visible(PRICE), "default deny");
    let s = session(
        r#"permit (principal, action, resource);
           forbid (principal == Improv::User::"eve", action, resource in Improv::Measure::"100");"#,
        "",
        "eve",
        &[],
    );
    assert!(!s.measure_visible(PRICE), "forbid wins");
    assert!(!s.cell_visible(PRICE, &at(&[(PRODUCT, A)])));
    assert!(s.measure_visible(QUANTITY));
}

#[test]
fn nothing_works_without_view_model() {
    let s = session(
        r#"permit (principal, action, resource);
           forbid (principal, action == Improv::Action::"ViewModel", resource);"#,
        "",
        "eve",
        &[],
    );
    assert!(!s.measure_visible(PRICE));
    let steps = s
        .explain(Action::ReadMeasure, &Resource::Measure(PRICE))
        .unwrap();
    assert_eq!(steps.len(), 1, "stops at the first denial");
    assert_eq!(steps[0].action, Action::ViewModel);
    assert_eq!(steps[0].decision.policies, vec!["policy1"]);
}

#[test]
fn an_evaluation_error_denies() {
    // An integer overflow is an evaluation error, which strict validation
    // cannot see. Cedar skips the erroring forbid; Improv still denies.
    let text = r#"permit (principal, action, resource);
        forbid (principal, action == Improv::Action::"ReadMeasure", resource)
        when { 9223372036854775807 + (if context.channel == "local" then 1 else 0) > 0 };"#;
    let s = session(text, "", "eve", &[]);
    let d = s
        .explain(Action::ReadMeasure, &Resource::Measure(PRICE))
        .unwrap();
    let last = &d.last().unwrap().decision;
    assert!(!last.errors.is_empty(), "{d:?}");
    assert!(!last.allow, "the broken forbid still denies");
    assert!(!s.measure_visible(PRICE));
}

#[test]
fn refuses_cells_templates_duplicates_and_nothing() {
    let bad = |text: &str| Policies::parse(text, "").unwrap_err().to_string();
    let e = bad(r#"permit (principal, action, resource == Improv::Cell::"100:20");"#);
    assert!(e.contains("names an Improv::Cell"), "{e}");
    let e = bad(r#"permit (principal == ?principal, action, resource);"#);
    assert!(e.contains("slot"), "{e}");
    let e = bad("// nothing\n");
    assert!(e.contains("no policies"), "{e}");
    let e = bad(r#"@id("x") permit (principal, action, resource);
                  @id("x") forbid (principal, action, resource);"#);
    assert!(e.contains("policy x: another policy has this id"), "{e}");
    let e = bad("permit (principal, action, resource)\n");
    assert!(e.starts_with("line "), "{e}");
    let e = bad("\n\npermit (principal, action, resource) when { resource.nope };");
    assert!(e.contains("nope"), "{e}");
    assert!(e.starts_with("line 3"), "{e}");
    let e =
        bad(r#"permit (principal, action, resource) when { principal in Improv::Bogus::"x" };"#);
    assert!(e.contains("Bogus"), "{e}");
    let huge = "permit (principal, action, resource);\n".repeat(crate::MAX_TEXT / 30);
    assert!(bad(&huge).contains("larger than"));
    let many = "permit (principal, action, resource);".repeat(crate::MAX_POLICIES + 1);
    assert!(bad(&many).contains("more than"));
}

#[test]
fn warnings_and_unknown_names() {
    let text = r#"@id("never") permit (principal, action == Improv::Action::"ReadItem", resource)
                  when { resource in [Improv::Measure::"100"] };
                  permit (principal, action, resource in Improv::Measure::"Revenue");
                  permit (principal, action, resource in Improv::Item::"20");
                  permit (principal, action, resource == Improv::Model::"other");"#;
    let p = Policies::parse(text, "").unwrap();
    assert_eq!(p.len(), 4);
    assert!(!p.is_empty());
    assert_eq!(p.text(), text);
    assert!(
        p.warnings()
            .iter()
            .any(|w| w.message.contains("impossible")),
        "{:?}",
        p.warnings()
    );
    assert_eq!(p.ids(), vec!["never", "policy1", "policy2", "policy3"]);
    assert_eq!(
        p.unknown_names(&model()),
        vec![
            "Improv::Measure::\"Revenue\"".to_string(),
            "Improv::Model::\"other\"".into()
        ]
    );
}

#[test]
fn groups_nest_and_are_checked() {
    let groups = r#"{"staff": {"groups": ["analysts"]},
                     "analysts": {"users": ["alice"], "groups": ["interns"]},
                     "interns": {"users": ["ian"]},
                     "loop": {"groups": ["loop"]}}"#;
    let g = crate::Groups::parse(groups).unwrap();
    assert_eq!(
        g.of("ian").into_iter().collect::<Vec<_>>(),
        vec!["analysts", "interns", "staff"]
    );
    assert!(g.of("nobody").is_empty());
    assert!(crate::Groups::parse("").unwrap().0.is_empty());
    let e = crate::Groups::parse(r#"{"a": {"groups": ["b"]}}"#).unwrap_err();
    assert!(e.to_string().contains("'b', which is not defined"), "{e}");
    let e = crate::Groups::parse(r#"{"a": {"users": [""]}}"#).unwrap_err();
    assert!(e.to_string().contains("empty"), "{e}");
    let e = crate::Groups::parse(r#"{"a": {"roles": ["x"]}}"#).unwrap_err();
    assert!(e.to_string().contains("roles"), "{e}");
    let e = crate::Groups::parse("[").unwrap_err();
    assert!(e.to_string().starts_with("groups:"), "{e}");
    let big = format!(
        r#"{{"a": {{"users": [{}]}}}}"#,
        vec!["\"u\""; crate::MAX_GROUPS].join(",")
    );
    assert!(crate::Groups::parse(&big).is_err());

    let text = r#"permit (principal in Improv::Group::"staff", action in Improv::Action::"Read", resource);"#;
    assert!(session(text, groups, "ian", &[]).measure_visible(PRICE));
    assert!(!session(text, groups, "eve", &[]).measure_visible(PRICE));
}

#[test]
fn roles_come_from_the_principal() {
    let text = r#"permit (principal in Improv::Role::"finance", action in Improv::Action::"Read", resource);"#;
    assert!(session(text, "", "bob", &["finance"]).measure_visible(PRICE));
    assert!(!session(text, "", "bob", &["sales"]).measure_visible(PRICE));
}

#[test]
fn context_channel_and_measure_kind() {
    let text = r#"permit (principal, action == Improv::Action::"ViewModel", resource);
        permit (principal, action == Improv::Action::"ReadMeasure", resource)
        when { resource.kind == "input" && context.channel == "local" };"#;
    let s = session(text, "", "eve", &[]);
    assert!(s.measure_visible(PRICE));
    assert!(!s.measure_visible(REVENUE));
    let p = Arc::new(Policies::parse(text, "").unwrap());
    let srv = Session::new(
        Some(p.clone()),
        &model(),
        &Principal::user("eve"),
        Channel::Server,
    );
    assert!(!srv.measure_visible(PRICE));
    let pg = Session::new(
        Some(p.clone()),
        &model(),
        &Principal::user("eve"),
        Channel::Postgres,
    );
    assert!(!pg.measure_visible(PRICE));
    assert_eq!(Channel::Postgres.name(), "postgres");
    let d = p
        .authorize(
            &model(),
            &Principal::user("eve"),
            Channel::Local,
            Action::ReadMeasure,
            &Resource::Measure(PRICE),
        )
        .unwrap();
    assert!(d.allow);
    assert_eq!(d.policies, vec!["policy1"]);
}

#[test]
fn authorize_refuses_what_is_not_there() {
    let p = Policies::parse("permit (principal, action, resource);", "").unwrap();
    let m = model();
    let eve = Principal::user("eve");
    let ask = |a, r: Resource| p.authorize(&m, &eve, Channel::Local, a, &r);
    assert!(ask(Action::ReadMeasure, Resource::Measure(MeasureId(9))).is_err());
    assert!(ask(Action::ReadItem, Resource::Item(ItemId(9))).is_err());
    assert!(ask(Action::EditStructure, Resource::Category(CategoryId(9))).is_err());
    assert!(ask(Action::ManageViews, Resource::View(ViewId(9))).is_err());
    assert!(ask(
        Action::ReadCell,
        Resource::Cell(PRICE, at(&[(TIME, ItemId(10))]))
    )
    .is_err());
    let e = ask(Action::ReadCell, Resource::Measure(PRICE)).unwrap_err();
    assert!(e.contains("does not apply"), "{e}");
    for (a, r) in [
        (Action::EditStructure, Resource::Category(TIME)),
        (Action::ManageViews, Resource::View(ViewId(1))),
        (Action::Admin, Resource::Model),
        (
            Action::ReadCell,
            Resource::Cell(QUANTITY, at(&[(TIME, ItemId(10)), (PRODUCT, A)])),
        ),
    ] {
        assert!(ask(a, r).unwrap().allow);
    }
}

/// The deterministic oracle: four principals' decisions on every cell of
/// the canonical model, under the shipped templates.
#[test]
fn the_canonical_oracle() {
    let text = [
        template("viewer", Who::Group("viewers".into()), &[], &[]),
        template("editor", Who::User("ed".into()), &[PRICE], &[]),
        template("analyst", Who::User("ann".into()), &[], &[B]),
        template("admin", Who::User("root".into()), &[], &[]),
    ]
    .join("\n");
    let groups = r#"{"viewers": {"users": ["vic"]}}"#;
    let mut table = BTreeMap::new();
    let m = model();
    for user in ["vic", "ed", "ann", "root", "eve"] {
        let s = session(&text, groups, user, &[]);
        let mut row = String::new();
        let mut ids: Vec<_> = m.measures.keys().copied().collect();
        ids.sort();
        for mid in ids {
            let cats = &m.measures[&mid].categories;
            let mut coords = vec![Coordinate::new()];
            for c in cats {
                coords = coords
                    .into_iter()
                    .flat_map(|co| {
                        m.categories[c]
                            .items
                            .iter()
                            .map(move |i| co.clone().with(*c, *i))
                    })
                    .collect();
            }
            for co in coords {
                row.push(
                    match (s.cell_visible(mid, &co), s.cell_editable(mid, &co)) {
                        (true, true) => 'E',
                        (true, false) => 'R',
                        (false, _) => '.',
                    },
                );
            }
            row.push(' ');
        }
        table.insert(user, row.trim_end().to_string());
    }
    // Price[A B]  Quantity[10A 10B 11A 11B]  Revenue[same]  Total[10 11]
    let want: BTreeMap<&str, String> = [
        ("vic", "RR RRRR RRRR RR"),
        ("ed", "EE RRRR RRRR RR"),
        ("ann", "R. R.R. R.R. RR"),
        ("root", "EE EEEE RRRR RR"),
        ("eve", ".. .... .... .."),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_string()))
    .collect();
    assert_eq!(table, want);

    // The analyst sees totals over all products, and lint says so.
    let ann = session(&text, groups, "ann", &[]);
    assert!(ann.cell_visible(BY_PRODUCT, &at(&[(TIME, ItemId(10))])));
    assert!(!ann.item_visible(B) && ann.item_visible(A));
    assert_eq!(
        ann.lint(),
        vec![(BY_PRODUCT, Reveals::Items(PRODUCT, vec![B]))]
    );
    assert!(session(&text, groups, "vic", &[]).lint().is_empty());
    // Structure and admin.
    let root = session(&text, groups, "root", &[]);
    assert!(root.allowed(Action::Admin, &Resource::Model));
    assert!(root.allowed(Action::DefineMeasure, &Resource::Measure(REVENUE)));
    assert!(root.allowed(Action::EditStructure, &Resource::Item(B)));
    assert!(root.allowed(Action::ManageViews, &Resource::View(ViewId(1))));
    let vic = session(&text, groups, "vic", &[]);
    assert!(!vic.allowed(Action::Admin, &Resource::Model));
    assert!(!vic.allowed(Action::DefineMeasure, &Resource::Model));
    assert!(vic.formula_visible(REVENUE));
    let e = root
        .explain(
            Action::EditCell,
            &Resource::Cell(REVENUE, at(&[(TIME, ItemId(10)), (PRODUCT, A)])),
        )
        .unwrap();
    assert!(e.last().unwrap().decision.errors[0].contains("derived"));
}

#[test]
fn hide_measures_hides_formulas_and_lint_reports_it() {
    let text = [
        template("viewer", Who::User("vic".into()), &[], &[]),
        template("hide-measures", Who::User("vic".into()), &[QUANTITY], &[]),
    ]
    .join("\n");
    let s = session(&text, "", "vic", &[]);
    assert!(!s.measure_visible(QUANTITY));
    assert!(s.measure_visible(REVENUE));
    assert!(!s.formula_visible(REVENUE), "reads a hidden measure");
    assert!(s.formula_visible(PRICE));
    assert!(s.cell_visible(BY_PRODUCT, &at(&[(TIME, ItemId(10))])));
    assert_eq!(s.lint(), vec![(REVENUE, Reveals::Measure(QUANTITY))]);
    let steps = s
        .explain(
            Action::ReadCell,
            &Resource::Cell(QUANTITY, at(&[(TIME, ItemId(10)), (PRODUCT, A)])),
        )
        .unwrap();
    assert_eq!(
        steps.last().unwrap().decision.policies,
        vec!["hide-measures:user:vic"]
    );
}

#[test]
fn templates_render_and_refuse() {
    let who = Who::Group("a \"q\"\\ \n\t\r\0\u{7}".into());
    let text = render("viewer", &who, &[], &[]).unwrap();
    assert!(
        text.contains(r#"Improv::Group::"a \"q\"\\ \n\t\r\0\u{7}""#),
        "{text}"
    );
    Policies::parse(&text, "").unwrap();
    for (name, m, i, want) in [
        ("nope", &[][..], &[][..], "unknown template"),
        ("viewer", &[PRICE][..], &[][..], "takes no --measure"),
        ("editor", &[][..], &[][..], "needs at least one --measure"),
        ("analyst", &[][..], &[][..], "needs at least one --item"),
        ("admin", &[][..], &[A][..], "takes no --item"),
    ] {
        let e = render(name, &Who::User("u".into()), m, i).unwrap_err();
        assert!(e.contains(want), "{name}: {e}");
    }
    for name in crate::templates::NAMES {
        let (m, i): (&[MeasureId], &[ItemId]) = match name {
            "editor" | "hide-measures" => (&[PRICE], &[]),
            "analyst" => (&[], &[A]),
            _ => (&[], &[]),
        };
        let text = render(name, &Who::User("u".into()), m, i).unwrap();
        let p = Policies::parse(&text, "").unwrap();
        assert!(p.warnings().is_empty(), "{name}: {:?}", p.warnings());
    }
}

#[test]
fn diagnostics_display() {
    let d = crate::Diagnostic {
        policy: Some("p".into()),
        line: Some(3),
        message: "m".into(),
    };
    assert_eq!(d.to_string(), "line 3: policy p: m");
}

#[test]
fn sessions_are_kept_per_version_and_evicted_oldest_first() {
    use crate::{SessionKey, Sessions};
    let p = Arc::new(
        Policies::parse(
            r#"permit (principal == Improv::User::"a", action, resource);"#,
            "",
        )
        .unwrap(),
    );
    let m = model();
    let key = |user: &str, policy_version| SessionKey {
        policy_version,
        model_version: 7,
        principal: Principal::user(user),
        channel: Channel::Server,
    };
    let cache = Sessions::new(2);
    assert!(cache.is_empty());
    let a1 = cache.get(&key("a", 1), Some(&p), &m);
    assert!(
        Arc::ptr_eq(&a1, &cache.get(&key("a", 1), Some(&p), &m)),
        "kept"
    );
    assert!(a1.measure_visible(PRICE));
    let a2 = cache.get(&key("a", 2), None, &m);
    assert!(!Arc::ptr_eq(&a1, &a2), "a new version is a new session");
    assert!(!a2.restricted());
    cache.get(&key("a", 1), Some(&p), &m); // a1 is now the newest
    cache.get(&key("b", 1), Some(&p), &m); // evicts a2
    assert_eq!(cache.len(), 2);
    assert!(Arc::ptr_eq(&a1, &cache.get(&key("a", 1), Some(&p), &m)));
    let b = cache.get(&key("b", 1), Some(&p), &m);
    assert!(!b.measure_visible(PRICE));
    assert!(!Sessions::new(0).get(&key("a", 1), None, &m).restricted());
}

// --- properties ---

/// A random model: 1-3 categories of 1-4 items, 1-4 input measures over
/// random subsets, and a derived total per input over its first category.
fn arb_model() -> impl Strategy<Value = Model> {
    (
        1..=3usize,
        prop::collection::vec(1..=4usize, 3),
        prop::collection::vec(0u8..8, 1..=4),
    )
        .prop_map(|(ncat, nitems, masks)| {
            let mut m = Model::new();
            let mut next = 10;
            for (c, n) in nitems.iter().take(ncat).enumerate() {
                let cid = CategoryId(c as u32 + 1);
                m.add_category(cid, format!("C{c}"));
                for _ in 0..*n {
                    m.add_item(ItemId(next), cid, format!("i{next}"));
                    next += 1;
                }
            }
            for (k, mask) in masks.iter().enumerate() {
                let cats: Vec<CategoryId> = (0..ncat)
                    .filter(|c| mask & (1 << c) != 0)
                    .map(|c| CategoryId(c as u32 + 1))
                    .collect();
                let id = MeasureId(100 + k as u32);
                m.add_measure(measure(
                    id,
                    &format!("M{k}"),
                    cats.clone(),
                    MeasureKind::Input,
                ));
                if let Some(first) = cats.first() {
                    let over = DimensionSpec {
                        over: vec![*first],
                        ..Default::default()
                    };
                    let e = Expr::Call(improv_core_model::FuncId(0), vec![Expr::Ref(id, over)]);
                    m.add_measure(measure(
                        MeasureId(200 + k as u32),
                        &format!("T{k}"),
                        cats[1..].to_vec(),
                        MeasureKind::Derived(Formula::new(e)),
                    ));
                }
            }
            m
        })
}

/// A random policy over the model's ids (some out of range), the users
/// u0-u2, the group g and the role r.
fn arb_policy() -> impl Strategy<Value = String> {
    let principal = prop_oneof![
        Just("principal".to_string()),
        (0..3u8).prop_map(|u| format!("principal == Improv::User::\"u{u}\"")),
        Just("principal in Improv::Group::\"g\"".to_string()),
        Just("principal in Improv::Role::\"r\"".to_string()),
    ];
    let action = prop_oneof![
        Just("action".to_string()),
        Just("action in Improv::Action::\"Read\"".to_string()),
        prop::sample::select(crate::Action::ALL.to_vec())
            .prop_map(|a| format!("action == Improv::Action::\"{}\"", a.name())),
    ];
    let resource = prop_oneof![
        Just("resource".to_string()),
        (100..104u32).prop_map(|m| format!("resource in Improv::Measure::\"{m}\"")),
        (200..204u32).prop_map(|m| format!("resource in Improv::Measure::\"{m}\"")),
        (10..22u32).prop_map(|i| format!("resource in Improv::Item::\"{i}\"")),
        (1..4u32).prop_map(|c| format!("resource in Improv::Category::\"{c}\"")),
    ];
    (any::<bool>(), principal, action, resource).prop_map(|(permit, p, a, r)| {
        format!(
            "{} ({p}, {a}, {r});",
            if permit { "permit" } else { "forbid" }
        )
    })
}

/// Every resource of the model, every action that applies to it.
fn questions(m: &Model) -> Vec<(Action, Resource)> {
    let mut out = vec![
        (Action::ViewModel, Resource::Model),
        (Action::Admin, Resource::Model),
    ];
    for c in m.categories.keys() {
        out.push((Action::EditStructure, Resource::Category(*c)));
    }
    for i in m.items.keys() {
        out.push((Action::ReadItem, Resource::Item(*i)));
        out.push((Action::EditStructure, Resource::Item(*i)));
    }
    for (id, me) in &m.measures {
        out.push((Action::ReadMeasure, Resource::Measure(*id)));
        out.push((Action::DefineMeasure, Resource::Measure(*id)));
        let mut coords = vec![Coordinate::new()];
        for c in &me.categories {
            coords = coords
                .into_iter()
                .flat_map(|co| {
                    m.categories[c]
                        .items
                        .iter()
                        .map(move |i| co.clone().with(*c, *i))
                })
                .collect();
        }
        for co in coords {
            out.push((Action::ReadCell, Resource::Cell(*id, co.clone())));
            out.push((Action::EditCell, Resource::Cell(*id, co)));
        }
    }
    out
}

/// The composite rule, asked of Cedar with no cache: what `allowed` must
/// equal.
fn uncached(p: &Policies, m: &Model, who: &Principal, a: Action, r: &Resource) -> bool {
    let fresh = Session::new(Some(Arc::new(p.clone())), m, who, Channel::Local);
    fresh
        .explain(a, r)
        .is_ok_and(|steps| steps.iter().all(|s| s.decision.allow))
}

/// User `u{u}`, with role "r" when `role`. Groups come from the document.
fn principal(u: u8, role: bool) -> Principal {
    Principal {
        user: format!("u{u}"),
        roles: if role { vec!["r".into()] } else { vec![] },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// The class cache is exact: one session answering every question
    /// (warming its cache in a random order) agrees with a fresh, uncached
    /// evaluation of the same rule.
    #[test]
    fn the_cache_equals_uncached_cedar(
        m in arb_model(),
        policies in prop::collection::vec(arb_policy(), 1..6),
        u in 0..3u8, in_group: bool, role: bool,
        order in any::<prop::sample::Index>(),
    ) {
        let text = policies.join("\n");
        let groups = if in_group { format!(r#"{{"g": {{"users": ["u{u}"]}}}}"#) } else { String::new() };
        let p = Policies::parse(&text, &groups).unwrap();
        let who = principal(u, role);
        let s = Session::new(Some(Arc::new(p.clone())), &m, &who, Channel::Local);
        let mut qs = questions(&m);
        let n = qs.len();
        qs.rotate_left(order.index(n));
        for (a, r) in &qs {
            prop_assert_eq!(s.allowed(*a, r), uncached(&p, &m, &who, *a, r), "{} on {:?}", a, r);
        }
    }

    /// A forbid never shows anything new, and a permit never hides
    /// anything.
    #[test]
    fn forbids_only_hide_and_permits_only_show(
        m in arb_model(),
        base in prop::collection::vec(arb_policy(), 1..5),
        extra in arb_policy(),
        u in 0..3u8,
    ) {
        let who = Principal::user(format!("u{u}"));
        let before = Policies::parse(&base.join("\n"), "").unwrap();
        let after = Policies::parse(&format!("{}\n{extra}", base.join("\n")), "").unwrap();
        let (b, a) = (
            Session::new(Some(Arc::new(before)), &m, &who, Channel::Local),
            Session::new(Some(Arc::new(after)), &m, &who, Channel::Local),
        );
        let forbid = extra.starts_with("forbid");
        for (act, r) in questions(&m) {
            let (was, now) = (b.allowed(act, &r), a.allowed(act, &r));
            if forbid {
                prop_assert!(was || !now, "a forbid allowed {} on {:?}", act, r);
            } else {
                prop_assert!(!was || now, "a permit denied {} on {:?}", act, r);
            }
        }
    }

    /// Template text for any principal name parses, and decides for that
    /// principal only.
    #[test]
    fn templates_quote_any_name(name in "\\PC{0,12}", other in "[a-z]{1,4}") {
        prop_assume!(name != other);
        let text = render("viewer", &Who::User(name.clone()), &[], &[]).unwrap();
        let p = Arc::new(Policies::parse(&text, "").unwrap());
        let me = Session::new(Some(p.clone()), &model(), &Principal::user(name), Channel::Local);
        let them = Session::new(Some(p), &model(), &Principal::user(other), Channel::Local);
        prop_assert!(me.measure_visible(PRICE));
        prop_assert!(!them.measure_visible(PRICE));
    }
}

/// What the design note's performance targets measure, on the canonical
/// model with 100 policies: one uncached `authorize`, and one cached cell
/// check. Run with `cargo test --release -p improv_policy -- --ignored
/// --nocapture perf`.
#[test]
#[ignore]
fn perf() {
    use std::time::Instant;
    let mut text = template("viewer", Who::Group("viewers".into()), &[], &[]);
    for i in 0..99 {
        text += &format!(
            "forbid (principal == Improv::User::\"u{i}\", action, resource in Improv::Item::\"{}\");\n",
            10 + i % 12
        );
    }
    let p = Arc::new(Policies::parse(&text, r#"{"viewers": {"users": ["vic"]}}"#).unwrap());
    assert_eq!(p.len(), 100);
    let (m, vic) = (model(), Principal::user("vic"));
    let cell = Resource::Cell(QUANTITY, at(&[(TIME, ItemId(10)), (PRODUCT, A)]));
    let mut times: Vec<u128> = (0..2001)
        .map(|_| {
            let t = Instant::now();
            assert!(
                p.authorize(&m, &vic, Channel::Server, Action::ReadCell, &cell)
                    .unwrap()
                    .allow
            );
            t.elapsed().as_nanos()
        })
        .collect();
    times.sort_unstable();
    println!("authorize, 100 policies: median {} ns", times[1000]);
    let s = Session::new(Some(p.clone()), &m, &vic, Channel::Server);
    let mut times: Vec<u128> = (0..2001)
        .map(|_| {
            let t = Instant::now();
            let steps = s.explain(Action::ReadCell, &cell).unwrap();
            assert!(steps.iter().all(|c| c.decision.allow));
            t.elapsed().as_nanos() / steps.len() as u128
        })
        .collect();
    times.sort_unstable();
    println!("session question (uncached): median {} ns", times[1000]);
    let coord = at(&[(TIME, ItemId(10)), (PRODUCT, A)]);
    assert!(s.cell_visible(QUANTITY, &coord));
    let n = 200_000;
    let t = Instant::now();
    for _ in 0..n {
        assert!(s.cell_visible(QUANTITY, &coord));
    }
    println!("cached cell check: {} ns", t.elapsed().as_nanos() / n);
    let t = Instant::now();
    for _ in 0..100 {
        Policies::parse(&text, "").unwrap();
    }
    println!(
        "parse and validate 100 policies: {} µs",
        t.elapsed().as_micros() / 100
    );
}

//! Decisions: one Cedar question ([`Policies::authorize`]), and a
//! [`Session`], one principal's cached view of one model.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cedar_policy::{
    Authorizer, Context, Entities, Entity, EntityUid, PolicySet, Request, RestrictedExpression,
};
use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, MeasureKind, Model, ViewId};

use crate::doc::{schema, uid, Policies};
use crate::{Action, Channel, Principal, Resource, MODEL_ID};

/// One Cedar decision: allowed or not, the policies that decided it (the
/// permits that allowed it, or the forbids that denied it), and any
/// evaluation errors. An error denies, whatever the policies say: Cedar
/// skips a policy that errors, which could turn a broken forbid into an
/// allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allow: bool,
    pub policies: Vec<String>,
    pub errors: Vec<String>,
}

/// One question a composite check asked, and its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub action: Action,
    pub resource: Resource,
    pub decision: Decision,
}

/// The schema's action entities (`ViewModel in Read`, …), built once.
fn action_entities() -> &'static [Entity] {
    static ACTIONS: OnceLock<Vec<Entity>> = OnceLock::new();
    ACTIONS.get_or_init(|| {
        let e = schema().action_entities().expect("the schema's actions");
        e.iter().cloned().collect()
    })
}

fn action_uid(a: Action) -> EntityUid {
    EntityUid::from_str(&format!("Improv::Action::{:?}", a.name())).expect("action uids parse")
}

fn model_uid() -> EntityUid {
    uid("Model", MODEL_ID)
}

fn entity(
    uid: EntityUid,
    parents: impl IntoIterator<Item = EntityUid>,
    attrs: Vec<(&str, RestrictedExpression)>,
) -> Entity {
    let attrs = attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    // Only string and entity attributes, which cannot fail to evaluate.
    Entity::new(uid, attrs, parents.into_iter().collect()).expect("plain attributes")
}

/// What a session needs of the model's structure: copied once, so a
/// session outlives the `Model` it was built from.
#[derive(Debug, Clone, Default)]
struct Structure {
    /// Each measure: an input?, its categories, the measures it reads.
    measures: HashMap<MeasureId, (bool, Vec<CategoryId>, Vec<MeasureId>)>,
    items: HashMap<ItemId, CategoryId>,
    categories: HashSet<CategoryId>,
    views: HashMap<ViewId, MeasureId>,
}

impl Structure {
    fn of(model: &Model) -> Structure {
        Structure {
            measures: (model.measures.iter())
                .map(|(id, m)| {
                    let reads = match &m.kind {
                        MeasureKind::Input => Vec::new(),
                        MeasureKind::Derived(f) => {
                            let mut r = f.referenced_measures();
                            r.sort_unstable();
                            r.dedup();
                            r
                        }
                    };
                    (*id, (m.is_input(), m.categories.clone(), reads))
                })
                .collect(),
            items: model
                .items
                .iter()
                .map(|(id, i)| (*id, i.category))
                .collect(),
            categories: model.categories.keys().copied().collect(),
            views: model.views.iter().map(|(id, v)| (*id, v.measure)).collect(),
        }
    }

    fn measure_entity(&self, m: MeasureId) -> Result<Entity, String> {
        let (input, _, _) = self
            .measures
            .get(&m)
            .ok_or_else(|| format!("no measure {}", m.0))?;
        let kind = if *input { "input" } else { "derived" };
        Ok(entity(
            uid("Measure", &m.0.to_string()),
            [model_uid()],
            vec![("kind", RestrictedExpression::new_string(kind.into()))],
        ))
    }

    fn category_entity(&self, c: CategoryId) -> Result<Entity, String> {
        if !self.categories.contains(&c) {
            return Err(format!("no category {}", c.0));
        }
        Ok(entity(
            uid("Category", &c.0.to_string()),
            [model_uid()],
            vec![],
        ))
    }

    /// An item's entity and its category's.
    fn item_entities(&self, i: ItemId) -> Result<[Entity; 2], String> {
        let c = *self
            .items
            .get(&i)
            .ok_or_else(|| format!("no item {}", i.0))?;
        Ok([
            entity(
                uid("Item", &i.0.to_string()),
                [uid("Category", &c.0.to_string())],
                vec![],
            ),
            self.category_entity(c)?,
        ])
    }

    /// The coordinate's items, in the measure's category order, after
    /// checking it names one item of each of the measure's categories.
    fn cell_items(&self, m: MeasureId, at: &Coordinate) -> Result<Vec<ItemId>, String> {
        let (_, cats, _) = self
            .measures
            .get(&m)
            .ok_or_else(|| format!("no measure {}", m.0))?;
        let fits = at.dims.len() == cats.len()
            && cats
                .iter()
                .all(|c| at.dims.get(c).is_some_and(|i| self.items.get(i) == Some(c)));
        if !fits {
            return Err(format!(
                "the coordinate does not name one item of each category of measure {}",
                m.0
            ));
        }
        Ok(cats.iter().map(|c| at.dims[c]).collect())
    }

    /// The resource's uid and the entities a request about it needs: it,
    /// its ancestors, and what its attributes point at.
    fn slice(&self, r: &Resource) -> Result<(EntityUid, Vec<Entity>), String> {
        let model = entity(model_uid(), [], vec![]);
        Ok(match r {
            Resource::Model => (model_uid(), vec![model]),
            Resource::Category(c) => {
                let e = self.category_entity(*c)?;
                (e.uid(), vec![e, model])
            }
            Resource::Item(i) => {
                let [item, cat] = self.item_entities(*i)?;
                (item.uid(), vec![item, cat, model])
            }
            Resource::Measure(m) => {
                let e = self.measure_entity(*m)?;
                (e.uid(), vec![e, model])
            }
            Resource::View(v) => {
                let m = *self
                    .views
                    .get(v)
                    .ok_or_else(|| format!("no view {}", v.0))?;
                let measure = self.measure_entity(m)?;
                let view = entity(
                    uid("View", &v.0.to_string()),
                    [model_uid()],
                    vec![(
                        "measure",
                        RestrictedExpression::new_entity_uid(measure.uid()),
                    )],
                );
                (view.uid(), vec![view, measure, model])
            }
            Resource::Cell(m, at) => {
                let items = self.cell_items(*m, at)?;
                let measure = self.measure_entity(*m)?;
                let id: Vec<String> = items.iter().map(|i| i.0.to_string()).collect();
                let cell_uid = uid("Cell", &format!("{}:{}", m.0, id.join(",")));
                let mut parents = vec![measure.uid()];
                let mut es = vec![measure, model];
                for i in items {
                    let [item, cat] = self.item_entities(i)?;
                    parents.push(item.uid());
                    es.extend([item, cat]);
                }
                let cell = entity(cell_uid.clone(), parents, vec![]);
                es.push(cell);
                (cell_uid, es)
            }
        })
    }
}

/// The principal's entity: a User whose parents are every group it is in
/// (transitively, so no Group entity is needed) and every role it has.
fn principal_entity(p: &Principal, groups: &BTreeSet<String>) -> Entity {
    let parents =
        (groups.iter().map(|g| uid("Group", g))).chain(p.roles.iter().map(|r| uid("Role", r)));
    entity(uid("User", &p.user), parents, vec![])
}

/// The policies that can apply to `principal` doing `action`: those whose
/// principal scope is any principal, `principal` itself, or one of its
/// groups or roles, and whose action scope can contain `action`. Exact: a
/// policy outside them is false for every such request, so dropping it
/// changes no decision and no reason. Cedar evaluates every policy of a
/// set, so a session asks of these alone.
fn applicable(policies: &Policies, principal: &Entity, action: Action) -> PolicySet {
    use cedar_policy::{ActionConstraint, PrincipalConstraint};
    let me = principal.uid();
    let (_, _, parents) = principal.clone().into_inner();
    let mine = |u: &EntityUid| *u == me || parents.contains(u);
    let act = action_uid(action);
    let groups = action_groups(action);
    let mut out = PolicySet::new();
    for p in policies.set.policies() {
        let principal_ok = match p.principal_constraint() {
            PrincipalConstraint::Any | PrincipalConstraint::Is(_) => true,
            PrincipalConstraint::Eq(u) => u == me,
            PrincipalConstraint::In(u) | PrincipalConstraint::IsIn(_, u) => mine(&u),
        };
        let action_ok = match p.action_constraint() {
            ActionConstraint::Any => true,
            ActionConstraint::Eq(a) => a == act,
            ActionConstraint::In(list) => list.iter().any(|a| *a == act || groups.contains(a)),
        };
        if principal_ok && action_ok {
            // Ids are unique in `policies.set`, so adding cannot fail.
            let _ = out.add(p.clone());
        }
    }
    out
}

/// The action groups `action` is in (`Read`, `Write`, `Build`).
fn action_groups(action: Action) -> Vec<EntityUid> {
    let group = |n: &str| EntityUid::from_str(&format!("Improv::Action::{n:?}")).expect("uid");
    match action {
        Action::ViewModel | Action::ReadMeasure | Action::ReadItem | Action::ReadCell => {
            vec![group("Read")]
        }
        Action::EditCell => vec![group("Write")],
        Action::DefineMeasure | Action::EditStructure | Action::ManageViews => {
            vec![group("Build")]
        }
        Action::Admin => vec![],
    }
}

/// The entities every request of `principal` shares: the schema's actions
/// and the principal.
fn base_entities(principal: &Entity) -> Entities {
    Entities::from_entities(
        action_entities().iter().cloned().chain([principal.clone()]),
        None,
    )
    .expect("actions and one user are a valid store")
}

/// Ask Cedar once: `set` holds the policies to evaluate, `base` the
/// entities from [`base_entities`].
fn ask(
    set: &PolicySet,
    base: &Entities,
    structure: &Structure,
    principal: &Entity,
    channel: Channel,
    action: Action,
    resource: &Resource,
) -> Result<Decision, String> {
    static AUTHORIZER: OnceLock<Authorizer> = OnceLock::new();
    let (resource, slice) = structure.slice(resource)?;
    let context = Context::from_pairs([(
        "channel".to_string(),
        RestrictedExpression::new_string(channel.name().into()),
    )])
    .map_err(|e| e.to_string())?;
    let request = Request::new(
        principal.uid(),
        action_uid(action),
        resource,
        context,
        Some(schema()),
    )
    .map_err(|e| format!("{action} does not apply to this resource: {e}"))?;
    let entities = (base.clone())
        .add_entities(slice, None)
        .map_err(|e| e.to_string())?;
    let response = AUTHORIZER
        .get_or_init(Authorizer::new)
        .is_authorized(&request, set, &entities);
    let errors: Vec<String> = response
        .diagnostics()
        .errors()
        .map(ToString::to_string)
        .collect();
    let mut decided: Vec<String> = response
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect();
    decided.sort();
    Ok(Decision {
        allow: response.decision() == cedar_policy::Decision::Allow && errors.is_empty(),
        policies: decided,
        errors,
    })
}

impl Policies {
    /// Ask once, uncached, whether `principal` may do `action` on
    /// `resource` of `model`. This is the raw Cedar question; enforcement
    /// uses a [`Session`]'s composite rules, which ask several. An error
    /// when the resource is not in the model, or the action does not apply
    /// to it (ReadCell on a measure).
    pub fn authorize(
        &self,
        model: &Model,
        principal: &Principal,
        channel: Channel,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, String> {
        let me = principal_entity(principal, &self.groups().of(&principal.user));
        let base = base_entities(&me);
        ask(
            &self.set,
            &base,
            &Structure::of(model),
            &me,
            channel,
            action,
            resource,
        )
    }
}

/// The cache's key: decisions are equal within a class. A cell's decision
/// depends only on its measure and on those of its items some policy names
/// (cells have no attributes and no policy names one); an item's on its
/// category, unless a policy names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Class {
    Model,
    Category(CategoryId),
    Item(CategoryId, Option<ItemId>),
    Measure(MeasureId),
    View(ViewId),
    Cell(MeasureId, Vec<ItemId>),
}

/// What a finding of [`Session::lint`] says a readable derived measure
/// reveals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reveals {
    /// It reads this hidden measure (directly, or through hidden measures).
    Measure(MeasureId),
    /// It aggregates over this category, which has these hidden items.
    Items(CategoryId, Vec<ItemId>),
}

/// One principal's view of one model under one policy version: decisions
/// cached by class, and the composite rules every enforcement point
/// applies. Build a new one when the policies or the model's structure
/// change. Without policies everything is allowed (0.12's behavior).
pub struct Session {
    policies: Option<Arc<Policies>>,
    principal: Entity,
    /// The entities every request shares (see [`base_entities`]).
    base: Entities,
    /// Per action, the policies that can apply ([`applicable`]).
    sets: Mutex<HashMap<Action, Arc<PolicySet>>>,
    channel: Channel,
    structure: Structure,
    named_items: HashSet<ItemId>,
    cache: Mutex<HashMap<(Action, Class), bool>>,
}

impl Session {
    /// A session for `principal` over `model`; `policies` `None` is an
    /// unrestricted model.
    pub fn new(
        policies: Option<Arc<Policies>>,
        model: &Model,
        principal: &Principal,
        channel: Channel,
    ) -> Session {
        let groups = (policies.as_ref())
            .map(|p| p.groups().of(&principal.user))
            .unwrap_or_default();
        let named_items = (policies.iter())
            .flat_map(|p| p.named.iter())
            .filter(|(ty, _)| ty == "Improv::Item")
            .filter_map(|(_, id)| id.parse().ok().map(ItemId))
            .collect();
        let me = principal_entity(principal, &groups);
        Session {
            base: base_entities(&me),
            sets: Mutex::new(HashMap::new()),
            principal: me,
            policies,
            channel,
            structure: Structure::of(model),
            named_items,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the model has policies (else everything is allowed).
    pub fn restricted(&self) -> bool {
        self.policies.is_some()
    }

    fn class(&self, r: &Resource) -> Option<Class> {
        let named = |i: &ItemId| self.named_items.contains(i);
        Some(match r {
            Resource::Model => Class::Model,
            Resource::Category(c) => Class::Category(*c),
            Resource::Item(i) => Class::Item(*self.structure.items.get(i)?, Some(*i).filter(named)),
            Resource::Measure(m) => Class::Measure(*m),
            Resource::View(v) => Class::View(*v),
            Resource::Cell(m, at) => {
                Class::Cell(*m, at.dims.values().copied().filter(named).collect())
            }
        })
    }

    /// One raw question, cached. False for a resource not in the model.
    fn decide(&self, action: Action, r: &Resource) -> bool {
        let Some(policies) = &self.policies else {
            return true;
        };
        let Some(class) = self.class(r) else {
            return false;
        };
        let key = (action, class);
        if let Some(d) = self.lock().get(&key) {
            return *d;
        }
        let allow = self.ask(policies, action, r).is_ok_and(|d| d.allow);
        self.lock().insert(key, allow);
        allow
    }

    /// Ask Cedar, uncached, of the policies that can apply to this
    /// principal and `action`.
    fn ask(&self, policies: &Policies, action: Action, r: &Resource) -> Result<Decision, String> {
        let set = {
            let mut sets = self.sets.lock().unwrap_or_else(PoisonError::into_inner);
            let set = sets
                .entry(action)
                .or_insert_with(|| Arc::new(applicable(policies, &self.principal, action)));
            set.clone()
        };
        ask(
            &set,
            &self.base,
            &self.structure,
            &self.principal,
            self.channel,
            action,
            r,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(Action, Class), bool>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The questions [`Session::allowed`] asks for `action` on `resource`,
    /// in order: ViewModel first, then what the resource needs to be seen,
    /// then the action itself.
    fn steps(&self, action: Action, r: &Resource) -> Result<Vec<(Action, Resource)>, String> {
        let mut steps = vec![(Action::ViewModel, Resource::Model)];
        match (action, r) {
            (Action::ViewModel, Resource::Model) => return Ok(steps),
            (Action::ReadCell | Action::EditCell, Resource::Cell(m, at)) => {
                steps.push((Action::ReadMeasure, Resource::Measure(*m)));
                for i in self.structure.cell_items(*m, at)? {
                    steps.push((Action::ReadItem, Resource::Item(i)));
                }
                steps.push((Action::ReadCell, r.clone()));
            }
            (Action::DefineMeasure, Resource::Measure(m)) => {
                steps.push((Action::ReadMeasure, Resource::Measure(*m)));
            }
            (Action::EditStructure, Resource::Item(i)) => {
                steps.push((Action::ReadItem, Resource::Item(*i)));
            }
            (Action::ManageViews, Resource::View(v)) => {
                let m = (self.structure.views.get(v)).ok_or_else(|| format!("no view {}", v.0))?;
                steps.push((Action::ReadMeasure, Resource::Measure(*m)));
            }
            _ => {}
        }
        if !steps.iter().any(|(a, res)| *a == action && res == r) {
            steps.push((action, r.clone()));
        }
        Ok(steps)
    }

    /// Whether `principal` may do `action` on `resource`, by the composite
    /// rules: every action needs ViewModel; a cell needs its measure, each
    /// of its items and the cell readable, and EditCell an input measure;
    /// redefining a measure, renaming an item or changing a view needs it
    /// (or its measure) readable. False for a resource not in the model.
    pub fn allowed(&self, action: Action, resource: &Resource) -> bool {
        if self.policies.is_none() {
            return self.structure.exists(resource);
        }
        if action == Action::EditCell && !self.is_input(resource) {
            return false;
        }
        if let (Action::ReadCell | Action::EditCell, Resource::Cell(m, at)) = (action, resource) {
            if let Some(hit) = self.cell_cached(action, *m, at) {
                return hit;
            }
        }
        match self.steps(action, resource) {
            Ok(steps) => steps.iter().all(|(a, r)| self.decide(*a, r)),
            Err(_) => false,
        }
    }

    /// The cell rule from the cache alone, with one lock and no clones:
    /// `None` when a step is not cached yet (the slow path then asks).
    fn cell_cached(&self, action: Action, m: MeasureId, at: &Coordinate) -> Option<bool> {
        let items = self.structure.cell_items(m, at).ok()?;
        let cache = self.lock();
        let get = |a: Action, c: Class| cache.get(&(a, c)).copied();
        let mut all =
            get(Action::ViewModel, Class::Model)? && get(Action::ReadMeasure, Class::Measure(m))?;
        for i in &items {
            let named = self.named_items.contains(i).then_some(*i);
            all = all
                && get(
                    Action::ReadItem,
                    Class::Item(self.structure.items[i], named),
                )?;
        }
        let named: Vec<ItemId> = items
            .iter()
            .copied()
            .filter(|i| self.named_items.contains(i))
            .collect();
        all = all && get(Action::ReadCell, Class::Cell(m, named.clone()))?;
        if action == Action::EditCell {
            all = all && get(Action::EditCell, Class::Cell(m, named))?;
        }
        Some(all)
    }

    fn is_input(&self, r: &Resource) -> bool {
        matches!(r, Resource::Cell(m, _) if self.structure.measures.get(m).is_some_and(|(input, ..)| *input))
    }

    /// A measure's name, type and dimensions may be shown.
    pub fn measure_visible(&self, m: MeasureId) -> bool {
        self.allowed(Action::ReadMeasure, &Resource::Measure(m))
    }

    /// An item's name may be shown.
    pub fn item_visible(&self, i: ItemId) -> bool {
        self.allowed(Action::ReadItem, &Resource::Item(i))
    }

    /// A cell's value may be shown.
    pub fn cell_visible(&self, m: MeasureId, at: &Coordinate) -> bool {
        self.allowed(Action::ReadCell, &Resource::Cell(m, at.clone()))
    }

    /// An input cell may be set or cleared.
    pub fn cell_editable(&self, m: MeasureId, at: &Coordinate) -> bool {
        self.allowed(Action::EditCell, &Resource::Cell(m, at.clone()))
    }

    /// A measure's formula may be shown: the measure and every measure the
    /// formula reads are visible.
    pub fn formula_visible(&self, m: MeasureId) -> bool {
        self.measure_visible(m)
            && (self.structure.measures.get(&m))
                .is_some_and(|(_, _, reads)| reads.iter().all(|r| self.measure_visible(*r)))
    }

    /// Every question [`Session::allowed`] asks, uncached, with the
    /// policies that decided each, stopping at the first denial. For a
    /// model without policies, one allowing step that names no policy.
    pub fn explain(&self, action: Action, resource: &Resource) -> Result<Vec<Check>, String> {
        let Some(policies) = &self.policies else {
            if !self.structure.exists(resource) {
                return Err("the resource is not in the model".into());
            }
            return Ok(vec![Check {
                action,
                resource: resource.clone(),
                decision: Decision {
                    allow: true,
                    policies: Vec::new(),
                    errors: Vec::new(),
                },
            }]);
        };
        let steps = self.steps(action, resource)?;
        // As in `allowed`: a derived cell is never edited, whatever permits.
        if action == Action::EditCell && !self.is_input(resource) {
            return Ok(vec![Check {
                action,
                resource: resource.clone(),
                decision: Decision {
                    allow: false,
                    policies: Vec::new(),
                    errors: vec!["the measure is derived: its cells are computed".into()],
                },
            }]);
        }
        let mut out = Vec::new();
        for (a, r) in steps {
            let decision = self.ask(policies, a, &r)?;
            let deny = !decision.allow;
            out.push(Check {
                action: a,
                resource: r,
                decision,
            });
            if deny {
                break;
            }
        }
        Ok(out)
    }

    /// What the readable derived measures reveal of what is hidden: each
    /// reads a hidden measure (directly, or through hidden measures it
    /// reads), or aggregates over a category with hidden items (it lacks a
    /// category an input it reads has). A total over hidden items is
    /// computed over all of them, so the visible items and the total give
    /// the hidden ones' sum. Sorted by measure.
    pub fn lint(&self) -> Vec<(MeasureId, Reveals)> {
        let mut out = Vec::new();
        let mut ids: Vec<MeasureId> = self.structure.measures.keys().copied().collect();
        ids.sort_unstable();
        for m in ids {
            let (input, cats, _) = &self.structure.measures[&m];
            if *input || !self.measure_visible(m) {
                continue;
            }
            let (hidden, inputs) = self.reaches(m);
            out.extend(hidden.into_iter().map(|h| (m, Reveals::Measure(h))));
            let mut over: Vec<CategoryId> = (inputs.iter())
                .flat_map(|i| self.structure.measures[i].1.iter().copied())
                .filter(|c| !cats.contains(c))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            over.retain(|c| self.structure.categories.contains(c));
            for c in over {
                let mut items: Vec<ItemId> = (self.structure.items.iter())
                    .filter(|(i, cat)| **cat == c && !self.item_visible(**i))
                    .map(|(i, _)| *i)
                    .collect();
                items.sort_unstable();
                if !items.is_empty() {
                    out.push((m, Reveals::Items(c, items)));
                }
            }
        }
        out
    }

    /// From derived measure `m`: the hidden measures it reads, stopping at
    /// each visible one (whose own reads it reports itself), and every
    /// input it reads at any depth.
    fn reaches(&self, m: MeasureId) -> (BTreeSet<MeasureId>, BTreeSet<MeasureId>) {
        let (mut hidden, mut inputs, mut seen) = (BTreeSet::new(), BTreeSet::new(), HashSet::new());
        let mut todo = vec![(m, true)];
        while let Some((at, report)) = todo.pop() {
            if !seen.insert((at, report)) {
                continue;
            }
            let Some((input, _, reads)) = self.structure.measures.get(&at) else {
                continue;
            };
            if *input {
                inputs.insert(at);
            }
            for r in reads {
                let visible = self.measure_visible(*r);
                if report && !visible {
                    hidden.insert(*r);
                }
                // Past a visible measure, its own finding covers what it reads.
                todo.push((*r, report && !visible));
            }
        }
        (hidden, inputs)
    }
}

impl Structure {
    fn exists(&self, r: &Resource) -> bool {
        match r {
            Resource::Model => true,
            Resource::Category(c) => self.categories.contains(c),
            Resource::Item(i) => self.items.contains_key(i),
            Resource::Measure(m) => self.measures.contains_key(m),
            Resource::View(v) => self.views.contains_key(v),
            Resource::Cell(m, at) => self.cell_items(*m, at).is_ok(),
        }
    }
}

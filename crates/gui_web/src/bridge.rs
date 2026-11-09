//! The accessibility bridge in the browser
//! (docs/reviews/2026-11-08-web-accessibility.md). An egui plugin takes
//! each pass's AccessKit tree before eframe's web runner drops it, and
//! keeps hidden DOM inside the `<canvas>` in step with it ([`crate::a11y`]
//! says what changes). Canvas content is in the accessibility tree and never
//! drawn. A screen reader's actions on that DOM come back to egui as
//! AccessKit requests: a click is a `Click`, moving onto an element is a
//! `Focus`. egui's keyboard focus is the canvas's `aria-activedescendant`.

use std::cell::RefCell;
use std::collections::HashMap;

use egui::accesskit::{Action, ActionRequest, NodeId, TreeId, TreeUpdate};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlCanvasElement, Text};

use crate::a11y::{dom_id, node_of, El, Op, Tree};

struct Dom {
    doc: web_sys::Document,
    canvas: HtmlCanvasElement,
    /// Each node's element, and its text, the element's first child.
    els: HashMap<u64, (Element, Option<Text>)>,
    tree: Tree,
    /// Requests from the DOM for the next pass.
    queue: Vec<ActionRequest>,
}

thread_local! {
    static DOM: RefCell<Option<Dom>> = const { RefCell::new(None) };
}

/// Turn the bridge on for the app drawn into `canvas`.
pub fn install(ctx: &egui::Context, canvas: &HtmlCanvasElement) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    // Every key goes to the app: a screen reader leaves it the keyboard.
    let _ = canvas.set_attribute("role", "application");
    let _ = canvas.set_attribute("aria-label", "Improv");
    DOM.set(Some(Dom {
        doc,
        canvas: canvas.clone(),
        els: HashMap::new(),
        tree: Tree::default(),
        queue: Vec::new(),
    }));
    for (event, action) in [("click", Action::Click), ("focusin", Action::Focus)] {
        let ctx = ctx.clone();
        let f = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
            let Some(node) = target_node(&e) else {
                return;
            };
            DOM.with_borrow_mut(|d| {
                let Some(d) = d else {
                    return;
                };
                d.queue.push(ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: NodeId(node),
                    data: None,
                });
                if action == Action::Focus {
                    // The keys go to the canvas (eframe reads them there);
                    // the element stays the focus as its active descendant.
                    let o = web_sys::FocusOptions::new();
                    o.set_prevent_scroll(true);
                    let _ = d.canvas.focus_with_options(&o);
                }
            });
            ctx.request_repaint();
        });
        let _ = canvas.add_event_listener_with_callback(event, f.as_ref().unchecked_ref());
        f.forget();
    }
    ctx.enable_accesskit();
    ctx.add_plugin(Bridge);
}

/// The mirror node an event happened on.
fn target_node(e: &web_sys::Event) -> Option<u64> {
    let el = e.target()?.dyn_into::<Element>().ok()?;
    node_of(&el.closest("[id^='ak-']").ok()??.id())
}

struct Bridge;

impl egui::plugin::Plugin for Bridge {
    fn debug_name(&self) -> &'static str {
        "improv_a11y"
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        DOM.with_borrow_mut(|d| {
            if let Some(d) = d {
                let reqs = d.queue.drain(..);
                input
                    .events
                    .extend(reqs.map(egui::Event::AccessKitActionRequest));
            }
        });
    }

    fn output_hook(&mut self, _ctx: &egui::Context, out: &mut egui::FullOutput) {
        let Some(u) = out.platform_output.accesskit_update.take() else {
            return;
        };
        DOM.with_borrow_mut(|d| {
            if let Some(d) = d {
                d.update(&u);
            }
        });
    }
}

impl Dom {
    fn update(&mut self, u: &TreeUpdate) {
        let next = Tree::from_update(u);
        for op in self.tree.diff(&next) {
            self.apply(op, &next);
        }
        self.tree = next;
    }

    fn apply(&mut self, op: Op, next: &Tree) {
        match op {
            Op::Put(id, el) => self.put(id, &el),
            Op::Kids(id, kids) => {
                let Some((e, text)) = self.els.get(&id) else {
                    return;
                };
                let all = js_sys::Array::new();
                if let Some(t) = text {
                    all.push(t);
                }
                for k in kids {
                    if let Some((c, _)) = self.els.get(&k) {
                        all.push(c);
                    }
                }
                e.replace_children_with_node(&all);
            }
            Op::Root(id) => {
                if let Some((e, _)) = self.els.get(&id) {
                    self.canvas.replace_children_with_node_1(e);
                }
            }
            Op::Drop(id) => {
                if let Some((e, _)) = self.els.remove(&id) {
                    e.remove();
                }
            }
            Op::Focus(f) => self.focus(f.filter(|f| Some(*f) != next.root), next),
        }
    }

    /// Make node `id`'s element, or bring it up to date in place.
    fn put(&mut self, id: u64, el: &El) {
        let old = self.tree.els.get(&id);
        let (e, text) = match self.els.get_mut(&id) {
            Some(x) => x,
            None => {
                let Ok(e) = self.doc.create_element("div") else {
                    return;
                };
                let _ = e.set_attribute("id", &dom_id(id));
                self.els.entry(id).or_insert((e, None))
            }
        };
        if let Some(old) = old {
            for (k, _) in &old.attrs {
                if !el.attrs.iter().any(|(n, _)| n == k) {
                    let _ = e.remove_attribute(k);
                }
            }
        }
        let set = |k: &str, v: Option<&str>| {
            let _ = match v {
                Some(v) => e.set_attribute(k, v),
                None => e.remove_attribute(k),
            };
        };
        set("role", el.role);
        set("tabindex", el.active.then_some("-1"));
        for (k, v) in &el.attrs {
            set(k, Some(v));
        }
        match (&el.text, text.as_ref()) {
            (Some(s), Some(t)) => t.set_data(s),
            (Some(s), None) => {
                let t = self.doc.create_text_node(s);
                let _ = e.prepend_with_node_1(&t);
                *text = Some(t);
            }
            (None, Some(t)) => {
                t.remove();
                *text = None;
            }
            (None, None) => {}
        }
    }

    /// egui's focus moved to node `f` (`None`: nothing has it).
    fn focus(&self, f: Option<u64>, next: &Tree) {
        let _ = match f {
            Some(id) => self
                .canvas
                .set_attribute("aria-activedescendant", &dom_id(id)),
            None => self.canvas.remove_attribute("aria-activedescendant"),
        };
        // A text field has eframe's hidden input (its text agent, the
        // canvas's next sibling) as its DOM focus: name it for the field.
        // Otherwise it is hidden, and out of the Tab order.
        let Some(agent) = self
            .canvas
            .next_element_sibling()
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let el = f.and_then(|id| next.els.get(&id));
        let field = el.filter(|e| e.role == Some("textbox"));
        let name = field.and_then(|e| {
            e.attrs
                .iter()
                .find(|(k, _)| *k == "aria-label" || *k == "aria-placeholder")
                .map(|(_, v)| v.as_str())
        });
        let _ = agent.set_attribute("tabindex", "-1");
        let _ = match field {
            Some(_) => agent.remove_attribute("aria-hidden"),
            None => agent.set_attribute("aria-hidden", "true"),
        };
        let _ = match name {
            Some(n) => agent.set_attribute("aria-label", n),
            None => agent.remove_attribute("aria-label"),
        };
        // What is typed in a password field is not read out.
        let kind = if el.is_some_and(|e| e.secret) {
            "password"
        } else {
            "text"
        };
        if agent.type_() != kind {
            agent.set_type(kind);
        }
    }
}

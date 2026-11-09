//! AccessKit in a browser: egui builds the same accessibility tree for the
//! web as for the desktop, but eframe's web runner drops it. This module
//! turns that tree into hidden DOM for the browser's accessibility tree
//! (docs/reviews/2026-11-08-web-accessibility.md): one element per node,
//! with its ARIA role, name and states, and the operations that bring
//! last frame's DOM up to date with this frame's tree. Pure, so it is
//! tested natively; `web.rs` applies the operations.

use std::collections::{HashMap, HashSet};

use accesskit::{Action, HasPopup, Live, Node, NodeId, Role, Toggled, TreeUpdate};

/// One element of the mirror: what the DOM needs to say about a node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct El {
    /// ARIA role; `None` for a plain container (it stays in the tree only
    /// to hold its children).
    pub role: Option<&'static str>,
    /// Attributes besides `role`, sorted by name.
    pub attrs: Vec<(&'static str, String)>,
    /// Text content (a label's words), shown to the accessibility tree.
    pub text: Option<String>,
    /// The node takes the focus: `tabindex=-1`, so a screen reader can
    /// move onto it. (Any element's activation is sent as a click.)
    pub active: bool,
    /// A password field: what is typed in it must not be echoed.
    pub secret: bool,
}

/// The mirror's whole state: an element and its children per node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    pub root: Option<u64>,
    pub els: HashMap<u64, El>,
    pub kids: HashMap<u64, Vec<u64>>,
    /// The node egui's keyboard focus is on (the root when none is).
    pub focus: Option<u64>,
}

/// A change to the DOM, in an order that keeps it valid: elements are
/// made before they are placed, and removed after they are unplaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Make node `id`'s element, or update it in place (its children stay).
    Put(u64, El),
    /// Set node `id`'s children to these, in this order.
    Kids(u64, Vec<u64>),
    /// Forget node `id`'s element (it is no longer in the tree).
    Drop(u64),
    /// The root changed.
    Root(u64),
    /// egui's focus moved here.
    Focus(Option<u64>),
}

impl Tree {
    /// The tree egui sent this frame. egui sends every node every frame, so
    /// the update is the whole tree; nodes not reachable from the root
    /// (none should be) are left out.
    pub fn from_update(u: &TreeUpdate) -> Tree {
        let nodes: HashMap<NodeId, &Node> = u.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let root = u
            .tree
            .as_ref()
            .map(|t| t.root)
            .or_else(|| u.nodes.first().map(|(id, _)| *id));
        let mut t = Tree {
            root: root.map(|r| r.0),
            focus: Some(u.focus.0),
            ..Tree::default()
        };
        let mut stack: Vec<NodeId> = root.into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = nodes.get(&id) else {
                continue;
            };
            // Text runs repeat their widget's value, which the widget's
            // element already says.
            let kids: Vec<u64> = n
                .children()
                .iter()
                .filter(|c| nodes.get(c).is_some_and(|k| k.role() != Role::TextRun))
                .map(|c| c.0)
                .collect();
            stack.extend(kids.iter().rev().map(|k| NodeId(*k)));
            t.els.insert(id.0, element(n, &nodes));
            t.kids.insert(id.0, kids);
        }
        if t.focus.is_some_and(|f| !t.els.contains_key(&f)) {
            t.focus = t.root;
        }
        t
    }

    /// The operations that turn `self` (last frame's DOM) into `next`.
    pub fn diff(&self, next: &Tree) -> Vec<Op> {
        let mut ops = Vec::new();
        // New and changed elements first, so every child exists before a
        // parent lists it.
        let mut ids: Vec<&u64> = next.els.keys().collect();
        ids.sort_unstable();
        for id in &ids {
            let el = &next.els[id];
            if self.els.get(id) != Some(el) {
                ops.push(Op::Put(**id, el.clone()));
            }
        }
        for id in &ids {
            let k = &next.kids[id];
            if self.kids.get(id).map_or(!k.is_empty(), |old| old != k) {
                ops.push(Op::Kids(**id, k.clone()));
            }
        }
        if next.root != self.root {
            if let Some(r) = next.root {
                ops.push(Op::Root(r));
            }
        }
        let mut gone: Vec<&u64> = self
            .els
            .keys()
            .filter(|id| !next.els.contains_key(id))
            .collect();
        gone.sort_unstable();
        ops.extend(gone.into_iter().map(|id| Op::Drop(*id)));
        if next.focus != self.focus {
            ops.push(Op::Focus(next.focus));
        }
        ops
    }
}

/// The element for one node.
fn element(n: &Node, nodes: &HashMap<NodeId, &Node>) -> El {
    let role = n.role();
    let mut a: Vec<(&'static str, String)> = Vec::new();
    // A label that takes the focus (a window's title) is a heading: a
    // screen reader says nothing for a focused element with no role.
    let aria = match role {
        Role::Label if n.supports_action(Action::Focus) => Some("heading"),
        _ => aria_role(role),
    };
    // A name: the label, else what labels it, else (for a control) its value.
    let label = n.label().map(str::to_owned).or_else(|| {
        let by: Vec<&str> = n
            .labelled_by()
            .iter()
            .filter_map(|id| nodes.get(id).and_then(|l| l.label().or(l.value())))
            .collect();
        (!by.is_empty()).then(|| by.join(" "))
    });
    // Content: a label's or a live region's words (egui puts a label's in
    // its value), and a text field's or a list's value (the browser reads a
    // textbox's and a combobox's value from its content). A password is
    // never shown.
    let words = matches!(role, Role::Label | Role::Status | Role::Alert | Role::Log);
    let text = match role {
        _ if words => n.value().or(n.label()),
        Role::PasswordInput => None,
        _ if matches!(aria, Some("textbox" | "combobox")) => n.value(),
        _ => None,
    }
    .map(str::to_owned);
    if !words {
        if let Some(l) = label.filter(|l| !l.is_empty()) {
            a.push(("aria-label", l));
        }
        if let Some(v) = n
            .value()
            .filter(|_| text.is_none() && role != Role::PasswordInput)
        {
            a.push(("aria-description", v.to_owned()));
        }
    }
    if let Some(p) = n.placeholder() {
        a.push(("aria-placeholder", p.to_owned()));
    }
    if n.is_disabled() {
        a.push(("aria-disabled", "true".into()));
    }
    if n.is_read_only() {
        a.push(("aria-readonly", "true".into()));
    }
    if let Some(t) = n.toggled() {
        let v = match t {
            Toggled::True => "true",
            Toggled::False => "false",
            Toggled::Mixed => "mixed",
        };
        // A button that toggles is pressed; a check box is checked.
        let k = match aria {
            Some("button") => "aria-pressed",
            _ => "aria-checked",
        };
        a.push((k, v.into()));
    }
    if let Some(s) = n.is_selected() {
        a.push(("aria-selected", s.to_string()));
    }
    if let Some(e) = n.is_expanded() {
        a.push(("aria-expanded", e.to_string()));
    }
    if let Some(p) = n.has_popup() {
        let v = match p {
            HasPopup::Menu => "menu",
            HasPopup::Listbox => "listbox",
            HasPopup::Tree => "tree",
            HasPopup::Grid => "grid",
            HasPopup::Dialog => "dialog",
        };
        a.push(("aria-haspopup", v.into()));
    }
    if let Some(l) = n.live() {
        let v = match l {
            Live::Off => "off",
            Live::Polite => "polite",
            Live::Assertive => "assertive",
        };
        a.push(("aria-live", v.into()));
    }
    // AccessKit counts from zero, ARIA from one.
    for (k, v) in [
        ("aria-rowcount", n.row_count()),
        ("aria-colcount", n.column_count()),
    ] {
        if let Some(v) = v {
            a.push((k, v.to_string()));
        }
    }
    for (k, v) in [
        ("aria-rowindex", n.row_index()),
        ("aria-colindex", n.column_index()),
    ] {
        if let Some(v) = v {
            a.push((k, (v + 1).to_string()));
        }
    }
    a.sort_unstable();
    El {
        role: aria,
        attrs: a,
        text,
        active: n.supports_action(Action::Focus),
        secret: role == Role::PasswordInput,
    }
}

/// The ARIA role for an AccessKit role (`None`: a plain container).
pub fn aria_role(r: Role) -> Option<&'static str> {
    Some(match r {
        Role::Button | Role::DefaultButton => "button",
        Role::CheckBox => "checkbox",
        Role::RadioButton => "radio",
        Role::Switch => "switch",
        Role::Link => "link",
        Role::TextInput
        | Role::MultilineTextInput
        | Role::SearchInput
        | Role::PasswordInput
        | Role::EmailInput
        | Role::NumberInput
        | Role::PhoneNumberInput
        | Role::UrlInput => "textbox",
        Role::ComboBox | Role::EditableComboBox => "combobox",
        Role::ListBox => "listbox",
        Role::ListBoxOption | Role::MenuListOption => "option",
        Role::Menu | Role::MenuListPopup => "menu",
        Role::MenuBar => "menubar",
        Role::MenuItem => "menuitem",
        Role::MenuItemCheckBox => "menuitemcheckbox",
        Role::MenuItemRadio => "menuitemradio",
        Role::Slider => "slider",
        Role::SpinButton => "spinbutton",
        Role::ProgressIndicator => "progressbar",
        Role::Grid => "grid",
        Role::Table => "table",
        Role::TreeGrid => "treegrid",
        Role::Row => "row",
        Role::RowGroup => "rowgroup",
        Role::RowHeader => "rowheader",
        Role::ColumnHeader => "columnheader",
        Role::GridCell => "gridcell",
        Role::Cell => "cell",
        Role::List => "list",
        Role::ListItem => "listitem",
        Role::Tree => "tree",
        Role::TreeItem => "treeitem",
        Role::Tab => "tab",
        Role::TabList => "tablist",
        Role::TabPanel => "tabpanel",
        Role::Dialog => "dialog",
        Role::AlertDialog => "alertdialog",
        Role::Alert => "alert",
        Role::Status => "status",
        Role::Log => "log",
        Role::Tooltip => "tooltip",
        Role::Toolbar => "toolbar",
        Role::Heading => "heading",
        Role::Image => "img",
        Role::Group | Role::RadioGroup => "group",
        // A worksheet window, a pane: a landmark a screen reader lists.
        Role::Window | Role::Pane | Role::Region => "region",
        Role::ScrollBar => "scrollbar",
        Role::Application => "application",
        _ => return None,
    })
}

/// The DOM id of node `id`'s element (what `aria-activedescendant` names).
pub fn dom_id(id: u64) -> String {
    format!("ak-{id}")
}

/// The node id of a mirror element's DOM id.
pub fn node_of(dom: &str) -> Option<u64> {
    dom.strip_prefix("ak-")?.parse().ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use accesskit::{TreeId, TreeInfo};

    fn node(role: Role, f: impl FnOnce(&mut Node)) -> Node {
        let mut n = Node::new(role);
        f(&mut n);
        n
    }

    fn update(nodes: Vec<(u64, Node)>, focus: u64) -> TreeUpdate {
        TreeUpdate {
            nodes: nodes.into_iter().map(|(i, n)| (NodeId(i), n)).collect(),
            tree: Some(TreeInfo::new(NodeId(1))),
            tree_id: TreeId::ROOT,
            focus: NodeId(focus),
        }
    }

    fn attr<'a>(el: &'a El, k: &str) -> Option<&'a str> {
        el.attrs
            .iter()
            .find(|(a, _)| *a == k)
            .map(|(_, v)| v.as_str())
    }

    /// A worksheet: a window holding a grid, one header row and one row.
    fn sheet(value: &str, focus: u64) -> TreeUpdate {
        update(
            vec![
                (
                    1,
                    node(Role::Window, |n| {
                        n.set_children(vec![NodeId(2)]);
                    }),
                ),
                (
                    2,
                    node(Role::Grid, |n| {
                        n.set_label("Quantity");
                        n.set_row_count(2);
                        n.set_column_count(2);
                        n.set_children(vec![NodeId(3), NodeId(4)]);
                    }),
                ),
                (
                    3,
                    node(Role::Row, |n| {
                        n.set_row_index(0);
                        n.set_children(vec![NodeId(5)]);
                    }),
                ),
                (
                    4,
                    node(Role::Row, |n| {
                        n.set_row_index(1);
                        n.set_children(vec![NodeId(6), NodeId(7)]);
                    }),
                ),
                (
                    5,
                    node(Role::ColumnHeader, |n| {
                        n.set_label("Widget");
                        n.set_column_index(1);
                    }),
                ),
                (
                    6,
                    node(Role::RowHeader, |n| {
                        n.set_label("2025Q1");
                        n.set_column_index(0);
                    }),
                ),
                (
                    7,
                    node(Role::GridCell, |n| {
                        n.set_label(format!("2025Q1, Widget: {value}"));
                        n.set_column_index(1);
                        n.add_action(Action::Focus);
                        n.add_action(Action::Click);
                    }),
                ),
            ],
            focus,
        )
    }

    #[test]
    fn a_grid_is_marked_up_with_its_headers_and_indices() {
        let t = Tree::from_update(&sheet("100", 7));
        assert_eq!(t.root, Some(1));
        assert_eq!(t.focus, Some(7));
        assert_eq!(t.kids[&1], vec![2]);
        assert_eq!(t.kids[&4], vec![6, 7]);
        let grid = &t.els[&2];
        assert_eq!(grid.role, Some("grid"));
        assert_eq!(attr(grid, "aria-label"), Some("Quantity"));
        assert_eq!(attr(grid, "aria-rowcount"), Some("2"));
        assert_eq!(attr(grid, "aria-colcount"), Some("2"));
        // ARIA counts from one.
        assert_eq!(attr(&t.els[&4], "aria-rowindex"), Some("2"));
        assert_eq!(t.els[&5].role, Some("columnheader"));
        assert_eq!(attr(&t.els[&6], "aria-colindex"), Some("1"));
        let cell = &t.els[&7];
        assert_eq!(cell.role, Some("gridcell"));
        assert_eq!(attr(cell, "aria-label"), Some("2025Q1, Widget: 100"));
        assert_eq!(attr(cell, "aria-colindex"), Some("2"));
        assert!(cell.active);
        assert_eq!(attr(cell, "aria-readonly"), None);
        assert!(!t.els[&5].active);
        assert_eq!(t.els[&1].role, Some("region"));
    }

    #[test]
    fn a_frame_changes_only_what_changed() {
        let a = Tree::from_update(&sheet("100", 7));
        // The first frame builds it all, children after their elements.
        let first = Tree::default().diff(&a);
        let put = first
            .iter()
            .position(|o| matches!(o, Op::Put(7, _)))
            .expect("the cell is made");
        let placed = first
            .iter()
            .position(|o| matches!(o, Op::Kids(4, _)))
            .expect("then placed");
        assert!(put < placed);
        assert!(first.contains(&Op::Root(1)));
        assert_eq!(first.last(), Some(&Op::Focus(Some(7))));
        // The same tree: nothing to do.
        assert!(a.diff(&a).is_empty());
        // A new value updates the one cell.
        let b = Tree::from_update(&sheet("150", 7));
        let ops = a.diff(&b);
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert!(
            matches!(&ops[0], Op::Put(7, el) if attr(el, "aria-label") == Some("2025Q1, Widget: 150"))
        );
        // Focus alone moves.
        let c = Tree::from_update(&sheet("150", 2));
        assert_eq!(b.diff(&c), vec![Op::Focus(Some(2))]);
    }

    #[test]
    fn a_removed_subtree_is_unplaced_then_dropped() {
        let a = Tree::from_update(&sheet("100", 7));
        let mut u = sheet("100", 7);
        // The second row goes; the grid lists one child.
        u.nodes.retain(|(id, _)| !matches!(id.0, 4 | 6 | 7));
        u.nodes[1].1.set_children(vec![NodeId(3)]);
        let b = Tree::from_update(&u);
        // The focused node is gone: focus falls back to the root.
        assert_eq!(b.focus, Some(1));
        let ops = a.diff(&b);
        let kids = ops
            .iter()
            .position(|o| matches!(o, Op::Kids(2, k) if *k == vec![3]))
            .expect("the grid's children");
        let drops: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, Op::Drop(_)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(drops.len(), 3);
        assert!(drops.iter().all(|d| *d > kids));
        assert!(ops.contains(&Op::Focus(Some(1))));
    }

    #[test]
    fn an_updated_parent_keeps_its_children() {
        let a = Tree::from_update(&sheet("100", 7));
        let mut u = sheet("100", 7);
        u.nodes[1].1.set_label("Revenue");
        let ops = a.diff(&Tree::from_update(&u));
        assert!(matches!(&ops[..], [Op::Put(2, _)]), "{ops:?}");
        // Text runs are left out: their widget says their words.
        let mut v = sheet("100", 7);
        v.nodes[6].1.set_children(vec![NodeId(8)]);
        v.nodes
            .push((NodeId(8), node(Role::TextRun, |n| n.set_value("100"))));
        let t = Tree::from_update(&v);
        assert!(t.kids[&7].is_empty());
        assert!(!t.els.contains_key(&8));
    }

    #[test]
    fn widgets_carry_their_names_and_states() {
        let t = Tree::from_update(&update(
            vec![
                (
                    1,
                    node(Role::Window, |n| {
                        n.set_children((2..=13).map(NodeId).collect::<Vec<_>>());
                    }),
                ),
                (2, node(Role::Label, |n| n.set_value("Pages"))),
                (
                    3,
                    node(Role::Button, |n| {
                        n.set_label("Undo");
                        n.set_disabled();
                        n.add_action(Action::Click);
                    }),
                ),
                (
                    4,
                    node(Role::TextInput, |n| {
                        n.set_value("Tax = 1");
                        n.set_placeholder("Name = formula");
                        n.push_labelled_by(NodeId(2));
                        n.add_action(Action::Focus);
                    }),
                ),
                (
                    5,
                    node(Role::CheckBox, |n| {
                        n.set_label("North");
                        n.set_toggled(Toggled::Mixed);
                    }),
                ),
                (
                    6,
                    node(Role::Button, |n| {
                        n.set_label("Bold");
                        n.set_toggled(Toggled::True);
                    }),
                ),
                (
                    7,
                    node(Role::ComboBox, |n| {
                        n.set_value("North");
                        n.set_expanded(false);
                        n.set_has_popup(HasPopup::Listbox);
                    }),
                ),
                (
                    8,
                    node(Role::Status, |n| {
                        n.set_label("ready");
                        n.set_live(Live::Polite);
                    }),
                ),
                (9, node(Role::PasswordInput, |n| n.set_value("•••"))),
                (
                    10,
                    node(Role::ListBoxOption, |n| {
                        n.set_label("South");
                        n.set_selected(true);
                    }),
                ),
                // A derived cell.
                (
                    11,
                    node(Role::GridCell, |n| {
                        n.set_label("2025Q1, Widget: 1200");
                        n.set_read_only();
                    }),
                ),
                (12, node(Role::GenericContainer, |_| {})),
                (
                    13,
                    node(Role::Label, |n| {
                        n.set_value("Price - Worksheet");
                        n.add_action(Action::Focus);
                    }),
                ),
            ],
            1,
        ));
        let el = |i: u64| &t.els[&i];
        assert_eq!(el(2).text.as_deref(), Some("Pages"));
        assert_eq!(el(2).role, None);
        assert_eq!(el(13).role, Some("heading"), "a focusable label");
        assert_eq!(el(13).text.as_deref(), Some("Price - Worksheet"));
        assert_eq!(attr(el(2), "aria-label"), None, "a label is its text");
        assert_eq!(attr(el(3), "aria-disabled"), Some("true"));
        assert!(!el(3).active, "clicks, but takes no focus");
        assert_eq!(el(4).role, Some("textbox"));
        assert_eq!(attr(el(4), "aria-label"), Some("Pages"), "labelled by");
        assert_eq!(el(4).text.as_deref(), Some("Tax = 1"));
        assert_eq!(attr(el(4), "aria-placeholder"), Some("Name = formula"));
        assert_eq!(attr(el(5), "aria-checked"), Some("mixed"));
        assert_eq!(attr(el(6), "aria-pressed"), Some("true"));
        assert_eq!(attr(el(7), "aria-expanded"), Some("false"));
        assert_eq!(attr(el(7), "aria-haspopup"), Some("listbox"));
        assert_eq!(attr(el(8), "aria-live"), Some("polite"));
        assert_eq!(
            el(8).text.as_deref(),
            Some("ready"),
            "a live region's words"
        );
        assert_eq!(attr(el(8), "aria-label"), None);
        assert_eq!(el(7).text.as_deref(), Some("North"));
        assert_eq!(el(9).text, None, "a password stays hidden");
        assert!(el(9).attrs.is_empty());
        assert!(el(9).secret && !el(4).secret);
        assert_eq!(attr(el(10), "aria-selected"), Some("true"));
        assert_eq!(attr(el(11), "aria-readonly"), Some("true"));
        assert_eq!(el(12).role, None);
        assert!(el(12).attrs.is_empty());
    }

    #[test]
    fn every_role_egui_uses_has_an_aria_role() {
        for r in [
            Role::Button,
            Role::DefaultButton,
            Role::CheckBox,
            Role::RadioButton,
            Role::Switch,
            Role::Link,
            Role::TextInput,
            Role::MultilineTextInput,
            Role::SearchInput,
            Role::PasswordInput,
            Role::EmailInput,
            Role::NumberInput,
            Role::PhoneNumberInput,
            Role::UrlInput,
            Role::ComboBox,
            Role::EditableComboBox,
            Role::ListBox,
            Role::ListBoxOption,
            Role::MenuListOption,
            Role::Menu,
            Role::MenuListPopup,
            Role::MenuBar,
            Role::MenuItem,
            Role::MenuItemCheckBox,
            Role::MenuItemRadio,
            Role::Slider,
            Role::SpinButton,
            Role::ProgressIndicator,
            Role::Grid,
            Role::Table,
            Role::TreeGrid,
            Role::Row,
            Role::RowGroup,
            Role::RowHeader,
            Role::ColumnHeader,
            Role::GridCell,
            Role::Cell,
            Role::List,
            Role::ListItem,
            Role::Tree,
            Role::TreeItem,
            Role::Tab,
            Role::TabList,
            Role::TabPanel,
            Role::Dialog,
            Role::AlertDialog,
            Role::Alert,
            Role::Status,
            Role::Log,
            Role::Tooltip,
            Role::Toolbar,
            Role::Heading,
            Role::Image,
            Role::Group,
            Role::RadioGroup,
            Role::Window,
            Role::Pane,
            Role::Region,
            Role::ScrollBar,
            Role::Application,
        ] {
            assert!(aria_role(r).is_some(), "{r:?}");
        }
        for r in [Role::GenericContainer, Role::TextRun, Role::Unknown] {
            assert_eq!(aria_role(r), None, "{r:?}");
        }
    }

    #[test]
    fn dom_ids_round_trip() {
        assert_eq!(node_of(&dom_id(42)), Some(42));
        assert_eq!(node_of("ak-x"), None);
        assert_eq!(node_of("improv"), None);
    }

    #[test]
    fn a_tree_without_info_or_with_strays_still_builds() {
        // No tree info: the first node is the root. A child id with no
        // node is skipped; a node listed twice is built once; the focus on
        // an unknown node falls back to the root.
        let u = TreeUpdate {
            nodes: vec![
                (
                    NodeId(1),
                    node(Role::Window, |n| {
                        n.set_children(vec![NodeId(2), NodeId(9), NodeId(2)]);
                    }),
                ),
                (NodeId(2), node(Role::Button, |n| n.set_label("OK"))),
                (NodeId(3), node(Role::Button, |n| n.set_label("stray"))),
            ],
            tree: None,
            tree_id: TreeId::ROOT,
            focus: NodeId(9),
        };
        let t = Tree::from_update(&u);
        assert_eq!(t.root, Some(1));
        assert_eq!(t.kids[&1], vec![2, 2]);
        assert!(!t.els.contains_key(&3), "unreachable");
        assert_eq!(t.focus, Some(1));
        // The same tree with no root: no root operation, all dropped.
        let none = Tree::default();
        let ops = t.diff(&none);
        assert!(!ops.iter().any(|o| matches!(o, Op::Root(_))), "{ops:?}");
        assert_eq!(ops.iter().filter(|o| matches!(o, Op::Drop(_))).count(), 2);
        // A button's value (none of egui's say one; a slider would) is
        // described, not shown.
        let mut b = Node::new(Role::Button);
        b.set_label("Zoom");
        b.set_value("150%");
        let el = element(&b, &HashMap::new());
        assert_eq!(attr(&el, "aria-description"), Some("150%"));
        assert_eq!(el.text, None);
        // Labelled by nodes with no words: no name.
        let mut l = Node::new(Role::TextInput);
        l.push_labelled_by(NodeId(3));
        let empty = Node::new(Role::Label);
        let nodes = HashMap::from([(NodeId(3), &empty)]);
        assert_eq!(attr(&element(&l, &nodes), "aria-label"), None);
    }
}

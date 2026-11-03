//! Improv's context help (fidelity gap 12): Info ▸ Context Help… opens the
//! help page for the window in front, and Info ▸ Point for Help turns the
//! pointer into a question mark whose next click opens the page for what
//! it lands on. Improv's pages were a Help book; here a page is a few lines
//! and a link to its section of the user guide.

/// A help page: its title, what it says, and its place in the guide
/// (`page.html#anchor`, under [`GUIDE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topic {
    pub title: &'static str,
    pub text: &'static str,
    pub guide: &'static str,
}

/// The published user guide.
pub const GUIDE: &str = "https://gburd.github.io/improv/";

const fn t(title: &'static str, text: &'static str, guide: &'static str) -> Topic {
    Topic { title, text, guide }
}

/// What the help knows, by the name of the thing it is about: an NxWindow
/// id's name, or a part of a worksheet window.
const TOPICS: &[(&str, Topic)] = &[
    ("worksheet", t(
        "Worksheet",
        "A worksheet shows one measure as a grid: rows and columns of items, one category's item per page. Click a cell to select it, Shift+click or drag for a range (its Sum is in the status bar). Type to replace a value; Enter or F2 edits it. Derived cells are computed by their formula.",
        "gui-tour.html#the-pivot-grid",
    )),
    ("tiles", t(
        "Category tiles",
        "Each category is a tile at the worksheet's edge: columns top right, pages top left with their item, rows bottom left. Drag a tile to another edge to pivot, with no formula changed. Right-click a tile to move it, to filter its items, or to add an item.",
        "gui-tour.html#the-pivot-grid",
    )),
    ("formulas", t(
        "Formulas",
        "The formula pane lists the model's formulas, numbered. A check marks one that checks; a cross one with an error, described under it. Click a row to edit it, Enter commits. Type Name = formula on the last line to add a measure. Calc recalculates.",
        "formulas.html",
    )),
    ("menu", t(
        "The main menu",
        "Improv's menu: drag its title bar to move it. An item with a triangle opens a submenu; one with a letter runs from the keyboard with Ctrl (Cmd on macOS) and that letter, Shift for a capital.",
        "gui-tour.html#key-equivalents",
    )),
    ("browser", t(
        "Model Browser",
        "The model's categories with their items, its measures (input or formula) and its saved views. Click a measure to show it; double-click a view to open it. Type in a New line to add a category, item or measure.",
        "gui-tour.html#building-a-model-from-scratch",
    )),
    ("info", t(
        "Info Panel",
        "The selected measure: its kind, value type, format, dimensions and, for a formula, what it depends on. An input measure's boxes change its categories.",
        "gui-tour.html#the-pivot-grid",
    )),
    ("inspector", t(
        "Inspector",
        "Edit ▸ Inspector…: the pop-up picks Grid (grid lines and where), Layout (the hidden-items marker) or Formulas (errors under their rows). OK applies it to the worksheet in front; Revert reads it back.",
        "gui-tour.html#the-inspector-and-the-format-panel",
    )),
    ("format", t(
        "Format Panel",
        "Format ▸ Format Panel…: Number sets General or a count of decimal places and commas, Alignment where cells sit. Set applies both to the worksheet in front.",
        "gui-tour.html#the-inspector-and-the-format-panel",
    )),
    ("print", t(
        "Print",
        "Print… prints the worksheet in front as it shows, to the print system (lp or lpr; the print dialog on Windows) or to a PDF or PostScript file. Format ▸ Page Layout… and Report Layout… set the paper, the header and footer, and what is printed.",
        "gui-tour.html#printing",
    )),
    ("page_layout", t(
        "Page Layout",
        "The paper (Letter, Legal, A4), its orientation and the scale every print uses.",
        "gui-tour.html#printing",
    )),
    ("report_layout", t(
        "Report Layout",
        "The header and footer (left, centered, right), with markers such as <Page #> and <Date>; the first page number; the page order; and whether data, formulas or both are printed.",
        "gui-tour.html#printing",
    )),
    ("graph", t(
        "Graph",
        "Graph ▸ New Graph charts the worksheet's selection, or all of it, and follows it as it changes. Type picks one of Improv's seven graphs; Series by reads the selection by rows, columns or Smart.",
        "gui-tour.html#graphs",
    )),
    ("fnhelp", t(
        "Functions",
        "Search the built-in functions; Insert puts the selected one into the formula last edited.",
        "gui-tour.html#function-help",
    )),
    ("keys", t(
        "Keys",
        "Every key equivalent, and the worksheet's own keys.",
        "gui-tour.html#key-equivalents",
    )),
    ("file_panel", t(
        "Open, New and Save As",
        "Type a model file or a postgres:// URL; Enter opens it, creates it or saves there. Esc closes the panel.",
        "gui-tour.html#the-pivot-grid",
    )),
    ("delete_panel", t(
        "Delete",
        "Deleting a category, item or measure takes its cells with it. Edit ▸ Undo brings it back.",
        "gui-tour.html#renaming-and-deleting",
    )),
    ("help", t(
        "Help",
        "Info ▸ Context Help… (Ctrl+?) shows the page for the window in front. Info ▸ Point for Help turns the pointer into a question mark: click a menu, a panel or a part of a worksheet for its page; Esc cancels.",
        "gui-tour.html#context-help",
    )),
];

/// The page for when nothing more particular applies.
pub const IMPROV: Topic = t(
    "Improv",
    "A multidimensional spreadsheet: measures over categories of items, formulas by name, and worksheets that pivot by dragging category tiles. Info ▸ Point for Help explains whatever you click next.",
    "gui-tour.html",
);

/// The page named `name`, or [`IMPROV`].
pub fn topic(name: &str) -> Topic {
    TOPICS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(IMPROV, |(_, t)| *t)
}

/// The names [`topic`] knows.
#[cfg(test)]
pub fn names() -> impl Iterator<Item = &'static str> {
    TOPICS.iter().map(|(n, _)| *n)
}

/// Point for Help: take the next primary press out of `raw` (and its
/// release), so nothing else acts on it, and return where it was. Called
/// on the frame's raw input, before egui reads it.
pub fn take_click(raw: &mut egui::RawInput) -> Option<egui::Pos2> {
    let at = raw.events.iter().find_map(|e| match e {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            ..
        } => Some(*pos),
        _ => None,
    })?;
    raw.events.retain(|e| {
        !matches!(
            e,
            egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                ..
            }
        )
    });
    Some(at)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_page_in_the_guide() {
        for n in names() {
            let t = topic(n);
            assert!(!t.title.is_empty() && t.text.len() > 20, "{n}");
            let page = t.guide.split('#').next().unwrap();
            let md = format!(
                "{}/../../docs/src/{}",
                env!("CARGO_MANIFEST_DIR"),
                page.replace(".html", ".md")
            );
            let text = std::fs::read_to_string(&md).unwrap_or_else(|e| panic!("{n}: {md}: {e}"));
            if let Some(anchor) = t.guide.split('#').nth(1) {
                let found = text.lines().filter(|l| l.starts_with('#')).any(|l| {
                    let slug: String = l
                        .trim_start_matches('#')
                        .trim()
                        .to_lowercase()
                        .chars()
                        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                        .map(|c| if c == ' ' { '-' } else { c })
                        .collect();
                    slug == anchor
                });
                assert!(found, "{n}: no heading #{anchor} in {md}");
            }
        }
        assert_eq!(topic("nothing"), IMPROV);
    }

    #[test]
    fn take_click_swallows_the_press_and_its_release() {
        let at = egui::pos2(10.0, 20.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut raw = egui::RawInput {
            events: vec![egui::Event::PointerMoved(at), button(true), button(false)],
            ..Default::default()
        };
        assert_eq!(take_click(&mut raw), Some(at));
        assert_eq!(raw.events, [egui::Event::PointerMoved(at)]);
        assert_eq!(take_click(&mut raw), None, "a move alone is no click");
    }
}

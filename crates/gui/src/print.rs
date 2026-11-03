//! Printing (fidelity gap 11): Improv's Print… panel and Format ▸ Page
//! Layout… and Report Layout…. A worksheet ([`Sheet`]) is laid out once into
//! pages of text and rules ([`Doc`]), in points from the page's top-left,
//! then written as PDF or PostScript, spooled to the platform's print system
//! ([`spooler`]: CUPS's `lp` takes PDF; illumos's LP service and the BSD
//! `lpr` take PostScript), or, on Windows, printed through the print dialog
//! with GDI.
//! Pure layout and writers; the panels are in `app.rs`.
//!
//! Text is Helvetica (Arial through GDI, which has its widths), one of the
//! fonts every PostScript printer and PDF reader has, so nothing is embedded.

/// A sheet of paper, in points (1/72 in).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Paper {
    #[default]
    Letter,
    Legal,
    A4,
}

impl Paper {
    pub const ALL: [Paper; 3] = [Paper::Letter, Paper::Legal, Paper::A4];

    /// Portrait width and height, in points.
    pub fn size(self) -> (f32, f32) {
        match self {
            Paper::Letter => (612.0, 792.0),
            Paper::Legal => (612.0, 1008.0),
            Paper::A4 => (595.0, 842.0),
        }
    }
}

/// Format ▸ Page Layout…: the paper, its orientation and the scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageLayout {
    pub paper: Paper,
    pub landscape: bool,
    /// Percent: 50 prints at half size.
    pub scale: u16,
}

impl Default for PageLayout {
    fn default() -> Self {
        PageLayout {
            paper: Paper::Letter,
            landscape: false,
            scale: 100,
        }
    }
}

impl PageLayout {
    /// The page as it is printed, in points.
    pub fn size(&self) -> (f32, f32) {
        let (w, h) = self.paper.size();
        if self.landscape {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// Improv's header and footer markers (its ReportPanel nib's strings), typed
/// in a header or footer as `<Date>`, `<Page #>` and so on.
pub const MARKERS: [&str; 6] = [
    "Date",
    "Time",
    "View Name",
    "Worksheet Name",
    "Model Name",
    "Page #",
];

/// Format ▸ Report Layout…: header and footer (left, center, right), the
/// first page's number, the page order and what is printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportLayout {
    pub header: [String; 3],
    pub footer: [String; 3],
    pub first_page: u32,
    /// Number Pages: Left to Right (across first), else Top to Bottom.
    pub across_first: bool,
    /// Content: Data (the items and cells) and Formulas.
    pub data: bool,
    pub formulas: bool,
}

impl Default for ReportLayout {
    fn default() -> Self {
        ReportLayout {
            header: [String::new(), "<Worksheet Name>".into(), String::new()],
            footer: [String::new(), "Page <Page #>".into(), String::new()],
            first_page: 1,
            across_first: false,
            data: true,
            formulas: false,
        }
    }
}

/// What the markers stand for in one print job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub date: String,
    pub time: String,
    pub view: String,
    pub worksheet: String,
    pub model: String,
}

/// `text` with its markers filled in, on page `page`.
pub fn expand(text: &str, names: &Names, page: u32) -> String {
    let page = page.to_string();
    let vals = [
        &names.date,
        &names.time,
        &names.view,
        &names.worksheet,
        &names.model,
        &page,
    ];
    MARKERS
        .iter()
        .zip(vals)
        .fold(text.to_owned(), |t, (m, v)| t.replace(&format!("<{m}>"), v))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A printed row: its items (one per row category), then its cells as they
/// show, each with its alignment.
pub type Row = (Vec<String>, Vec<(String, Align)>);

/// A worksheet to print: its item names and cells as they show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    /// The row categories' names, one per stub column.
    pub corner: Vec<String>,
    /// One row per column category (outer first): each column's item.
    pub col_heads: Vec<Vec<String>>,
    /// Each row: its items (one per row category), then its cells.
    pub rows: Vec<Row>,
    /// `1. Revenue = Price * Quantity`, for Content ▸ Formulas.
    pub formulas: Vec<String>,
    /// Which areas are ruled (the Inspector's Grid page).
    pub item_rules: bool,
    pub cell_rules: bool,
}

/// One mark on a page, in points from its top-left (`y` is the baseline).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Text {
        x: f32,
        y: f32,
        size: f32,
        s: String,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

/// A laid-out print job.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub title: String,
    /// Width and height, in points.
    pub size: (f32, f32),
    pub pages: Vec<Vec<Op>>,
}

const MARGIN: f32 = 36.0;
const BAND_FONT: f32 = 9.0;
const FONT: f32 = 10.0;
const ROW: f32 = 14.0;
const PAD: f32 = 4.0;
const RULE: f32 = 0.5;

/// Helvetica's advance widths for ' ' to '~', per 1000 units (Adobe's
/// core-font metrics).
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278,
    278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584,
    556, // '0'..'?'
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722,
    778, // '@'..'O'
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469,
    556, // 'P'..'_'
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556,
    556, // '`'..'o'
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 'p'..'~'
];

/// `s`'s width in Helvetica at `size` points.
pub fn text_width(s: &str, size: f32) -> f32 {
    // shortcut: Latin-1 letters past '~' count as a digit's width; add their
    // metrics if accented names must line up exactly.
    let units: u32 = s
        .chars()
        .map(|c| match c {
            ' '..='~' => u32::from(HELVETICA[c as usize - 32]),
            _ => 556,
        })
        .sum();
    units as f32 * size / 1000.0
}

/// Lay `sheet` out on pages: the data (columns split across pages, the
/// item names repeated on each), then the formulas, each page with its
/// header and footer.
pub fn layout(sheet: &Sheet, page: &PageLayout, report: &ReportLayout, names: &Names) -> Doc {
    let (w, h) = page.size();
    let s = f32::from(page.scale.clamp(10, 400)) / 100.0;
    let area = Area {
        left: MARGIN,
        right: w - MARGIN,
        top: MARGIN + 24.0,
        bottom: h - MARGIN - 18.0,
        font: FONT * s,
        row: ROW * s,
        pad: PAD * s,
    };
    let mut pages = Vec::new();
    if report.data {
        pages.extend(data_pages(sheet, &area, report.across_first));
    }
    if report.formulas && !sheet.formulas.is_empty() {
        pages.extend(formula_pages(&sheet.formulas, &area));
    }
    for (i, ops) in pages.iter_mut().enumerate() {
        let n = report.first_page.saturating_add(i as u32);
        band(ops, &report.header, names, n, MARGIN + BAND_FONT, &area);
        band(ops, &report.footer, names, n, h - MARGIN, &area);
    }
    Doc {
        title: names.worksheet.clone(),
        size: (w, h),
        pages,
    }
}

/// The printable area and the scaled text metrics.
struct Area {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font: f32,
    row: f32,
    pad: f32,
}

impl Area {
    /// The baseline of the `i`-th row from `y0`.
    fn base(&self, y0: f32, i: usize) -> f32 {
        y0 + self.row * i as f32 + (self.row + 0.7 * self.font) / 2.0
    }
}

fn text(ops: &mut Vec<Op>, x: f32, y: f32, size: f32, s: &str, align: Align) {
    if s.is_empty() {
        return;
    }
    let w = text_width(s, size);
    let x = match align {
        Align::Left => x,
        Align::Center => x - w / 2.0,
        Align::Right => x - w,
    };
    ops.push(Op::Text {
        x,
        y,
        size,
        s: s.to_owned(),
    });
}

fn line(ops: &mut Vec<Op>, x1: f32, y1: f32, x2: f32, y2: f32) {
    ops.push(Op::Line { x1, y1, x2, y2 });
}

/// A header or footer: left, centered and right slots, on baseline `y`.
fn band(ops: &mut Vec<Op>, slots: &[String; 3], names: &Names, page: u32, y: f32, a: &Area) {
    let xs = [
        (a.left, Align::Left),
        ((a.left + a.right) / 2.0, Align::Center),
        (a.right, Align::Right),
    ];
    for (t, (x, align)) in slots.iter().zip(xs) {
        text(ops, x, y, BAND_FONT, &expand(t, names, page), align);
    }
}

/// `n` items in runs that fit `room` by `width`, at least one per run.
fn runs(n: usize, room: f32, width: impl Fn(usize) -> f32) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let (mut end, mut used) = (start, 0.0);
        while end < n && (end == start || used + width(end) <= room) {
            used += width(end);
            end += 1;
        }
        out.push(start..end);
        start = end;
    }
    if out.is_empty() {
        out.push(0..0);
    }
    out
}

fn data_pages(sheet: &Sheet, a: &Area, across_first: bool) -> Vec<Vec<Op>> {
    let n_stub = sheet.corner.len();
    let n_cols = sheet
        .col_heads
        .iter()
        .map(Vec::len)
        .chain(sheet.rows.iter().map(|r| r.1.len()))
        .max()
        .unwrap_or(0);
    let tw = |s: &str| text_width(s, a.font) + 2.0 * a.pad;
    let min_w = 3.0 * a.font;
    let stub_w: Vec<f32> = (0..n_stub)
        .map(|k| {
            sheet
                .rows
                .iter()
                .filter_map(|r| r.0.get(k))
                .chain(sheet.corner.get(k))
                .map(|s| tw(s))
                .fold(min_w, f32::max)
        })
        .collect();
    let col_w: Vec<f32> = (0..n_cols)
        .map(|c| {
            let heads = sheet.col_heads.iter().filter_map(|h| h.get(c));
            let cells = sheet.rows.iter().filter_map(|r| r.1.get(c).map(|x| &x.0));
            heads.chain(cells).map(|s| tw(s)).fold(min_w, f32::max)
        })
        .collect();
    let stub_total: f32 = stub_w.iter().sum();
    let cols = runs(n_cols, a.right - a.left - stub_total, |c| col_w[c]);
    let hdr = sheet.col_heads.len().max(1);
    let per = (((a.bottom - a.top) / a.row).floor() as usize)
        .saturating_sub(hdr)
        .max(1);
    let rows = runs(sheet.rows.len(), per as f32, |_| 1.0);
    let mut order = Vec::new();
    if across_first {
        for r in &rows {
            order.extend(cols.iter().map(|c| (r.clone(), c.clone())));
        }
    } else {
        for c in &cols {
            order.extend(rows.iter().map(|r| (r.clone(), c.clone())));
        }
    }
    order
        .into_iter()
        .map(|(rr, cc)| data_page(sheet, a, &stub_w, &col_w, rr, cc))
        .collect()
}

fn data_page(
    sheet: &Sheet,
    a: &Area,
    stub_w: &[f32],
    col_w: &[f32],
    rr: std::ops::Range<usize>,
    cc: std::ops::Range<usize>,
) -> Vec<Op> {
    let mut ops = Vec::new();
    let hdr = sheet.col_heads.len().max(1);
    // Column edges: the stubs, then this page's columns.
    let mut xs = vec![a.left];
    for w in stub_w.iter().chain(&col_w[cc.clone()]) {
        xs.push(xs[xs.len() - 1] + w);
    }
    let n_stub = stub_w.len();
    let body = a.top + a.row * hdr as f32;
    let end = body + a.row * rr.len() as f32;
    // The corner: the row categories' names, level with the last header row.
    for (k, name) in sheet.corner.iter().enumerate() {
        let y = a.base(a.top, hdr - 1);
        text(&mut ops, xs[k] + a.pad, y, a.font, name, Align::Left);
    }
    // Column items, outer first; an outer item is named once per group on
    // the page (its first column here).
    for (lvl, heads) in sheet.col_heads.iter().enumerate() {
        for (i, c) in cc.clone().enumerate() {
            let same = i > 0
                && (0..=lvl).all(|k| sheet.col_heads[k].get(c) == sheet.col_heads[k].get(c - 1));
            if !same {
                let s = heads.get(c).map_or("", String::as_str);
                text(
                    &mut ops,
                    xs[n_stub + i] + a.pad,
                    a.base(a.top, lvl),
                    a.font,
                    s,
                    Align::Left,
                );
            }
        }
    }
    for (i, r) in rr.clone().enumerate() {
        let (items, cells) = &sheet.rows[r];
        let y = a.base(body, i);
        for (k, s) in items.iter().enumerate() {
            let same = i > 0 && (0..=k).all(|j| sheet.rows[r - 1].0.get(j) == items.get(j));
            if !same {
                text(&mut ops, xs[k] + a.pad, y, a.font, s, Align::Left);
            }
        }
        for (j, c) in cc.clone().enumerate() {
            let Some((s, align)) = cells.get(c) else {
                continue;
            };
            let (l, rgt) = (xs[n_stub + j], xs[n_stub + j + 1]);
            let x = match align {
                Align::Left => l + a.pad,
                Align::Center => (l + rgt) / 2.0,
                Align::Right => rgt - a.pad,
            };
            text(&mut ops, x, y, a.font, s, *align);
        }
    }
    let right = xs[xs.len() - 1];
    let mid = xs[n_stub];
    if sheet.item_rules {
        line(&mut ops, a.left, a.top, right, a.top);
        line(&mut ops, a.left, body, right, body);
        for x in &xs[..=n_stub] {
            line(&mut ops, *x, a.top, *x, end);
        }
        for x in &xs[n_stub..] {
            line(&mut ops, *x, a.top, *x, body);
        }
        for i in 1..=rr.len() {
            let y = body + a.row * i as f32;
            line(&mut ops, a.left, y, mid, y);
        }
    }
    if sheet.cell_rules {
        for i in 0..=rr.len() {
            let y = body + a.row * i as f32;
            line(&mut ops, mid, y, right, y);
        }
        for x in &xs[n_stub..] {
            line(&mut ops, *x, body, *x, end);
        }
    }
    ops
}

fn formula_pages(formulas: &[String], a: &Area) -> Vec<Vec<Op>> {
    // shortcut: a formula longer than the page is cut at its edge, not
    // wrapped; wrap it if long formulas are printed.
    let per = (((a.bottom - a.top) / a.row).floor() as usize).max(1);
    formulas
        .chunks(per)
        .map(|chunk| {
            let mut ops = Vec::new();
            for (i, f) in chunk.iter().enumerate() {
                text(&mut ops, a.left, a.base(a.top, i), a.font, f, Align::Left);
            }
            ops
        })
        .collect()
}

/// `s` as a PostScript or PDF string literal's body: Latin-1, with `\`,
/// `(` and `)` escaped and bytes past ASCII in octal. A character Latin-1
/// lacks prints as `?`.
fn literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            '\u{A0}'..='\u{FF}' => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }
    out
}

impl Doc {
    /// The pages `from..=to` (1-based; clamped), or all of them.
    pub fn range(&self, pages: Option<(usize, usize)>) -> Doc {
        let n = self.pages.len();
        let (from, to) = pages.map_or((1, n), |(f, t)| (f.max(1), t.min(n)));
        Doc {
            title: self.title.clone(),
            size: self.size,
            pages: self
                .pages
                .get(from - 1..to.max(from - 1))
                .unwrap_or_default()
                .to_vec(),
        }
    }

    /// A PDF 1.4 file: one content stream per page, Helvetica in
    /// WinAnsiEncoding (Latin-1 for what [`literal`] passes).
    pub fn to_pdf(&self) -> Vec<u8> {
        let (w, h) = self.size;
        let n = self.pages.len();
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 {w:.0} {h:.0}] >>",
                (0..n)
                    .map(|i| format!("{} 0 R", 5 + 2 * i))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
            format!("<< /Title ({}) /Producer (Improv) >>", literal(&self.title)),
        ];
        for (i, page) in self.pages.iter().enumerate() {
            let mut body = format!("{RULE} w\n");
            for op in page {
                match op {
                    Op::Text { x, y, size, s } => body.push_str(&format!(
                        "BT /F1 {size:.2} Tf {x:.2} {:.2} Td ({}) Tj ET\n",
                        h - y,
                        literal(s)
                    )),
                    Op::Line { x1, y1, x2, y2 } => body.push_str(&format!(
                        "{x1:.2} {:.2} m {x2:.2} {:.2} l S\n",
                        h - y1,
                        h - y2
                    )),
                }
            }
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                6 + 2 * i
            ));
            objs.push(format!(
                "<< /Length {} >>\nstream\n{body}endstream",
                body.len()
            ));
        }
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objs.len() + 1
        ));
        for off in offsets {
            out.push_str(&format!("{off:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        ));
        out.into_bytes()
    }

    /// A DSC-conforming PostScript file, Helvetica re-encoded to Latin-1.
    pub fn to_postscript(&self) -> Vec<u8> {
        let (w, h) = self.size;
        let n = self.pages.len();
        let mut out = format!(
            "%!PS-Adobe-3.0\n%%Creator: Improv\n%%Title: ({})\n%%Pages: {n}\n\
             %%BoundingBox: 0 0 {w:.0} {h:.0}\n%%DocumentNeededResources: font Helvetica\n\
             %%EndComments\n%%BeginProlog\n\
             /F {{ /Helvetica-Latin1 findfont exch scalefont setfont }} bind def\n\
             /T {{ moveto show }} bind def\n\
             /L {{ newpath moveto lineto stroke }} bind def\n%%EndProlog\n\
             %%BeginSetup\n/Helvetica findfont dup length dict begin\n\
             {{ 1 index /FID ne {{ def }} {{ pop pop }} ifelse }} forall\n\
             /Encoding ISOLatin1Encoding def currentdict end\n\
             /Helvetica-Latin1 exch definefont pop\n%%EndSetup\n",
            literal(&self.title)
        );
        for (i, page) in self.pages.iter().enumerate() {
            out.push_str(&format!(
                "%%Page: {0} {0}\ngsave {RULE} setlinewidth\n",
                i + 1
            ));
            let mut size = 0.0;
            for op in page {
                match op {
                    Op::Text {
                        x,
                        y,
                        size: s,
                        s: t,
                    } => {
                        if *s != size {
                            size = *s;
                            out.push_str(&format!("{size:.2} F\n"));
                        }
                        out.push_str(&format!("({}) {x:.2} {:.2} T\n", literal(t), h - y));
                    }
                    Op::Line { x1, y1, x2, y2 } => {
                        out.push_str(&format!("{x1:.2} {:.2} {x2:.2} {:.2} L\n", h - y1, h - y2))
                    }
                }
            }
            out.push_str("grestore showpage\n");
        }
        out.push_str("%%Trailer\n%%EOF\n");
        out.into_bytes()
    }
}

/// What a spooler is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    PostScript,
}

/// The platform print system's command for a job, by which `exists` finds:
/// `lp`, else BSD `lpr`. Empty `printer` is the default destination. `lp`
/// is CUPS on Linux, macOS and the BSDs, whose queues all take PDF; on
/// illumos (`illumos` true) it is the LP service, and `lpr` is the BSD
/// spooler, which both take PostScript.
pub fn spooler(
    printer: &str,
    copies: u32,
    illumos: bool,
    exists: impl Fn(&str) -> bool,
) -> Option<(&'static str, Vec<String>, Format)> {
    let printer = printer.trim();
    if exists("lp") {
        let mut args = vec!["-n".to_owned(), copies.max(1).to_string()];
        if !printer.is_empty() {
            args.extend(["-d".to_owned(), printer.to_owned()]);
        }
        let f = if illumos {
            Format::PostScript
        } else {
            Format::Pdf
        };
        return Some(("lp", args, f));
    }
    exists("lpr").then(|| {
        let mut args = vec![format!("-#{}", copies.max(1))];
        if !printer.is_empty() {
            args.extend(["-P".to_owned(), printer.to_owned()]);
        }
        ("lpr", args, Format::PostScript)
    })
}

/// [`spooler`] on this machine.
pub fn local_spooler(printer: &str, copies: u32) -> Option<(&'static str, Vec<String>, Format)> {
    let illumos = cfg!(any(target_os = "illumos", target_os = "solaris"));
    spooler(printer, copies, illumos, on_path)
}

/// Whether `name` is an executable file on `PATH`.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            let f = d.join(name);
            f.is_file()
        })
    })
}

/// Run `program args`, writing `data` to its standard input; its error
/// output is the error when it fails. Not on Windows, which prints through
/// [`print_windows`] instead.
#[cfg(not(windows))]
pub fn spool(program: &str, args: &[String], data: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let wrote = child
        .stdin
        .take()
        .map(|mut i| i.write_all(data))
        .transpose()
        .map_err(|e| format!("{program}: {e}"));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{program}: {e}"))?;
    wrote?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{program}: {}", err.trim()))
    }
}

/// Windows: the system print dialog, then the pages drawn with GDI on the
/// printer it returns. `copies` and `pages` seed the dialog.
#[cfg(windows)]
pub fn print_windows(doc: &Doc, copies: u32) -> Result<String, String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::{EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let n = u16::try_from(doc.pages.len()).unwrap_or(u16::MAX);
    let mut pd = PRINTDLGW {
        lStructSize: std::mem::size_of::<PRINTDLGW>() as u32,
        Flags: PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE,
        nFromPage: 1,
        nToPage: n,
        nMinPage: 1,
        nMaxPage: n,
        nCopies: u16::try_from(copies.max(1)).unwrap_or(1),
        ..Default::default()
    };
    // SAFETY: `pd` is a zeroed PRINTDLGW with its size set, as PrintDlgW
    // requires; it fills in hDC, hDevMode and hDevNames, freed below.
    if unsafe { PrintDlgW(&mut pd) } == 0 {
        // SAFETY: no arguments; reads the dialog's last error.
        return match unsafe { CommDlgExtendedError() } {
            0 => Ok("printing cancelled".into()),
            e => Err(format!("the print dialog failed (error {e:#x})")),
        };
    }
    let hdc = pd.hDC;
    let picked =
        (pd.Flags & PD_PAGENUMS != 0).then(|| (usize::from(pd.nFromPage), usize::from(pd.nToPage)));
    let pages = doc.range(picked).pages;
    // SAFETY: `hdc` is the printer DC PrintDlgW returned (PD_RETURNDC);
    // every handle created here is selected out and deleted before the DC.
    let result = unsafe {
        let dpi = (
            GetDeviceCaps(hdc, LOGPIXELSX as i32) as f32 / 72.0,
            GetDeviceCaps(hdc, LOGPIXELSY as i32) as f32 / 72.0,
        );
        let off = (
            GetDeviceCaps(hdc, PHYSICALOFFSETX as i32) as f32,
            GetDeviceCaps(hdc, PHYSICALOFFSETY as i32) as f32,
        );
        let dev = |x: f32, y: f32| ((x * dpi.0 - off.0) as i32, (y * dpi.1 - off.1) as i32);
        let name = wide(&doc.title);
        let di = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: name.as_ptr(),
            ..Default::default()
        };
        if StartDocW(hdc, &di) <= 0 {
            Err("the printer refused the job".to_owned())
        } else {
            let pen = CreatePen(PS_SOLID, (RULE * dpi.0).round().max(1.0) as i32, 0);
            let old_pen = SelectObject(hdc, pen);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextAlign(hdc, TA_BASELINE | TA_LEFT);
            let face = wide("Arial");
            let mut font: (f32, HFONT) = (0.0, std::ptr::null_mut());
            let old_font = SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
            for page in &pages {
                StartPage(hdc);
                for op in page {
                    match op {
                        Op::Text { x, y, size, s } => {
                            if *size != font.0 {
                                let f = CreateFontW(
                                    -((size * dpi.1).round() as i32),
                                    0,
                                    0,
                                    0,
                                    FW_NORMAL as i32,
                                    0,
                                    0,
                                    0,
                                    u32::from(DEFAULT_CHARSET),
                                    u32::from(OUT_DEFAULT_PRECIS),
                                    u32::from(CLIP_DEFAULT_PRECIS),
                                    u32::from(DEFAULT_QUALITY),
                                    u32::from(FF_SWISS),
                                    face.as_ptr(),
                                );
                                SelectObject(hdc, f);
                                if !font.1.is_null() {
                                    DeleteObject(font.1);
                                }
                                font = (*size, f);
                            }
                            let (dx, dy) = dev(*x, *y);
                            let t: Vec<u16> = s.encode_utf16().collect();
                            TextOutW(hdc, dx, dy, t.as_ptr(), t.len() as i32);
                        }
                        Op::Line { x1, y1, x2, y2 } => {
                            let (a, b) = (dev(*x1, *y1), dev(*x2, *y2));
                            MoveToEx(hdc, a.0, a.1, std::ptr::null_mut());
                            LineTo(hdc, b.0, b.1);
                        }
                    }
                }
                EndPage(hdc);
            }
            EndDoc(hdc);
            SelectObject(hdc, old_font);
            SelectObject(hdc, old_pen);
            if !font.1.is_null() {
                DeleteObject(font.1);
            }
            DeleteObject(pen);
            Ok(format!(
                "printed {} page{}",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ))
        }
    };
    // SAFETY: the DC and the two global handles PrintDlgW allocated.
    unsafe {
        DeleteDC(hdc);
        if !pd.hDevMode.is_null() {
            GlobalFree(pd.hDevMode);
        }
        if !pd.hDevNames.is_null() {
            GlobalFree(pd.hDevNames);
        }
    }
    result
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            date: "2026-11-02".into(),
            time: "21:30".into(),
            view: "Plan".into(),
            worksheet: "Revenue".into(),
            model: "demo".into(),
        }
    }

    /// A `rows` x `cols` sheet over Time x Product, numbers right-aligned.
    fn sheet(rows: usize, cols: usize) -> Sheet {
        Sheet {
            corner: vec!["Time".into()],
            col_heads: vec![(0..cols).map(|c| format!("Product {c}")).collect()],
            rows: (0..rows)
                .map(|r| {
                    let cells = (0..cols).map(|c| (format!("{}", r * 100 + c), Align::Right));
                    (vec![format!("Q{r}")], cells.collect())
                })
                .collect(),
            formulas: vec!["1. Revenue = Price * Quantity".into()],
            item_rules: true,
            cell_rules: true,
        }
    }

    fn texts(page: &[Op]) -> Vec<&str> {
        page.iter()
            .filter_map(|o| match o {
                Op::Text { s, .. } => Some(s.as_str()),
                Op::Line { .. } => None,
            })
            .collect()
    }

    #[test]
    fn markers_expand_per_page() {
        let n = names();
        assert_eq!(
            expand(
                "<Model Name>: <Worksheet Name> (<View Name>) p. <Page #>, <Date> <Time>",
                &n,
                7
            ),
            "demo: Revenue (Plan) p. 7, 2026-11-02 21:30"
        );
        assert_eq!(expand("<Nope> stays", &n, 1), "<Nope> stays");
    }

    #[test]
    fn helvetica_widths_are_adobes() {
        assert_eq!(text_width("0123456789", 10.0), 55.6);
        assert_eq!(text_width("W", 1000.0), 944.0);
        assert!((text_width("Il", 10.0) - 5.0).abs() < 1e-4);
    }

    /// A small sheet is one page with its header, footer, items and cells;
    /// numbers end at their column's right edge.
    #[test]
    fn a_small_sheet_is_one_page() {
        let doc = layout(
            &sheet(4, 2),
            &PageLayout::default(),
            &ReportLayout::default(),
            &names(),
        );
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.size, (612.0, 792.0));
        let t = texts(&doc.pages[0]);
        for want in ["Revenue", "Page 1", "Time", "Product 0", "Q3", "301"] {
            assert!(t.contains(&want), "{want} in {t:?}");
        }
        let right = |s: &str| {
            doc.pages[0].iter().find_map(|o| match o {
                Op::Text { x, s: t, size, .. } if t == s => Some(x + text_width(t, *size)),
                _ => None,
            })
        };
        assert!((right("1").unwrap() - right("301").unwrap()).abs() < 0.01);
    }

    /// Too many rows and columns for one page: the data splits across
    /// pages, down first (Top to Bottom) or across first, every page
    /// repeating the item names; landscape and the scale fit more.
    #[test]
    fn a_large_sheet_splits_across_pages_and_repeats_items() {
        let (s, r) = (sheet(120, 14), ReportLayout::default());
        let doc = layout(&s, &PageLayout::default(), &r, &names());
        assert!(doc.pages.len() >= 4, "{}", doc.pages.len());
        for p in &doc.pages {
            let t = texts(p);
            assert!(t.contains(&"Time"), "the corner on every page");
            assert!(t.iter().any(|x| x.starts_with("Product ")));
            assert!(t.iter().any(|x| x.starts_with('Q')));
        }
        // Top to Bottom: page 2 continues page 1's columns further down.
        let first_col = |p: &[Op]| {
            texts(p)
                .into_iter()
                .find(|t| t.starts_with("Product "))
                .map(str::to_owned)
        };
        assert_eq!(first_col(&doc.pages[0]), first_col(&doc.pages[1]));
        let across = ReportLayout {
            across_first: true,
            ..r.clone()
        };
        let doc2 = layout(&s, &PageLayout::default(), &across, &names());
        assert_eq!(doc2.pages.len(), doc.pages.len());
        assert_ne!(first_col(&doc2.pages[0]), first_col(&doc2.pages[1]));
        let wide = PageLayout {
            landscape: true,
            scale: 50,
            ..PageLayout::default()
        };
        let small = layout(&s, &wide, &r, &names());
        assert_eq!(small.size, (792.0, 612.0));
        assert!(small.pages.len() < doc.pages.len());
        // Every mark stays on its page.
        for p in doc.pages.iter().chain(&small.pages) {
            for o in p {
                if let Op::Text { x, y, .. } = o {
                    assert!(*x >= 0.0 && *y <= 792.0, "{o:?}");
                }
            }
        }
    }

    #[test]
    fn content_picks_data_formulas_or_both() {
        let s = sheet(3, 2);
        let p = PageLayout::default();
        let only = |data, formulas| ReportLayout {
            data,
            formulas,
            ..ReportLayout::default()
        };
        let both = layout(&s, &p, &only(true, true), &names());
        assert_eq!(both.pages.len(), 2);
        assert!(texts(&both.pages[1]).contains(&"1. Revenue = Price * Quantity"));
        assert_eq!(
            texts(&both.pages[1])
                .iter()
                .filter(|t| **t == "Page 2")
                .count(),
            1
        );
        let f = layout(&s, &p, &only(false, true), &names());
        assert_eq!(f.pages.len(), 1);
        assert!(!texts(&f.pages[0]).contains(&"Q1"));
        assert!(layout(&s, &p, &only(false, false), &names())
            .pages
            .is_empty());
        let numbered = ReportLayout {
            first_page: 5,
            ..ReportLayout::default()
        };
        assert!(texts(&layout(&s, &p, &numbered, &names()).pages[0]).contains(&"Page 5"));
    }

    /// No rules unless the sheet's areas are ruled.
    #[test]
    fn rules_follow_the_grid_options() {
        let mut s = sheet(2, 2);
        let lines = |s: &Sheet| {
            layout(
                s,
                &PageLayout::default(),
                &ReportLayout::default(),
                &names(),
            )
            .pages[0]
                .iter()
                .filter(|o| matches!(o, Op::Line { .. }))
                .count()
        };
        let all = lines(&s);
        s.cell_rules = false;
        let items = lines(&s);
        s.item_rules = false;
        assert!(all > items && items > 0 && lines(&s) == 0);
    }

    #[test]
    fn literals_escape_and_encode_latin1() {
        assert_eq!(literal(r"a(b)\c"), r"a\(b\)\\c");
        assert_eq!(literal("Été ×2 ▸"), "\\311t\\351 \\3272 ?");
    }

    /// The PDF's cross-reference table points at each object, and its
    /// stream lengths are right.
    #[test]
    fn the_pdf_is_well_formed() {
        let doc = layout(
            &sheet(120, 14),
            &PageLayout::default(),
            &ReportLayout::default(),
            &names(),
        );
        let pdf = String::from_utf8(doc.to_pdf()).unwrap();
        assert!(pdf.starts_with("%PDF-1.4\n") && pdf.ends_with("%%EOF\n"));
        let start: usize = pdf
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[start..].starts_with("xref\n0 "));
        let entries: Vec<usize> = pdf[start..]
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .map(|l| l[..10].parse().unwrap())
            .collect();
        assert_eq!(entries.len(), 4 + 2 * doc.pages.len());
        for (i, off) in entries.iter().enumerate() {
            assert!(
                pdf[*off..].starts_with(&format!("{} 0 obj\n", i + 1)),
                "object {}",
                i + 1
            );
        }
        for part in pdf.split("<< /Length ").skip(1) {
            let (len, rest) = part.split_once(" >>\nstream\n").unwrap();
            let len: usize = len.parse().unwrap();
            assert!(rest[len..].starts_with("endstream"), "stream length {len}");
        }
        assert!(pdf.contains(&format!("/Count {}", doc.pages.len())));
        assert!(pdf.contains("(Q119) Tj"));
    }

    /// `IMPROV_PRINT_SAMPLE=dir cargo test` leaves a sample PDF and
    /// PostScript there, for checking them with qpdf and Ghostscript.
    #[test]
    fn sample_files_for_external_checks() {
        let Some(dir) = std::env::var_os("IMPROV_PRINT_SAMPLE") else {
            return;
        };
        let mut s = sheet(120, 14);
        s.rows[0].0[0] = "\u{c9}t\u{e9} (sp\\ecial)".into();
        let r = ReportLayout {
            formulas: true,
            ..ReportLayout::default()
        };
        let doc = layout(&s, &PageLayout::default(), &r, &names());
        let dir = std::path::Path::new(&dir);
        std::fs::write(dir.join("sample.pdf"), doc.to_pdf()).unwrap();
        std::fs::write(dir.join("sample.ps"), doc.to_postscript()).unwrap();
    }

    /// `IMPROV_PRINT_LIVE=<printer>` (with `lp` or `lpr` on PATH, and
    /// CUPS_SERVER if the scheduler is not the system's) sends a real job.
    #[cfg(not(windows))]
    #[test]
    fn a_live_job_reaches_the_print_system() {
        let Ok(printer) = std::env::var("IMPROV_PRINT_LIVE") else {
            return;
        };
        let doc = layout(
            &sheet(60, 4),
            &PageLayout::default(),
            &ReportLayout::default(),
            &names(),
        );
        let (cmd, args, f) = local_spooler(&printer, 1).expect("lp or lpr on PATH");
        let data = match f {
            Format::Pdf => doc.to_pdf(),
            Format::PostScript => doc.to_postscript(),
        };
        spool(cmd, &args, &data).expect("the spooler takes the job");
    }

    #[test]
    fn the_postscript_has_one_dsc_page_per_page() {
        let doc = layout(
            &sheet(120, 14),
            &PageLayout::default(),
            &ReportLayout::default(),
            &names(),
        );
        let ps = String::from_utf8(doc.to_postscript()).unwrap();
        assert!(ps.starts_with("%!PS-Adobe-3.0\n") && ps.ends_with("%%EOF\n"));
        assert!(ps.contains(&format!("%%Pages: {}\n", doc.pages.len())));
        assert_eq!(ps.matches("%%Page: ").count(), doc.pages.len());
        assert_eq!(ps.matches("showpage").count(), doc.pages.len());
        assert!(ps.contains("(Q119) "));
    }

    #[test]
    fn a_page_range_is_clamped() {
        let doc = layout(
            &sheet(120, 14),
            &PageLayout::default(),
            &ReportLayout::default(),
            &names(),
        );
        let n = doc.pages.len();
        assert_eq!(doc.range(None).pages.len(), n);
        assert_eq!(doc.range(Some((2, 3))).pages, doc.pages[1..3]);
        assert_eq!(doc.range(Some((0, 99))).pages.len(), n);
        assert!(doc.range(Some((n + 1, n + 5))).pages.is_empty());
        assert!(doc.range(Some((3, 2))).pages.is_empty());
    }

    #[test]
    fn the_spooler_is_lp_else_lpr() {
        let lp = spooler("", 2, false, |c| c == "lp" || c == "lpr").unwrap();
        assert_eq!(lp, ("lp", vec!["-n".into(), "2".into()], Format::Pdf));
        let named = spooler(" hp ", 1, false, |c| c == "lp").unwrap();
        assert_eq!(named.1, ["-n", "1", "-d", "hp"]);
        assert_eq!(
            spooler("", 1, true, |c| c == "lp").unwrap().2,
            Format::PostScript
        );
        let lpr = spooler("hp", 3, false, |c| c == "lpr").unwrap();
        assert_eq!(
            lpr,
            (
                "lpr",
                vec!["-#3".into(), "-P".into(), "hp".into()],
                Format::PostScript
            )
        );
        assert_eq!(spooler("", 0, false, |c| c == "lpr").unwrap().1, ["-#1"]);
        assert!(spooler("", 1, false, |_| false).is_none());
    }

    /// The job goes to the spooler's standard input; its error output is
    /// the error.
    #[cfg(unix)]
    #[test]
    fn spool_feeds_the_job_and_reports_failure() {
        let dir = std::env::temp_dir().join(format!("improv-spool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("job");
        let args = vec!["-c".to_owned(), format!("cat > '{}'", out.display())];
        spool("sh", &args, b"%!PS\n").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"%!PS\n");
        let fail = vec![
            "-c".to_owned(),
            "cat >/dev/null; echo no printer >&2; exit 1".to_owned(),
        ];
        assert_eq!(spool("sh", &fail, b"x").unwrap_err(), "sh: no printer");
        assert!(spool("improv-no-such-spooler", &[], b"x").is_err());
        assert!(on_path("sh") && !on_path("improv-no-such-spooler"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

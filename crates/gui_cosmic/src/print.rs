//! The grid as a PDF, for printing: the shown measure's headers and cells
//! in pages of A4 landscape, Helvetica (a PDF base font, so nothing is
//! embedded), rules between rows. Pure: rows of text in, bytes out.

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

/// A4 landscape, in points.
const PAGE: (f32, f32) = (842.0, 595.0);
const MARGIN: f32 = 36.0;
const ROW: f32 = 16.0;
const SIZE: f32 = 9.0;

/// `rows` (the first is the column headers, each row's first cell its row
/// header) as a PDF titled `title`, as many pages as the rows need. Columns
/// share the page width; a page that is too narrow for them clips each
/// cell's text to its column.
pub fn table(title: &str, rows: &[Vec<String>]) -> Vec<u8> {
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let col_w = (PAGE.0 - 2.0 * MARGIN) / ncols as f32;
    let per_page = (((PAGE.1 - 2.0 * MARGIN) / ROW) as usize)
        .saturating_sub(2)
        .max(1);
    let (head, body) = rows
        .split_first()
        .map_or((None, &[][..]), |(h, b)| (Some(h), b));
    let chunks: Vec<&[Vec<String>]> = if body.is_empty() {
        vec![&[]]
    } else {
        body.chunks(per_page).collect()
    };

    let mut pdf = Pdf::new();
    let (catalog, tree, font) = (Ref::new(1), Ref::new(2), Ref::new(3));
    let font_name = Name(b"F1");
    let pages: Vec<(Ref, Ref)> = (0..chunks.len())
        .map(|i| (Ref::new(4 + 2 * i as i32), Ref::new(5 + 2 * i as i32)))
        .collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(pages.iter().map(|(p, _)| *p))
        .count(pages.len() as i32);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));

    for (n, (chunk, (page_id, content_id))) in chunks.iter().zip(&pages).enumerate() {
        let mut page = pdf.page(*page_id);
        page.media_box(Rect::new(0.0, 0.0, PAGE.0, PAGE.1));
        page.parent(tree);
        page.contents(*content_id);
        page.resources().fonts().pair(font_name, font);
        page.finish();

        let mut c = Content::new();
        let mut y = PAGE.1 - MARGIN;
        let footer = format!("{title}  page {} of {}", n + 1, chunks.len());
        text(&mut c, font_name, MARGIN, y, &footer, f32::MAX);
        y -= ROW * 1.5;
        let lines = head.into_iter().chain(chunk.iter());
        for (i, row) in lines.enumerate() {
            for (j, cell) in row.iter().enumerate() {
                text(
                    &mut c,
                    font_name,
                    MARGIN + j as f32 * col_w + 2.0,
                    y,
                    cell,
                    col_w - 4.0,
                );
            }
            // A rule under the header, a light one under each row.
            c.set_line_width(if i == 0 && head.is_some() { 0.8 } else { 0.2 });
            c.move_to(MARGIN, y - 4.0);
            c.line_to(PAGE.0 - MARGIN, y - 4.0);
            c.stroke();
            y -= ROW;
        }
        pdf.stream(*content_id, &c.finish());
    }
    pdf.finish()
}

/// Print `pdf`: through the XDG print portal (its dialog picks the
/// printer and settings) on the free Unixes; elsewhere it is saved beside
/// the other temporary files and opened with the default handler, whose
/// own Print prints it. Returns what to tell the user.
pub async fn send(title: &str, pdf: Vec<u8>) -> String {
    let path = std::env::temp_dir().join(format!(
        "improv-{}-{}.pdf",
        std::process::id(),
        title
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    ));
    if let Err(e) = std::fs::write(&path, &pdf) {
        return format!("not printed: {}: {e}", path.display());
    }
    portal(title, &path).await
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn portal(title: &str, path: &std::path::Path) -> String {
    use cosmic::dialog::ashpd::desktop::print::PrintProxy;
    use std::os::fd::AsFd;
    let run = async {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let proxy = PrintProxy::new().await.map_err(|e| e.to_string())?;
        let pre = proxy
            .prepare_print(
                None,
                title,
                Default::default(),
                Default::default(),
                None,
                true,
            )
            .await
            .map_err(|e| e.to_string())?
            .response()
            .map_err(|e| e.to_string())?;
        proxy
            .print(None, title, &file.as_fd(), Some(pre.token), true)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    };
    let r = run.await;
    let _ = std::fs::remove_file(path);
    match r {
        Ok(()) => format!("sent {title} to the printer"),
        Err(e) if e.contains("Cancelled") || e.contains("cancelled") => "not printed".into(),
        Err(e) => format!("not printed (the print portal: {e})"),
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
async fn portal(_title: &str, path: &std::path::Path) -> String {
    // shortcut: no print dialog of our own off the portal; the PDF opens in
    // the default viewer, which prints. A native print path (Windows'
    // print verb) is the upgrade.
    // `start "" <file>`: the shell's default handler, with no new dependency.
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn();
    match r {
        Ok(_) => format!("opened {} to print", path.display()),
        Err(e) => format!("not printed: {}: {e}", path.display()),
    }
}

/// Draw `s` at (`x`, `y`) in the base font, clipped to about `width`.
fn text(c: &mut Content, font: Name<'_>, x: f32, y: f32, s: &str, width: f32) {
    // Helvetica averages about half the size per character.
    let fits = (width / (SIZE * 0.5)).max(1.0) as usize;
    let mut bytes = winansi(s);
    if bytes.len() > fits {
        bytes.truncate(fits.saturating_sub(1));
        bytes.push(b'.');
    }
    c.begin_text();
    c.set_font(font, SIZE);
    c.next_line(x, y);
    c.show(Str(&bytes));
    c.end_text();
}

/// `s` in the base fonts' encoding: Latin-1 passes, the rest becomes `?`.
///
/// shortcut: names outside Latin-1 (CJK, emoji) print as `?`; embed a font
/// (and its CMap) if models with them need printing.
pub fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| {
            u8::try_from(u32::from(c))
                .ok()
                .filter(|b| *b >= 0x20)
                .unwrap_or(b'?')
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize) -> Vec<Vec<String>> {
        let mut rows = vec![vec![String::new(), "Widget".into(), "Gadget".into()]];
        rows.extend((0..n).map(|i| vec![format!("Q{i}"), i.to_string(), (2 * i).to_string()]));
        rows
    }

    #[test]
    fn a_table_is_a_pdf_with_its_text() {
        let pdf = table("Price", &grid(4));
        assert!(pdf.starts_with(b"%PDF-"));
        let s = String::from_utf8_lossy(&pdf);
        for t in [
            "(Widget)",
            "(Q3)",
            "(6)",
            "Helvetica",
            "(Price  page 1 of 1)",
        ] {
            assert!(s.contains(t), "{t} in the PDF");
        }
    }

    #[test]
    fn many_rows_take_pages_and_each_repeats_the_header() {
        let pdf = table("T", &grid(100));
        let s = String::from_utf8_lossy(&pdf);
        let pages = s.matches("/Type /Page\n").count() + s.matches("/Type /Page ").count();
        assert!(s.contains("page 4 of 4"), "{pages} pages");
        assert_eq!(s.matches("(Gadget)").count(), 4);
        // Empty, it is still one page.
        assert!(String::from_utf8_lossy(&table("E", &[])).contains("page 1 of 1"));
    }

    /// Writes the 100-row PDF for a reader to check (`IMPROV_PDF_OUT`).
    #[test]
    fn write_one_for_a_reader() {
        if let Some(p) = std::env::var_os("IMPROV_PDF_OUT") {
            std::fs::write(p, table("Price", &grid(100))).unwrap();
        }
    }

    #[test]
    fn text_outside_latin_1_is_replaced() {
        assert_eq!(winansi("é ü"), vec![0xe9, b' ', 0xfc]);
        assert_eq!(winansi("年度\t"), b"???".to_vec());
    }
}

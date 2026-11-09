//! A range of cells on the clipboard: written as tab-separated text and as
//! an HTML table (what spreadsheets and editors paste), read back from
//! either. Pure; `app` hands these to the clipboard.
//!
//! TSV is the spreadsheet convention: one line per row, a tab between
//! cells, a field quoted (`"…"`, `""` for a quote) only when it holds a
//! tab, a newline or a quote. HTML is one `<table>` with each cell's text
//! escaped, the headers in `<th>`; on paste the cells are read from its
//! `<td>` elements, tags inside them dropped and entities decoded, and the
//! `<th>` headers skipped (they label the values, they are not values).

use std::borrow::Cow;

use cosmic::iced::clipboard::mime::{AllowedMimeTypes, AsMimeTypes};

/// The MIME types written, most specific first.
pub const HTML: &str = "text/html";
pub const TEXT: &str = "text/plain;charset=utf-8";
const TEXT_PLAIN: &str = "text/plain";
const UTF8_STRING: &str = "UTF8_STRING";

/// Rows of cell texts as TSV.
pub fn to_tsv(rows: &[Vec<String>]) -> String {
    let field = |s: &String| {
        if s.contains(['\t', '\n', '\r', '"']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.clone()
        }
    };
    rows.iter()
        .map(|r| r.iter().map(field).collect::<Vec<_>>().join("\t"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// TSV text as rows of cell texts (a trailing newline ends the last row,
/// as spreadsheets write it). Quoted fields may hold tabs and newlines.
pub fn from_tsv(text: &str) -> Vec<Vec<String>> {
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let mut rows = vec![];
    let mut row = vec![];
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut at_start = true;
    while let Some(c) = chars.next() {
        match c {
            '"' if at_start && !quoted => quoted = true,
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    let _ = chars.next();
                } else {
                    quoted = false;
                }
            }
            '\t' if !quoted => {
                row.push(std::mem::take(&mut cell));
                at_start = true;
                continue;
            }
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                at_start = true;
                continue;
            }
            c => cell.push(c),
        }
        at_start = false;
    }
    row.push(cell);
    rows.push(row);
    rows
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Rows of cell texts as an HTML table: when `header`, the first row and
/// the first column in `<th>` (the grid's column and row headers), the
/// values in `<td>`.
pub fn to_html(rows: &[Vec<String>], header: bool) -> String {
    let mut out = String::from("<meta charset=\"utf-8\"><table>\n");
    for (i, r) in rows.iter().enumerate() {
        out.push_str("<tr>");
        for (j, c) in r.iter().enumerate() {
            let tag = if header && (i == 0 || j == 0) {
                "th"
            } else {
                "td"
            };
            out.push_str(&format!("<{tag}>{}</{tag}>", escape(c)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>");
    out
}

/// The cells of the first `<table>` in `html`, row by row; `None` without
/// one. Text inside a cell keeps its words, its tags dropped. Header cells
/// (`<th>`) are skipped, and a row of headers only with them.
pub fn from_html(html: &str) -> Option<Vec<Vec<String>>> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<table")?;
    let end = lower[start..]
        .find("</table>")
        .map_or(html.len(), |e| start + e);
    let (t, tl) = (&html[start..end], &lower[start..end]);
    let mut rows = Vec::new();
    let mut at = 0;
    while let Some(r) = tl[at..].find("<tr") {
        let r0 = at + r;
        let r1 = tl[r0 + 3..].find("<tr").map_or(t.len(), |n| r0 + 3 + n);
        let (row, rowl) = (&t[r0..r1], &tl[r0..r1]);
        let mut cells = Vec::new();
        let mut headers = 0;
        let mut c = 0;
        while let Some(o) = [rowl[c..].find("<td"), rowl[c..].find("<th")]
            .into_iter()
            .flatten()
            .min()
        {
            let open = c + o;
            let header = rowl[open..].starts_with("<th");
            let Some(gt) = rowl[open..].find('>') else {
                break;
            };
            let body = open + gt + 1;
            let close = [
                rowl[body..].find("</td"),
                rowl[body..].find("</th"),
                rowl[body..].find("<td"),
                rowl[body..].find("<th"),
            ]
            .into_iter()
            .flatten()
            .min()
            .map_or(row.len(), |e| body + e);
            if header {
                headers += 1;
            } else {
                cells.push(unescape(strip_tags(&row[body..close]).trim()));
            }
            c = close;
        }
        if headers == 0 || !cells.is_empty() {
            rows.push(cells);
        }
        at = r1;
    }
    Some(rows)
}

/// `s` without its tags, a run of whitespace as one space.
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A copied range, offered as HTML and as text.
#[derive(Debug, Clone)]
pub struct Range {
    pub tsv: String,
    pub html: String,
}

impl AsMimeTypes for Range {
    fn available(&self) -> Cow<'static, [String]> {
        Cow::Owned(
            [HTML, TEXT, TEXT_PLAIN, UTF8_STRING]
                .map(String::from)
                .to_vec(),
        )
    }

    fn as_bytes(&self, mime: &str) -> Option<Cow<'static, [u8]>> {
        match mime {
            HTML => Some(Cow::Owned(self.html.clone().into_bytes())),
            TEXT | TEXT_PLAIN | UTF8_STRING => Some(Cow::Owned(self.tsv.clone().into_bytes())),
            _ => None,
        }
    }
}

/// What a paste read: cells, from HTML when it had a table, else text.
#[derive(Debug, Clone, PartialEq)]
pub struct Pasted(pub Vec<Vec<String>>);

impl AllowedMimeTypes for Pasted {
    fn allowed() -> Cow<'static, [String]> {
        Cow::Owned(
            [HTML, TEXT, TEXT_PLAIN, UTF8_STRING]
                .map(String::from)
                .to_vec(),
        )
    }
}

impl TryFrom<(Vec<u8>, String)> for Pasted {
    type Error = std::string::FromUtf8Error;

    fn try_from((data, mime): (Vec<u8>, String)) -> Result<Self, Self::Error> {
        let text = String::from_utf8(data)?;
        Ok(Pasted(
            (mime == HTML)
                .then(|| from_html(&text))
                .flatten()
                .unwrap_or_else(|| from_tsv(&text)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn tsv_round_trips_awkward_cells() {
        let r = rows(&[&["a", "b\tc", "say \"hi\""], &["", "two\nlines", "1,500.5"]]);
        let t = to_tsv(&r);
        assert_eq!(
            t,
            "a\t\"b\tc\"\t\"say \"\"hi\"\"\"\n\t\"two\nlines\"\t1,500.5"
        );
        assert_eq!(from_tsv(&t), r);
    }

    #[test]
    fn tsv_from_a_spreadsheet_reads() {
        // LibreOffice ends with a newline; Excel with CRLF.
        assert_eq!(from_tsv("1\t2\n3\t4\n"), rows(&[&["1", "2"], &["3", "4"]]));
        assert_eq!(
            from_tsv("1\t2\r\n3\t4\r\n"),
            rows(&[&["1", "2"], &["3", "4"]])
        );
        assert_eq!(from_tsv("x"), rows(&[&["x"]]));
    }

    #[test]
    fn html_round_trips_and_escapes() {
        let r = rows(&[&["Q1", "<b>&"], &["1", "2"]]);
        let h = to_html(&r, false);
        assert!(h.contains("<td>&lt;b&gt;&amp;</td>"), "{h}");
        assert_eq!(from_html(&h), Some(r));
    }

    #[test]
    fn headers_go_in_th_and_a_paste_reads_the_values_only() {
        // What a copy writes: the corner, column headers, row headers.
        let h = to_html(&rows(&[&["", "Q1", "Q2"], &["2025", "1", "<2>"]]), true);
        assert!(
            h.contains("<tr><th></th><th>Q1</th><th>Q2</th></tr>"),
            "{h}"
        );
        assert!(
            h.contains("<th>2025</th><td>1</td><td>&lt;2&gt;</td>"),
            "{h}"
        );
        assert_eq!(from_html(&h), Some(rows(&[&["1", "<2>"]])));
        // A row with no cells at all still counts (it moves the next one).
        assert_eq!(
            from_html("<table><tr></tr><tr><td>x</td></tr></table>"),
            Some(vec![vec![], vec!["x".to_string()]])
        );
    }

    #[test]
    fn html_from_other_apps_reads() {
        // As LibreOffice and browsers write it: attributes, nested tags,
        // entities, cells without closing tags.
        let h =
            "<html><body><TABLE border=1><TR><TD class=x><p>A&nbsp;1</p></TD><td>2&#44;5</td></tr>\
                 <tr><td>3<td>&#x34;</tr></TABLE></body></html>";
        assert_eq!(
            from_html(h),
            Some(rows(&[&["A\u{a0}1", "2,5"], &["3", "4"]]))
        );
        assert_eq!(from_html("<p>no table</p>"), None);
        assert_eq!(unescape("a & b &bogus; &#xZZ;"), "a & b &bogus; &#xZZ;");
    }

    #[test]
    fn the_range_offers_html_and_text_and_a_paste_prefers_html() {
        let r = Range {
            tsv: "1\t2".into(),
            html: to_html(&rows(&[&["1", "2"]]), false),
        };
        assert_eq!(r.available()[0], HTML);
        assert_eq!(r.as_bytes(TEXT).as_deref(), Some(&b"1\t2"[..]));
        assert!(r.as_bytes("image/png").is_none());
        let p = Pasted::try_from((r.html.clone().into_bytes(), HTML.to_string())).unwrap();
        assert_eq!(p, Pasted(rows(&[&["1", "2"]])));
        let p = Pasted::try_from((b"5\t6".to_vec(), TEXT_PLAIN.to_string())).unwrap();
        assert_eq!(p, Pasted(rows(&[&["5", "6"]])));
    }
}

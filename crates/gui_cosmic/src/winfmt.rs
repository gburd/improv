//! The Windows data formats improv-cosmic exchanges with other
//! applications, as pure functions (so they are tested on every platform;
//! `win` moves them through the clipboard and OLE drag and drop):
//!
//! - **"HTML Format"** (CF_HTML): an HTML fragment in a small document,
//!   after a header of byte offsets into the whole buffer:
//!
//!   ```text
//!   Version:0.9
//!   StartHTML:0000000105        offset of "<html>"
//!   EndHTML:0000000187          offset just past "</html>" (the length)
//!   StartFragment:0000000141    just past "<!--StartFragment-->"
//!   EndFragment:0000000151      at "<!--EndFragment-->"
//!   <html>\r\n<body>\r\n<!--StartFragment-->…<!--EndFragment-->\r\n</body>\r\n</html>
//!   ```
//!
//!   Offsets count bytes of the UTF-8 buffer, not characters. Reference:
//!   <https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format>.
//! - **CF_UNICODETEXT**: UTF-16LE, NUL-terminated.
//! - **CF_HDROP** (files dropped from Explorer): a `DROPFILES` header, then
//!   NUL-terminated paths and an empty one.

const START_MARK: &str = "<!--StartFragment-->";
const END_MARK: &str = "<!--EndFragment-->";
const BODY_HEAD: &str = "<html>\r\n<body>\r\n<!--StartFragment-->";
const BODY_TAIL: &str = "<!--EndFragment-->\r\n</body>\r\n</html>";

/// `fragment` as a CF_HTML buffer (UTF-8, no terminating NUL).
pub fn cf_html(fragment: &str) -> String {
    // Every offset is written as 10 digits, so the header's length is fixed.
    let header = |sh: usize, eh: usize, sf: usize, ef: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{sh:010}\r\nEndHTML:{eh:010}\r\n\
             StartFragment:{sf:010}\r\nEndFragment:{ef:010}\r\n"
        )
    };
    let start_html = header(0, 0, 0, 0).len();
    let start_fragment = start_html + BODY_HEAD.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + BODY_TAIL.len();
    let mut out = header(start_html, end_html, start_fragment, end_fragment);
    out.push_str(BODY_HEAD);
    out.push_str(fragment);
    out.push_str(BODY_TAIL);
    debug_assert_eq!(out.len(), end_html);
    out
}

/// The header's offsets that parse, by key.
#[derive(Debug, Default, PartialEq)]
struct Header {
    start_html: Option<usize>,
    end_html: Option<usize>,
    start_fragment: Option<usize>,
    end_fragment: Option<usize>,
}

fn header(data: &[u8]) -> Header {
    let mut h = Header::default();
    // The header is ASCII lines of `Key:value` before the first tag.
    for line in data.split(|b| *b == b'\n').take(16) {
        let line = String::from_utf8_lossy(line);
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once(':').filter(|_| !line.starts_with('<')) else {
            break;
        };
        let value = value.trim().parse().ok();
        match key.trim() {
            "StartHTML" => h.start_html = value,
            "EndHTML" => h.end_html = value,
            "StartFragment" => h.start_fragment = value,
            "EndFragment" => h.end_fragment = value,
            _ => {}
        }
    }
    h
}

fn slice(data: &[u8], from: Option<usize>, to: Option<usize>) -> Option<&str> {
    let (from, to) = (from?, to?);
    std::str::from_utf8(data.get(from..to)?).ok()
}

/// The HTML fragment of a CF_HTML buffer: by its header's offsets; else
/// (a writer that got them wrong) between the fragment comments; else the
/// whole document. `None` when nothing in it is UTF-8 HTML.
pub fn html_fragment(data: &[u8]) -> Option<&str> {
    let h = header(data);
    if let Some(f) = slice(data, h.start_fragment, h.end_fragment) {
        return Some(f);
    }
    // A buffer from the clipboard may end in NULs (its allocation's size).
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let text = std::str::from_utf8(&data[..end]).ok()?;
    if let (Some(a), Some(b)) = (text.find(START_MARK), text.rfind(END_MARK)) {
        if a + START_MARK.len() <= b {
            return Some(&text[a + START_MARK.len()..b]);
        }
    }
    slice(data, h.start_html, h.end_html.or(Some(end)))
        .or_else(|| text.find('<').map(|i| &text[i..]))
}

/// The cells of an HTML fragment, row by row; `None` when it holds no
/// table row. Excel puts the fragment comments inside its `<table>`, so the
/// fragment may be bare `<tr>` rows: it is read inside a table either way.
pub fn html_cells(fragment: &str) -> Option<Vec<Vec<String>>> {
    crate::clip::from_html(&format!("<table>{fragment}</table>")).filter(|rows| !rows.is_empty())
}

/// `text` as CF_UNICODETEXT: UTF-16LE bytes and a NUL.
pub fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Little-endian UTF-16 units (an odd last byte is dropped).
fn utf16_units(data: &[u8]) -> impl Iterator<Item = u16> + '_ {
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
}

/// CF_UNICODETEXT bytes as text, up to the first NUL (a buffer may be
/// longer than its text). Unpaired surrogates read as U+FFFD.
pub fn from_utf16z(data: &[u8]) -> String {
    let units: Vec<u16> = utf16_units(data).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// The paths in a CF_HDROP buffer, in order. `DROPFILES` is `pFiles` (the
/// list's offset, a u32), a point, `fNC`, then `fWide` (a u32 at byte 16):
/// UTF-16 paths when set, else ANSI (read as UTF-8 here).
pub fn drop_files(data: &[u8]) -> Vec<String> {
    let u32_at = |i: usize| {
        data.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let (Some(start), Some(wide)) = (u32_at(0), u32_at(16)) else {
        return Vec::new();
    };
    let Some(list) = data.get(start..) else {
        return Vec::new();
    };
    let paths: Vec<String> = if wide != 0 {
        let units: Vec<u16> = utf16_units(list).collect();
        units
            .split(|u| *u == 0)
            .take_while(|p| !p.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    } else {
        // shortcut: ANSI paths are read as UTF-8; Explorer always sends
        // wide ones, so read the code page only if an ANSI sender appears.
        list.split(|b| *b == 0)
            .take_while(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()
    };
    paths
}

/// A CF_HDROP buffer of wide `paths` (what Explorer drops), for tests.
#[cfg(test)]
pub fn hdrop(paths: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(20u32.to_le_bytes()); // pFiles: just past DROPFILES
    out.extend([0; 12]); // pt, fNC
    out.extend(1u32.to_le_bytes()); // fWide
    for p in paths {
        out.extend(p.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    }
    out.extend([0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    /// The header as a reader that trusts it sees it.
    fn offsets(buf: &str) -> Header {
        header(buf.as_bytes())
    }

    #[test]
    fn the_header_matches_the_reference_sample_byte_for_byte() {
        // Microsoft's sample, with this encoder's body: a fragment of 10 bytes.
        let buf = cf_html("<tr>1</tr>");
        assert_eq!(
            buf,
            "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000187\r\n\
             StartFragment:0000000141\r\nEndFragment:0000000151\r\n<html>\r\n<body>\r\n\
             <!--StartFragment--><tr>1</tr><!--EndFragment-->\r\n</body>\r\n</html>"
        );
        assert_eq!(html_fragment(buf.as_bytes()), Some("<tr>1</tr>"));
    }

    #[test]
    fn offsets_count_bytes_not_characters() {
        let frag = "<td>Größe ≥ 5 € 🎉</td>";
        let buf = cf_html(frag);
        let h = offsets(&buf);
        let (sf, ef) = (h.start_fragment.unwrap(), h.end_fragment.unwrap());
        assert_eq!(ef - sf, frag.len());
        assert!(frag.len() > frag.chars().count());
        assert_eq!(&buf.as_bytes()[sf..ef], frag.as_bytes());
        assert_eq!(html_fragment(buf.as_bytes()), Some(frag));
    }

    #[test]
    fn a_range_round_trips_through_cf_html() {
        // As a copy writes it: headers in <th>, which a paste skips.
        let r = rows(&[&["", "Q1 – Größe", "<b>"], &["Région", "1", "a & b"]]);
        let buf = cf_html(&crate::clip::to_html(&r, true));
        assert_eq!(
            html_cells(html_fragment(buf.as_bytes()).unwrap()),
            Some(rows(&[&["1", "a & b"]]))
        );
    }

    #[test]
    fn other_writers_are_read() {
        // .NET (PowerShell's Set-Clipboard -AsHtml): 8-digit offsets, a
        // SourceURL with colons, and NULs after the end.
        let body = "<html><body><!--StartFragment--><table><tr><td>7</td><td>8</td></tr></table><!--EndFragment--></body></html>";
        let head_len = "Version:1.0\r\nStartHTML:00000000\r\nEndHTML:00000000\r\nStartFragment:00000000\r\nEndFragment:00000000\r\nSourceURL:about:blank\r\n".len();
        let sf = head_len + body.find(START_MARK).unwrap() + START_MARK.len();
        let ef = head_len + body.find(END_MARK).unwrap();
        let buf = format!(
            "Version:1.0\r\nStartHTML:{head_len:08}\r\nEndHTML:{:08}\r\nStartFragment:{sf:08}\r\nEndFragment:{ef:08}\r\nSourceURL:about:blank\r\n{body}\0\0",
            head_len + body.len()
        );
        assert_eq!(
            html_cells(html_fragment(buf.as_bytes()).unwrap()),
            Some(rows(&[&["7", "8"]]))
        );
        // Excel: the fragment comments inside the table, bare rows between.
        let excel = "Version:0.9\r\nStartHTML:-1\r\nEndHTML:-1\r\nStartFragment:999999\r\nEndFragment:9\r\n\
                     <html><body><table border=0>\r\n<!--StartFragment-->\r\n <col width=64>\r\n \
                     <tr height=20>\r\n  <td height=20 width=64>a</td>\r\n  <td align=right>1.5</td>\r\n </tr>\r\n\
                     <!--EndFragment-->\r\n</table></body></html>";
        assert_eq!(
            html_cells(html_fragment(excel.as_bytes()).unwrap()),
            Some(rows(&[&["a", "1.5"]]))
        );
    }

    #[test]
    fn bad_offsets_fall_back_to_the_comments_then_the_document() {
        let frag = "<table><tr><td>x</td></tr></table>";
        // An offset inside a multi-byte character: not UTF-8 there.
        let buf = cf_html(&format!("é{frag}"))
            .replace("StartFragment:0000000141", "StartFragment:0000000142");
        assert_eq!(html_fragment(buf.as_bytes()), Some(&*format!("é{frag}")));
        // No usable offsets, no comments: the document from its first tag.
        assert_eq!(
            html_fragment(b"Version:0.9\r\n<p>hi</p>"),
            Some("<p>hi</p>")
        );
        assert_eq!(html_fragment(b"Version:0.9\r\nStartHTML:x\r\n"), None);
        assert_eq!(html_fragment(&[0xff, 0xfe, b'<']), None);
        assert_eq!(html_cells("<p>no rows</p>"), None);
    }

    #[test]
    fn unicode_text_round_trips_and_stops_at_the_nul() {
        let t = "a\tGröße 🎉\n€";
        let b = utf16z(t);
        assert_eq!(b.len(), (t.encode_utf16().count() + 1) * 2);
        assert_eq!(&b[b.len() - 2..], &[0, 0]);
        assert_eq!(from_utf16z(&b), t);
        // Padding after the NUL, and an odd trailing byte, are ignored.
        let mut padded = b.clone();
        padded.extend([b'x', 0, 0, 0, 7]);
        assert_eq!(from_utf16z(&padded), t);
        // A lone high surrogate (0xd83c), then a NUL and more.
        assert_eq!(from_utf16z(&[0x3c, 0xd8, 0, 0, 0x41, 0]), "\u{fffd}");
        assert_eq!(from_utf16z(&[]), "");
    }

    #[test]
    fn dropped_files_are_read_wide_or_ansi() {
        let b = hdrop(&[r"C:\Users\me\sales.db", r"D:\Größe\b.improv"]);
        assert_eq!(
            drop_files(&b),
            vec![r"C:\Users\me\sales.db", r"D:\Größe\b.improv"]
        );
        let mut ansi = 20u32.to_le_bytes().to_vec();
        ansi.extend([0; 16]);
        ansi.extend(b"C:\\a.db\0C:\\b.db\0\0");
        assert_eq!(drop_files(&ansi), vec!["C:\\a.db", "C:\\b.db"]);
        assert!(drop_files(&[1, 2, 3]).is_empty());
        let mut past = hdrop(&["x"]);
        past[0] = 200;
        assert!(drop_files(&past).is_empty());
    }

    proptest! {
        #[test]
        fn any_text_round_trips_as_utf16(t in "[^\u{0}]*") {
            prop_assert_eq!(from_utf16z(&utf16z(&t)), t);
        }

        #[test]
        fn any_buffer_reads_as_files_without_panicking(data in prop::collection::vec(any::<u8>(), 0..128)) {
            let _ = drop_files(&data);
            let _ = from_utf16z(&data);
        }

        #[test]
        fn any_fragment_round_trips(frag in any::<String>()) {
            let buf = cf_html(&frag);
            prop_assert_eq!(html_fragment(buf.as_bytes()), Some(frag.as_str()));
        }

        #[test]
        fn every_offset_points_where_it_says(frag in any::<String>()) {
            let buf = cf_html(&frag);
            let b = buf.as_bytes();
            let h = offsets(&buf);
            let (sh, eh) = (h.start_html.unwrap(), h.end_html.unwrap());
            let (sf, ef) = (h.start_fragment.unwrap(), h.end_fragment.unwrap());
            prop_assert!(b[sh..].starts_with(b"<html>"));
            prop_assert_eq!(eh, b.len());
            prop_assert!(b[..eh].ends_with(b"</html>"));
            prop_assert!(b[..sf].ends_with(START_MARK.as_bytes()));
            prop_assert!(b[ef..].starts_with(END_MARK.as_bytes()));
            prop_assert_eq!(&b[sf..ef], frag.as_bytes());
            // The header is ASCII and ends where the document starts.
            prop_assert!(b[..sh].is_ascii());
            prop_assert!(sh <= sf && sf <= ef && ef <= eh);
        }

        #[test]
        fn any_range_round_trips(r in prop::collection::vec(prop::collection::vec("[^\u{0}]{0,12}", 1..4), 1..4)) {
            // Cells keep their words; their whitespace runs read as one space.
            let norm = |s: &String| s.split_whitespace().collect::<Vec<_>>().join(" ");
            let want: Vec<Vec<String>> = r.iter().map(|row| row.iter().map(norm).collect()).collect();
            let buf = cf_html(&crate::clip::to_html(&r, false));
            prop_assert_eq!(html_cells(html_fragment(buf.as_bytes()).unwrap()), Some(want));
        }

        #[test]
        fn any_buffer_decodes_without_panicking(data in prop::collection::vec(any::<u8>(), 0..256)) {
            let _ = html_fragment(&data).map(html_cells);
        }

        #[test]
        fn a_header_with_any_offsets_decodes_without_panicking(
            sh in any::<i64>(), eh in any::<i64>(), sf in any::<i64>(), ef in any::<i64>(),
            body in any::<String>(),
        ) {
            let buf = format!("Version:0.9\r\nStartHTML:{sh}\r\nEndHTML:{eh}\r\nStartFragment:{sf}\r\nEndFragment:{ef}\r\n{body}");
            let _ = html_fragment(buf.as_bytes()).map(html_cells);
        }
    }
}

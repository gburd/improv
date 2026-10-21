//! The man pages (`docs/man`), generated from the clap definitions (this
//! crate's `args.rs` and the other binaries' `src/args.rs`), the
//! hand-written sections in `crates/cli/man/`, and the function registry. The tests fail when the committed files differ;
//! `UPDATE_DOCS=1 cargo test -p improv_cli` rewrites them.

use clap::CommandFactory;
use clap_mangen::roff::{bold, roman, Roff};
use clap_mangen::Man;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[path = "../../gui/src/args.rs"]
mod gui_args;
#[allow(dead_code)]
#[path = "../../server/src/args.rs"]
mod server_args;
#[allow(dead_code)]
#[path = "../../tui/src/args.rs"]
mod tui_args;

const REGEN: &str = "UPDATE_DOCS=1 cargo test -p improv_cli";
/// What `roff` writes before every rendering; kept once per page.
const PREAMBLE: &str = ".ie \\n(.g .ds Aq \\(aq\n.el .ds Aq '\n";

/// Every generated page, as (file name, roff).
fn pages() -> Vec<(String, String)> {
    let mut improv = crate::args::Cli::command().disable_help_subcommand(true);
    improv.build();
    let mut out = vec![page(
        &improv,
        include_str!("../man/improv.1.roff"),
        "improv.1.roff",
    )];
    for sub in improv.get_subcommands() {
        let mut see = vec!["improv (1)"];
        match sub.get_name() {
            "add-derived" | "define" => see.extend(["improv-formulas (7)", "improv-functions (7)"]),
            "init" | "import-csv" | "export-csv" => see.push("improv-files (5)"),
            _ => {}
        }
        let see: Vec<String> = see.iter().map(|s| format!(".BR {s}")).collect();
        let hand = format!(".SH SEE ALSO\n{}.\n", see.join(",\n"));
        out.push(page(sub, &hand, ""));
    }
    for (mut cmd, hand, file) in [
        (
            gui_args::Cli::command(),
            include_str!("../man/improv-gui.1.roff"),
            "improv-gui.1.roff",
        ),
        (
            tui_args::Cli::command(),
            include_str!("../man/improv-tui.1.roff"),
            "improv-tui.1.roff",
        ),
        (
            server_args::Cli::command(),
            include_str!("../man/improv-server.1.roff"),
            "improv-server.1.roff",
        ),
    ] {
        cmd.build();
        out.push(page(&cmd, hand, file));
    }
    out.push(("improv-functions.7".into(), functions_page()));
    out
}

/// One command's page: NAME, SYNOPSIS (the usage line `--help` prints),
/// DESCRIPTION, OPTIONS and COMMANDS from clap; a hand-written DESCRIPTION
/// replaces clap's, and the other hand-written sections follow.
fn page(cmd: &clap::Command, hand: &str, source: &str) -> (String, String) {
    let name = cmd.get_display_name().unwrap_or(cmd.get_name()).to_string();
    let man = Man::new(cmd.clone().subcommand_help_heading("COMMANDS"));
    let part = |f: &dyn Fn(&mut Vec<u8>) -> std::io::Result<()>| {
        let mut buf = Vec::new();
        f(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        s.strip_prefix(PREAMBLE).unwrap_or(&s).to_string()
    };
    let from = if source.is_empty() {
        "crates/cli/src/args.rs".to_string()
    } else {
        format!("its parser and crates/cli/man/{source}")
    };
    let mut s = format!(
        ".\\\" Generated from {from} by `{REGEN}`. Do not edit.\n{PREAMBLE}\
         .TH {} 1 \"\" \"improv\" \"Improv Manual\"\n",
        name.to_uppercase()
    );
    s += &part(&|w| man.render_name_section(w));

    let usage = cmd.clone().render_usage().to_string();
    let usage = usage.trim_start_matches("Usage: ");
    let bin = cmd.get_bin_name().unwrap_or(cmd.get_name());
    let mut synopsis = Roff::new();
    synopsis.control("SH", ["SYNOPSIS"]);
    synopsis.text([bold(bin), roman(usage.strip_prefix(bin).unwrap_or(usage))]);
    s += &part(&|w| synopsis.to_writer(w));

    // The hand-written DESCRIPTION, if any, then the rest of the hand text.
    let (desc, rest) = match hand.find(".SH DESCRIPTION") {
        Some(at) => {
            let end = hand[at + 1..]
                .find("\n.SH ")
                .map_or(hand.len(), |e| at + e + 2);
            (&hand[at..end], format!("{}{}", &hand[..at], &hand[end..]))
        }
        None => ("", hand.to_string()),
    };
    if desc.is_empty() {
        s += &part(&|w| man.render_description_section(w));
    } else {
        s += desc;
    }
    s += &part(&|w| man.render_options_section(w));
    if cmd.has_subcommands() {
        s += &part(&|w| man.render_subcommands_section(w));
    }
    // A leading comment line in a hand file is its own; drop it.
    let rest: String = rest
        .lines()
        .filter(|l| !l.starts_with(".\\\""))
        .map(|l| format!("{l}\n"))
        .collect();
    s += &rest;
    (format!("{name}.1"), s)
}

/// `improv-functions(7)`, from the same text as `docs/src/functions.md`.
fn functions_page() -> String {
    let md = improv_core_model::functions::reference_markdown();
    let mut lines = vec![".SH DESCRIPTION".to_string()];
    let (mut in_comment, mut paragraph) = (false, false);
    for line in md.lines() {
        if in_comment || line.starts_with("<!--") {
            in_comment = !line.contains("-->");
        } else if line.is_empty() {
            paragraph = true;
        } else if line.starts_with("# ")
            || line.starts_with("| Function |")
            || line.starts_with("|---")
        {
        } else if let Some(title) = line.strip_prefix("## ") {
            lines.push(format!(".SH \"{}\"", title.to_uppercase()));
            paragraph = false;
        } else if let Some(row) = line.strip_prefix("| ").and_then(|r| r.strip_suffix(" |")) {
            // `| signature | returns | description |`
            let cells: Vec<&str> = row.splitn(3, " | ").collect();
            let [sig, ret, doc] = cells[..] else {
                panic!("not a function row: {line}");
            };
            lines.push(".TP".into());
            lines.push(format!("{} \\(-> {}", inline(sig), inline(ret)));
            lines.push(inline(doc));
            paragraph = false;
        } else {
            if paragraph && !lines.last().is_some_and(|l| l.starts_with(".SH")) {
                lines.push(".PP".into());
            }
            lines.push(inline(line));
            paragraph = false;
        }
    }
    let body = lines.join("\n") + "\n";
    // groff 1.23 has no glyph for the superscripts, subscripts, modifier
    // letters and Hebrew the registry uses: give each a fallback, so every
    // output device prints something and `-ww` stays quiet.
    let mut chars: Vec<char> = body.chars().filter(|&c| no_glyph(c)).collect();
    chars.sort_unstable();
    chars.dedup();
    let fallbacks: String = chars
        .iter()
        .map(|&c| {
            format!(
                ".if !c\\[u{0:04X}] .fchar \\[u{0:04X}] {1}\n",
                c as u32,
                ascii(c)
            )
        })
        .collect();
    let body: String = body
        .chars()
        .map(|c| match c {
            c if c.is_ascii() => c.to_string(),
            c => format!("\\[u{:04X}]", c as u32),
        })
        .collect();
    format!(
        ".\\\" Generated from crates/core_model/src/functions.rs by `{REGEN}`. Do not edit.\n\
         .TH IMPROV-FUNCTIONS 7 \"\" \"improv\" \"Improv Manual\"\n{fallbacks}\
         .SH NAME\nimprov-functions \\- the built-in functions of Improv formulas\n\
         {body}.SH SEE ALSO\n.BR improv-formulas (7),\n.BR improv (1).\n"
    )
}

/// One Markdown line as roff text: code spans bold, links as their text,
/// backslashes escaped, and a leading `.` or `'` kept from reading as a
/// request.
fn inline(md: &str) -> String {
    let md = md
        .replace("\\[", "[")
        .replace("\\]", "]")
        .replace('\\', "\\e");
    let mut out = String::new();
    let mut code = false;
    let mut rest = md.as_str();
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            out += if code { "\\fR" } else { "\\fB" };
            code = !code;
        } else if let Some((text, after)) = link(rest).filter(|_| !code) {
            out += text;
            rest = after;
            continue;
        } else {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    if out.starts_with(['.', '\'']) {
        out.insert_str(0, "\\&");
    }
    out
}

/// A Markdown link `[text](url)` at the start of `s`: its text and the rest.
fn link(s: &str) -> Option<(&str, &str)> {
    let (text, after) = s.strip_prefix('[')?.split_once("](")?;
    let (_, rest) = after.split_once(')')?;
    (!text.contains(['[', ']'])).then_some((text, rest))
}

/// Whether groff 1.23 lacks a glyph for `c` (it warns "special character
/// not defined").
fn no_glyph(c: char) -> bool {
    matches!(c as u32, 0x02B0..=0x036F | 0x0590..=0x05FF | 0x1D00..=0x1DBF | 0x2070..=0x209F)
}

/// What an ASCII terminal shows for `c` when groff has no glyph for it.
fn ascii(c: char) -> String {
    const SUP: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻ⁱᵀᵉᵏʷˣ";
    const SUP_ASCII: &str = "0123456789+-iTekwx";
    const SUB: &str = "₀ᵢₙ";
    const SUB_ASCII: &str = "0in";
    if let Some(i) = SUP.chars().position(|s| s == c) {
        format!("^{}", SUP_ASCII.as_bytes()[i] as char)
    } else if let Some(i) = SUB.chars().position(|s| s == c) {
        format!("_{}", SUB_ASCII.as_bytes()[i] as char)
    } else {
        "?".into()
    }
}

fn repo(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir)
}

/// Compare `files` with `dir` (rewriting it under `UPDATE_DOCS`). `owned`
/// says which files in `dir` the generator makes, so a page for a removed
/// command is stale too.
fn check(dir: &str, files: &[(String, String)], owned: impl Fn(&str) -> bool) {
    let path = repo(dir);
    let update = std::env::var_os("UPDATE_DOCS").is_some();
    std::fs::create_dir_all(&path).unwrap();
    let mut stale: Vec<String> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| owned(n) && !files.iter().any(|(f, _)| f == n))
        .collect();
    for (name, want) in files {
        // The hand-written sources may have come out of a Windows checkout
        // with CRLF line ends.
        let want = &want.replace("\r\n", "\n");
        let file = path.join(name);
        if update {
            std::fs::write(&file, want).unwrap();
        }
        // A Windows checkout may have turned LF into CRLF.
        let have = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if have != *want {
            stale.push(name.clone());
        }
    }
    if update {
        for name in &stale {
            let _ = std::fs::remove_file(path.join(name));
        }
        return;
    }
    stale.sort();
    assert!(stale.is_empty(), "{dir} is stale ({stale:?}): run {REGEN}");
}

/// Pages from a hand-written source, or written by hand in `docs/man`.
fn generated_page(name: &str) -> bool {
    name.ends_with(".1") || name == "improv-functions.7"
}

/// The pages match the generator, and `groff -man -ww -z` is silent on
/// every page. The groff part runs on Linux only (macOS has mandoc or an
/// old groff), and is skipped without groff.
#[test]
fn man_pages_are_current_and_groff_clean() {
    check("docs/man", &pages(), generated_page);
    let mut files: Vec<PathBuf> = std::fs::read_dir(repo("docs/man"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x == "1" || x == "5" || x == "7")
        })
        .collect();
    files.sort();
    assert!(files.len() > 25, "{files:?}");
    if !cfg!(target_os = "linux") {
        return;
    }
    let Ok(out) = std::process::Command::new("groff")
        .args(["-man", "-ww", "-z"])
        .args(&files)
        .output()
    else {
        println!("skipped: groff is not installed");
        return;
    };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && err.is_empty(), "groff: {err}");
}

/// Each command's page has the synopsis `improv help COMMAND` prints, a
/// NAME line `man -k` finds, and a SEE ALSO back to `improv(1)`.
#[test]
fn every_command_has_a_page() {
    let pages = pages();
    let mut improv = crate::args::Cli::command();
    improv.build();
    for sub in improv.get_subcommands().filter(|s| s.get_name() != "help") {
        let file = format!("improv-{}.1", sub.get_name());
        let (_, text) = pages.iter().find(|(f, _)| *f == file).expect(&file);
        let usage = sub.clone().render_usage().to_string();
        let args = usage.trim_start_matches(&format!("Usage: improv {}", sub.get_name()));
        let roff_args = args.replace('-', "\\-").replace('\'', "\\*(Aq");
        let roff_name = sub.get_name().replace('-', "\\-");
        let synopsis = format!(".SH SYNOPSIS\n\\fBimprov {roff_name}\\fR{roff_args}\n");
        assert!(text.contains(&synopsis), "{file}: {usage}");
        assert!(
            text.contains(&format!("\nimprov\\-{roff_name} \\- ")),
            "{file}"
        );
        assert!(text.contains(".BR improv (1)"), "{file}");
    }
    assert!(inline("`a\\b` [x](y) z").ends_with("\\fBa\\eb\\fR x z"));
    assert_eq!(inline(".x"), "\\&.x");
    assert_eq!(ascii('²'), "^2");
    assert_eq!(ascii('ₙ'), "_n");
    assert_eq!(ascii('ב'), "?");
    assert!(no_glyph('ᵀ') && no_glyph('ב') && !no_glyph('≤') && !no_glyph('²'));
}

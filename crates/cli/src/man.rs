//! The man pages (`docs/man`) and shell completions (`docs/completions`),
//! generated from the clap definitions (this crate's `args.rs` and the other
//! binaries' `src/args.rs`), the hand-written sections in `crates/cli/man/`,
//! and the function registry. The tests fail when the committed files differ;
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
    // Which glyphs a groff device has varies by device (ps has no Greek,
    // ascii no Greek or superscripts) and by groff version, so every
    // non-ASCII character gets a fallback; `.if !c` makes it a no-op where
    // the glyph exists, and `-ww` stays quiet on every device.
    let mut chars: Vec<char> = body.chars().filter(|c| !c.is_ascii()).collect();
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
    // `\` and `-` as roff wants them: a backslash, and a minus sign that
    // copies as `-` (a bare `-` is a hyphen).
    let md = md
        .replace("\\[", "[")
        .replace("\\]", "]")
        .replace('\\', "\\e")
        .replace('-', "\\-");
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

/// What an ASCII terminal shows for `c` when groff has no glyph for it.
fn ascii(c: char) -> String {
    const SUP: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻ⁱᵀᵉᵏʷˣ";
    const SUP_ASCII: &str = "0123456789+-iTekwx";
    const SUB: &str = "₀ᵢₙ";
    const SUB_ASCII: &str = "0in";
    const NAMED: [(char, &str); 25] = [
        ('²', "^2"),
        ('³', "^3"),
        ('¹', "^1"),
        ('·', "*"),
        ('×', "x"),
        ('±', "+/-"),
        ('√', "sqrt"),
        ('≠', "!="),
        ('≤', "<="),
        ('≥', ">="),
        ('−', "-"),
        ('–', "-"),
        ('…', "..."),
        ('é', "e"),
        ('ö', "o"),
        ('Γ', "Gamma"),
        ('Δ', "Delta"),
        ('Π', "Pi"),
        ('Σ', "Sigma"),
        ('Φ', "Phi"),
        ('α', "alpha"),
        ('β', "beta"),
        ('γ', "gamma"),
        ('ε', "epsilon"),
        ('λ', "lambda"),
    ];
    const NAMED2: [(char, &str); 6] = [
        ('μ', "mu"),
        ('π', "pi"),
        ('ρ', "rho"),
        ('σ', "sigma"),
        ('χ', "chi"),
        ('ψ', "psi"),
    ];
    if let Some(i) = SUP.chars().position(|s| s == c) {
        format!("^{}", SUP_ASCII.as_bytes()[i] as char)
    } else if let Some(i) = SUB.chars().position(|s| s == c) {
        format!("_{}", SUB_ASCII.as_bytes()[i] as char)
    } else if let Some((_, a)) = NAMED.iter().chain(&NAMED2).find(|(n, _)| *n == c) {
        (*a).into()
    } else {
        "?".into()
    }
}

/// Completion scripts for each binary, for bash, zsh and fish.
fn completions() -> Vec<(String, String)> {
    use clap_complete::{generate, Generator, Shell};
    let mut out = Vec::new();
    for mut cmd in [
        crate::args::Cli::command(),
        gui_args::Cli::command(),
        tui_args::Cli::command(),
        server_args::Cli::command(),
    ] {
        let bin = cmd.get_name().to_string();
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let mut buf = Vec::new();
            generate(shell, &mut cmd, &bin, &mut buf);
            out.push((shell.file_name(&bin), String::from_utf8(buf).unwrap()));
        }
    }
    out
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
        // ascii: the device with the fewest glyphs, as CI checks.
        .args(["-man", "-ww", "-z", "-Tascii"])
        .args(&files)
        .output()
    else {
        println!("skipped: groff is not installed");
        return;
    };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && err.is_empty(), "groff: {err}");
}

#[test]
fn completions_are_current() {
    check("docs/completions", &completions(), |_| true);
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
    assert_eq!(inline("-1"), "\\-1");
    assert_eq!(inline(".x"), "\\&.x");
    assert_eq!(ascii('²'), "^2");
    assert_eq!(ascii('ₙ'), "_n");
    assert_eq!(ascii('ב'), "?");
    assert_eq!(ascii('π'), "pi");
    assert_eq!(ascii('≤'), "<=");
}

/// `docs/src/cli-reference.md`: every binary, command, argument and flag,
/// from the same clap definitions as the man pages.
fn cli_reference() -> String {
    let mut improv = crate::args::Cli::command().disable_help_subcommand(true);
    improv.build();
    let mut s = format!(
        "# Command-line reference\n\n\
         <!-- Generated from the clap definitions (crates/*/src/args.rs) by\n     \
         `{REGEN}`. Do not edit. -->\n\n\
         Every command and flag of the four binaries, as `--help` prints them.\n\
         The same text is in the man pages: `man improv`, `man improv-set`, and\n\
         so on. Every command also takes `-h` (a summary) and `--help`, and each\n\
         binary `-V`/`--version`. [Running Improv](./running.md) shows which\n\
         binary to use when.\n"
    );
    s += &md_command(&improv, "improv", 2);
    for sub in improv.get_subcommands() {
        s += &md_command(sub, &format!("improv {}", sub.get_name()), 3);
    }
    for mut cmd in [
        gui_args::Cli::command(),
        tui_args::Cli::command(),
        server_args::Cli::command(),
    ] {
        cmd.build();
        let name = cmd.get_name().to_string();
        s += &md_command(&cmd, &name, 2);
    }
    s
}

/// One command's section: its long help, usage line, and a table of its
/// arguments and options.
fn md_command(cmd: &clap::Command, title: &str, level: usize) -> String {
    let mut s = format!("\n{} `{title}`\n\n", "#".repeat(level));
    if let Some(about) = cmd.get_long_about().or(cmd.get_about()) {
        let about = about.to_string().replace('<', "&lt;").replace('[', "\\[");
        s += &format!("{}\n\n", about.trim_end());
    }
    let usage = cmd.clone().render_usage().to_string();
    s += &format!("```text\n{usage}\n```\n");
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !["help", "version"].contains(&a.get_id().as_str()))
        .collect();
    if !args.is_empty() {
        s += "\n| Argument | Description |\n| --- | --- |\n";
    }
    for a in args {
        let names = match (a.get_short(), a.get_long()) {
            (Some(c), Some(l)) => format!("`-{c}`, `--{l}`"),
            (None, Some(l)) => format!("`--{l}`"),
            (Some(c), None) => format!("`-{c}`"),
            (None, None) => String::new(),
        };
        let value = a
            .get_value_names()
            .map(|v| v.iter().map(|n| format!("<{n}>")).collect::<Vec<_>>())
            .unwrap_or_default()
            .join(" ");
        let names = match (names.is_empty(), value.is_empty()) {
            (true, _) => format!("`{value}`"),
            (false, true) => names,
            (false, false) => format!("{names} `{value}`"),
        };
        let mut help = a
            .get_long_help()
            .or(a.get_help())
            .map(|h| md_text(&h.to_string()))
            .unwrap_or_default();
        let defaults: Vec<_> = a
            .get_default_values()
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        if !defaults.is_empty() && a.get_action().takes_values() {
            help += &format!(" (default: `{}`)", defaults.join(","));
        }
        if let Some(env) = a.get_env() {
            help += &format!(" (env: `{}`)", env.to_string_lossy());
        }
        s += &format!("| {names} | {} |\n", help.trim());
    }
    s
}

/// clap help text as one Markdown table cell or paragraph: `|` and `<`
/// escaped, line breaks as spaces.
fn md_text(t: &str) -> String {
    // `[` escaped: help text like `args[0]` or `[Cat=Item,...]` is not a link.
    t.replace('|', "\\|")
        .replace('[', "\\[")
        .replace('<', "&lt;")
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// `docs/src/http-reference.md`: every operation of `docs/openapi.json`,
/// with its scope, parameters, body and responses.
fn http_reference() -> String {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/openapi.json")).unwrap();
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let mut s = format!(
        "# HTTP API reference\n\n\
         <!-- Generated from docs/openapi.json by `{REGEN}`. Do not edit. -->\n\n\
         {}\n\n\
         The server serves this document at `/v1/openapi.json`. Point any\n\
         OpenAPI viewer at that URL, or at\n\
         [`docs/openapi.json`]({REPO}/docs/openapi.json), for an interactive\n\
         page. [The HTTP Server](./server.md) covers running, tokens and\n\
         limits.\n\n\
         | Method | Path | Scope | Summary |\n| --- | --- | --- | --- |\n",
        text(&doc["info"]["description"]),
    );
    let paths = doc["paths"].as_object().unwrap();
    let mut ops = Vec::new();
    for (path, item) in paths {
        for (method, op) in item.as_object().unwrap() {
            ops.push((path.as_str(), method.to_uppercase(), op));
        }
    }
    for (path, method, op) in &ops {
        s += &format!(
            "| {method} | [`{path}`](#{}) | {} | {} |\n",
            anchor(&format!("{method} {path}")),
            text(&op["x-improv-scope"]),
            text(&op["summary"])
        );
    }
    for (path, method, op) in &ops {
        s += &format!("\n## {method} {path}\n\n**{}**", text(&op["summary"]));
        s += &format!(" Scope: `{}`.", text(&op["x-improv-scope"]));
        if op["x-improv-unstable"] == true {
            s += " **Unstable:** not covered by the compatibility promise.";
        }
        s += "\n";
        if let Some(d) = op["description"].as_str() {
            s += &format!("\n{d}\n");
        }
        for p in op["parameters"].as_array().into_iter().flatten() {
            let p = resolve(&doc, p);
            s += &format!(
                "\n- Path parameter `{}`: `{}`.\n",
                text(&p["name"]),
                compact(&p["schema"])
            );
        }
        if let Some(body) = op["requestBody"]["content"]["application/json"].as_object() {
            let schema = resolve(&doc, &body["schema"]);
            let example = body.get("example").unwrap_or(&schema["example"]);
            if body["schema"]["$ref"].is_string() {
                s += &format!("\nRequest body: {}.\n", schema_md(&body["schema"]));
            } else {
                s += &format!(
                    "\nRequest body:\n\n```json\n{}\n```\n",
                    serde_json::to_string_pretty(schema).unwrap()
                );
            }
            if !example.is_null() {
                s += &format!("\nExample:\n\n```json\n{}\n```\n", compact(example));
            }
        }
        s += "\n| Status | Response |\n| --- | --- |\n";
        for (status, r) in op["responses"].as_object().unwrap() {
            let r = resolve(&doc, r);
            let mut cell = text(&r["description"]);
            let schema = &r["content"]["application/json"]["schema"];
            if !schema.is_null() {
                cell = format!(
                    "{}. Body: {}.",
                    cell.trim_end_matches('.'),
                    schema_md(schema)
                );
            }
            s += &format!("| {status} | {} |\n", cell.replace('|', "\\|"));
        }
    }
    s += "\n## Schemas\n";
    for (name, schema) in doc["components"]["schemas"].as_object().unwrap() {
        let mut schema = schema.clone();
        schema.as_object_mut().unwrap().remove("example");
        s += &format!(
            "\n### {name}\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&schema).unwrap()
        );
    }
    s
}

const REPO: &str = "https://codeberg.org/gregburd/improv/src/branch/main";

/// The mdBook heading anchor of `text`.
fn anchor(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// `v`, or what its `$ref` points at.
fn resolve<'a>(doc: &'a serde_json::Value, v: &'a serde_json::Value) -> &'a serde_json::Value {
    match v["$ref"].as_str().and_then(|r| r.strip_prefix('#')) {
        Some(ptr) => doc.pointer(ptr).unwrap(),
        None => v,
    }
}

/// A response schema in a table cell: a link to a named schema, an array
/// of one, or the inline JSON.
fn schema_md(v: &serde_json::Value) -> String {
    let named = |v: &serde_json::Value| {
        v["$ref"].as_str().map(|r| {
            let name = r.rsplit('/').next().unwrap();
            format!("[{name}](#{})", name.to_lowercase())
        })
    };
    if let Some(link) = named(v) {
        link
    } else if let Some(link) = (v["type"] == "array").then(|| named(&v["items"])).flatten() {
        format!("array of {link}")
    } else {
        format!("`{}`", compact(v))
    }
}

fn compact(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap()
}

/// The generated reference pages under `docs/src`.
#[test]
fn reference_pages_are_current() {
    let pages = [
        ("cli-reference.md".to_string(), cli_reference()),
        ("http-reference.md".to_string(), http_reference()),
    ];
    check("docs/src", &pages, |n| {
        n == "cli-reference.md" || n == "http-reference.md"
    });
    assert_eq!(
        anchor("POST /v1/measures/{id}/eval"),
        "post-v1measuresideval"
    );
    assert_eq!(md_text("a | b\n<c>"), "a \\| b &lt;c>");
    assert_eq!(md_text("args[0] [Cat=Item]"), "args\\[0] \\[Cat=Item]");
    let doc = serde_json::json!({"$ref": "#/components/schemas/Cell"});
    assert_eq!(schema_md(&doc), "[Cell](#cell)");
    assert_eq!(
        schema_md(&serde_json::json!({"type": "array", "items": doc})),
        "array of [Cell](#cell)"
    );
    assert_eq!(
        schema_md(&serde_json::json!({"type": "object"})),
        "`{\"type\":\"object\"}`"
    );
}

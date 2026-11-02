//! A tripwire for the Windows gaps the COSMIC GUI ships with (see
//! `docs/reviews/2026-11-02-cosmic-gui.md`, "Windows gaps"). They live in
//! libcosmic's dependencies, at the commits `Cargo.lock` pins. When any of
//! those commits moves, this test fails until someone re-checks each gap on
//! the new commit and records the commit they checked here. It never passes
//! by accident: a bump is the moment the gaps may have closed.

/// The commits each gap was last assessed at, and what to re-check there.
const ASSESSED: &[(&str, &str, &str)] = &[
    (
        "libcosmic",
        "95236a944bfd51c63a2fc65fe50a592c47168d40",
        "build on Windows; run it on a desktop",
    ),
    (
        "window_clipboard",
        "f68595ee0e62fbd6589f4709b5aaa5c3c7ea5f6c",
        "src/platform/windows.rs: write_data/read_data and the DndProvider are \
         no-ops (release blocker: range copy/paste and drag and drop on Windows)",
    ),
    (
        "accesskit_winit",
        "6c20249d3b9fd45e7002efad9f6b6171a9257912",
        "builds on Windows? (E0782 here; libcosmic#1152). If so, enable a11y \
         on Windows in Cargo.toml",
    ),
];

/// The commit `Cargo.lock` pins `name` to, from its `git+…#<commit>` source.
fn locked_commit(lock: &str, name: &str) -> Option<String> {
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l == format!("name = \"{name}\"") {
            for l in lines.by_ref().take(3) {
                if let Some(src) = l.strip_prefix("source = \"git+") {
                    return src
                        .rsplit('#')
                        .next()
                        .map(|c| c.trim_end_matches('"').to_string());
                }
            }
        }
    }
    None
}

#[test]
fn the_windows_gaps_are_rechecked_when_the_pin_moves() {
    let lock = include_str!("../Cargo.lock");
    let mut moved = Vec::new();
    for (name, assessed, check) in ASSESSED {
        let now = locked_commit(lock, name).unwrap_or_else(|| panic!("{name} not in Cargo.lock"));
        if now != *assessed {
            moved.push(format!(
                "  {name}: {assessed} -> {now}\n    re-check: {check}"
            ));
        }
    }
    assert!(
        moved.is_empty(),
        "the libcosmic pin moved. Re-check the Windows gaps (review note, \
         \"Windows gaps\"), update the note, then record the new commits in \
         tests/windows_gaps.rs:\n{}",
        moved.join("\n")
    );
}

#[test]
fn locked_commits_are_read_from_git_sources_only() {
    let lock = "[[package]]\nname = \"a\"\nversion = \"1\"\nsource = \"git+https://x/a#abc\"\n\
                [[package]]\nname = \"b\"\nversion = \"1\"\nsource = \"registry+https://x\"\n";
    assert_eq!(locked_commit(lock, "a").as_deref(), Some("abc"));
    assert_eq!(locked_commit(lock, "b"), None);
    assert_eq!(locked_commit(lock, "c"), None);
}

# Improv documentation

This directory holds the Improv documentation:

- `src/` + `book.toml` — the mdBook user and design guide.
- `man/improv.1` — the `improv(1)` man page for the CLI.

## Building the guide

```sh
mdbook build docs        # output goes to docs/book/ (gitignored)
mdbook serve docs        # live-reload preview at http://localhost:3000
```

The build uses two preprocessors:

- `mdbook-mermaid` — renders the ```` ```mermaid ```` diagrams. The
  `mermaid.min.js` and `mermaid-init.js` assets in this directory are installed
  by `mdbook-mermaid install docs`.
- `mdbook-linkcheck` — fails the build on broken intra-book links.

All three tools (`mdbook`, `mdbook-mermaid`, `mdbook-linkcheck`) must be on
`PATH` for `mdbook build` to succeed.

## Viewing the man page

```sh
man -l docs/man/improv.1                 # render and page it
groff -man -Tascii docs/man/improv.1     # render to stdout
```

## Steering rule: docs track code

The documentation must stay accurate to the code. There are no invented
features: anything not yet built is labeled "planned / not yet implemented".
When the code changes, update the docs in the same change:

- CLI command surface (`crates/cli/src/main.rs`) ⇄ the *Getting Started*
  chapter and `man/improv.1`.
- The formula AST and compiler (`crates/core_model`, `crates/engine`) ⇄ the
  *Formula Language* and *Architecture* chapters.
- The CNL grammar (`crates/nl_formula/src/lib.rs`) ⇄ the
  *Controlled Natural Language* chapter.
- The datom schema (`crates/storage_mentat/src/schema.rs`) ⇄ the
  *Storage & Persistence* chapter.

`AGENT_STEERING.md` in the repository root is the source of truth for
architecture, constraints, and per-phase status.

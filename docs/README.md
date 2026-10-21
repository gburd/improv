# Improv documentation

This directory holds the Improv documentation:

- `src/` + `book.toml` — the mdBook user and design guide.
- `man/` — man pages: `improv(1)` (CLI), `improv-tui(1)`, `improv-gui(1)`,
  `improv-server(1)`.

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

## Viewing the man pages

```sh
man -l docs/man/improv.1                 # render and page it (also improv-tui.1, …)
groff -man -Tascii docs/man/improv.1     # render to stdout
UPDATE_DOCS=1 cargo test -p improv_cli   # regenerate the generated pages
```

The `(1)` pages and `improv-functions.7` are generated (see
[Running Improv](src/running.md#man-pages-and-shell-completions)); edit
`crates/cli/src/args.rs` or `crates/cli/man/*.roff`, not the page.

## Where the docs are published

Every push to `main` publishes the guide: to GitHub Pages
(https://gburd.github.io/improv/, with the rustdoc API reference at `/api/`)
by `.github/workflows/docs.yml`, and to Codeberg Pages
(https://gregburd.codeberg.page/improv/) by `.forgejo/workflows/pages.yml`.

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
- The Postgres store (`crates/storage_pg`, `crates/store`) ⇄ *Storage &
  Persistence* and *Running Improv*.
- The pg_improv SQL API, GUCs and sync modes (`crates/pg_improv/src`) ⇄ the
  *Improv in Postgres* chapter and `crates/pg_improv/README.md`; its example
  (`crates/pg_improv/examples/family-finance`) ⇄ *A Family's Finances*.
- The HTTP routes and environment (`crates/server/src/main.rs`) ⇄ *The HTTP
  Server* chapter and `man/improv-server.1`.
- TUI and GUI keys and menus (`crates/tui/src/main.rs`, `crates/gui/src/menu.rs`)
  ⇄ the README, *The Desktop GUI*, `man/improv-tui.1` and `man/improv-gui.1`.

`.agent/AGENT_STEERING.md` is the source of truth for live per-phase status;
`.agent/steering/` holds the detailed architecture and constraints.

# improv-cosmic

Improv's COSMIC desktop GUI, on [libcosmic](https://github.com/pop-os/libcosmic)
(iced). A second GUI beside `crates/gui`; the same model, engine and stores.
Design and platform notes: `docs/reviews/2026-11-02-cosmic-gui.md`.

```sh
cd crates/gui_cosmic
cargo run --locked -- path/to/model.db        # or postgres://…?model=NAME
```

Its own Cargo workspace (and `Cargo.lock`, and `deny.toml`), excluded from
the root: libcosmic's iced, wgpu 28 and winit fork must not unify with the
egui GUI's wgpu 30 and winit 0.30, and its git sources stay out of the
root's cargo-deny gate. `ci/gui_cosmic.sh` is the CI job.

## Pins

libcosmic is git-only, and so are eleven of its companions. Four of those
(cosmic-settings-daemon, dbus-settings-bindings, freedesktop-icons,
rust-atomicwrites) are named by libcosmic with no rev at all, so
`Cargo.lock` is the only thing that pins any of them. **Every build is
`--locked`**: `ci/gui_cosmic.sh`, `ci/solnix-build.sh`, and the commands
here. A bare `cargo build` or `cargo update` would move them silently.

To move the pin (and only on purpose):

```sh
cd crates/gui_cosmic
cargo update -p libcosmic --precise <libcosmic-commit>
git diff --stat Cargo.lock      # see what else moved with it
```

`--precise` moves libcosmic to that commit and re-resolves its git
dependencies, the unpinned four to their current heads, so read the
lockfile diff. Then, before committing the new `Cargo.lock`:

0. `cargo test --locked --test windows_gaps` fails when libcosmic,
   window_clipboard or accesskit_winit moved: re-check each Windows gap
   below on the new commits, then record them in `tests/windows_gaps.rs`;
1. `ci/gui_cosmic.sh` (fmt, clippy, tests, rustdoc, cargo-deny);
2. re-read the license files `deny.toml` clarifies, for every git crate
   whose commit moved (cargo-deny does not hash git crates' licenses), and
   update `deny.toml`'s advisory ignores;
3. build and run on each platform of the review note's matrix: Linux,
   FreeBSD, illumos (`ci/solnix-build.sh --release` on a Solnix host, run
   in its COSMIC session) and Windows (does `accesskit_winit` build there
   yet? then turn `a11y` on for Windows);
4. record the new commit and anything that changed in the review note.

## Release builds

libcosmic is MPL-2.0, except one file: `src/localize.rs` (its Fluent loader
and `fl!` macro) says `SPDX-License-Identifier: GPL-3.0-only`, and it is
always compiled. Improv is MIT OR Apache-2.0, so until upstream answers
[pop-os/libcosmic#1500](https://github.com/pop-os/libcosmic/issues/1500),
**every release or distributed build replaces that file** with Improv's own
implementation, `release/localize.rs` (MIT OR Apache-2.0, written from the
module's interface and i18n-embed's documentation, without reading
upstream's file). Developer builds (`cargo build`, `ci/gui_cosmic.sh`'s
tests) use upstream as is.

```sh
release/build.sh                # vendor, patch, scan, then cargo build --release
release/build.sh test           # the tests, over the same patched tree
release/vendor-libcosmic.sh DIR # vendor + patch + scan only; prints the config
```

`vendor-libcosmic.sh` runs `cargo vendor --locked`, replaces the file,
re-stamps its checksum, and fails if the file is gone or its first line is
no longer that GPL header (a bump that moves or relicenses it needs a
human look). Then `license-scan.py` reads every vendored file's SPDX
headers: GPL and AGPL fail, and so does an LGPL file that is not listed in
`release/license-allow.txt` or differs from its upstream crate (the
policy: CONTRIBUTING.md, "Licences of dependencies").
`ci/solnix-build.sh` applies the same patch and scan after Solnix's
fixups. Once upstream relicenses the file, the patch and this check are
dropped.

## Windows: known limitation and release blockers

It builds and starts on Windows (arm64 here; x64 in CI). At this pin:

- **Accessibility is off on Windows** (known limitation, accepted):
  AccessKit's winit adapter does not build there,
  [pop-os/libcosmic#1152](https://github.com/pop-os/libcosmic/issues/1152).
- **Rich copy and paste, and drag and drop with other applications,** are
  done here (`src/win.rs`, formats in `src/winfmt.rs`), since libcosmic's
  Windows clipboard backend (pop-os `window_clipboard`) leaves both
  unimplemented. The upstream fix is outlined in
  `docs/reviews/patches/window_clipboard-windows-formats.md`. **Still a
  release blocker until checked by hand:** a real drag between two
  applications on an interactive desktop. That file's checklist takes two
  minutes at the console.

`tests/windows_gaps.rs` fails whenever the pin moves, so each bump
re-checks these. Details: the review note, "Windows gaps".

## Linux build dependencies

`libxkbcommon-dev` and `libwayland-dev` (Debian names), as libcosmic's own
CI installs. The renderer is wgpu (Vulkan or GL), with tiny-skia as the
software fallback (`ICED_BACKEND=tiny-skia`).

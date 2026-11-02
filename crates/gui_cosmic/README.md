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

1. `ci/gui_cosmic.sh` (fmt, clippy, tests, rustdoc, cargo-deny);
2. re-read the license files `deny.toml` clarifies, for every git crate
   whose commit moved (cargo-deny does not hash git crates' licenses), and
   update `deny.toml`'s advisory ignores;
3. build and run on each platform of the review note's matrix: Linux,
   FreeBSD, illumos (`ci/solnix-build.sh --release` on a Solnix host, run
   in its COSMIC session) and Windows (does `accesskit_winit` build there
   yet? then turn `a11y` on for Windows);
4. record the new commit and anything that changed in the review note.

## Linux build dependencies

`libxkbcommon-dev` and `libwayland-dev` (Debian names), as libcosmic's own
CI installs. The renderer is wgpu (Vulkan or GL), with tiny-skia as the
software fallback (`ICED_BACKEND=tiny-skia`).

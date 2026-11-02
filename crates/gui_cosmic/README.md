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

libcosmic is git-only. `Cargo.lock` pins it (and its companion forks) to
one commit, and every build is `--locked`. To bump:

1. `cargo update -p libcosmic` (moves only libcosmic's git sources);
2. re-read the licenses `deny.toml` clarifies, and run
   `cargo deny --manifest-path Cargo.toml --config deny.toml check`;
3. build on Linux, FreeBSD, illumos (Solnix) and Windows, as the review
   note's matrix did.

## Linux build dependencies

`libxkbcommon-dev` and `libwayland-dev` (Debian names), as libcosmic's own
CI installs. The renderer is wgpu (Vulkan or GL), with tiny-skia as the
software fallback (`ICED_BACKEND=tiny-skia`).

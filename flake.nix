{
  description = "Improv — a multidimensional spreadsheet (Improv/Quantrix lineage) in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Stable toolchain (MSRV floor is 1.97, see Cargo.toml rust-version).
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
        };

        # Runtime libraries the egui/eframe desktop GUI (`improv-gui`) needs at
        # run time (winit/wgpu/glow load these via dlopen, so they must be on
        # LD_LIBRARY_PATH). Covers both Wayland and X11 (XWayland) sessions.
        guiRuntimeLibs = with pkgs; [
          wayland
          libxkbcommon
          libGL # libglvnd: libGL/libEGL
          vulkan-loader
          libx11
          libxcursor
          libxi
          libxrandr
          libxcb
        ];

        # Dev tooling used by the quality gate and docs (see AGENTS.md).
        devTools = with pkgs; [
          rust
          pkg-config
          cargo-nextest
          cargo-deny
          cargo-fuzz
          typos
          mdbook
          mdbook-linkcheck2
          mdbook-mermaid
          git
          sqlite # for the SQL-connectivity crate + manual testing
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = devTools ++ guiRuntimeLibs;

          # SQLite is bundled by rusqlite; nothing to link at build time, but
          # keep pkg-config discoverable for any C deps.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiRuntimeLibs;

          shellHook = ''
            echo "improv dev shell — rust $(rustc --version | cut -d' ' -f2)"
            echo "  cargo test --workspace | cargo run -p improv_gui -- <db> | improv-tui"
            echo "  note: the embedded Mentat backend is a sibling path dep at ../mentat"
          '';
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}

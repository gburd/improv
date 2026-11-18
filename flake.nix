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
          # androidenv (the `android` devShell) ships Google's SDK, whose
          # licence must be accepted and which is unfree.
          config = {
            android_sdk.accept_license = true;
            allowUnfree = true;
          };
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

        # The same toolchain plus the two Android targets the FFI cdylib is
        # built for (arm64 phones and Googlebooks, x86_64 emulators and Intel
        # Googlebooks). Kept apart from `rust` so the default shell stays lean.
        rustAndroid = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
          targets = [
            "aarch64-linux-android"
            "x86_64-linux-android"
          ];
        };

        # The Android SDK the Gradle build needs: platform 35 and build-tools
        # 35 (android/app/build.gradle.kts pins compileSdk/targetSdk 35), the
        # command-line tools and platform-tools, and NDK r27c (27.2.12479018,
        # the pinned ndkVersion). cargo-ndk cross-compiles the FFI crate with
        # it. Bump these in lockstep with android/gradle/libs.versions.toml.
        androidSdk =
          (pkgs.androidenv.composeAndroidPackages {
            platformVersions = [ "35" ];
            buildToolsVersions = [ "35.0.0" ];
            cmdLineToolsVersion = "latest";
            platformToolsVersion = "latest";
            includeNDK = true;
            ndkVersion = "27.2.12479018";
            includeEmulator = false;
            includeSystemImages = false;
          }).androidsdk;
        androidSdkRoot = "${androidSdk}/libexec/android-sdk";
        androidNdkRoot = "${androidSdkRoot}/ndk/27.2.12479018";

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
          # docs/book.toml's [output.linkcheck] runs `mdbook-linkcheck`.
          (writeShellScriptBin "mdbook-linkcheck" ''exec ${mdbook-linkcheck2}/bin/mdbook-linkcheck2 "$@"'')
          mdbook-mermaid
          git
          sqlite # for the SQL-connectivity crate + manual testing
        ];

        # Headless screenshot tools for scripts/gui-shot.sh (X11, Linux only).
        shotTools = pkgs.lib.optionals pkgs.stdenv.isLinux (
          with pkgs;
          [
            xorg-server # Xvfb
            xwd
            imagemagick
            xdotool
          ]
        );
      in
      {
        devShells.default = pkgs.mkShell {
          packages = devTools ++ guiRuntimeLibs ++ shotTools;

          # SQLite is bundled by rusqlite; nothing to link at build time, but
          # keep pkg-config discoverable for any C deps.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiRuntimeLibs;

          shellHook = ''
            echo "improv dev shell — rust $(rustc --version | cut -d' ' -f2)"
            echo "  cargo test --workspace | cargo run -p improv_gui -- <db> | improv-tui"
            echo "  note: the embedded Mentat backend is a pinned git dep (no sibling checkout needed)"
            echo "  android target: nix develop .#android (SDK + NDK + cargo-ndk)"
          '';
        };

        # The Android build chain, kept out of the default shell so a
        # Rust-only developer does not pull Google's SDK (several GB, unfree).
        # `cd android && ./gradlew assembleDebug` builds the FFI cdylib for
        # both ABIs through cargo-ndk and packages the APK; `assembleRelease`
        # builds the release APK/AAB. JDK 17 is what the Gradle build targets.
        devShells.android = pkgs.mkShell {
          packages =
            (with pkgs; [
              rustAndroid
              cargo-ndk
              jdk17
              gradle
              androidSdk
              git
            ])
            ++ guiRuntimeLibs;

          ANDROID_HOME = androidSdkRoot;
          ANDROID_SDK_ROOT = androidSdkRoot;
          ANDROID_NDK_ROOT = androidNdkRoot;
          ANDROID_NDK_HOME = androidNdkRoot;
          # Gradle's AGP shells out to aapt2 from build-tools; let it use the
          # SDK copy rather than downloading one (the store is read-only).
          GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidSdkRoot}/build-tools/35.0.0/aapt2";

          shellHook = ''
            echo "improv android shell — rust $(rustc --version | cut -d' ' -f2), $(java -version 2>&1 | head -1)"
            echo "  SDK  $ANDROID_HOME"
            echo "  NDK  $ANDROID_NDK_ROOT"
            echo "  build: (cd android && ./gradlew assembleDebug)"
          '';
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}

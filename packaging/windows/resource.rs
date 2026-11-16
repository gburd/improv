//! The build script of the four Improv binaries (each crate's Cargo.toml
//! names it as `build`): on Windows, it embeds the Improv icon and the
//! version information that Explorer shows in a file's Properties.
//!
//! Elsewhere it does nothing. The target is read from Cargo's environment,
//! not with `cfg!`, because a build script is compiled for the host and
//! would see the host's `cfg`.

use std::path::Path;

fn main() {
    // Every crate using this script is a sibling of crates/gui.
    let icon = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("gui")
        .join("assets")
        .join("icon")
        .join("improv.ico");
    println!("cargo::rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    windows(&icon);
}

/// Compile the icon and the version block into a resource file, and have
/// Cargo link it into the crate's binary. An MSVC target needs the Windows
/// SDK's rc.exe, which winresource finds through the registry.
#[cfg(windows)]
fn windows(icon: &Path) {
    let bin = std::env::var("CARGO_PKG_NAME").expect("Cargo sets CARGO_PKG_NAME");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("a UTF-8 path"))
        .set("ProductName", "Improv")
        .set("FileDescription", description(&bin))
        .set("CompanyName", "Greg Burd")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 Greg Burd. MIT OR Apache-2.0.",
        );
    if let Err(e) = res.compile() {
        panic!("embedding the Windows icon and version information: {e}");
    }
}

/// A Windows target built on another host: there is no rc.exe to embed the
/// resources with, so the binary goes without them. Releases build each
/// Windows target on Windows.
#[cfg(not(windows))]
fn windows(_icon: &Path) {
    println!("cargo::warning=not embedding the Windows icon: this host is not Windows");
}

/// What Windows shows for each binary (Task Manager, the file's Properties).
#[cfg(windows)]
fn description(crate_name: &str) -> &'static str {
    match crate_name {
        "improv_cli" => "Improv command-line interface",
        "improv_tui" => "Improv terminal interface",
        "improv_server" => "Improv HTTP server",
        "improv_gui" => "Improv",
        other => panic!("packaging/windows/resource.rs: no description for {other}"),
    }
}

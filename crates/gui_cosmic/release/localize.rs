// SPDX-License-Identifier: MIT OR Apache-2.0
// Improv's own replacement for libcosmic's src/localize.rs, carried by
// release builds until pop-os/libcosmic#1500 settles that file's licence.

//! libcosmic's translations: the Fluent files under `i18n/`, built into the
//! binary, and the `fl!` macro its widgets look their strings up with.

use std::sync::{LazyLock, Once};

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DefaultLocalizer, DesktopLanguageRequester, LanguageLoader, Localizer};
use rust_embed::RustEmbed;

/// The crate's `i18n/<language>/libcosmic.ftl` files.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Messages;

/// The loader behind `fl!`: domain and fallback language come from
/// `i18n.toml` at compile time. The fallback is loaded up front, so a lookup
/// always has an answer, whichever languages are selected later.
pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();
    if let Err(err) = loader.load_fallback_language(&Messages) {
        tracing::warn!("improv localize: libcosmic's fallback messages did not load: {err}");
    }
    loader
});

/// A [`Localizer`] over [`LANGUAGE_LOADER`] and the embedded messages, for a
/// caller that chooses the languages itself.
pub fn localizer() -> Box<dyn Localizer> {
    Box::new(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Messages))
}

/// Switches libcosmic's messages to the desktop's languages. Only the first
/// call in a process does anything. When no language can be selected, that
/// is logged and the messages stay in the fallback language.
pub fn localize() {
    static SELECTED: Once = Once::new();
    SELECTED.call_once(|| {
        let wanted = DesktopLanguageRequester::requested_languages();
        if let Err(err) = localizer().select(&wanted) {
            tracing::warn!("improv localize: no libcosmic messages for {wanted:?}: {err}");
        }
    });
}

/// A libcosmic message by its Fluent id, in the desktop's languages:
/// `fl!("close")`, or with arguments, `fl!("january", year = 2026)`.
/// i18n-embed-fl checks the id and the argument names against the fallback
/// language's file when this expands.
#[macro_export]
macro_rules! fl {
    ($id:literal) => {{
        $crate::localize::localize();
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $id)
    }};
    ($id:literal, $($args:tt)*) => {{
        $crate::localize::localize();
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $id, $($args)*)
    }};
}

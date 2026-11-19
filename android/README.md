# Improv for Android

An **Android (Jetpack Compose)** front end for [Greg Burd's Improv
(`gburd/improv`)](https://github.com/gburd/improv), a Lotus Improv /
Quantrix-style multidimensional spreadsheet with incremental recalculation on
differential dataflow. The Android app drives the same Rust engine the desktop
builds use, through the `improv_android_ffi` UniFFI crate.

This is a **generic multidimensional-spreadsheet app**: a workbook library
above the engine, plus an adaptive spreadsheet UI that reshapes to the device.
It targets Android phones, foldables and tablets, and ChromeOS / Googlebook
devices that run Android apps, on current and last-generation Android.

## What is in this cut

- **Workbook library**: the app opens on a library of the workbooks stored on
  the device (newest first), each shown with its name and a category x measure
  summary. Create a new workbook (a blank `Period x Department` starter, or the
  `Time x Product x Region` example model), open, rename, or delete them. First
  launch lands on an empty library, not on a model.
- **Shared Rust multidimensional engine** via `improv_android_ffi`:
  - Wraps `improv_core_model`, `improv_engine`, `improv_nl_formula`,
    `improv_store` (Mentat / SQLite), `improv_storage_csv`, and `improv_client`
    through **UniFFI**.
  - Native builds for `arm64-v8a` and `x86_64`.
  - A direct `ByteBuffer` viewport transfer (`16 B` header + `24 B` cell
    stride) read with zero per-cell allocation.
  - The ViewModel opens and switches the engine between workbook `.db` stores at
    runtime over an Android-free `ModelStore`; no single engine is pinned for
    the app's life.
- **Adaptive Material 3 Compose UI** that reshapes by `WindowSizeClass`:
  - **Phone (compact width)**: the grid is the hero, a scrollable 2D slice of
    the model, with collapsible bottom sheets for category pivoting, formulas,
    and analytics, and a Library action to switch workbooks. Touch only.
  - **Tablet / laptop (medium or expanded width)**: a multi-pane layout with
    drag-and-drop category tile trays, a model inspector, and a persistent
    formula pane, with hover cursors and arrow/Tab cell navigation.
  - **Foldable tabletop posture**: the grid on the top (angled) half and the
    pivot / studio controls on the bottom (flat) half.
- **Spreadsheet features**: category pivoting (Row / Column / Page axes) with
  presets derived from the model's own categories, page slicing, input and
  derived measures, natural-language and symbolic formula editing with
  autocomplete, slice analytics (sum / mean / median / min / max / stddev with
  distribution bars), what-if multipliers over the model's input measures, and
  CSV import/export.

## Not in this cut

On-device AI, Android 17 cross-device handoff, stylus handwriting, camera / OCR
table capture, and sensor telemetry ingestion are deferred. They are not built
and not claimed here.

## Documentation

- **[Getting Started](docs/GETTING_STARTED.md)** — the 3D demo data, building a
  blank model, and connecting CSV datasets.
- **[Comparison cards](share_cards/)** — infographics comparing the original
  Lotus Improv, `gburd/improv`, and this Android port.

## Building & testing

### Prerequisites
- **JDK 17**
- **Android SDK** with `compileSdk = 35` (also `targetSdk = 35`, `minSdk = 31`)
  and an installed **NDK** (see `ndkVersion` in `app/build.gradle.kts`)
- **Rust stable** with the `aarch64-linux-android` and `x86_64-linux-android`
  targets and **`cargo-ndk`**

Gradle points at the repository root (`rootProject.file("..")`) and runs
`cargo ndk ... build -p improv_android_ffi` to compile the engine before
packaging.

### Build a debug APK (compiles the Rust engine, then packages the APK)
```bash
./gradlew assembleDebug
```

### Run the unit tests
```bash
# Rust FFI tests (from the repository root)
cargo test -p improv_android_ffi --lib

# Android ViewModel and analytics tests
./gradlew testDebugUnitTest
```

## Credits

The Android port is the work of **David Gerbino**
(<david.gerbino@dmgerbino.com>, https://github.com/dmgerbino/improv-android).
See [AUTHORS](AUTHORS).

## License

Dual-licensed under **Apache-2.0 OR MIT**, matching upstream
[gburd/improv](https://github.com/gburd/improv).

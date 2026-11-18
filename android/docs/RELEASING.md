# Releasing the Improv Android app to Google Play

The first cut builds a signed Android App Bundle (`.aab`) for upload to
Google Play.  The app ships the Rust engine as a native library (`cargo
ndk`) inside a Jetpack Compose UI.  Package name: `llc.improv.app` (this is
**permanent** once the app is published; it cannot be changed).

Two things are the maintainer's, not the build's, and are never committed to
this public repo:

1. the **upload key** (a keystore you create and keep), and
2. the **Play Console account** and listing.

Everything else (the AAB itself) is reproducible from the source.

## 1. One-time: create the upload key

Google Play uses two keys.  You generate an **upload key** and give Google
only its certificate; Google holds the **app signing key** and re-signs what
you upload (Play App Signing).  If the upload key is ever lost you can reset
it with Google; losing the app signing key would be fatal, so letting Google
hold it is the safe default.  Accept Play App Signing when you create the app.

Generate the upload key with the JDK's `keytool` (the `android` dev shell has
a JDK 17):

```sh
nix develop .#android   # provides keytool
keytool -genkeypair -v \
  -keystore ~/keys/improv-upload.jks \
  -alias improv-upload \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=Improv LLC, O=Improv LLC, C=US"
```

It will ask for a keystore password and (optionally) a key password.  Use a
strong, unique password and record it in your password manager.  **Back up
`~/keys/improv-upload.jks` somewhere safe and offline.**  Without it you
cannot upload a new version signed with the same upload key (you would have
to ask Google to reset the upload key).

Keep the keystore OUTSIDE the repo.  `*.jks`, `*.keystore`, and
`keystore.properties` are gitignored, but the safe habit is to store the key
under `~/keys`, not in the working tree at all.

## 2. Point the build at the key

The release build reads the key from `android/keystore.properties` (preferred
for local builds) or from environment variables (preferred for CI).  Neither
is committed.

`android/keystore.properties` (gitignored):

```properties
storeFile=/home/you/keys/improv-upload.jks
storePassword=...
keyAlias=improv-upload
keyPassword=...
```

or, equivalently, export:

```sh
export IMPROV_KEYSTORE_FILE=~/keys/improv-upload.jks
export IMPROV_KEYSTORE_PASSWORD=...
export IMPROV_KEY_ALIAS=improv-upload
export IMPROV_KEY_PASSWORD=...
```

When neither is present the release build is produced **unsigned** (fine for
a local size check or CI, not uploadable).

## 3. Build the signed AAB

```sh
nix develop .#android
cd android
./gradlew bundleRelease
# -> app/build/outputs/bundle/release/app-release.aab
```

`bundleRelease` runs `cargo ndk` to build the engine for `arm64-v8a` and
`x86_64`, compiles the Kotlin, shrinks with R8/ProGuard
(`app/proguard-rules.pro` keeps the JNA and UniFFI classes the native path
needs by reflection), and signs the bundle with the upload key.  The native
libraries are built with 16 KB ELF page alignment (`.cargo/config.toml` at
the repo root), which Google Play requires.

Bump `versionCode` (an integer, must increase every upload) and `versionName`
in `android/app/build.gradle.kts` before each release.

### Check it before uploading

```sh
# The bundletool and apksigner from the SDK:
apksigner verify --print-certs \
  <(bundletool build-apks --bundle=app/build/outputs/bundle/release/app-release.aab \
      --output=/dev/stdout --mode=universal)   # or build-apks to a file and verify that
```

More simply, upload to the Play Console's **internal testing** track first:
the Console runs pre-launch checks (install on a range of devices, crawl,
accessibility, 16 KB page size, target API level) and reports problems before
any public release.

## 4. One-time: the Play Console account and app

This is the maintainer's to do; the agent does not create accounts or publish.

1. **Developer account.**  Register at
   <https://play.google.com/console/signup> with the Improv LLC identity.
   There is a one-time US$25 fee.  An organisation account (not personal)
   matches `improv.llc`; Google will verify the organisation (D-U-N-S number,
   website).  Use an address at `improv.llc`.
2. **Create the app.**  App name "Improv", default language, "App", free.
   Accept the developer program policies and US export laws.
3. **Accept Play App Signing** when prompted (let Google hold the app signing
   key; you upload with the upload key from step 1).  Upload the upload-key
   certificate if asked: export it with
   `keytool -export -rfc -keystore ~/keys/improv-upload.jks -alias improv-upload -file upload-cert.pem`.
4. **Package name** is set from the first bundle you upload:
   `llc.improv.app`.  It cannot change afterward, so the first upload freezes
   it.
5. **Store listing.**  Short and full description, an icon (512x512), a
   feature graphic (1024x500), and at least two screenshots per form factor
   (phone, 7"/10" tablet, Chromebook/Googlebook).  The `share_cards/` images
   are marketing comparisons, not store screenshots; capture real screenshots
   from the app (an emulator covers phone and tablet; a Chromebook/Googlebook
   or the ARC emulator covers the large-screen form factor).
6. **Content rating, data safety, target audience, privacy policy.**  The
   first cut collects no data and talks to no server by default, so the data
   safety form is short; still required.  A privacy policy URL under
   `improv.llc` is required.
7. **Release tracks.**  Internal testing -> closed testing -> production.
   Start on internal testing with the AAB from step 3, clear the pre-launch
   report, then promote.

## Chromebook / Googlebook / large screens

The adaptive UI already lays out for phone, tablet, and foldable by
`WindowSizeClass` and device posture.  For Play's large-screen quality:

- `minSdk = 31`, `targetSdk = 35`: current and last-generation Android.
- The app declares no `android:screenOrientation` lock and resizes (Compose
  adaptive layout), so it runs windowed on ChromeOS/Googlebook.
- Test on a Chromebook/Googlebook (Android apps) or the ChromeOS ARC
  emulator before promoting beyond internal testing.

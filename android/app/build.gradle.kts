plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.android)
  alias(libs.plugins.compose.compiler)
}

// shortcut: rust workspace is the repo root (one level above android/); confirmed by Cargo.toml there.
val rustDir = rootProject.file("..")
val cargoTargetDir = System.getenv("CARGO_TARGET_DIR")
  ?: "${System.getProperty("user.home")}/.cache/projects/improv-android-rust/cargo"

android {
  namespace = "llc.improv.app"
  compileSdk = 35
  // Pin build-tools to the version the Nix SDK provides; AGP would otherwise
  // reach for its built-in default (34.0.0) and fail to auto-install it into
  // the read-only Nix store.
  buildToolsVersion = "35.0.0"
  ndkVersion = "27.2.12479018"

  defaultConfig {
    applicationId = "llc.improv.app"
    minSdk = 31
    targetSdk = 35
    versionCode = 1
    versionName = "0.15.0"
  }

  sourceSets {
    getByName("debug").jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs/debug").get().asFile)
    getByName("release").jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs/release").get().asFile)
  }

  buildTypes {
    debug {
      enableUnitTestCoverage = true
    }
    release {
      isMinifyEnabled = true
      isShrinkResources = true
      proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
    }
  }
  lint {
    // UniFFI's generated JavaLangRefCleaner checks Class.forName("java.lang.ref.Cleaner")
    // at runtime and falls back to JnaCleaner on API 31-32.
    disable += "NewApi"
  }
  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
  }
  buildFeatures {
    compose = true
    aidl = false
    buildConfig = false
    shaders = false
  }

  packaging {
    resources {
      excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
  }

  testOptions {
    unitTests.isReturnDefaultValues = true
    unitTests.all {
      it.systemProperty("jna.library.path", "$cargoTargetDir/debug")
    }
  }
}

kotlin {
  jvmToolchain(17)
}

dependencies {
  val composeBom = platform(libs.androidx.compose.bom)
  implementation(composeBom)
  androidTestImplementation(composeBom)

  // Core Android dependencies
  implementation(libs.androidx.core.ktx)
  implementation(libs.androidx.lifecycle.runtime.ktx)
  implementation(libs.androidx.activity.compose)

  // Arch Components
  implementation(libs.androidx.lifecycle.runtime.compose)
  implementation(libs.androidx.lifecycle.viewmodel.compose)

  // Compose
  implementation(libs.androidx.compose.ui)
  implementation(libs.androidx.compose.ui.tooling.preview)
  implementation(libs.androidx.compose.material3)
  // Adaptive layout (WindowSizeClass + foldable posture) is a separate artifact group,
  // not part of the Compose BOM, so it carries an explicit version in the catalog.  It also
  // supplies the androidx.window.core.layout WindowSizeClass the adaptive root reads.
  implementation(libs.androidx.compose.material3.adaptive)
  // Tooling
  debugImplementation(libs.androidx.compose.ui.tooling)
  // Instrumented tests
  androidTestImplementation(libs.androidx.compose.ui.test.junit4)
  debugImplementation(libs.androidx.compose.ui.test.manifest)

  // Local tests: jUnit, coroutines, Android runner
  testImplementation(libs.junit)
  testImplementation(libs.kotlinx.coroutines.test)

  // Instrumented tests: jUnit rules and runners
  androidTestImplementation(libs.androidx.test.core)
  androidTestImplementation(libs.androidx.test.ext.junit)
  androidTestImplementation(libs.androidx.test.runner)
  androidTestImplementation(libs.androidx.test.espresso.core)

  // JNA required by UniFFI Kotlin bindings
  implementation("net.java.dev.jna:jna:5.19.1@aar")
  testImplementation("net.java.dev.jna:jna:5.19.1")
}

// Compile Rust for both ARM64 (Pixel + ARM Googlebook) and x86_64 (Intel Googlebook)
val buildRustDebug by tasks.registering(Exec::class) {
  workingDir = rustDir
  environment("CARGO_TARGET_DIR", cargoTargetDir)
  commandLine(
    "cargo", "ndk",
    "-t", "arm64-v8a",
    "-t", "x86_64",
    "--platform", "31",
    "-o", layout.buildDirectory.dir("rustJniLibs/debug").get().asFile.absolutePath,
    "build", "-p", "improv_android_ffi"
  )
}

val buildRustRelease by tasks.registering(Exec::class) {
  workingDir = rustDir
  environment("CARGO_TARGET_DIR", cargoTargetDir)
  commandLine(
    "cargo", "ndk",
    "-t", "arm64-v8a",
    "-t", "x86_64",
    "--platform", "31",
    "-o", layout.buildDirectory.dir("rustJniLibs/release").get().asFile.absolutePath,
    "build", "--release", "-p", "improv_android_ffi"
  )
}

tasks.matching { it.name == "mergeDebugJniLibFolders" }.configureEach {
  dependsOn(buildRustDebug)
}
tasks.matching { it.name == "mergeReleaseJniLibFolders" }.configureEach {
  dependsOn(buildRustRelease)
}

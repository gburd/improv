.PHONY: clean clean_all

PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXTENSION_NAME=improv

# STABLE C API: USE_UNSTABLE_C_API is deliberately NOT set. The footer is
# stamped C_STRUCT with TARGET_DUCKDB_VERSION as the C API floor, so one
# binary loads into DuckDB v1.5.6 and every later release that keeps the v1
# extension API (v2.0 does). With USE_UNSTABLE_C_API=1 (mentat's pin) the
# binary loads ONLY into the exact DuckDB it was built for. See README.md
# "Pins" before changing either.
#
# The C API floor. Must match the duckdb-rs pin `duckdb = "~1.10506.1"`
# (1.10506.x = DuckDB v1.5.6): the extension uses the v1.5.6 function-pointer
# struct, so it may not claim an older floor.
TARGET_DUCKDB_VERSION=v1.5.6

# Test (make test_release) with the v1.5.6 Python package: the floor. CI
# also loads the binary into DuckDB v2.0 (test/smoke.sh with DUCKDB=...).
DUCKDB_TEST_VERSION ?= 1.5.6

# The version stamped into the footer: the root Cargo.toml's, so it cannot
# drift from the release. The registry CI overrides it when it builds.
EXTENSION_VERSION ?= v$(shell sed -n 's/^version = "\(.*\)"/\1/p' $(PROJ_DIR)../../Cargo.toml | head -1)

# A standalone workspace, but say where its target dir is so the ci-tools
# rust.Makefile finds ./target/<profile>/lib<name>.so even when the caller
# exported a CARGO_TARGET_DIR of its own.
export CARGO_TARGET_DIR=$(PROJ_DIR)target

all: configure debug

include extension-ci-tools/makefiles/c_api_extensions/base.Makefile
include extension-ci-tools/makefiles/c_api_extensions/rust.Makefile

# The cdylib package is `duckdb_improv`, so cargo emits libduckdb_improv.{so,
# dylib} (unix) or duckdb_improv.dll (Windows: no `lib` prefix), not
# lib$(EXTENSION_NAME).*. Override only RUST_LIBNAME, the SOURCE artifact;
# base.Makefile's EXTENSION_LIB_FILENAME is the per-platform copy destination
# and the metadata step reads it (overriding it broke mentat's Windows and
# macOS builds).
ifeq ($(OS),Windows_NT)
	RUST_LIBNAME=duckdb_improv.dll
else ifeq ($(DUCKDB_PLATFORM),osx_amd64)
	RUST_LIBNAME=libduckdb_improv.dylib
else ifeq ($(DUCKDB_PLATFORM),osx_arm64)
	RUST_LIBNAME=libduckdb_improv.dylib
else ifeq ($(shell uname -s),Darwin)
	RUST_LIBNAME=libduckdb_improv.dylib
else
	RUST_LIBNAME=libduckdb_improv.so
endif

configure: venv platform extension_version

debug: build_extension_library_debug build_extension_with_metadata_debug
release: build_extension_library_release build_extension_with_metadata_release

test: test_debug
test_debug: test_extension_debug
test_release: test_extension_release

clean: clean_build clean_rust
clean_all: clean_configure clean

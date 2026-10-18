# Improv's root Makefile exists for the DuckDB Community Extensions CI.
# Everyday work uses cargo; see CONTRIBUTING.md.
#
# --- DuckDB extension (crates/duckdb_improv) ---------------------------------
# The registry CI (duckdb/community-extensions -> extension-ci-tools'
# _extension_distribution.yml) clones THIS repo at its root and runs
# `make configure_ci`, `make release` / `make debug`, `make test_release` /
# `make test_debug` here, then collects
#   build/<type>/extension/improv/improv.duckdb_extension
# from the repo root. The extension lives in crates/duckdb_improv, so these
# targets forward there and copy the artifacts to where the registry looks.
# The registry also checks out its own extension-ci-tools at
# ./extension-ci-tools; we use the pinned submodule under crates/duckdb_improv
# either way.
DUCKDB_DIR := crates/duckdb_improv
.PHONY: configure configure_ci debug release test_debug test_release \
        set_duckdb_version set_duckdb_tag set_duckdb_repository

configure configure_ci:
	$(MAKE) -C $(DUCKDB_DIR) $@

debug release:
	$(MAKE) -C $(DUCKDB_DIR) $@
	mkdir -p build/$@/extension/improv
	cp $(DUCKDB_DIR)/build/$@/improv.duckdb_extension build/$@/
	cp $(DUCKDB_DIR)/build/$@/improv.duckdb_extension build/$@/extension/improv/

test_debug test_release:
	$(MAKE) -C $(DUCKDB_DIR) $@

# The registry sets these to pin the DuckDB it builds against. The extension
# uses the stable C API with a v1.5.6 floor (crates/duckdb_improv/Makefile),
# so the binary does not depend on them: accepted and ignored.
set_duckdb_version set_duckdb_tag set_duckdb_repository:
	@echo "improv: $@ is a no-op (stable C API, floor in $(DUCKDB_DIR)/Makefile)"

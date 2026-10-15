#!/usr/bin/env bash
# Coverage gate: every improv_* crate must reach >= 90% of functions AND
# >= 90% of branches (inline #[cfg(test)] modules are excluded from the
# denominator via `coverage_nightly`; see CONTRIBUTING.md).
#
# Usage: scripts/coverage.sh [--html] [MIN_PERCENT]
#   needs: rustup nightly with llvm-tools-preview, cargo-llvm-cov.
#   optional: IMPROV_TEST_PG="host=... user=... password=... dbname=..." to
#   include the live Postgres tests (storage_sql, storage_pg, store, cli, gui).
set -euo pipefail
cd "$(dirname "$0")/.."

html=0
if [[ ${1:-} == --html ]]; then html=1; shift; fi
min=${1:-90}

mkdir -p target
cargo +nightly llvm-cov --workspace --branch --no-fail-fast --summary-only \
	>target/coverage-summary.txt
if ((html)); then
	cargo +nightly llvm-cov report --branch --html --output-dir target/coverage-html
	echo "HTML report: target/coverage-html/html/index.html"
fi

# Columns of `--summary-only`: Filename Regions Missed Cover Functions Missed
# Executed Lines Missed Cover Branches Missed Cover. Aggregate per crate.
awk -v min="$min" '
	NR > 2 && $1 ~ /\.rs$/ {
		split($1, p, "/"); c = p[1]
		F[c] += $5; FM[c] += $6; B[c] += $11; BM[c] += $12
	}
	/^TOTAL/ { total = $0 }
	END {
		bad = 0
		printf "%-16s %8s %8s\n", "crate", "fn%", "branch%"
		for (c in F) {
			fp = 100 * (F[c] - FM[c]) / F[c]
			bp = B[c] ? 100 * (B[c] - BM[c]) / B[c] : 100
			flag = (fp < min || bp < min) ? "  <-- below " min "%" : ""
			if (flag != "") bad = 1
			printf "%-16s %7.2f%% %7.2f%%%s\n", c, fp, bp, flag
		}
		print total
		exit bad
	}' target/coverage-summary.txt | sort

#!/usr/bin/env python3
"""The scale regression gate (.github/workflows/scale-gate.yml runs it weekly).

    scale-gate.py run   [--out results.json]   run every probe, print a table
    scale-gate.py check results.json            compare with baseline.json
    scale-gate.py baseline results.json...      write baseline.json (the medians)
    scale-gate.py selftest                      check `check` on made-up runs

Each probe run is timed as a ratio to a run of the reference probe
(reference.rs, a fixed non-Improv workload) just before it: a runner twice
as slow, or busy for a moment, slows both, so the ratio stays put. A probe's
ratio is the median of its REPS runs' ratios. `check` fails when a probe's
ratio grows by more than 25% over the baseline's, and its absolute time is
also more than the probe's noise floor (`floor_ms`) slower, so a 0.2 ms edit
going 0.3 ms on a noisy runner is not a regression.

Run from the workspace root after `cargo build --release -p improv_cli` and
`cargo test --release -p improv_engine --test probe --test stress --no-run`.
PG_BIN names a Postgres bindir whose install has pg_improv; without it the
pg_improv probe is skipped, and `check` fails on the probes not run.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.getcwd()
HERE = os.path.dirname(os.path.abspath(__file__))
BASELINE = os.path.join(HERE, "baseline.json")
LIMIT = 1.25  # fail above 25% slower than the baseline, relative to the reference
REPS = int(os.environ.get("GATE_REPS", "3"))  # each probe's median of REPS runs


def sh(cmd, env=None, timeout=1800):
    e = dict(os.environ, **(env or {}))
    p = subprocess.run(cmd, cwd=ROOT, env=e, capture_output=True, text=True, timeout=timeout)
    if p.returncode != 0:
        sys.exit(f"FAIL: {' '.join(cmd)}\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}")
    return p.stdout + p.stderr


def dur_ms(s):
    """A Rust Duration's Debug form (12.3ms, 1.2s, 45µs) in ms."""
    m = re.fullmatch(r"([0-9.]+)(ns|µs|us|ms|s)", s)
    return float(m.group(1)) * {"ns": 1e-6, "µs": 1e-3, "us": 1e-3, "ms": 1, "s": 1e3}[m.group(2)]


def cargo_probe(test, case, env):
    """An #[ignore]d engine probe's one `[tag] ...` line."""
    out = sh(["cargo", "test", "-q", "--release", "-p", "improv_engine", "--test", test, case,
              "--", "--ignored", "--exact", "--nocapture"], env)
    return next(line for line in out.splitlines() if line.startswith("["))


def field(line, name):
    return dur_ms(re.search(re.escape(name) + r" ([0-9.]+(?:ns|µs|us|ms|s))", line).group(1))


def median_of(line, name):
    """The median of a `name min / median / max ms` triple."""
    return float(re.search(re.escape(name) + r" [0-9.]+ / ([0-9.]+) / [0-9.]+ ms", line).group(1))


REF = os.path.join(ROOT, "target", "scale-gate", "reference")


def reference():
    return float(re.search(r"ms ([0-9.]+)", sh([REF])).group(1))


def wide(formula, tag, n, edits):
    line = cargo_probe("probe", "wide", {"PROBE_N": str(n), "PROBE_EDITS": str(edits), "PROBE_FORMULA": formula})
    return {f"wide.{tag}.engine_new": field(line, "Engine::new"),
            f"wide.{tag}.evaluate": field(line, "evaluate"),
            f"wide.{tag}.edit_max": median_of(line, "edit new max"),
            f"wide.{tag}.edit_tail": median_of(line, "edit tail")}


def measures(shape, n):
    line = cargo_probe("probe", "measures", {"PROBE_N": str(n), "PROBE_SHAPE": shape})
    return {f"measures.{shape}.engine_new": field(line, "Engine::new"),
            f"measures.{shape}.root_edit": median_of(line, "root edit")}


def pagerank(n):
    line = cargo_probe("probe", "pagerank", {"PROBE_N": str(n)})
    return {"pagerank.engine_new": field(line, "Engine::new"), "pagerank.edit": field(line, "edit")}


def stress():
    line = cargo_probe("stress", "scale_evaluate_100k", {})
    return {"stress.100k.evaluate": field(line, "evaluate")}


def import_csv(rows):
    """`improv import-csv` of rows x 1000 distinct cells into a fresh Mentat file."""
    exe = os.path.join(ROOT, "target", "release", "improv")
    with tempfile.TemporaryDirectory(dir=os.path.join(ROOT, "target")) as d:
        csv, db = os.path.join(d, "g.csv"), os.path.join(d, "m.db")
        with open(csv, "w") as f:
            f.write("r,c,v\n")
            for i in range(rows):
                f.writelines(f"r{i},c{j},{(i * 31 + j * 7) % 997}\n" for j in range(1000))
        sh([exe, "init", db])
        sh([exe, "add-category", db, "1", "Row"])
        sh([exe, "add-category", db, "2", "Col"])
        t = time.perf_counter()
        sh([exe, "import-csv", db, csv, "100", "Amount", "v", "r:1:Row", "c:2:Col"])
        return {f"import_csv.{rows}x1000": (time.perf_counter() - t) * 1e3}


def pg_scale():
    """pg_improv's tests/scale.sh at its small default size (50k rows): the
    recalc, and the worker's initial build (slot, load, fold seed, engine).
    (Not its random batches: their commits do no Improv work in logical mode.)"""
    if not os.environ.get("PG_BIN"):
        return {}
    out = sh(["crates/pg_improv/tests/scale.sh"], {"PGI_DATA": os.path.join(ROOT, "target", "scale-gate", "pg")})
    num = lambda pat: float(re.search(pat, out).group(1))
    return {"pg_scale.recalc": num(r"improv.recalc .*: ([0-9]+) ms"),
            "pg_scale.initial_build": num(r"initial build .*: ([0-9]+) ms")}


# The probes at sizes a GitHub runner (4 vCPU, 16 GB) runs in a few minutes:
# the v0.10.0 benchmark's shapes (docs/reviews/2026-10-20-v0.10.0-benchmark.md
# §3a, §3c, §3d, §3g), its 100k rows of §3g, and scale.sh's own small default.
PROBES = [
    lambda: wide("SUM(X OVER W)", "sum", 100_000, 20),
    lambda: wide("RANK(X OVER W)", "rank", 100_000, 10),
    lambda: measures("fan", 2_000),
    lambda: measures("chain", 2_000),
    lambda: pagerank(1_500),
    stress,
    lambda: import_csv(100),
    pg_scale,
]


def run(args):
    os.makedirs(os.path.dirname(REF), exist_ok=True)
    sh(["rustc", "-C", "opt-level=3", "--edition", "2021", "-o", REF, os.path.join(HERE, "reference.rs")])
    refs, ms, ratio = [], {}, {}
    for probe in PROBES:
        for _ in range(REPS):  # REPS runs of one probe back to back, each after a reference run
            refs.append(reference())
            for k, v in probe().items():
                ms.setdefault(k, []).append(v)
                ratio.setdefault(k, []).append(v / refs[-1])
    res = {"reference_ms": statistics.median(refs), "reference_samples": refs,
           "probes": {k: {"ms": statistics.median(v), "ratio": statistics.median(ratio[k]),
                          "ms_runs": v, "ratio_runs": ratio[k]} for k, v in ms.items()}}
    for k, v in sorted(res["probes"].items()):
        print(f"{k:32} {v['ms']:10.2f} ms  ratio {v['ratio']:.4f}")
    print(f"{'reference':32} {res['reference_ms']:10.2f} ms")
    if args.out:
        with open(args.out, "w") as f:
            json.dump(res, f, indent=1, sort_keys=True)


def compare(base, res):
    """The report lines, and the probes that regressed or did not run."""
    speed = res["reference_ms"] / base["reference_ms"]
    lines = [f"reference {res['reference_ms']:.1f} ms (baseline {base['reference_ms']:.1f} ms): "
             f"this runner takes {speed:.2f}x the baseline machine's time"]
    bad = []
    for k, b in sorted(base["probes"].items()):
        r = res["probes"].get(k)
        if r is None:
            lines.append(f"{k:32} NOT RUN")
            bad.append(k)
            continue
        rel = r["ratio"] / b["ratio"]
        # How much slower than the baseline predicts, in this runner's ms.
        slower = (r["ratio"] - b["ratio"]) * res["reference_ms"]
        verdict = "ok"
        if rel > LIMIT and slower > b["floor_ms"]:
            verdict = "REGRESSED"
            bad.append(k)
        lines.append(f"{k:32} {r['ms']:10.2f} ms  {rel:6.2f}x baseline  {verdict}")
    return lines, bad


def check(args):
    with open(BASELINE) as f:
        base = json.load(f)
    with open(args.results) as f:
        res = json.load(f)
    lines, bad = compare(base, res)
    print("\n".join(lines))
    if bad:
        sys.exit(f"FAIL: {len(bad)} probe(s) not run or more than {LIMIT - 1:.0%} slower than the "
                 f"baseline, relative to the reference: {', '.join(bad)}")
    print(f"PASS: every probe is within {LIMIT - 1:.0%} of the baseline, relative to the reference")


def selftest(_args):
    base = {"reference_ms": 800.0, "probes": {
        "big": {"ms": 400.0, "ratio": 0.5, "floor_ms": 20.0},
        "tiny": {"ms": 0.4, "ratio": 0.0005, "floor_ms": 1.0}}}

    def runner(scale, big=1.0, tiny=1.0, drop=None):
        ref = 800.0 * scale
        ms = {"big": 400.0 * scale * big, "tiny": 0.4 * scale * tiny}
        return {"reference_ms": ref,
                "probes": {k: {"ms": v, "ratio": v / ref} for k, v in ms.items() if k != drop}}

    cases = [
        (runner(1.0), []),  # the baseline machine
        (runner(2.0), []),  # a runner twice as slow: the ratios hold
        (runner(0.6, big=1.2), []),  # faster, 20% worse: within the limit
        (runner(2.0, big=1.3), ["big"]),  # slower runner, 30% worse: caught
        (runner(1.0, tiny=3.0), []),  # 0.8 ms slower: under the noise floor
        (runner(1.0, tiny=5.0), ["tiny"]),  # 1.6 ms slower: over it
        (runner(1.0, drop="big"), ["big"]),  # a probe that did not run
    ]
    for i, (res, want) in enumerate(cases):
        got = compare(base, res)[1]
        assert got == want, f"case {i}: {got} != {want}"
    print(f"selftest: {len(cases)} cases ok")


def baseline(args):
    """The median of several `run --out` files. A probe's noise floor is the
    spread of its ratio across them, in ms (at least 1 ms and 5% of its time)."""
    runs = []
    for p in args.results:
        with open(p) as f:
            runs.append(json.load(f))
    ref = statistics.median(r["reference_ms"] for r in runs)
    probes = {}
    for k in runs[0]["probes"]:
        ms = [r["probes"][k]["ms"] for r in runs]
        ratios = [r["probes"][k]["ratio"] for r in runs]
        ratio = statistics.median(ratios)
        floor = max(1.0, 0.05 * statistics.median(ms), (max(ratios) - min(ratios)) * ref)
        probes[k] = {"ms": round(statistics.median(ms), 3), "ratio": round(ratio, 6), "floor_ms": round(floor, 3)}
    with open(BASELINE, "w") as f:
        json.dump({"reference_ms": round(ref, 3), "runs": len(runs), "probes": probes}, f, indent=1, sort_keys=True)
        f.write("\n")
    print(f"wrote {BASELINE}: {len(probes)} probes from {len(runs)} runs")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--out")
    p.set_defaults(f=run)
    p = sub.add_parser("check")
    p.add_argument("results")
    p.set_defaults(f=check)
    p = sub.add_parser("baseline")
    p.add_argument("results", nargs="+")
    p.set_defaults(f=baseline)
    p = sub.add_parser("selftest")
    p.set_defaults(f=selftest)
    a = ap.parse_args()
    a.f(a)


if __name__ == "__main__":
    main()

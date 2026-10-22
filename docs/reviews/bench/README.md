# The benchmark harnesses and the scale gate

- `bench-*.sh`, `summarize.py`: the release benchmark's harnesses (the
  v0.10.0 benchmark, `docs/reviews/2026-10-20-v0.10.0-benchmark.md`; A/B
  runs such as a Mentat bump, AGENTS.md).
- `scale-gate.py`, `reference.rs`, `baseline.json`: the weekly scale
  regression gate (`.github/workflows/scale-gate.yml`; CONTRIBUTING.md,
  "The scale gate").

## The scale gate

The probes are the benchmark's shapes at sizes a GitHub runner runs in a few
minutes; each is the median of 3 runs:

| Probe | What | Benchmark § |
|---|---|---|
| `wide.sum.*`, `wide.rank.*` | `tests/probe.rs wide`, 100k items: `evaluate`, `Engine::new`, median one-cell edit (new max, tail) | 3d |
| `measures.fan.*`, `measures.chain.*` | `tests/probe.rs measures`, 2,000 measures × 100 items: `Engine::new`, median root edit | 3c |
| `pagerank.*` | `tests/probe.rs pagerank`, 1,500 nodes × 150k edges: `Engine::new` (build + solve), one edge edit | 2 |
| `stress.100k.evaluate` | `tests/stress.rs scale_evaluate_100k` | 3a |
| `import_csv.100x1000` | `improv import-csv` of 100k distinct cells into a fresh Mentat file | 3g |
| `pg_scale.*` | pg_improv `tests/scale.sh` small (50k rows): `improv.recalc`, the worker's initial build | 1 |
| `reference` | `reference.rs`: no Improv code; the yardstick | — |

`run` times a reference run just before each probe run, and divides the
probe's time by it: a slower runner, or a few seconds of a noisy neighbour,
slows both. A probe's ratio is the median of its 3 runs'. `check` compares
it with the baseline's. It fails a probe when the ratio is more than 25%
higher **and** the probe is more than its `floor_ms` slower than the
baseline predicts for this runner. `floor_ms` is the spread of the baseline
runs' ratios, in ms, at least 1 ms and 5% of the time, so sub-millisecond
edits cannot fail on jitter alone. A probe in the baseline that did not run
fails too. `scale-gate.py selftest` checks these rules on made-up runs.

The reference measures CPU, cache and allocator speed. It does not see disk
speed, so the import and pg_improv probes, which also write files, are only
partly normalized, and ratios shift a little between CPU models. The
baseline was made on the EC2 box (c7i, Sapphire Rapids); if GitHub's runners
sit consistently off it, build the baseline from the workflow's uploaded
`scale-gate.json` files instead (`scale-gate.py baseline a.json b.json ...`).

# v0.6.0 review — 2026-09-22

Reviewed Improv `42afb1c`, Mentat `82101f9`, mino-rs `9f104d2`.

## Strengths

Named multidimensional formulas, differential-dataflow sessions, separate
storage/interface crates, atomic logical saves, and CSV/SQL adapters provide
a useful foundation. CLI/TUI/GUI share the same modeling concepts.

## Weaknesses

Feature presence is not product qualification. The GUI is not a verified
Lotus Improv reproduction. Most tests exercise state helpers, not rendered
frames, pointer gestures, focus, or recovery from failed saves.

## EC2 baseline

Used hotdog, us-east-1, m7i.2xlarge (8 vCPU, 32 GiB), Ubuntu 24.04,
Rust 1.98.0. Uploaded tracked source archives only, no credentials/databases.
Core-model, GUI, and storage tests: 82 passed. GUI built and remained alive
under Xvfb until an intentional 8-second timeout (exit 124); not a visual test.
The additional Unicode highlighter probe failed, as reported below.

Three direct debug-binary runs, 100,000-cell scale-eval test:

| Run | Evaluate seconds | Process peak RSS KiB |
| --- | ---: | ---: |
| 1 | 2.043435 | 85540 |
| 2 | 2.037170 | 86488 |
| 3 | 2.047609 | 85468 |

Median evaluation: 2.043435 seconds. RSS includes fixture construction and
assertions. No performance-change claim: these are baseline repetitions,
not an A/B experiment. Direct-run logs and the Unicode reproduction are
archived in `docs/reviews/2026-09-22-ec2/`; full local build logs remain in
`/tmp/improv-ec2-review.Ivy1ov/results`.
Instance `i-09947bb6d682e7806` confirmed terminated; temporary security group
and AWS SSH key pair deleted. A guest shutdown guard was also installed.

## Correctness priorities

- EC2 reproduced `formula_highlight::scan("é")` panicking at
  `crates/gui/src/formula_highlight.rs:93`: invalid UTF-8 slice boundaries.
- Empty filtered row products become synthetic rows, but `nth_tuple` requires
  nonempty radices. Add headless frame tests for empty rows/columns/pages.
- Formula bar initializes CNL through `describe_formula` but commits DSL
  through `parse_expr`. Unchanged displayed formulas must commit successfully.
- Grid shortcuts ignore focus in other editors. Typed input display/editing
  is numeric-only. Add focus and typed-edit regressions.
- Save failures can be overwritten by success messages; formula changes can
  disable the working engine. Validate and persist before publishing changes.
- CSV imports reset item allocation at a fixed base and mutate before full
  validation. Test repeated imports and failure rollback.
- Derived CSV export reads input storage, not computed view results.
- `load_partial` still materializes all coordinate rows before filtering;
  it is not bounded-memory loading. Charts and columns still materialize
  Cartesian products; row virtualization alone does not imply billion-cell UX.
- Sandbox read-only root is not private-file isolation. WASM receive timeout
  does not terminate the worker. These need security qualification, not DONE.

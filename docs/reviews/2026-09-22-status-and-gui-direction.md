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

## First repair batch — landed 2026-09-22

Each fix carries a regression verified to fail against the pre-fix code.

| Finding | Commit | Outcome |
| --- | --- | --- |
| Unicode highlighter panic (reproduced on EC2) | `6a4b862` | Fixed; tokens asserted to tile the string on char boundaries |
| Empty filtered axis divide-by-zero | `6a4b862` | Fixed; empty axis renders zero lines, no under-specified coordinates |
| CSV item-ID aliasing across imports | `8241d81` | Fixed; ids allocated above existing maximum |
| CSV import mutates before validating | `8241d81` | Fixed; validation precedes all mutation |
| `load_partial` enumerated every cell | `5d80eba` | Bounded in-query via `ground`; residual index scan documented |
| Save failures reported as success | `b81abc3` | `save()` returns `Result`, `#[must_use]` |
| Bad formula could disable the engine | `b81abc3` | Validated clone published only on success |
| Formula bar showed CNL, committed DSL | `b81abc3` | DSL printer added; commit accepts DSL or CNL |

### Additional critical bug found during the batch

**`save_model` panicked on any model over ~5,461 cells** (`3b8f725`). Mentat
asserts `6 * count < 32766` per transact, so a single large group aborted the
save outright. The advertised multi-million-cell scale was therefore unreachable
for any workflow that saves; the 5M stress tests never caught it because they
exercise `evaluate()` only and never call `save_model()`.

**Consequence for the plan:** scale claims must be re-derived end-to-end
(build, save, reload, evaluate), not from `evaluate()` in isolation. The
verified save+reload ceiling is now 12,000 cells (behind `#[ignore]`), with
6,000 covered by the default suite — far below the 5,000,000 figure quoted for
`evaluate()` alone.

### Still open from this audit

Typed (non-numeric) cell editing; keyboard focus stealing between the grid and
other text fields; derived CSV export reading input storage rather than computed
results; multi-measure display; undo/redo. The historical-reference and
GUI-reconstruction gates in `.agent/steering/AGENT_GUI_STEERING.md` remain
unstarted.

## Independent review of the repair batch, and a second round

An independent reviewer probed the five fixes above and found **7 real
defects** — including one case of the original bug relocated rather than
fixed. A re-reviewer then verified the second round. Both rounds are closed.

| Reviewer finding | Fix | Note |
| --- | --- | --- |
| `formula_dsl` emitted DSL its own parser rejects (aggregation with empty `over`, or a non-`Ref` arg) | `ec51384` | The bug `b81abc3` claimed to fix, relocated; reachable in three keystrokes via CNL input |
| Bar text no parser accepts for non-identifier measure names (`"Unit Price"` — what a CSV header gives) | `ec51384` | `formula_source` now verifies its own invariant by reparsing |
| `chart.rs` kept the `.max(1)` empty-axis synthesis the grid fix removed | `ec51384` | Root-cause miss: the pattern had two call sites, one was fixed |
| Stale inline parse error survived a later save failure | `ec51384` | |
| `save_model` still aborted the process for one entity wider than the queue limit | `25ba0b6` | Chunking cannot split a single entity |
| Stale non-dimension category blanked a fully-addressable grid | `148c1cd` | Unassigned in round two; reproduced, then fixed |
| `set_cell` not atomic on save failure | `148c1cd` | Unassigned in round two |

Two corrections the fixers made to their own briefs, both verified rather than
assumed:

- The transact ceiling is **per attribute queue** (cardinality-many vs
  cardinality-one), not per transact, so the binding quantity for a measure is
  its category count alone. Guarding on `7 + N` would have wrongly rejected the
  measurably-working 5454–5460 range.
- The missing-retraction bug (cardinality-many `:measure/categories`
  accumulating stale refs, pre-existing) does **not** undermine that guard:
  on-disk accumulation adds zero rows to the search tables, because `search()`
  LEFT JOINs existing datoms. A comment warns that a future retraction fix puts
  retracts in the same queue as adds, which would require halving the guard.

### Test-quality finding

The reviewer reverted each fix in isolation and confirmed every one had a test
that fails on revert — no decorative tests. It also identified why the round-one
formula fixture missed two defects: 12 hand-picked shapes that all happened to
round-trip. The round-two test enumerates the aggregation argument space
instead. `nth_tuple`'s `assert!` promotion has no failing-on-revert test
because the panic is genuinely unreachable (fuzzed across 96 axis/filter/cursor
configurations) — untested belt-and-braces, not a guarded fix.

### Still open after both rounds

Typed (non-numeric) cell editing; keyboard focus stealing between the grid and
other text fields; derived CSV export reading input storage rather than computed
results (a derived measure currently exports a header and zero rows);
multi-measure display; undo/redo. Pre-existing and unfixed: stale
`:measure/categories` refs are never retracted, so changing a measure's category
set accumulates the union across saves.

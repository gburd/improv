# docs/demo: the scripts that make the guide's movies

Each script records its movie end to end, from a real run of this checkout,
so a release can re-record it with one command from the repository root.
The console movies are recorded by `record.py` (pexpect to an asciicast v2
`.cast`, committed here beside its script) and rendered by `render.sh` into
`docs/src/img/NAME.gif`, `NAME.mp4` and `NAME.png` (the MP4's poster).

| Script | Records | Needs | Takes |
| --- | --- | --- | --- |
| `tui-demo.sh` | improv-tui on the sample model: an input typed as `1,500`, Revenue recomputed, a number format (`#`), paging, pivoting, a range Sum, a saved view the next start opens on | a release build of `improv_cli` and `improv_tui` (`BIN=` their directory, default `target/release`) | 2 min |
| `cli-demo.sh` | the `improv` CLI: a model built command by command, `eval`, `format`, `policy template`, `set` and `check`, `export-csv` | a release build of `improv_cli` | 3 min |
| `server-demo.sh` | improv-server with a token: a refused request, `/v1/structure`, a window of cells, a `POST /v1/cells` edit the `/v1/changes` stream delivers, the audit log; port 3071 on loopback | a release build of `improv_cli` and `improv_server`; curl, jq | 2 min |
| `pg-demo.sh` | pg_improv in psql: a model over a table, an `UPDATE` reaching the results, `improv.status`, a `LANGUAGE improv` function, then improv-tui editing the model by URL | podman (below) | 4 min, plus the first build |
| `policies-demo.sh` | pg_improv access policies: the owner sets Cedar policies and grants a role; as the role, filtered rows and cells, a hidden item answering as missing, `explain_access`, a refused and an allowed `set_cells`; the owner reads `explain_access` and `improv.policy_audit` | podman (below) | 6 min, plus the first build |
| `duckdb-demo.sh` | the DuckDB v1.5.6 CLI with the `improv` extension running the DuckDB tutorial's queries | x86_64 Linux, gh, curl, unzip (it downloads both, below) | 2 min |
| `gui-movie.sh` | the desktop GUI on a headless Xvfb, driven by xdotool, recorded by ffmpeg, a caption per step drawn under the screen: pivoting, paging, an input edit, a formula with autocomplete, a color graph, the Inspector. `gui-demo.mp4`, `gui-demo.gif` and the poster `gui-demo.png` | Xvfb, xdotool, ImageMagick, xwd (dev shell); gifsicle and an ffmpeg with x11grab and drawtext (`nix shell nixpkgs#ffmpeg-full nixpkgs#gifsicle`); a release build of `improv_cli` and `improv_gui` (`IMPROV_BIN=`) | 2 min; the movie is 90 s |
| `gui-stills.sh` | the desktop GUI's stills for the GUI chapter, from one run on a headless Xvfb: the workspace, a worksheet, the Inspector, the Format panel, two graphs, Print with Page Layout, context help, the Functions window, autocomplete, stacked rows and a group selection | Xvfb, xdotool, ImageMagick, xwd (dev shell); a release build of `improv_cli` and `improv_gui` (`IMPROV_BIN=`) | 1 min |
| `recalc-demo.sh` | the older recalculation demo, `recalc-demo.svg`, recorded with asciinema and svg-term-cli as its header says (not `record.py`) | asciinema, npx | not timed here |
| `sample-model.sh` | not a movie: the 3-D sales model (Time x Product x Region) the TUI, server and GUI movies open | `improv` (`IMPROV=` the binary) | seconds |

Every console movie also needs python3 with pexpect, agg, gifsicle,
ffmpeg and ImageMagick (in the repo's dev shell: ffmpeg and ImageMagick;
agg, gifsicle and pexpect from nixpkgs). The times are on a 48-core
Linux box with the binaries already built.

**Postgres movies** (`pg-demo.sh`, `policies-demo.sh`) record inside the
`pg17.Containerfile` image: rust:1.97-trixie, as CI uses, with PGDG
Postgres 17 and cargo-pgrx 0.17.0. `pg-container.sh` builds the image
(podman caches it), mounts the checkout at `/src`, installs pg_improv
from it (`cargo pgrx install --release`, as `ci/pg_improv.sh` does),
builds `improv` and `improv-tui`, and runs the script, which writes its
`.cast` to the mount; the script then renders on the host. The cargo
registry and the target dir are kept in `CACHE` (default
`~/.cache/improv-demo`); the first run builds for about 10 minutes.

**The DuckDB movie** uses the v0.12.2 GitHub Release's linux_amd64
extension, checked against that release's `SHA256SUMS`, because
`crates/duckdb_improv` is unchanged since v0.12.2 and its build takes
long. After a change to it, build it (`make release` in
`crates/duckdb_improv`) and pass `EXT=path/to/improv.duckdb_extension`.
The DuckDB v1.5.6 CLI comes from DuckDB's GitHub release, checked
against the digest GitHub lists for the asset. Both are kept in `DUCK`
(default `~/.cache/improv-demo/duckdb`).

## Writing a script

`record.py OUT.cast --size COLSxROWS -- CMD ...` reads steps from stdin;
its docstring lists them. The ones that make a movie readable:

- `say TEXT` types a tidbit as a comment (`comment --` for psql and
  DuckDB, `#` by default), presses Enter and holds it long enough to
  read. A tidbit says what the next command does, in at most about 70
  characters, and never claims a result the movie does not show.
- `caption TEXT` puts the tidbit in a bar under a full-screen program
  (the TUI), which never sees it.
- `clear PROMPT` wipes the screen between parts, as Ctrl-L would.
- `sleep` at least 2.5 s on every result worth reading. `render.sh`
  cuts pauses longer than 3 s to 3 s.
- `until SHELLCMD` waits, unrecorded, for something asynchronous (the
  sync worker) before the next step.

`python3 docs/demo/record.py --selftest` checks the recorder. Look at the
rendered frames before committing (`magick 'x.gif' -coalesce f-%d.png`):
tidbits legible, captions clear of the program, nothing cut off. Keep a
GIF under about 1.2 MB and an MP4 under about 400 KB.

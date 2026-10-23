#!/usr/bin/env python3
"""Run a guide page's code blocks and check their printed output.

A tutorial page is its own test script: the blocks a reader copies are the
ones this runs, so the page and the test cannot drift.

    mdtest.py PAGE.md [--sql CMD] [--sql-prelude TEXT] [--dir DIR]
              [--from HEADING --to HEADING]

- A ```sh block runs in bash (`set -euo pipefail`) in DIR. Variables carry
  over to the next block (`set -a`, then the environment is kept).
- A ```sql block is piped to CMD (a shell command, run in DIR) with
  --sql-prelude (e.g. a LOAD of a local build) in front of it. Without
  --sql, sql blocks are an error.
- A ```text block right after a run block is the output that block must
  print (compared line by line, trailing spaces ignored).
- `<!-- mdtest: skip -->` on the line before a block: not run (and a text
  block after it is not checked). `<!-- mdtest: retry -->`: re-run until the
  output matches, for up to 30 s (results an asynchronous worker writes).
  `<!-- mdtest: sh ... -->`: a hidden shell step run at that point.

--from/--to run only the part of the page from the heading line --from
(e.g. "## Quick start") up to the heading line --to. Every block must succeed. Exits 1 on the first failure, after printing it.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time

FENCE = re.compile(r"^```(\w*)\s*$")
DIRECTIVE = re.compile(r"^<!--\s*mdtest:\s*(\w+)\s*(.*?)\s*-->\s*$", re.S)


def blocks(text):
    """Yield (lang, body, directive, directive_arg, line) in page order;
    a hidden `sh` directive comes out as a block of lang 'sh'."""
    lines = text.split("\n")
    i, pending = 0, (None, "")
    while i < len(lines):
        m = DIRECTIVE.match(lines[i])
        if m:
            if m.group(1) == "sh":
                yield "sh", m.group(2) + "\n", None, "", i + 1
            else:
                pending = (m.group(1), m.group(2))
            i += 1
            continue
        m = FENCE.match(lines[i])
        if m:
            start, j = i + 1, i + 1
            while j < len(lines) and lines[j].rstrip() != "```":
                j += 1
            yield m.group(1), "\n".join(lines[start:j]) + "\n", pending[0], pending[1], start
            pending = (None, "")
            i = j + 1
            continue
        if lines[i].strip():
            pending = (None, "")
        i += 1


def norm(s):
    return [line.rstrip() for line in s.rstrip("\n").split("\n")]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("page")
    ap.add_argument("--sql")
    ap.add_argument("--sql-prelude", default="")
    ap.add_argument("--dir", default=os.getcwd())
    ap.add_argument("--from", dest="start")
    ap.add_argument("--to", dest="stop")
    a = ap.parse_args()
    page = open(a.page, encoding="utf-8").read()
    for heading, keep_after in ((a.start, True), (a.stop, False)):
        if heading:
            at = page.find("\n" + heading + "\n")
            if at < 0:
                sys.exit(f"{a.page}: no heading line {heading!r}")
            page = page[at:] if keep_after else page[:at]
    env = dict(os.environ)
    last = None  # (run function, line) of the previous run block
    ran = checked = 0
    with tempfile.TemporaryDirectory() as tmp:
        state = os.path.join(tmp, "env")

        def sh(body):
            nonlocal env
            # (`env -0` is not portable: the shell's variables leave as JSON.)
            env["DUMP"] = "import json,os,sys; json.dump(dict(os.environ), open(sys.argv[1], 'w'))"
            script = os.path.join(tmp, "block.sh")
            with open(script, "w", encoding="utf-8") as f:
                f.write(body)
            p = subprocess.run(
                ["bash", "-c", 'set -euo pipefail -a; . "$0"; "$2" -c "$DUMP" "$1"',
                 script, state, sys.executable],
                cwd=a.dir, env=env, capture_output=True, text=True)
            if p.returncode == 0:
                env = json.load(open(state, encoding="utf-8"))
                env.pop("DUMP", None)
            return p

        def sql(body):
            if not a.sql:
                sys.exit(f"{a.page}: a sql block, but no --sql command")
            return subprocess.run(["bash", "-c", a.sql], input=a.sql_prelude + body,
                                  cwd=a.dir, env=env, capture_output=True, text=True)

        for lang, body, directive, arg, line in blocks(page):
            if lang == "text":
                if last is None:
                    continue
                run, at, retry = last
                last = None
                want, deadline = norm(body), time.time() + (30 if retry else 0)
                while True:
                    got = norm(out)
                    if got == want or time.time() > deadline:
                        break
                    time.sleep(0.2)
                    out = run().stdout
                if got != want:
                    print(f"FAIL {a.page}:{at}: output differs from the text block at line {line}")
                    print("--- want\n" + "\n".join(want) + "\n--- got\n" + "\n".join(got))
                    return 1
                checked += 1
                continue
            last = None
            if lang not in ("sh", "sql") or directive == "skip":
                continue
            run = (lambda b=body: sh(b)) if lang == "sh" else (lambda b=body: sql(b))
            p = run()
            ran += 1
            if p.returncode != 0:
                print(f"FAIL {a.page}:{line}: the {lang} block exited {p.returncode}")
                print(body + "--- stdout\n" + p.stdout + "--- stderr\n" + p.stderr)
                return 1
            out = p.stdout
            last = (run, line, directive == "retry")
    print(f"ok: {a.page}: {ran} blocks ran, {checked} outputs matched")
    return 0


if __name__ == "__main__":
    sys.exit(main())

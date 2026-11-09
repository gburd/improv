#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Copyleft licence headers in a `cargo vendor` tree, against Improv's policy
(CONTRIBUTING.md, "Licences of dependencies"):

    license-scan.py vendor MANIFEST_DIR VENDOR
        `cargo vendor --locked` that workspace into VENDOR, then snapshot and
        check it (a workspace we do not patch: the root).
    license-scan.py snapshot VENDOR
        Record each crate's upstream file checksums, as cargo vendor wrote
        them into .cargo-checksum.json. Run it right after `cargo vendor`,
        before anything edits the tree.
    license-scan.py check VENDOR
        Fail on any SPDX-License-Identifier naming GPL or AGPL; on an LGPL one
        the allow list does not name with that exact expression; on a listed
        LGPL file that differs from upstream, unless its entry gives the
        upstream PR (patched-upstream: URL); and on an entry that no longer
        matches its crate.
    license-scan.py selftest
        The checks above, on a made-up tree.

The allow list is license-allow.txt beside this script (--allow FILE).
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
SNAPSHOT = ".upstream-checksums.json"
TAG = b"SPDX-License-Identifier:"
LINE = re.compile(rb"SPDX-License-Identifier:[ \t]*([^\r\n]*)")
KIND = re.compile(r"^(AGPL|LGPL|GPL)(-|\+|$)", re.IGNORECASE)
RANK = {None: 0, "LGPL": 1, "GPL": 2, "AGPL": 3}


def die(msg):
    sys.exit(f"license-scan: {msg}")


def clean(expr):
    """The expression without a comment's closing token."""
    return re.sub(r"\s*(\*/|-->)\s*$", "", expr).strip()


def kind(lic):
    m = KIND.match(lic)
    return m and m.group(1).upper()


def classify(expr):
    """What an SPDX expression obliges: 'AGPL', 'GPL', 'LGPL' or None. OR takes
    the freest alternative, AND the strictest term, `X WITH e` counts as X."""
    words = expr.replace("(", " ( ").replace(")", " ) ").split()
    terms, skip = [], False
    for w in words:
        if skip:
            skip = False
        elif w.upper() == "WITH":
            skip = True
        else:
            terms.append(w)

    def strictest(ts):
        return max((kind(t) for t in ts), key=RANK.get, default=None)

    if "(" in terms:
        # shortcut: no precedence with parentheses, so the strictest term
        # decides; parse them if a real header ever needs it.
        return strictest(terms)
    alts, cur = [], []
    for t in terms + ["OR"]:
        if t.upper() == "OR":
            alts.append([x for x in cur if x.upper() != "AND"])
            cur = []
        else:
            cur.append(t)
    return min((strictest(a) for a in alts), key=RANK.get)


def crates(vendor):
    for d in sorted(os.listdir(vendor)):
        p = os.path.join(vendor, d)
        if not d.startswith(".") and os.path.isdir(p):
            yield d, p


def ident(d, path):
    """`name@version`, from the vendored manifest (the directory name else)."""
    try:
        with open(os.path.join(path, "Cargo.toml"), "rb") as f:
            pkg = tomllib.load(f)["package"]
        return f"{pkg['name']}@{pkg['version']}"
    except (OSError, KeyError, TypeError, tomllib.TOMLDecodeError):
        return d


def snapshot(vendor):
    snap = {}
    for d, p in crates(vendor):
        with open(os.path.join(p, ".cargo-checksum.json"), encoding="utf-8") as f:
            snap[d] = json.load(f)["files"]
    with open(os.path.join(vendor, SNAPSHOT), "w", encoding="utf-8") as f:
        json.dump(snap, f)


def load_allow(path):
    allow = {}
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            s = line.strip()
            if not s or s.startswith("#"):
                continue
            key, _, rest = s.partition(" ")
            expr, has_url, url = rest.partition("patched-upstream:")
            expr, url = expr.strip(), url.strip()
            if classify(expr) != "LGPL":
                die(
                    f"{path}:{n}: only LGPL files may be listed, not '{expr}': "
                    "GPL and AGPL are forbidden, other licences need a separate decision"
                )
            if has_url and not url.startswith("https://"):
                die(f"{path}:{n}: patched-upstream needs the upstream PR's https:// URL")
            allow[key] = (expr, url, n)
    return allow


def check(vendor, allow_path):
    allow = load_allow(allow_path)
    try:
        with open(os.path.join(vendor, SNAPSHOT), encoding="utf-8") as f:
            snap = json.load(f)
    except OSError:
        die(f"no {SNAPSHOT} in {vendor}: run `snapshot` right after cargo vendor")
    errors, notes, seen, lgpl, tagged, present = [], [], set(), 0, 0, set()
    for d, p in crates(vendor):
        crate = ident(d, p)
        present.add(crate)
        for root, _, files in os.walk(p):
            for name in files:
                fp = os.path.join(root, name)
                with open(fp, "rb") as f:
                    data = f.read()
                if TAG not in data:
                    continue
                tagged += 1
                rel = os.path.relpath(fp, p).replace(os.sep, "/")
                for m in LINE.finditer(data):
                    expr = clean(m.group(1).decode("utf-8", "replace"))
                    k = classify(expr)
                    where = f"{d}/{rel}: '{expr}'"
                    if k in ("GPL", "AGPL"):
                        errors.append(f"{where}: {k} is forbidden")
                    elif k is None:
                        if any(kind(w) for w in expr.split()):
                            notes.append(f"{where}: has a non-copyleft alternative, taken")
                    else:
                        lgpl += 1
                        key = f"{crate}/{rel}"
                        entry = allow.get(key)
                        if entry is None or entry[0] != expr:
                            errors.append(f"{where}: LGPL, not in {allow_path} as `{key} {expr}`")
                            continue
                        seen.add(key)
                        upstream = snap.get(d, {}).get(rel)
                        if hashlib.sha256(data).hexdigest() == upstream:
                            continue
                        if entry[1]:
                            notes.append(f"{where}: modified, published upstream at {entry[1]}")
                        else:
                            errors.append(
                                f"{where}: LGPL file differs from upstream; an edit must be "
                                "offered upstream, then listed with patched-upstream: <PR URL>"
                            )
    for key, (expr, _, n) in allow.items():
        if key not in seen and key.split("/", 1)[0] in present:
            errors.append(f"{allow_path}:{n}: `{key} {expr}` no longer matches the vendored crate")
    for s in notes:
        print(f"license-scan: note: {s}", file=sys.stderr)
    for s in errors:
        print(f"license-scan: FAIL: {s}", file=sys.stderr)
    print(
        f"license-scan: {vendor}: {len(present)} crates, {tagged} files with SPDX "
        f"headers, {lgpl} LGPL (listed, unmodified unless noted), {len(errors)} failures",
        file=sys.stderr,
    )
    if errors:
        sys.exit(1)


def selftest():
    import tempfile

    for expr, want in {
        "GPL-3.0-only": "GPL",
        "AGPL-3.0-or-later": "AGPL",
        "LGPL-2.1-or-later": "LGPL",
        "GPL-2.0+ or MIT": None,
        "MIT OR Apache-2.0": None,
        "MIT AND LGPL-2.1-only": "LGPL",
        "GPL-2.0-only WITH Classpath-exception-2.0": "GPL",
        "Apache-2.0 WITH LLVM-exception": None,
        "LGPL-2.1-only OR GPL-2.0-only": "LGPL",
        "GPL-2.0-only AND (MIT OR BSD-3-Clause)": "GPL",
        "MPL-2.0": None,
    }.items():
        assert classify(expr) == want, (expr, classify(expr), want)
    assert clean("LGPL-2.1-or-later */") == "LGPL-2.1-or-later"

    def run(tree, allow):
        with tempfile.TemporaryDirectory() as v:
            for rel, text in tree.items():
                os.makedirs(os.path.dirname(os.path.join(v, rel)), exist_ok=True)
                with open(os.path.join(v, rel), "w", encoding="utf-8") as f:
                    f.write(text)
            for _, p in crates(v):
                files = {}
                for root, _, names in os.walk(p):
                    for n in names:
                        with open(os.path.join(root, n), "rb") as f:
                            rel = os.path.relpath(os.path.join(root, n), p)
                            files[rel] = hashlib.sha256(f.read()).hexdigest()
                with open(os.path.join(p, ".cargo-checksum.json"), "w", encoding="utf-8") as f:
                    json.dump({"files": files, "package": None}, f)
            snapshot(v)
            with open(os.path.join(v, "a", "p.xml"), "a", encoding="utf-8") as f:
                f.write(tree.get("edit", ""))
            al = os.path.join(v, "allow.txt")
            with open(al, "w", encoding="utf-8") as f:
                f.write(allow)
            err, sys.stderr = sys.stderr, open(os.devnull, "w")
            try:
                check(v, al)
            except SystemExit as e:
                return e.code or 0
            finally:
                sys.stderr.close()
                sys.stderr = err
            return 0

    base = {
        "a/Cargo.toml": '[package]\nname = "a"\nversion = "1.0.0"\n',
        "a/p.xml": "<!-- SPDX-License-Identifier: LGPL-2.1-or-later -->\n",
        "a/t.c": "/* SPDX-License-Identifier: GPL-2.0+ or MIT */\n",
    }
    ok = "a@1.0.0/p.xml LGPL-2.1-or-later\n"
    pr = "a@1.0.0/p.xml LGPL-2.1-or-later patched-upstream: https://example.org/pr/1\n"
    assert run(base, ok) == 0
    assert run(base, "") != 0, "an unlisted LGPL file passed"
    assert run(base, "a@1.0.0/p.xml LGPL-3.0-only\n") != 0, "a changed expression passed"
    assert run({**base, "edit": "x"}, ok) != 0, "a modified LGPL file passed"
    assert run({**base, "edit": "x"}, pr) == 0, "patched-upstream was not honoured"
    assert run({**base, "a/g.rs": "// SPDX-License-Identifier: GPL-3.0-only\n"}, ok) != 0
    assert run({**base, "a/g.rs": "// SPDX-License-Identifier: AGPL-3.0\n"}, ok) != 0
    assert run(base, ok + "a@1.0.0/gone.xml LGPL-2.1-only\n") != 0, "a stale entry passed"
    assert run(base, ok + "a@1.0.0/g.rs GPL-3.0-only\n") != 0, "a GPL entry was accepted"
    print("license-scan: selftest ok", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--allow", default=os.path.join(HERE, "license-allow.txt"))
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("vendor")
    v.add_argument("manifest_dir")
    v.add_argument("vendor")
    for c in ("snapshot", "check"):
        sub.add_parser(c).add_argument("vendor")
    sub.add_parser("selftest")
    a = ap.parse_args()
    if a.cmd == "selftest":
        return selftest()
    if a.cmd == "vendor":
        manifest = os.path.join(a.manifest_dir, "Cargo.toml")
        cmd = ["cargo", "vendor", "--locked", "--manifest-path", manifest, a.vendor]
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)
    if a.cmd in ("vendor", "snapshot"):
        snapshot(a.vendor)
    if a.cmd in ("vendor", "check"):
        check(a.vendor, a.allow)


if __name__ == "__main__":
    main()

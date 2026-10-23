#!/usr/bin/env python3
"""Generate docs/src/pg-reference.md from a live pg_improv, and check it.

    crates/pg_improv/tests/pg-reference.py            # fail if the page is stale
    UPDATE_DOCS=1 crates/pg_improv/tests/pg-reference.py   # rewrite it

Starts a scratch cluster with pg_improv preloaded (PG_BIN, default Debian's
PG 17; port PGI_PORT, default 55472) and reads, from the catalog: every
function in schema improv with its signature and result, every improv.*
GUC from pg_settings, and the columns of improv.status and improv.models.
The prose comes from the sources a reader already sees: function
descriptions from the "SQL API" table in crates/pg_improv/README.md
(INTERNAL below for the functions the extension calls itself), status
columns from the table in docs/src/monitoring.md, GUC text from the GUCs'
own descriptions. It fails when a function, GUC or column has no
description, when a documented one no longer exists, and when a function
is SECURITY DEFINER.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "../../.."))
PAGE = os.path.join(ROOT, "docs/src/pg-reference.md")
README = os.path.join(ROOT, "crates/pg_improv/README.md")
MONITORING = os.path.join(ROOT, "docs/src/monitoring.md")
REPO = "https://codeberg.org/gregburd/improv/src/branch/main"
PG_BIN = os.environ.get("PG_BIN", "/usr/lib/postgresql/17/bin")
PORT = os.environ.get("PGI_PORT", "55472")

# Functions the extension installs for its own use: triggers, the language
# handler, and helpers its policies and loads call.
INTERNAL = {
    "check_owner": "Trigger on `improv.model`: only a role that may act as both the current and the new owner may set or change a model's owner.",
    "commit_capture": "Commit mode's row-capture statement trigger (`improv.commit_capture = 'rows'`): notes the coordinates a statement touched for the pre-commit recompute.",
    "commit_catalog_changed": "Statement trigger on `improv.model` and `improv.binding`: invalidates every backend's cache of the commit-mode relations.",
    "may_act_as": "Whether the caller may act as role `r`: a superuser, or a member with the right to `SET ROLE` to it (any member on PG 15). The row-level security policies on the catalog call it.",
    "mint_items": "Assigns ids to item names it has not seen, in lexical order, never reusing one. Shared with the `postgres://` model store.",
    "on_change": "Trigger mode's sync trigger: recomputes the changed inputs' dependents in the writer's transaction, as the model's owner.",
    "plimprov_call_handler": "The `LANGUAGE improv` call handler.",
    "plimprov_validator": "The `LANGUAGE improv` validator: checks the signature, and the body when `check_function_bodies` is on.",
}

# improv.models has no table elsewhere; its columns, described once here.
MODELS = {
    "name": "The model's name.",
    "owner": "The role the model reads its tables and writes its results as.",
    "sync_mode": "`logical`, `trigger`, `commit` or `manual`.",
    "may_change": "Whether the caller may act as the owner: change the model, read its items and cells, `what_if` it.",
    "result_schema": "The schema of its result tables, `improv_<lower(name)>`.",
}

QUERY = r"""
SELECT json_build_object(
 'functions', (SELECT json_agg(json_build_object(
     'name', p.proname, 'nargs', p.pronargs,
     'args', pg_get_function_arguments(p.oid),
     'result', pg_get_function_result(p.oid),
     'definer', p.prosecdef, 'lang', l.lanname) ORDER BY p.proname, p.pronargs)
   FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
   JOIN pg_language l ON l.oid = p.prolang WHERE n.nspname = 'improv'),
 'gucs', (SELECT json_agg(json_build_object(
     'name', name, 'type', vartype, 'default', boot_val, 'unit', unit,
     'min', min_val, 'max', max_val, 'enum', enumvals, 'context', context,
     'short', short_desc, 'extra', extra_desc) ORDER BY name)
   FROM pg_settings WHERE name LIKE 'improv.%'),
 'models', (SELECT json_agg(json_build_object(
     'name', a.attname, 'type', format_type(a.atttypid, a.atttypmod)) ORDER BY a.attnum)
   FROM pg_attribute a WHERE a.attrelid = 'improv.models'::regclass AND a.attnum > 0
   AND NOT a.attisdropped),
 'version', (SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'))
"""

CONTEXT = {
    "user": "any session (`SET`)",
    "sighup": "`postgresql.conf`, reload",
    "postmaster": "`postgresql.conf`, restart",
    "superuser": "superuser `SET`",
}


def catalog():
    d = tempfile.mkdtemp(prefix="pgi-ref.", dir=os.path.expanduser("~"))
    psql = [f"{PG_BIN}/psql", "-h", d, "-p", PORT, "-U", "improv", "-d", "ref", "-qAtX",
            "-v", "ON_ERROR_STOP=1"]
    try:
        subprocess.run([f"{PG_BIN}/initdb", "-D", d, "-A", "trust", "-U", "improv"],
                       check=True, stdout=subprocess.DEVNULL)
        with open(f"{d}/postgresql.conf", "a") as f:
            f.write(f"port = {PORT}\nunix_socket_directories = '{d}'\n"
                    "shared_preload_libraries = 'pg_improv'\nwal_level = logical\n")
        subprocess.run([f"{PG_BIN}/pg_ctl", "-D", d, "-l", f"{d}/log", "-w", "start"],
                       check=True, stdout=subprocess.DEVNULL)
        subprocess.run([f"{PG_BIN}/createdb", "-h", d, "-p", PORT, "-U", "improv", "ref"],
                       check=True)
        subprocess.run(psql + ["-c", "CREATE EXTENSION pg_improv"], check=True)
        out = subprocess.run(psql + ["-c", QUERY], check=True, capture_output=True, text=True)
        return json.loads(out.stdout)
    finally:
        subprocess.run([f"{PG_BIN}/pg_ctl", "-D", d, "stop", "-m", "fast"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["find", d, "-delete"], stderr=subprocess.DEVNULL)


def readme_functions():
    """(name, nargs) -> (signature, description) from the README's SQL API table."""
    text = open(README, encoding="utf-8").read()
    table = text[text.index("## SQL API"):]
    table = table[:table.index("\n\n", table.index("| --- |"))]
    out = {}
    for m in re.finditer(r"^\| `(\w+)\(([^)]*)\)[^`]*` \| (.*) \|$", table, re.M):
        nargs = len([a for a in m.group(2).split(",") if a.strip()])
        # The README's own anchors point into the guide's Postgres chapter.
        desc = m.group(3).replace("](#", "](./postgres.md#")
        # "see below" is the README's own prose, which follows its table.
        desc = re.sub(r" \(see below\)", " (see the [README](" + REPO + "/crates/pg_improv/README.md))", desc)
        out[(m.group(1), nargs)] = desc
    return out


def status_columns():
    """name -> (type, meaning) from monitoring.md's improv.status table."""
    text = open(MONITORING, encoding="utf-8").read()
    out = {}
    for m in re.finditer(r"^\| `(\w+)` \| (\w+) \| (.*?) \| .* \|$", text, re.M):
        out[m.group(1)] = (m.group(2), m.group(3))
    return out


def render(cat):
    errors = []
    docs = readme_functions()
    status = status_columns()
    lines = [
        "# pg_improv SQL reference",
        "",
        "<!-- Generated by crates/pg_improv/tests/pg-reference.py from a live",
        "     pg_improv; UPDATE_DOCS=1 rewrites it. Do not edit. -->",
        "",
        f"Every function, setting and status column of pg_improv {cat['version']}, read from",
        "the extension's catalog. Every function is in schema `improv` and runs",
        "`SECURITY INVOKER`; a function on a model acts as the model's owner (see",
        "[Privileges](./postgres.md#privileges)). [Improv in Postgres](./postgres.md)",
        "explains how they fit together, and [Compatibility](./compatibility.md)",
        "which of them are stable.",
        "",
        "## Functions",
        "",
    ]
    internal = []
    seen = set()
    for f in cat["functions"]:
        if f["definer"]:
            errors.append(f"improv.{f['name']} is SECURITY DEFINER")
        key = (f["name"], f["nargs"])
        sig = f"improv.{f['name']}({f['args']}) → {f['result']}"
        if key in docs:
            seen.add(key)
            lines += [f"### `{f['name']}`" if sum(g["name"] == f["name"] for g in cat["functions"]) == 1
                      else f"### `{f['name']}` ({f['nargs']} arguments)", "",
                      "```sql", sig, "```", "", docs[key], ""]
        elif f["name"] in INTERNAL:
            internal.append(f"| `{f['name']}({f['args']})` | {f['result']} | {INTERNAL[f['name']]} |")
        else:
            errors.append(f"improv.{f['name']}/{f['nargs']} has no description in README.md's SQL API table")
    for key in sorted(set(docs) - seen):
        errors.append(f"README.md documents {key[0]}/{key[1]}, which the extension does not have")
    for name in sorted(set(INTERNAL) - {f["name"] for f in cat["functions"]}):
        errors.append(f"INTERNAL describes {name}, which the extension does not have")
    lines += [
        "### Internal functions",
        "",
        "The extension calls these itself (as triggers, as the `LANGUAGE improv`",
        "handler, or from its row-level security policies). Do not call them.",
        "",
        "| Function | Returns | What it is |",
        "| --- | --- | --- |",
        *internal,
        "",
        "## Settings (GUCs)",
        "",
        "`Set in` is the GUC's context: where a change takes effect.",
        "",
        "| GUC | Type | Default | Range | Set in | Description |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for g in cat["gucs"]:
        unit = f" {g['unit']}" if g["unit"] else ""
        if g["enum"]:
            rng = ", ".join(f"`{v}`" for v in g["enum"])
        elif g["min"] is not None and g["type"] in ("integer", "real"):
            rng = f"{g['min']}–{g['max']}{unit}"
        else:
            rng = ""
        default = f"`{g['default']}`{unit}" if g["default"] != "" else "(empty)"
        desc = g["short"] + (f" {g['extra']}" if g["extra"] else "")
        lines.append(f"| `{g['name']}` | {g['type']} | {default} | {rng} | "
                     f"{CONTEXT.get(g['context'], g['context'])} | {desc} |")
    lines += ["", "`improv.database` exists only when pg_improv is in",
              "`shared_preload_libraries`.", "",
              "## `improv.status(model)` columns", "",
              "One row per call; [Monitoring](./monitoring.md) says what a healthy value",
              "looks like and which to alert on.", "",
              "| Column | Type | Meaning |", "| --- | --- | --- |"]
    st = [f for f in cat["functions"] if f["name"] == "status"]
    cols = re.findall(r"(\w+) ([a-z ]+?)(?:,|\)$)", st[0]["result"]) if st else []
    for name, typ in cols:
        if name not in status:
            errors.append(f"improv.status column {name} is not in monitoring.md's table")
            continue
        short = {"timestamp with time zone": "timestamptz"}.get(typ, typ)
        if status[name][0] != short:
            errors.append(f"monitoring.md says status.{name} is {status[name][0]}; it is {short}")
        lines.append(f"| `{name}` | {short} | {status[name][1]} |")
    for name in sorted(set(status) - {c[0] for c in cols}):
        errors.append(f"monitoring.md documents status.{name}, which improv.status does not return")
    lines += ["", "## `improv.models` columns", "",
              "A view of every model, for anyone who may read the catalog.", "",
              "| Column | Type | Meaning |", "| --- | --- | --- |"]
    for c in cat["models"]:
        if c["name"] not in MODELS:
            errors.append(f"improv.models column {c['name']} has no description")
            continue
        lines.append(f"| `{c['name']}` | {c['type']} | {MODELS[c['name']]} |")
    for name in sorted(set(MODELS) - {c["name"] for c in cat["models"]}):
        errors.append(f"MODELS describes {name}, which improv.models does not have")
    return "\n".join(lines) + "\n", errors


def main():
    page, errors = render(catalog())
    if errors:
        print("pg-reference: " + "\n  ".join(["the reference is incomplete:"] + errors))
        return 1
    if os.environ.get("UPDATE_DOCS"):
        open(PAGE, "w", encoding="utf-8").write(page)
        print(f"wrote {PAGE}")
        return 0
    have = open(PAGE, encoding="utf-8").read() if os.path.exists(PAGE) else ""
    if have != page:
        sys.stdout.writelines(__import__("difflib").unified_diff(
            have.splitlines(True), page.splitlines(True), "committed", "generated"))
        print("docs/src/pg-reference.md is stale: run UPDATE_DOCS=1 crates/pg_improv/tests/pg-reference.py")
        return 1
    print("ok: docs/src/pg-reference.md matches the extension")
    return 0


if __name__ == "__main__":
    sys.exit(main())

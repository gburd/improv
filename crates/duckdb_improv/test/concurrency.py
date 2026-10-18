#!/usr/bin/env python3
"""Concurrency test for the improv DuckDB extension: client threads, each with
its own cursor on ONE database, call improv_calc (the shared formula cache)
and improv(...) (the connection pool) at once.

Every thread checks its own results, so a cross-thread mix-up in the cache or
a pooled connection shows up as a wrong value, not just a crash.

Usage: python3 test/concurrency.py build/release/improv.duckdb_extension
(needs the duckdb Python package: 1.5.6, or a 2.0 dev build)."""
import os
import sys
import tempfile
import threading

import duckdb

EXT = sys.argv[1]
THREADS, ROUNDS = 16, 25

db = os.path.join(tempfile.mkdtemp(), "conc.duckdb")
con = duckdb.connect(db, config={"allow_unsigned_extensions": "true"})
con.load_extension(EXT)
con.execute("CREATE TABLE f AS SELECT i % 7 AS k, i AS v FROM range(700) t(i)")
MODEL = "category K = k\ninput V = v sum\nW = V * {m}\n"
# sum(v) for k: v = 7j + k, j = 0..99
want_k = {k: sum(7 * j + k for j in range(100)) for k in range(7)}

errors, lock = [], threading.Lock()


def worker(t):
    cur = con.cursor()
    try:
        for r in range(ROUNDS):
            # Shared formula, per-thread inputs: one cache entry, many evals.
            got = cur.execute(
                "SELECT sum(improv_calc('x * 2 + t', json_object('x', i, 't', ?))) FROM range(200) s(i)",
                [t]).fetchone()[0]
            if got != sum(i * 2 + t for i in range(200)):
                raise AssertionError(f"improv_calc: {got}")
            # Per-thread formula text: distinct cache entries.
            got = cur.execute(f"SELECT improv_calc('{t} * 1000 + {r}')").fetchone()[0]
            if got != t * 1000 + r:
                raise AssertionError(f"improv_calc literal: {got}")
            # improv(...) reads the table on a pooled connection.
            rows = cur.execute("SELECT \"K\", value FROM improv('f', ?) ORDER BY 1",
                               [MODEL.format(m=t + 1)]).fetchall()
            if rows != [(str(k), float(want_k[k] * (t + 1))) for k in range(7)]:
                raise AssertionError(f"improv: {rows[:2]}...")
    except Exception as e:  # noqa: BLE001
        with lock:
            errors.append(f"thread {t}: {e}")


ts = [threading.Thread(target=worker, args=(t,)) for t in range(THREADS)]
for t in ts:
    t.start()
for t in ts:
    t.join()
print("\n".join(errors) or f"concurrency OK: {THREADS} threads x {ROUNDS} rounds (duckdb {duckdb.__version__})")
sys.exit(1 if errors else 0)

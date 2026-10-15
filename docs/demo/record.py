#!/usr/bin/env python3
"""Record a scripted terminal session as an asciicast v2 file.

    record.py OUT.cast [--size 100x30] -- CMD [ARG...]  < STEPS

STEPS is read from stdin, one step per line (blank lines and `#` comments
are skipped):

    expect REGEX    wait (up to 30 s) for REGEX in the output
    type TEXT       type TEXT at a human-ish pace (escapes like \\r decoded)
    send KEYS       send KEYS at once (escapes decoded: \\r, \\x1b[A, ...)
    sleep SECONDS   hold the frame (empty keepalive events every second, so
                    the player's idle-time limit does not cut the pause)
    pace MIN MAX    per-key typing delay range for later `type` steps
    until SHELLCMD  keep recording until SHELLCMD exits 0 (polled, <= 30 s)

Everything the program prints is recorded with its arrival time. A
cursor-position query (ESC[6n) or a device-attributes query (ESC[c) is
answered, so ratatui/crossterm never stall waiting on a real terminal.
"""
import codecs
import json
import os
import random
import re
import subprocess
import sys
import time

import pexpect

ANSWERS = {"\x1b[6n": "\x1b[1;1R", "\x1b[c": "\x1b[?62c"}


class Cast:
    """pexpect logfile_read sink: timestamps every chunk read from the pty."""

    def __init__(self):
        self.t0 = time.monotonic()
        self.events = []
        self.child = None

    def write(self, data):
        self.events.append([round(time.monotonic() - self.t0, 4), "o", data])
        for q, a in ANSWERS.items():
            if q in data and self.child is not None:
                self.child.send(a)

    def flush(self):
        pass


def main():
    argv = sys.argv[1:]
    out, size = argv[0], "100x30"
    if "--size" in argv:
        size = argv[argv.index("--size") + 1]
    cmd = argv[argv.index("--") + 1 :]
    cols, rows = map(int, size.split("x"))
    rng = random.Random(1)  # fixed seed: the same typing rhythm every run
    pace = (0.04, 0.12)

    cast = Cast()
    child = pexpect.spawn(
        cmd[0], cmd[1:], dimensions=(rows, cols), encoding="utf-8",
        codec_errors="replace", env={**os.environ, "TERM": "xterm-256color"},
    )
    child.delaybeforesend = None  # the step list sets the pace
    cast.child = child
    child.logfile_read = cast

    def pump(seconds):
        child.expect([pexpect.TIMEOUT, pexpect.EOF], timeout=seconds)

    try:
        for n, line in enumerate(sys.stdin, 1):
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            op, _, arg = line.partition(" ")
            if op == "expect":
                try:
                    child.expect(arg, timeout=30)
                except pexpect.TIMEOUT:
                    sys.exit(f"step {n}: timed out waiting for {arg!r}\n"
                             f"last output: {child.before[-400:]!r}")
            elif op in ("type", "send"):
                keys = codecs.decode(arg, "unicode_escape")
                if op == "send":
                    child.send(keys)
                    pump(0.05)
                else:
                    for c in keys:
                        child.send(c)
                        pump(rng.uniform(*pace))
            elif op == "until":
                for _ in range(300):
                    if subprocess.run(arg, shell=True).returncode == 0:
                        break
                    pump(0.1)
                else:
                    sys.exit(f"step {n}: never true: {arg}")
            elif op == "pace":
                pace = tuple(map(float, arg.split()))
            elif op == "sleep":
                left = float(arg)
                while left > 0:
                    pump(min(left, 1.0))
                    left -= 1.0
                    cast.write("")
            else:
                sys.exit(f"step {n}: unknown op {op!r}")

        pump(0.5)
    finally:  # write what was captured even when a step fails
        child.close(force=True)
        _write(out, cols, rows, cmd, cast.events)


def _write(out, cols, rows, cmd, events):
    header = {"version": 2, "width": cols, "height": rows,
              "timestamp": int(time.time()), "idle_time_limit": 1.5,
              "command": " ".join(cmd), "env": {"TERM": "xterm-256color"}}
    with open(out, "w", encoding="utf-8") as f:
        f.write(json.dumps(header) + "\n")
        for e in events:
            # Escape each CSI final byte (ESC[2;2H -> ESC[2;2\u0048) so a
            # spell checker does not see the H glued to the next word. Same JSON.
            line = json.dumps(e, ensure_ascii=False)
            f.write(re.sub(r"(\\u001b\[[0-9;?]*)([A-Za-z])",
                           lambda m: f"{m[1]}\\u{ord(m[2]):04x}", line) + "\n")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Record a scripted terminal session as an asciicast v2 file.

    record.py OUT.cast [--size 100x30] -- CMD [ARG...]  < STEPS
    record.py --selftest

STEPS is read from stdin, one step per line (blank lines and `#` comments
are skipped):

    expect REGEX    wait (up to 30 s) for REGEX in the output
    type TEXT       type TEXT at a human-ish pace (escapes like \\r decoded)
    send KEYS       send KEYS at once (escapes decoded: \\r, \\x1b[A, ...)
    sleep SECONDS   hold the frame (empty keepalive events every second, so
                    the player's idle-time limit does not cut the pause)
    pace MIN MAX    per-key typing delay range for later `type` steps
    until SHELLCMD  keep recording until SHELLCMD exits 0 (polled, <= 30 s)
    comment PREFIX  the comment marker `say` types: `#` (the default), `--`
    say TEXT        type PREFIX TEXT quickly, press Enter, and hold the frame
                    long enough to read it (50 ms a character, >= 1.5 s)
    caption TEXT    show TEXT in a caption bar under the program's screen
                    (an empty TEXT clears it); for full-screen programs,
                    where a typed comment is impossible
    clear PROMPT    wipe the recorded screen and show PROMPT at the top, as
                    Ctrl-L would (the program sees nothing; for a shell
                    without readline, at its prompt)

With any `caption` step the pty has the --size rows but the cast two more:
the program never sees the bar's rows, and a scroll region keeps its
output above them. The bar is drawn again after any output that could
have erased it (ED 0/2, new margins, a screen switch, a reset).

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
import tempfile
import time

import pexpect

ANSWERS = {"\x1b[6n": "\x1b[1;1R", "\x1b[c": "\x1b[?62c"}
SAY_PACE = (0.02, 0.045)
# An escape sequence complete at the end of a chunk: CSI, OSC, or a short ESC x.
COMPLETE = re.compile(r"\x1b(\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[ -/]*(?![\[\]])[0-~])")
# Output that can erase the caption bar or reset the scroll region: erase
# below / all, set margins, switch screens, full reset.
ERASES = re.compile(r"\x1b\[[02]?J|\x1b\[[0-9;]*r|\x1b\[\?(1049|1047|47)[hl]|\x1bc")


def ends_clean(data):
    """True when `data` does not end inside an escape sequence."""
    i = data.rfind("\x1b")
    return i < 0 or COMPLETE.match(data, i) is not None


class Cast:
    """pexpect logfile_read sink: timestamps every chunk read from the pty."""

    def __init__(self, cols, rows, bar):
        self.t0 = time.monotonic()
        self.events = []
        self.child = None
        self.cols, self.rows, self.bar = cols, rows, bar
        self.caption = ""
        self.dirty = False  # a chunk ended inside an escape: redraw later

    def write(self, data):
        self.events.append([round(time.monotonic() - self.t0, 4), "o", data])
        for q, a in ANSWERS.items():
            if q in data and self.child is not None:
                self.child.send(a)
        if self.bar and (self.dirty or ERASES.search(data) or not ends_clean(data)):
            # (A chunk cut inside an escape may hide an erase: redraw after the next.)
            self.dirty = not ends_clean(data)
            if not self.dirty:
                self.draw_bar()

    def draw_bar(self):
        """Inject the caption rows (R+1 blank, R+2 the text) and the scroll
        region that keeps the program above them; the cursor is restored.
        shortcut: this takes the DECSC slot and the margins, which ratatui
        never uses; a program that sets its own scroll region needs more."""
        r = self.rows
        s = f"\x1b7\x1b[1;{r}r\x1b[{r + 1};1H\x1b[0m\x1b[2K\x1b[{r + 2};1H\x1b[2K"
        if self.caption:
            s += "\x1b[1;97;40m" + f" » {self.caption}".ljust(self.cols) + "\x1b[0m"
        s += "\x1b8"
        self.events.append([round(time.monotonic() - self.t0, 4), "o", s])
        self.dirty = False

    def flush(self):
        pass


def record(out, cols, rows, cmd, steps):
    """Run CMD in a cols x rows pty, drive it with STEPS, write OUT."""
    bar = any(s.split(" ", 1)[0] == "caption" for s in steps)
    rng = random.Random(1)  # fixed seed: the same typing rhythm every run
    pace = (0.06, 0.16)
    prefix = "# "

    cast = Cast(cols, rows, bar)
    child = pexpect.spawn(
        cmd[0], cmd[1:], dimensions=(rows, cols), encoding="utf-8",
        codec_errors="replace", env={**os.environ, "TERM": "xterm-256color"},
    )
    child.delaybeforesend = None  # the step list sets the pace
    cast.child = child
    if bar:
        cast.draw_bar()
    child.logfile_read = cast

    def pump(seconds):
        child.expect([pexpect.TIMEOUT, pexpect.EOF], timeout=seconds)

    def hold(seconds):
        while seconds > 0:
            pump(min(seconds, 1.0))
            seconds -= 1.0
            cast.write("")

    def typed(keys, speed):
        for c in keys:
            child.send(c)
            pump(rng.uniform(*speed))

    try:
        for n, line in enumerate(steps, 1):
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
                    typed(keys, pace)
            elif op == "say":
                text = prefix + arg
                if len(text) > 76:
                    sys.exit(f"step {n}: a tidbit of {len(text)} characters: keep it under 77")
                typed(text, SAY_PACE)
                child.send("\r")
                hold(max(1.5, 0.05 * len(arg)))
            elif op == "clear":
                cast.write("\x1b[H\x1b[2J" + arg)
            elif op == "comment":
                prefix = arg.strip() + " "
            elif op == "caption":
                if len(arg) + 3 > cols:
                    sys.exit(f"step {n}: the caption is wider than {cols - 3} columns")
                cast.caption = arg
                cast.draw_bar()
                hold(max(1.5, 0.05 * len(arg)) if arg else 0)
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
                hold(float(arg))
            else:
                sys.exit(f"step {n}: unknown op {op!r}")

        pump(0.5)
    finally:  # write what was captured even when a step fails
        child.close(force=True)
        _write(out, cols, rows + 2 * bar, cmd, cast.events)


def _write(out, cols, rows, cmd, events):
    header = {"version": 2, "width": cols, "height": rows,
              "timestamp": int(time.time()), "idle_time_limit": 3,
              "command": " ".join(cmd), "env": {"TERM": "xterm-256color"}}
    with open(out, "w", encoding="utf-8") as f:
        f.write(json.dumps(header) + "\n")
        for e in events:
            f.write(escape(json.dumps(e, ensure_ascii=False)) + "\n")


def escape(line):
    """Hide glued and cut words from a spell checker, in the same JSON.
    Each CSI final byte is escaped (ESC[2;2H -> ESC[2;2\\u0048), so the H
    is not glued to the next word, and so is a run of letters just before
    a control character or at the end of an event: a line editor redraws a
    word one key at a time (ext, then exte, ...), and the cuts read as
    misspellings."""
    line = re.sub(r"(\\u001b\[[0-9;?]*)([A-Za-z])",
                  lambda m: f"{m[1]}\\u{ord(m[2]):04x}", line)
    cut = lambda m: m[0] if m[1] is None else "".join(f"\\u{ord(c):04x}" for c in m[1])
    return re.sub(r'\\u[0-9a-fA-F]{4}|\\.|([A-Za-z]+)(?=\\|"\]$)', cut, line)


def selftest():
    """Record `bash -c` with one say and one caption; check the events."""
    for raw in ["exte\x1b[0m", "a\rSELE\r\x1b[1;2Hxy", "plain words", "\x1b[0m"]:
        line = escape(json.dumps([0.5, "o", raw], ensure_ascii=False))
        assert json.loads(line)[2] == raw, line
        assert not any(w in line for w in ("exte", "SELE", "Hxy")), line
    assert ends_clean("ab\x1b[2J") and ends_clean("x\x1b7") and ends_clean("plain")
    assert not ends_clean("ab\x1b[2") and not ends_clean("ab\x1b")
    steps = ["say hello there", "caption Look below", "clear $ ", "sleep 0.2"]
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "t.cast")
        t0 = time.monotonic()
        record(out, 40, 6, ["bash", "-c", 'read -r l; printf "got[%s]\\n" "$l"; read -r m'], steps)
        took = time.monotonic() - t0
        with open(out, encoding="utf-8") as f:
            header, *events = [json.loads(line) for line in f]
    assert header["height"] == 8 and header["width"] == 40, header
    text = "".join(e[2] for e in events)
    assert "got[# hello there]" in text, text
    assert "\x1b[1;6r" in events[0][2], "the scroll region comes first"
    bars = [e for e in events if "\x1b[8;1H" in e[2] and "Look below" in e[2]]
    assert bars and bars[0][2].startswith("\x1b7") and bars[0][2].endswith("\x1b8"), bars
    say_end = next(e[0] for e in events if "got[" in e[2])
    assert bars[0][0] - say_end >= 1.4, "say holds the frame to read it"
    assert took >= 3.0, took  # say's hold + caption's hold
    wiped = [i for i, e in enumerate(events) if e[2] == "\x1b[H\x1b[2J$ "]
    assert wiped and "Look below" in events[wiped[0] + 1][2], "clear, then the bar again"
    print("record.py selftest: ok")


def main():
    argv = sys.argv[1:]
    if argv == ["--selftest"]:
        return selftest()
    out, size = argv[0], "100x30"
    if "--size" in argv:
        size = argv[argv.index("--size") + 1]
    cmd = argv[argv.index("--") + 1 :]
    cols, rows = map(int, size.split("x"))
    record(out, cols, rows, cmd, [line.rstrip("\n") for line in sys.stdin])


if __name__ == "__main__":
    main()

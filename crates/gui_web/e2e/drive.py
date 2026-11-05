#!/usr/bin/env python3
"""Drive the web GUI in headless Chromium over the DevTools protocol.

    drive.py <url> <token> <out-dir>

Two browser sessions (two tabs, two logins) on one improv-server:

1. both log in with the token typed into the login panel;
2. tab A double-clicks Quantity[2025Q1, Widget], types 150, Enter;
3. tab B shows the new Quantity and the server's recalculated Revenue
   (Price x 150) within a second, through the change stream;
4. with RESTART set (a command that restarts improv-server), the stream
   drops; A edits again, and B, reconnecting with ?since=, catches up;
5. A pivots Revenue by dragging the Time tile to the columns;
6. A types a formula with a misspelled name: the error is shown, located;
7. Ctrl+Z in A undoes A's last edit, and B shows it;
8. with ADD_REGION set (a command that adds a measure over a third
   category), A picks a page item and sees that page's cells;
9. screenshots of both tabs.

The app publishes its visible cells (position and text) on <body
data-cells> and its status in the page title; that is how a test reads a
canvas.
"""
import base64, json, os, subprocess, sys, tempfile, time, urllib.request
import websocket

url, token, out = sys.argv[1], sys.argv[2], sys.argv[3]
chrome_bin = os.environ.get("CHROMIUM", "chromium")
port = int(os.environ.get("CDP_PORT", "9333"))
prof = tempfile.mkdtemp(prefix="improv-web-chrome.", dir=os.environ.get("SCRATCH"))
# A CI container runs as root, where Chromium needs its sandbox off.
root = ["--no-sandbox"] if os.geteuid() == 0 else []
chrome = subprocess.Popen(
    [chrome_bin, *root, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={prof}",
     "--no-first-run", "--no-default-browser-check", "--window-size=1280,800",
     "--remote-allow-origins=*", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
     "about:blank"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


class Tab:
    def __init__(self, ws_url, page=True):
        self.ws = websocket.create_connection(ws_url, timeout=60)
        self.seq = 0
        if page:
            self.cdp("Page.enable")
            self.cdp("Runtime.enable")

    def cdp(self, method, **params):
        self.seq += 1
        self.ws.send(json.dumps({"id": self.seq, "method": method, "params": params}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("method") == "Runtime.exceptionThrown":
                print("page exception:", json.dumps(m["params"])[:400], file=sys.stderr)
            if m.get("id") == self.seq:
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})

    def js(self, expr):
        return self.cdp("Runtime.evaluate", expression=expr, returnByValue=True)["result"].get("value")

    def title(self):
        return self.js("document.title") or ""

    def wait(self, pred, what, secs=60):
        t0 = time.time()
        while time.time() - t0 < secs:
            v = pred()
            if v:
                return v
            time.sleep(0.05)
        raise SystemExit(f"timed out waiting for {what}; title={self.title()!r}")

    def settled(self, pred, what, secs=60):
        """pred's answer once two reads 0.1 s apart agree: a window's first
        frame is laid out before it settles, so a widget's first position
        can be off by the window's final move."""
        last = self.wait(pred, what, secs)
        for _ in range(50):
            time.sleep(0.1)
            now = self.wait(pred, what, secs)
            if now == last:
                return now
            last = now
        return last

    def cells(self):
        return json.loads(self.js("document.body.dataset.cells || '[]'"))

    def cell(self, measure, at):
        return next((c for c in self.cells() if c["measure"] == measure and c["at"] == at), None)

    def mouse(self, kind, x, y, clicks=1, button="left", **kw):
        self.cdp("Input.dispatchMouseEvent", type=kind, x=x, y=y, button=button, clickCount=clicks, **kw)

    def click(self, x, y, clicks=1):
        self.mouse("mouseMoved", x, y, 0, button="none")
        for n in range(1, clicks + 1):
            self.mouse("mousePressed", x, y, n)
            self.mouse("mouseReleased", x, y, n)

    def drag(self, x0, y0, x1, y1, mid=None):
        self.mouse("mouseMoved", x0, y0, 0, button="none")
        time.sleep(0.1)
        self.mouse("mousePressed", x0, y0, buttons=1)
        time.sleep(0.1)
        steps = 20
        for k in range(1, steps + 1):
            self.mouse("mouseMoved", x0 + (x1 - x0) * k / steps, y0 + (y1 - y0) * k / steps, 0,
                       buttons=1)
            time.sleep(0.05)
        time.sleep(0.2)
        if mid:
            self.shot(mid)
        self.mouse("mouseReleased", x1, y1, buttons=0)
        time.sleep(0.2)

    def key(self, key, code=None, vk=0):
        for kind in ("keyDown", "keyUp"):
            self.cdp("Input.dispatchKeyEvent", type=kind, key=key, code=code or key,
                     windowsVirtualKeyCode=vk, nativeVirtualKeyCode=vk)

    def type(self, text):
        self.cdp("Input.insertText", text=text)

    def shot(self, path):
        png = self.cdp("Page.captureScreenshot", format="png")["data"]
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))


def browser():
    v = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version"))
    return Tab(v["webSocketDebuggerUrl"], page=False)


def new_tab(b):
    """A tab in a browser context of its own (its own session, as a second
    browser would be), never throttled as a background tab."""
    ctx = b.cdp("Target.createBrowserContext")["browserContextId"]
    tid = b.cdp("Target.createTarget", url="about:blank", browserContextId=ctx,
                background=False)["targetId"]
    t = Tab(f"ws://127.0.0.1:{port}/devtools/page/{tid}")
    # Headless tabs not in front stop drawing; keep both drawing.
    t.cdp("Emulation.setFocusEmulationEnabled", enabled=True)
    t.cdp("Page.setWebLifecycleState", state="active")
    return t


def login(tab):
    tab.cdp("Page.navigate", url=url)
    tab.wait(lambda: tab.title() in ("improv-web: log in", "improv-web: ready"), "the page", secs=120)
    if tab.title() == "improv-web: log in":
        time.sleep(0.3)
        tab.type(token)
        tab.key("Enter", vk=13)
    tab.wait(lambda: tab.title() == "improv-web: ready" and tab.cells(), "login and the first cells")


try:
    for _ in range(100):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version")
            break
        except Exception:
            time.sleep(0.2)
    br = browser()
    a, b = new_tab(br), new_tab(br)
    t0 = time.time()
    login(a)
    print(f"A logged in and drew its first cells in {time.time() - t0:.1f}s")
    login(b)
    Q, R = "Quantity", "Revenue"
    at = [[1, 10], [2, 20]]
    loaded = lambda t, m: (c := t.cell(m, at)) and c["text"] not in ("", "…") and c
    # The golden: B as it first draws the model (before any edit, the
    # pointer off the windows), once every window has its cells.
    b.wait(lambda: loaded(b, R) and loaded(b, Q) and b.cell("Price", [[2, 20]]), "B's cells")
    b.wait(lambda: b.js("document.body.dataset.live") == "true", "B's stream", secs=10)
    b.mouse("mouseMoved", 1270, 700, 0, button="none")
    time.sleep(1.0)
    b.shot(os.path.join(out, "web-golden.png"))
    price = float(a.wait(lambda: (c := a.cell("Price", [[2, 20]])) and c["text"] != "…" and c,
                         "Price[Widget]")["text"])
    q = a.wait(lambda: loaded(a, Q), "Quantity[2025Q1, Widget]")
    b.wait(lambda: loaded(b, R), "B's Revenue")
    for t in (a, b):
        t.wait(lambda: t.js("document.body.dataset.live") == "true", "the change stream", secs=10)
    print(f"before: Price={price} Quantity={q['text']} Revenue={b.cell(R, at)['text']}")

    # 2-3: an edit in A appears in B, recalculated, within a second.
    a.click(q["x"], q["y"], clicks=2)
    time.sleep(0.3)
    a.type("150")
    t0 = time.time()
    a.key("Enter", vk=13)
    want = f"{price * 150:g}"
    try:
        b.wait(lambda: (c := b.cell(R, at)) and c["text"] == want, f"B's Revenue = {want}", secs=5)
    except SystemExit:
        print("A:", a.cell(Q, at), a.cell(R, at), a.title())
        print("B:", b.cell(Q, at), b.cell(R, at), b.title(), b.js("document.body.dataset.live"))
        a.shot(os.path.join(out, "fail-a.png")); b.shot(os.path.join(out, "fail-b.png"))
        raise
    dt = time.time() - t0
    print(f"B saw Revenue = {want} {dt:.2f}s after A's Enter")
    assert b.cell(Q, at)["text"] == "150", b.cell(Q, at)
    assert dt < 1.0, f"took {dt:.2f}s"

    # 4: the stream drops mid-session (the server restarts, as a deploy or
    # a crash would), A edits while B is cut off: when B reconnects with
    # ?since= it catches up.
    restart = os.environ.get("RESTART")
    if restart:
        b_seen = b.cell(R, at)["text"]
        subprocess.run(["bash", "-c", restart], check=True)
        a.wait(lambda: "connecting" not in a.title(), "A back", secs=15)
        a.click(q["x"], q["y"], clicks=2)
        time.sleep(0.3)
        a.type("7")
        a.key("Enter", vk=13)
        t0 = time.time()
        a.wait(lambda: (c := a.cell(R, at)) and c["text"] == f"{price * 7:g}", "A's edit", secs=15)
        b.wait(lambda: (c := b.cell(R, at)) and c["text"] == f"{price * 7:g}", "B to catch up", secs=15)
        print(f"after a server restart B caught up from {b_seen} in {time.time() - t0:.1f}s")

    # 5: pivot Revenue in A: drag the Time tile from Rows onto Columns.
    tiles = json.loads(a.js("document.body.dataset.tiles || '[]'"))
    time_tile = next(t for t in tiles if t["measure"] == R and t["name"] == "Time")
    cols = next(t for t in json.loads(a.js("document.body.dataset.zones || '[]'"))
                if t["measure"] == R and t["axis"] == "Columns")
    print("drag", time_tile, "->", cols)
    a.shot(os.path.join(out, "before-drag.png"))
    a.drag(time_tile["x"], time_tile["y"], cols["x"], cols["y"], mid=os.path.join(out, "mid-drag.png"))
    a.shot(os.path.join(out, "after-drag.png"))
    a.wait(lambda: any(t["measure"] == R and t["name"] == "Time" and t["axis"] == "Columns"
                       for t in json.loads(a.js("document.body.dataset.tiles || '[]'"))),
           "Time on the columns")
    print("pivoted: Revenue's Time tile is on the columns")

    # 6: a formula with a misspelled name: the located error.
    line = json.loads(a.js("document.body.dataset.newformula || 'null'"))
    a.click(line["x"], line["y"])
    time.sleep(0.2)
    if os.environ.get("STEPS"):
        a.shot(os.path.join(out, "s1-clicked.png"))
    a.type("Tax = Revenu * 0.1")
    time.sleep(0.3)
    if os.environ.get("STEPS"):
        a.shot(os.path.join(out, "s2-typed.png"))
    a.key("Enter", vk=13)
    err = a.wait(lambda: json.loads(a.js("document.body.dataset.formulaerror || 'null'")),
                 "the formula error")
    print(f"formula error: {err['message']!r} at {err['span']}; hint {err.get('hint')!r}")
    assert err["span"] == [6, 12], err

    # 7: undo with Ctrl+Z (A's own last edit, the 7): both tabs show 150's
    # Revenue again.
    a.key("Escape", vk=27)
    a.click(1200, 650)  # the desk: no text edit has the keyboard
    time.sleep(0.2)
    for kind in ("keyDown", "keyUp"):
        a.cdp("Input.dispatchKeyEvent", type=kind, key="z", code="KeyZ",
              windowsVirtualKeyCode=90, modifiers=2)
    back = f"{price * 150:g}"
    t0 = time.time()
    b.wait(lambda: (c := b.cell(R, at)) and c["text"] == back, "B to see the undo", secs=5)
    print(f"undo in A: B shows Revenue = {back} again after {time.time() - t0:.2f}s")

    # 8: a third category, as another program adds it, pages Revenue's
    # Quantity: pick its second item and see that page's cells.
    if os.environ.get("ADD_REGION"):
        subprocess.run(["bash", "-c", os.environ["ADD_REGION"]], check=True)
        # Open it from the Model Browser (the status bar's Browser button,
        # as View > Model Browser).
        btn = json.loads(a.js("document.body.dataset.browserbutton || 'null'"))
        a.click(btn["x"], btn["y"])
        def region_tile():
            return any(t["name"] == "Region" for t in json.loads(
                a.js("document.body.dataset.tiles || '[]'")))
        # The browser window's first frames still lay out (its links move a
        # few pixels), and about one run in thirty a click there was lost:
        # click the settled link, and again (at most three times) when no
        # Sales worksheet shows within 3 s. The step tests that the browser
        # lists another program's measure and opens it, not click timing.
        for attempt in range(3):
            link = a.settled(lambda: next((l for l in json.loads(a.js("document.body.dataset.links || '[]'"))
                                           if l["name"] == "Sales"), None), "Sales in the browser")
            a.click(link["x"], link["y"])
            try:
                a.wait(region_tile, "the Region tile", secs=3)
                break
            except SystemExit:
                if attempt == 2:
                    raise
                print(f"the click on Sales opened nothing; again (attempt {attempt + 2})")
        pages = json.loads(a.js("document.body.dataset.pages || '[]'"))
        pick = next(p for p in pages if p["measure"] == "Sales")
        a.click(pick["x"], pick["y"])
        time.sleep(0.3)
        opt = json.loads(a.js("document.body.dataset.pageitems || '[]'"))
        south = next(o for o in opt if o["name"] == "South")
        a.click(south["x"], south["y"])
        a.wait(lambda: (c := a.cell("Sales", [[1, 10], [2, 20], [3, 31]])) and c["text"] == "9", "South's page")
        print("paged: Sales on Region = South shows its cells")

    time.sleep(0.5)
    a.shot(os.path.join(out, "web-a.png"))
    b.shot(os.path.join(out, "web-b.png"))
    print(f"screenshots in {out}")
finally:
    chrome.terminate()
    try:
        chrome.wait(timeout=10)
    except subprocess.TimeoutExpired:
        chrome.kill()
        chrome.wait()
    import shutil
    # Chromium's helpers may still be writing the profile for a moment.
    for _ in range(20):
        shutil.rmtree(prof, ignore_errors=True)
        if not os.path.exists(prof):
            break
        time.sleep(0.25)

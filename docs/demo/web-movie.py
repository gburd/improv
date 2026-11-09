#!/usr/bin/env python3
"""Record the web GUI movie's frames, in headless Chromium over the
DevTools protocol (as docs/demo/web-stills.py drives it).

    web-movie.py <url> <token> <frames-dir>

Two browser sessions on one improv-server. Each session's screencast
(Page.startScreencast) is saved as frames-dir/<a|b>-<n>.jpg with its time
in frames-dir/<a|b>.txt ("<n> <seconds>"), and the beats, each with the
session the movie shows and its caption, in frames-dir/beats.txt
("<seconds>\t<a|b|ab>\t<caption>"). docs/demo/web-movie.sh turns them
into docs/src/img/web-demo.{mp4,gif,png}.
"""
import base64, json, os, subprocess, sys, tempfile, threading, time, urllib.request
import websocket

url, token, out = sys.argv[1:4]
chrome_bin = os.environ.get("CHROMIUM", "chromium")
port = int(os.environ.get("CDP_PORT", "9335"))
W, H = 1280, 800
prof = tempfile.mkdtemp(prefix="improv-web-movie.", dir=os.environ.get("SCRATCH"))
root = ["--no-sandbox"] if os.geteuid() == 0 else []
chrome = subprocess.Popen(
    [chrome_bin, *root, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={prof}",
     "--no-first-run", "--no-default-browser-check", f"--window-size={W},{H}",
     "--remote-allow-origins=*", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
     "about:blank"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)
t0 = time.time()


class Tab:
    """One page. A reader thread takes every message: replies go to the
    caller waiting on their id, screencast frames to disk."""

    def __init__(self, ws_url, name=None):
        self.ws = websocket.create_connection(ws_url, timeout=60)
        self.seq, self.name, self.frames = 0, name, 0
        self.replies, self.cv = {}, threading.Condition()
        self.log = open(os.path.join(out, f"{name}.txt"), "w") if name else None
        threading.Thread(target=self.read, daemon=True).start()
        if name:
            self.cdp("Page.enable")
            self.cdp("Runtime.enable")

    def read(self):
        while True:
            try:
                m = json.loads(self.ws.recv())
            except Exception:
                return
            if m.get("method") == "Page.screencastFrame":
                p = m["params"]
                with open(os.path.join(out, f"{self.name}-{self.frames}.jpg"), "wb") as f:
                    f.write(base64.b64decode(p["data"]))
                self.log.write(f"{self.frames} {time.time() - t0:.3f}\n")
                self.log.flush()
                self.frames += 1
                self.send("Page.screencastFrameAck", sessionId=p["sessionId"])
            elif "id" in m:
                with self.cv:
                    self.replies[m["id"]] = m
                    self.cv.notify_all()

    def send(self, method, **params):
        with self.cv:
            self.seq += 1
            n = self.seq
        self.ws.send(json.dumps({"id": n, "method": method, "params": params}))
        return n

    def cdp(self, method, **params):
        n = self.send(method, **params)
        with self.cv:
            self.cv.wait_for(lambda: n in self.replies, timeout=60)
            m = self.replies.pop(n, None)
        if m is None:
            raise RuntimeError(f"{method}: no reply")
        if "error" in m:
            raise RuntimeError(f"{method}: {m['error']}")
        return m.get("result", {})

    def js(self, expr):
        return self.cdp("Runtime.evaluate", expression=expr, returnByValue=True)["result"].get("value")

    def data(self, key, default="[]"):
        return json.loads(self.js(f"document.body.dataset.{key} || '{default}'"))

    def title(self):
        return self.js("document.title") or ""

    def wait(self, pred, what, secs=60):
        t = time.time()
        while time.time() - t < secs:
            v = pred()
            if v:
                return v
            time.sleep(0.05)
        raise SystemExit(f"timed out waiting for {what}; title={self.title()!r}")

    def cell(self, measure, at):
        return next((c for c in self.data("cells") if c["measure"] == measure and c["at"] == at), None)

    def mouse(self, kind, x, y, clicks=1, button="left", **kw):
        self.cdp("Input.dispatchMouseEvent", type=kind, x=x, y=y, button=button, clickCount=clicks, **kw)

    def glide(self, x0, y0, x1, y1, steps=14, **kw):
        for k in range(1, steps + 1):
            self.mouse("mouseMoved", x0 + (x1 - x0) * k / steps, y0 + (y1 - y0) * k / steps, 0,
                       button="none" if not kw else "left", **kw)
            time.sleep(0.03)

    def click(self, x, y, clicks=1, start=None):
        if start:
            self.glide(*start, x, y)
        self.mouse("mouseMoved", x, y, 0, button="none")
        time.sleep(0.2)
        for n in range(1, clicks + 1):
            self.mouse("mousePressed", x, y, n)
            self.mouse("mouseReleased", x, y, n)

    def drag(self, x0, y0, x1, y1):
        self.mouse("mouseMoved", x0, y0, 0, button="none")
        time.sleep(0.3)
        self.mouse("mousePressed", x0, y0, buttons=1)
        time.sleep(0.2)
        self.glide(x0, y0, x1, y1, steps=24, buttons=1)
        time.sleep(0.4)
        self.mouse("mouseReleased", x1, y1, buttons=0)

    def key(self, key, vk=0, mods=0):
        for kind in ("keyDown", "keyUp"):
            self.cdp("Input.dispatchKeyEvent", type=kind, key=key, code=key, modifiers=mods,
                     windowsVirtualKeyCode=vk, nativeVirtualKeyCode=vk)

    def type(self, text, slow=0.0):
        if not slow:
            self.cdp("Input.insertText", text=text)
            return
        for ch in text:
            self.cdp("Input.insertText", text=ch)
            time.sleep(slow)

    def cast(self):
        self.cdp("Page.startScreencast", format="jpeg", quality=88, maxWidth=W, maxHeight=H,
                 everyNthFrame=1)


def new_tab(b, name):
    ctx = b.cdp("Target.createBrowserContext")["browserContextId"]
    tid = b.cdp("Target.createTarget", url="about:blank", browserContextId=ctx,
                background=False)["targetId"]
    t = Tab(f"ws://127.0.0.1:{port}/devtools/page/{tid}", name)
    t.cdp("Emulation.setFocusEmulationEnabled", enabled=True)
    t.cdp("Emulation.setDeviceMetricsOverride", width=W, height=H, deviceScaleFactor=1, mobile=False)
    t.cdp("Page.setWebLifecycleState", state="active")
    return t


beats = open(os.path.join(out, "beats.txt"), "w")


def beat(who, caption):
    beats.write(f"{time.time() - t0:.3f}\t{who}\t{caption}\n")
    beats.flush()


def login(tab, typed):
    tab.cdp("Page.navigate", url=url)
    tab.wait(lambda: tab.title() == "improv-web: log in", "the login panel", secs=120)
    time.sleep(0.6)
    tab.type(token, slow=0.06 if typed else 0.0)
    time.sleep(0.4)
    tab.key("Enter", vk=13)
    tab.wait(lambda: tab.title() == "improv-web: ready" and tab.data("cells"), "the first cells")
    tab.wait(lambda: tab.js("document.body.dataset.live") == "true", "the change stream", secs=10)


def loaded(t, m, at):
    c = t.cell(m, at)
    return c and c["text"] not in ("", "…") and c


try:
    for _ in range(100):
        try:
            v = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version"))
            break
        except Exception:
            time.sleep(0.2)
    br = Tab(v["webSocketDebuggerUrl"])
    a, b = new_tab(br, "a"), new_tab(br, "b")
    a.cast()
    b.cast()
    Q, R, at = "Quantity", "Revenue", [[1, 10], [2, 20]]
    beat("a", "improv-server serves the model; a browser logs in with its token")
    login(a, typed=True)
    login(b, typed=False)
    for t in (a, b):
        t.wait(lambda: loaded(t, R, at) and loaded(t, Q, at), "the cells")
        t.mouse("mouseMoved", 1260, 700, 0, button="none")
    beat("a", "The same NeXT-style worksheets as the desktop GUI; the server computes")
    time.sleep(3.5)

    beat("a", "Pivot: drag Revenue's Time tile from the rows onto the columns")
    tiles = a.data("tiles")
    tt = next(t for t in tiles if t["measure"] == R and t["name"] == "Time")
    cols = next(z for z in a.data("zones") if z["measure"] == R and z["axis"] == "Columns")
    a.glide(1260, 700, tt["x"], tt["y"], steps=20)
    a.drag(tt["x"], tt["y"], cols["x"], cols["y"])
    a.wait(lambda: any(t["measure"] == R and t["name"] == "Time" and t["axis"] == "Columns"
                       for t in a.data("tiles")), "Time on the columns")
    a.glide(cols["x"], cols["y"], 1260, 700)
    time.sleep(2.5)
    beat("a", "And back: no formula changes, only the layout")
    tt = next(t for t in a.data("tiles") if t["measure"] == R and t["name"] == "Time")
    rows = next(z for z in a.data("zones") if z["measure"] == R and z["axis"] == "Rows")
    a.drag(tt["x"], tt["y"], rows["x"], rows["y"])
    a.wait(lambda: any(t["measure"] == R and t["name"] == "Time" and t["axis"] == "Rows"
                       for t in a.data("tiles")), "Time on the rows")
    a.glide(rows["x"], rows["y"], 1260, 700)
    time.sleep(2.0)

    at2 = [[1, 11], [2, 20]]
    price = float(a.wait(lambda: loaded(a, "Price", [[2, 20]]), "Price[Widget]")["text"])
    q2 = a.wait(lambda: loaded(a, Q, at2), "A's Quantity[2025Q2, Widget]")
    for t in (a, b):
        t.wait(lambda: loaded(t, R, at2), "Revenue[2025Q2, Widget]")
    beat("ab", "Two browsers on one model. Left types 250 into 2025Q2 Widget")
    time.sleep(1.5)
    a.click(q2["x"], q2["y"], clicks=2, start=(1260, 700))
    time.sleep(0.5)
    a.type("250", slow=0.18)
    time.sleep(0.6)
    a.key("Enter", vk=13)
    want = f"{price * 250:g}"
    b.wait(lambda: (c := b.cell(R, at2)) and c["text"] == want, f"B's Revenue = {want}", secs=10)
    beat("ab", f"The right one gets the edit and the new Revenue ({want}) at once")
    a.glide(q2["x"], q2["y"], 1260, 700)
    time.sleep(4.0)

    beat("a", "A formula with a misspelled name: refused, the error located")
    line = a.data("newformula", "null")
    a.click(line["x"], line["y"], start=(1260, 700))
    time.sleep(0.3)
    a.type("Tax = Revenu * 0.1", slow=0.09)
    time.sleep(0.5)
    a.key("Enter", vk=13)
    a.wait(lambda: a.data("formulaerror", "null"), "the formula error")
    time.sleep(4.0)
    a.key("Escape", vk=27)
    a.click(1200, 650)
    time.sleep(0.3)

    beat("a", "Edit > Undo (Ctrl+Z) takes the 250 back, for every browser")
    a.key("z", vk=90, mods=2)
    back = f"{price * 250:g}"
    a.wait(lambda: (c := a.cell(R, at2)) and c["text"] != back, "the undo", secs=10)
    time.sleep(3.0)

    beat("a", "The Model Browser opens any measure in a window of its own")
    btn = a.data("browserbutton", "null")
    a.click(btn["x"], btn["y"], start=(1200, 650))
    a.wait(lambda: a.data("links"), "the model browser")
    time.sleep(1.2)
    a.mouse("mouseMoved", 1260, 700, 0, button="none")
    time.sleep(3.5)
    beat("end", "")
    for t in (a, b):
        t.cdp("Page.stopScreencast")
    time.sleep(0.5)
    print(f"web movie frames: a {a.frames}, b {b.frames}, {time.time() - t0:.1f}s")
finally:
    beats.close()
    chrome.terminate()
    try:
        chrome.wait(10)
    except subprocess.TimeoutExpired:
        chrome.kill()

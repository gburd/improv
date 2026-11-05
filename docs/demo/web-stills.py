#!/usr/bin/env python3
"""Take the web GUI's stills for docs/src/web-gui.md, in headless Chromium
over the DevTools protocol (as crates/gui_web/e2e/drive.py drives it).

    web-stills.py <url> <token> <out-dir>

Two browser sessions (two browser contexts, two logins) on one
improv-server:

  web-login.png    the login panel, the token half typed;
  web-worksheet.png  session A's worksheets after login;
  web-live-a.png, web-live-b.png  session A after it edits a Quantity
                   cell, and session B with the edit and the server's
                   recalculated Revenue, from the change stream;
  web-browser.png  A's model browser, opened from the status bar.
"""
import base64, json, os, subprocess, sys, tempfile, time, urllib.request
import websocket

url, token, out = sys.argv[1:4]
chrome_bin = os.environ.get("CHROMIUM", "chromium")
port = int(os.environ.get("CDP_PORT", "9334"))
prof = tempfile.mkdtemp(prefix="improv-web-stills.", dir=os.environ.get("SCRATCH"))
root = ["--no-sandbox"] if os.geteuid() == 0 else []
chrome = subprocess.Popen(
    [chrome_bin, *root, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={prof}",
     "--no-first-run", "--no-default-browser-check", "--window-size=1280,800",
     "--remote-allow-origins=*", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
     "about:blank"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)


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
            if m.get("id") == self.seq:
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
        t0 = time.time()
        while time.time() - t0 < secs:
            v = pred()
            if v:
                return v
            time.sleep(0.05)
        raise SystemExit(f"timed out waiting for {what}; title={self.title()!r}")

    def cell(self, measure, at):
        return next((c for c in self.data("cells") if c["measure"] == measure and c["at"] == at), None)

    def mouse(self, kind, x, y, clicks=1, button="left", **kw):
        self.cdp("Input.dispatchMouseEvent", type=kind, x=x, y=y, button=button, clickCount=clicks, **kw)

    def click(self, x, y, clicks=1):
        self.mouse("mouseMoved", x, y, 0, button="none")
        for n in range(1, clicks + 1):
            self.mouse("mousePressed", x, y, n)
            self.mouse("mouseReleased", x, y, n)

    def key(self, key, vk=0):
        for kind in ("keyDown", "keyUp"):
            self.cdp("Input.dispatchKeyEvent", type=kind, key=key, code=key,
                     windowsVirtualKeyCode=vk, nativeVirtualKeyCode=vk)

    def type(self, text):
        self.cdp("Input.insertText", text=text)

    def shot(self, name, clip=None):
        p = {"format": "png"}
        if clip:
            x, y, w, h = clip
            p["clip"] = {"x": x, "y": y, "width": w, "height": h, "scale": 1}
        png = self.cdp("Page.captureScreenshot", **p)["data"]
        with open(os.path.join(out, name), "wb") as f:
            f.write(base64.b64decode(png))


def new_tab(b):
    ctx = b.cdp("Target.createBrowserContext")["browserContextId"]
    tid = b.cdp("Target.createTarget", url="about:blank", browserContextId=ctx,
                background=False)["targetId"]
    t = Tab(f"ws://127.0.0.1:{port}/devtools/page/{tid}")
    t.cdp("Emulation.setFocusEmulationEnabled", enabled=True)
    t.cdp("Page.setWebLifecycleState", state="active")
    return t


def login(tab, shot=None):
    tab.cdp("Page.navigate", url=url)
    tab.wait(lambda: tab.title() == "improv-web: log in", "the login panel", secs=120)
    time.sleep(0.5)
    if shot:
        tab.type(token[:9])
        time.sleep(0.6)
        tab.shot(shot)
        tab.type(token[9:])
    else:
        tab.type(token)
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
    br = Tab(v["webSocketDebuggerUrl"], page=False)
    a, b = new_tab(br), new_tab(br)
    login(a, shot="web-login.png")
    login(b)
    Q, R, at = "Quantity", "Revenue", [[1, 10], [2, 20]]
    for t in (a, b):
        t.wait(lambda: loaded(t, R, at) and loaded(t, Q, at), "the cells")
        t.mouse("mouseMoved", 1270, 600, 0, button="none")
    time.sleep(1.0)
    a.shot("web-worksheet.png")

    # A edits Quantity[2025Q2, Widget]; B shows it and the server's
    # recalculated Revenue, through the change stream.
    at2 = [[1, 11], [2, 20]]
    price = float(a.wait(lambda: loaded(a, "Price", [[2, 20]]), "Price[Widget]")["text"])
    q2 = a.wait(lambda: loaded(a, Q, at2), "A's Quantity[2025Q2, Widget]")
    a.click(q2["x"], q2["y"], clicks=2)
    time.sleep(0.4)
    a.type("250")
    a.key("Enter", vk=13)
    want = f"{price * 250:g}"
    b.wait(lambda: (c := b.cell(R, at2)) and c["text"] == want, f"B's Revenue = {want}", secs=10)
    a.mouse("mouseMoved", 1270, 600, 0, button="none")
    time.sleep(1.0)
    a.shot("web-live-a.png")
    b.shot("web-live-b.png")

    btn = a.data("browserbutton", "null")
    a.click(btn["x"], btn["y"])
    a.wait(lambda: a.data("links"), "the model browser")
    time.sleep(1.5)
    a.mouse("mouseMoved", 1270, 600, 0, button="none")
    time.sleep(0.5)
    a.shot("web-browser.png")
    print("web stills taken")
finally:
    chrome.terminate()
    try:
        chrome.wait(10)
    except subprocess.TimeoutExpired:
        chrome.kill()

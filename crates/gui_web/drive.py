#!/usr/bin/env python3
"""Drive the web spike in headless Chromium over the DevTools protocol.

    drive.py <page-url> <screenshot.png> [--probe]

Waits for the worksheet to load, scrolls the Revenue window, double-clicks
Quantity[Widget, 2024Q1], types 150 and Enter, then waits until the server's
recalculated Revenue shows in the page, and screenshots it. The app publishes
its visible cells (position and text) on <body data-cells>.
"""
import base64, json, os, subprocess, sys, time, urllib.request
import websocket

url, shot = sys.argv[1], sys.argv[2]
probe = "--probe" in sys.argv
port = 9333
prof = os.path.expanduser("~/wd-chrome-profile")
chrome = subprocess.Popen(
    ["chromium", "--headless=new", f"--remote-debugging-port={port}",
     f"--user-data-dir={prof}", "--no-first-run", "--window-size=1280,720", "--remote-allow-origins=*",
     "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "about:blank"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(100):
        try:
            targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json"))
            page = next(t for t in targets if t["type"] == "page")
            break
        except Exception:
            time.sleep(0.2)
    ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=30)
    seq = [0]

    def cdp(method, **params):
        seq[0] += 1
        ws.send(json.dumps({"id": seq[0], "method": method, "params": params}))
        while True:
            m = json.loads(ws.recv())
            if m.get("id") == seq[0]:
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})

    def js(expr):
        return cdp("Runtime.evaluate", expression=expr, returnByValue=True)["result"].get("value")

    def wait(pred, what, secs=60):
        t0 = time.time()
        while time.time() - t0 < secs:
            v = pred()
            if v:
                return v
            time.sleep(0.25)
        raise SystemExit(f"timed out waiting for {what}; title={js('document.title')!r}")

    cdp("Page.enable")
    cdp("Runtime.enable")
    t0 = time.time()
    cdp("Page.navigate", url=url + ("?probe" if probe else ""))
    if probe:
        title = wait(lambda: (js("document.title") or "").startswith("probe") and js("document.title"),
                     "the probe")
        print(title)
        sys.exit(0 if title.startswith("probe ok") else 1)
    wait(lambda: js("document.title") == "improv-web: ready", "ready")
    print(f"loaded in {time.time() - t0:.1f}s")

    def cells():
        return json.loads(js("document.body.dataset.cells || '[]'"))

    def cell(measure, key):
        return next((c for c in cells() if c["measure"] == measure and c["key"] == key), None)

    def mouse(kind, x, y, clicks=1, **kw):
        cdp("Input.dispatchMouseEvent", type=kind, x=x, y=y, button="left",
            clickCount=clicks, **kw)

    # Scroll: Revenue shows 5 of its 8 rows; the wheel brings 2025Q4 into view.
    rev = cell("Revenue", ["Widget", "2024Q1"])
    assert rev and cell("Revenue", ["Widget", "2025Q4"]) is None, "2025Q4 starts hidden"
    mouse("mouseMoved", rev["x"], rev["y"], 0)  # egui scrolls the area under the pointer
    time.sleep(0.2)
    for _ in range(4):
        cdp("Input.dispatchMouseEvent", type="mouseWheel", x=rev["x"], y=rev["y"], deltaX=0, deltaY=100)
        time.sleep(0.1)
    last = wait(lambda: cell("Revenue", ["Widget", "2025Q4"]), "the scroll", 10)
    print(f"scrolled: Revenue[Widget, 2025Q4] = {last['text']}")
    for _ in range(4):
        cdp("Input.dispatchMouseEvent", type="mouseWheel", x=rev["x"], y=rev["y"], deltaX=0, deltaY=-100)
        time.sleep(0.1)
    wait(lambda: cell("Revenue", ["Widget", "2024Q1"]), "the scroll back")

    price = float(cell("Price", ["Widget"])["text"])
    q = cell("Quantity", ["Widget", "2024Q1"])
    before = cell("Revenue", ["Widget", "2024Q1"])["text"]
    print(f"before: Price={price} Quantity={q['text']} Revenue={before}")
    for n in (1, 2):
        mouse("mousePressed", q["x"], q["y"], n)
        mouse("mouseReleased", q["x"], q["y"], n)
    time.sleep(0.5)
    cdp("Input.insertText", text="150")
    time.sleep(0.3)
    for kind in ("keyDown", "keyUp"):
        cdp("Input.dispatchKeyEvent", type=kind, key="Enter", code="Enter",
            windowsVirtualKeyCode=13, nativeVirtualKeyCode=13)
    want = price * 150
    after = wait(lambda: (c := cell("Revenue", ["Widget", "2024Q1"])) and
                 c["text"] != before and c, "the recalculated Revenue")
    print(f"after:  Quantity={cell('Quantity', ['Widget', '2024Q1'])['text']} "
          f"Revenue={after['text']} (want {want:g}); title={js('document.title')!r}")
    assert float(after["text"]) == want, "Revenue = Price x the new Quantity"
    time.sleep(0.5)
    png = cdp("Page.captureScreenshot", format="png")["data"]
    with open(shot, "wb") as f:
        f.write(base64.b64decode(png))
    print(f"screenshot: {shot}")
finally:
    chrome.terminate()
    chrome.wait()

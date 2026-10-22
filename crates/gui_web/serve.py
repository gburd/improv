#!/usr/bin/env python3
"""Spike host: serve web/ (index.html + the wasm-bindgen output) and forward
/api/* to improv-server, so the page and the API share one origin.

    serve.py <listen-port> <improv-server-url>

shortcut: stands in for improv-server serving the static files itself
(I2), which also removes the need for CORS.
"""
import http.server, os, sys, urllib.request, urllib.error

PORT, UPSTREAM = int(sys.argv[1]), sys.argv[2].rstrip("/")
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "web")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=ROOT, **k)

    def forward(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n) if n else None
        req = urllib.request.Request(UPSTREAM + self.path[len("/api"):], data=body, method=self.command)
        for h in ("content-type", "authorization"):
            if self.headers.get(h):
                req.add_header(h, self.headers[h])
        try:
            with urllib.request.urlopen(req) as r:
                status, data, ctype = r.status, r.read(), r.headers.get("content-type", "")
        except urllib.error.HTTPError as e:
            status, data, ctype = e.code, e.read(), e.headers.get("content-type", "")
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/api/"):
            return self.forward()
        return super().do_GET()

    def do_POST(self):
        if self.path.startswith("/api/"):
            return self.forward()
        self.send_error(405)

    def log_message(self, *a):
        pass


http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

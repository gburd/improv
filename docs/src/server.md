# The HTTP Server

`improv-server` serves a JSON HTTP API over one model store:

```sh
improv-server model.db 127.0.0.1:3000   # or IMPROV_ADDR=...; model defaults to improv.db
improv-server 'postgres://me@127.0.0.1/sales?sslmode=disable&model=plan'
```

## Endpoints

The API is versioned: every route is served under `/v1`, the path the 1.x
releases keep stable. The same routes without the prefix are aliases kept
for older clients. [`docs/openapi.json`](https://codeberg.org/gregburd/improv/src/branch/main/docs/openapi.json)
is the OpenAPI 3.1 description, and the server serves it at
`/v1/openapi.json`. A test fails when the document and the routes disagree.

| Method | Path | Scope | Body | Notes |
| --- | --- | --- | --- | --- |
| GET | `/v1/health` | public | | Liveness probe. |
| GET | `/v1/openapi.json` | public | | The OpenAPI document. |
| GET | `/v1/model` | read | | The whole model. |
| GET | `/v1/measures` | read | | Id, name, kind, categories. |
| GET | `/v1/measures/:id/values` | read | | Stored inputs, or computed values. |
| POST | `/v1/measures/:id/eval` | read; write to persist | `{"edits": [{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 90}], "persist": false}` | What-if edits through the live engine; `"persist": true` writes them. Ids are numeric; `"value": null` clears a cell. The answer names coordinates by category and item name. |
| POST | `/v1/measures/:id/cells` | write | `{"coord": {"dims": {"1": 10, "2": 20}}, "value": 90}` | Set one input cell (category id → item id). |
| POST | `/v1/nl/parse` | read | `{"text": "revenue is price times quantity"}` | Controlled English to a formula. |
| POST | `/v1/nl/describe` | read | `{"formula": …}` | A formula (as `/nl/parse` returns it) to controlled English. |
| GET | `/v1/scheduler/status` | read | | Refresh scheduler state. |

Values written through the API are numbers.

## Authentication

Once any token is configured, every route except the public ones needs
`Authorization: Bearer <secret>`. Each token has a name and a scope: a
**read** token may call the read routes, a **write** token every route. A
missing or unknown token gets **401**; a read token on a write route (or
`"persist": true`) gets **403**. Secrets are compared in constant time and
never logged; the audit log and the rate limiter use the token's name.

| Variable | Meaning |
| --- | --- |
| `IMPROV_API_TOKEN` | One write token, named `IMPROV_API_TOKEN`. |
| `IMPROV_API_TOKENS` | Write tokens, comma-separated, named `IMPROV_API_TOKENS#1`, `#2`, … |
| `IMPROV_API_TOKENS_RO` | Read tokens, comma-separated, named `IMPROV_API_TOKENS_RO#1`, … |
| `IMPROV_API_TOKEN_FILE` | A file of tokens, so secrets stay out of the environment (below). |
| `IMPROV_ALLOW_OPEN=1` | Allow open mode (no tokens) on a non-loopback address. Without it the server refuses to start. |

The token file has one `name:scope:secret` per line, `scope` being `read`
or `write`; the secret may contain `:`. Blank lines and `#` comments are
skipped:

```text
# /etc/improv/tokens, mode 600
dashboard:read:9f2c…
loader:write:b71e…
```

The server refuses to start if the file is unreadable or malformed, or if two
tokens share a name or a secret, and warns if the file is readable by other
users. All the sources combine. Generate secrets with
`openssl rand -hex 32`.

In open mode (no tokens) every caller has write access and the audit log
names it `anonymous`.

## Writes

A write must name an existing **input** measure and a coordinate with exactly
one item for each of the measure's categories; values must be finite. Anything
else is a 400. Writes are serialized, so concurrent writers do not lose
updates. The server caches the loaded model, and before every request it
checks the store's high-water mark (one cheap query: Mentat's newest
transaction, or a Postgres model's `improv.model_version`). When another
process (the GUI, the CLI, SQL on a Postgres model) has saved since, it
reloads, so the next read shows that change and the next write builds on
it. A write that races such a save (it lands between the server's load
and its save) is refused with **409 Conflict**, writing nothing, and the
cache is dropped, so a retry reloads and succeeds.

## Limits

| Limit | Default | Variable |
| --- | --- | --- |
| Request body | 1 MiB (413 above) | `IMPROV_MAX_BODY_BYTES` |
| Requests a minute, per caller | 600 (429 above, with `Retry-After`) | `IMPROV_RATE_LIMIT` (0 turns it off) |
| Edits per `/eval` | 10,000 (400 above) | |
| Request time | 30 s (408), including time queued for a slot | |
| Requests in flight | 64; the rest queue | |
| Header read | 10 s | `IMPROV_HEADER_TIMEOUT_MS` |

The rate limit is a token bucket per token name. A request without a valid
token (and every request in open mode) is counted against the client's IP
address instead. Behind a reverse proxy every client has the proxy's
address, so in open mode set the limit in the proxy. Public routes are not
limited.

The server speaks HTTP/1.1 only.

Internal errors return `{"error": "internal error", "request_id": N}`; the
detail is logged to stderr under the same id.

## Audit log

Every write is logged as one JSON object per line: `POST /cells`, and
`/eval` with `"persist": true`. Refused writes are logged too, with the
status they got (401, 403, 400, 409, 408, …):

```json
{"ts":"2026-10-22T09:14:03.512Z","audit":"write","who":"loader","route":"POST /v1/measures/100/cells","model":"postgres://me@db/sales?model=plan","status":200}
```

`who` is the token's name (`unauthenticated` for a write without a valid
token, `anonymous` in open mode); `model` is the model path or URL with any
password removed. The log goes to stderr, or is appended to the file named
by `IMPROV_AUDIT_LOG`. Scheduler refreshes are not API writes and are logged
as plain lines.

## Deployment

The server stops on SIGTERM or Ctrl-C, at once: `docker stop`, `systemctl
stop` and a terminal's Ctrl-C need no kill timeout. A request still running
is cut off; a write in it is one store transaction, so it is saved whole or
not at all.

`improv-server` speaks plain HTTP; it has no TLS of its own. To serve it
beyond the machine, bind it to loopback and put a reverse proxy in front
that terminates TLS:

```sh
IMPROV_API_TOKEN_FILE=/etc/improv/tokens IMPROV_AUDIT_LOG=/var/log/improv/audit.log \
  improv-server /srv/improv/sales.db 127.0.0.1:3000
```

Caddy (it fetches and renews the certificate itself):

```text
improv.example.com {
	reverse_proxy 127.0.0.1:3000
	request_body {
		max_size 1MB
	}
}
```

nginx:

```nginx
server {
    listen 443 ssl;
    server_name improv.example.com;
    ssl_certificate     /etc/letsencrypt/live/improv.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/improv.example.com/privkey.pem;
    client_max_body_size 1m;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_read_timeout 35s;
    }
}
```

Keep the proxy's body limit at or below `IMPROV_MAX_BODY_BYTES`, and its read
timeout above the server's 30 s request time. Bearer tokens travel in a
header, so never expose the plain-HTTP port itself: the server refuses to
listen on a non-loopback address without tokens (unless `IMPROV_ALLOW_OPEN=1`),
but with tokens set it will, and the tokens would then cross the network in
clear text.

## A web front end

Improv has no web GUI for 1.0 ([the decision](https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-22-web-gui.md)),
but the server has what one needs, for anyone building on the API.

**Static files.** `--static DIR` serves the files under `DIR` at `/`,
beside the API: `/` and a directory are their `index.html`, `.wasm` is
`application/wasm`, and a path outside `DIR` (`..`, a symlink out) is a
404. The files are public, like `/health`; the API calls the page makes
still need their tokens. Same-origin, the page needs no CORS.

```sh
IMPROV_API_TOKEN_FILE=/etc/improv/tokens improv-server sales.db 127.0.0.1:3000 --static ./web/dist
```

**CORS** is off by default: without it a browser lets only same-origin
pages call the API. To serve the page from elsewhere (a development
server, another host), list its origins, matched exactly:

```sh
IMPROV_CORS_ORIGINS=http://localhost:8080,https://ui.example.com improv-server sales.db
```

An allowed origin's preflight (`OPTIONS`) gets 204 with `GET, POST` and
the `authorization` and `content-type` headers; its responses carry
`Access-Control-Allow-Origin`. Other origins get no CORS headers, so the
browser blocks them. `*` is refused, because a page from any site could
then use a visitor's token.

**Seeing other writers.** There is no push feed yet. Each request checks
the store's high-water mark and reloads when another writer saved (see
[Writes](#writes)), so a page that polls sees their change on its next
request: poll `GET /v1/measures` or the values it shows, every few
seconds, and redraw what changed. A save that races another writer's is
refused with 409; reload and retry. A change feed (`GET
/v1/changes?since=…` or Server-Sent Events, carrying the high-water) is
planned for the web GUI after 1.0, with per-user undo and presence (plan
rows I2, I3).

## Refresh scheduler

The scheduler refreshes `CALL` (external-function) measures in the
background. Refreshing runs code stored in the model, so it is **off by
default**.

| Variable | Meaning |
| --- | --- |
| `IMPROV_SCHEDULER=1` | Enable the scheduler. |
| `IMPROV_EXTFN_ALLOW` | Path to the allowlist file. |
| `IMPROV_SCHEDULER_TICK_SECS` | Tick period (default 5). |

The scheduler runs a function only if its digest is listed in the allowlist
file, one lowercase hex digest per line (`#` comments allowed). Other
functions are skipped, and a warning is logged. The digest is the SHA-256 of
the function name, a NUL byte and the body:

```sh
printf '%s\0%s' double 'result = args[0] * 2' | sha256sum
```

Scheduler refreshes always run in the `Restricted` sandbox, whatever the
function's `pure` flag says.

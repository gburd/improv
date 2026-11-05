# The HTTP Server

`improv-server` serves a JSON HTTP API over one model store:

```sh
improv-server model.db 127.0.0.1:3000   # or IMPROV_ADDR=...; model defaults to improv.db
improv-server 'postgres://me@127.0.0.1/sales?sslmode=disable&model=plan'
```

<video src="./img/server-demo.mp4" controls muted loop playsinline width="100%"
  poster="./img/server-demo.png">
  <img src="./img/server-demo.gif" alt="improv-server: a request refused without a token, the structure, a window of cells, a background change stream, and a cell edit the stream delivers">
</video>

*improv-server on the sample model: a request without the token refused,
`/v1/structure`, a window of Revenue cells, then a cell edit that the
`/v1/changes` stream in the background delivers with every cell it moved,
and the audit log.*

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
| GET | `/v1/model` | read | | The whole model. Refused (403) for a model with [access policies](./policies.md). |
| GET | `/v1/measures` | read | | Id, name, kind, categories. |
| GET | `/v1/measures/:id/values` | read | | Stored inputs, or computed values. |
| POST | `/v1/measures/:id/eval` | read; write to persist | `{"edits": [{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 90}], "persist": false}` | What-if edits through the live engine; `"persist": true` writes them. Ids are numeric; `"value": null` clears a cell. The answer names coordinates by category and item name. |
| POST | `/v1/measures/:id/cells` | write | `{"coord": {"dims": {"1": 10, "2": 20}}, "value": 90}` | Set one input cell (category id → item id). |
| POST | `/v1/nl/parse` | read | `{"text": "revenue is price times quantity"}` | Controlled English to a formula. |
| POST | `/v1/nl/describe` | read | `{"formula": …}` | A formula (as `/nl/parse` returns it) to controlled English. |
| GET | `/v1/scheduler/status` | read | | Refresh scheduler state. |

Values written through these routes are numbers. The routes below are
what a GUI drives ([The Web GUI](./web-gui.md) uses them); they take and
return typed values, and answer with only what the caller may see.

| Method | Path | Scope | Body | Notes |
| --- | --- | --- | --- | --- |
| GET | `/v1/capabilities` | read | | What the caller may do: the model-wide actions, each visible measure's `define` and `cells` (`all`, `some`, `none`), and the items it may rename or delete. Hints for a GUI; every request is still checked. |
| GET | `/v1/structure` | read | | Categories with their items in grid order, measures (with formulas as typed), LAMBDAs, views, and the `version`. The stable shape, in place of `/v1/model`. |
| POST | `/v1/measures/:id/window` | read | `{"at": {"1": [10, 11]}}` | The cells at these items (per category id; a category left out: all its items), computed by the server. |
| POST | `/v1/cells` | write | `{"edits": [{"measure": 101, "at": [[1, 10], [2, 20]], "value": 150}]}` | Set or clear (`"value": null`) cells, all or none. A refusal says which edit (`index`). |
| POST | `/v1/measures` | write | `{"definition": "Tax = Revenue * 0.1"}` or `{"name": "Notes", "categories": [2], "type": "Text"}` | Define (or redefine) a derived measure or a LAMBDA, or add an input measure. |
| PUT | `/v1/measures/:id` | write | `{"formula": "Revenue * 0.2", "name": "Duty", "format": {"decimals": 2}}` | Any of them, as one step. |
| DELETE | `/v1/measures/:id` | write | | Refused while a formula reads it. |
| POST | `/v1/categories` | write | `{"name": "Region"}` | |
| PUT, DELETE | `/v1/categories/:id` | write | `{"name": …}` | Rename; delete (with its items; refused while a measure uses it). |
| POST | `/v1/categories/:id/items` | write | `{"name": "North"}` | |
| PUT, DELETE | `/v1/items/:id` | write | `{"name": …}` | Rename; delete (with its cells). |
| POST | `/v1/views` | write | `{"name": "By product", "matrices": [{"measure": 102, "axis_order": [2, 1]}]}` | Save a view. |
| PUT, DELETE | `/v1/views/:id` | write | `{"name": …, "matrices": …}` | |
| POST | `/v1/undo`, `/v1/redo` | write | | The session's last change. |
| GET | `/v1/changes` | read | | Server-Sent Events: what changed (see below). |

A value is a JSON number, boolean or string, or `{"date": "2025-01-31T00:00:00+00:00"}`
or `{"error": "#DIV/0!"}` (a computed error). A coordinate (`at`) is
`[category id, item id]` pairs. A refused request gets
`{"error": …, "index": n, "diagnostic": {"span": [6, 12], "message": …, "hint": …}}`:
`index` names the edit of a batch that failed, and `diagnostic` places a
formula error in the text sent (byte offsets), as the desktop's formula
editor underlines it.

**Versions** (`"version"` in every answer and event) are opaque strings:
compare them for equality only. A cell edit never conflicts: when another
writer saved first, the server reloads, checks the edits again, and
applies them. A structural change that races another writer's gets 409.

**Undo** is per session: send `x-improv-session: <any id>` (one per
window, up to 64 characters) with each write; without it a token's writes
share one stack. Undo writes back the cells an edit replaced (each checked
as a new edit), or restores the structure and the cells a structural
change dropped.

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

## Access policies

A model with [access policies](./policies.md) (`improv policy set`) is
served under them, to every route. The caller is the Cedar user named by
its token (`dashboard`, `loader`, …; `anonymous` in open mode), with no
roles; groups come from the model's policy document. A token's scope
stays an outer limit: a read token never writes, whatever the policies
allow.

- **Hidden is absent.** A measure, item or cell a caller may not read is
  missing from every answer: lists, cells, windows, what-if results,
  `/nl/parse`'s names, the change stream, `/v1/scheduler/status`. A
  route about a hidden measure answers exactly as for a measure that
  does not exist (the same status and message). An edit of a cell the
  caller may not edit answers as an edit of a cell that does not exist.
- **Totals are the model's.** Derived values are computed over all the
  data, hidden cells included, and then filtered. See
  [Totals and what they reveal](./policies.md#totals-and-what-they-reveal).
- **`/v1/model` is refused** (403) for a model with policies, to every
  caller: it is the whole model, unfiltered. Read the model through
  `/v1/structure` and the measure routes.
- **Changes take effect at once.** The server reads the policies'
  version on every request, so a `set`, a `clear` or a change of groups
  is seen by the next request, with no restart. Policies that no longer
  validate refuse every request.
- **Refusals are audited** like any write: who, the route and the status
  (see [Audit log](#audit-log)).
- **The scheduler's errors are generic.** Under policies,
  `/v1/scheduler/status` reports a failed refresh as "a refresh failed;
  the server's log says why", since the error may name a hidden measure;
  the server's log has the full text.

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
| Change streams open (`/v1/changes`) | 256 in all, 8 per caller (429 above) | `IMPROV_MAX_STREAMS` (the total) |

The rate limit is a token bucket per token name. A request without a valid
token (and every request in open mode) is counted against the client's IP
address instead. Behind a reverse proxy every client has the proxy's
address, so in open mode set the limit in the proxy. Public routes are not
limited.

A change stream is long-lived: it is not bound by the request time and
takes no in-flight slot, so open streams never hold up other requests.
The stream caps bound them instead, counted per principal (a token's
name). However many are open, the server reads the store's high-water
mark for other writers' changes once every 0.5 s, from one task, and
wakes the streams when it moves.

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

**In a container.** Each release from 0.12 publishes
`ghcr.io/gburd/improv:<version>` (and `:latest`, amd64): `improv-server` and
the `improv` CLI on Debian trixie-slim, running as uid 10001 in `/data`,
listening on port 3000. It refuses to start without a token:

```sh
podman run -d -p 127.0.0.1:3000:3000 -v "$PWD:/data" \
  -e IMPROV_API_TOKEN_FILE=/data/tokens ghcr.io/gburd/improv /data/sales.db
podman run --rm -v "$PWD:/data" --entrypoint improv ghcr.io/gburd/improv list /data/sales.db
```

The directory must be writable by uid 10001. Build it yourself with
`podman build -f Containerfile .` from the repository.

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

The web GUI ([The Web GUI](./web-gui.md)) is served by `--static`, and
the routes above are what it drives. They are there for anyone building
another front end too.

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

An allowed origin's preflight (`OPTIONS`) gets 204 with `GET, POST, PUT,
DELETE` and the `authorization`, `content-type`, `x-improv-session` and
`last-event-id` headers; its responses carry
`Access-Control-Allow-Origin`. Other origins get no CORS headers, so the
browser blocks them. `*` is refused, because a page from any site could
then use a visitor's token.

**Seeing changes.** `GET /v1/changes` is a Server-Sent Events stream.
Its first event is the current version, `{"version": "…"}`; after that,
one event per change the caller may see, carrying the changed cells
(inputs and the computed cells they moved) as the server computed them:

```text
id: 268435470
data: {"version":"268435470","cells":[{"measure":101,"at":[[1,10],[2,20]],"value":150},{"measure":102,"at":[[1,10],[2,20]],"value":1800}]}
```

A structural change, or another program's save, sends
`{"version": "…", "reload": true}`: re-read the structure and what is on
screen. A change the caller may see none of sends nothing. To resume
after a dropped connection, reconnect with `?since=<the last version>`
(or `Last-Event-ID`): the server sends what changed since, or a reload
when it no longer has the gap. Each request also checks the store's
high-water mark (see [Writes](#writes)), so a client without the stream
sees other writers on its next request. A browser's `EventSource` cannot
send the `Authorization` header: read the stream with `fetch` instead,
as the web GUI does.

**Access.** Every GUI route asks the model's access policies, for each
measure, item and cell it touches, within what the token's scope allows
(see [Access policies](#access-policies)). A hidden measure, item or cell
is absent from every answer, and is refused exactly as one that does not
exist.

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

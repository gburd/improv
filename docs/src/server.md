# The HTTP Server

`improv-server` serves a JSON HTTP API over one model store:

```sh
improv-server model.db 127.0.0.1:3000   # or IMPROV_ADDR=...
```

## Endpoints

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/health` | Public liveness probe. |
| GET | `/model` | The whole model. |
| GET | `/measures` | Id, name, kind, categories. |
| GET | `/measures/:id/values` | Stored inputs, or computed values. |
| POST | `/measures/:id/eval` | What-if edits through the live engine; `"persist": true` writes through. |
| POST | `/measures/:id/cells` | Set one input cell. |
| POST | `/nl/parse`, `/nl/describe` | CNL to formula and back. |
| GET | `/scheduler/status` | Refresh scheduler state. |

## Authentication

Every route except `/health` needs `Authorization: Bearer <token>` once a
token is configured. Tokens are compared in constant time.

| Variable | Meaning |
| --- | --- |
| `IMPROV_API_TOKEN`, `IMPROV_API_TOKENS` | Read-write tokens (one, or comma-separated). |
| `IMPROV_API_TOKENS_RO` | Read-only tokens (comma-separated). GETs and what-if `/eval` work; `POST /cells` and `"persist": true` return 403. |
| `IMPROV_ALLOW_OPEN=1` | Allow open mode (no tokens) on a non-loopback address. Without it the server refuses to start. |

## Writes

A write must name an existing **input** measure and a coordinate with exactly
one item for each of the measure's categories; values must be finite. Anything
else is a 400. Writes are serialized, so concurrent writers do not lose
updates. The server caches the loaded model and replaces the cache on each
write, so it assumes it is the store's only writer: changes made by another
process show up after a restart.

## Limits

| Limit | Value |
| --- | --- |
| Request body | 1 MiB (413 above) |
| Edits per `/eval` | 10,000 (400 above) |
| Request time | 30 s (408), including time queued for a slot |
| Requests in flight | 64; the rest queue |
| Header read | 10 s; `IMPROV_HEADER_TIMEOUT_MS` overrides |

The server speaks HTTP/1.1 only.

Internal errors return `{"error": "internal error", "request_id": N}`; the
detail is logged to stderr under the same id.

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

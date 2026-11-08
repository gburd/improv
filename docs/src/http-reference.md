# HTTP API reference

<!-- Generated from docs/openapi.json by `UPDATE_DOCS=1 cargo test -p improv_cli`. Do not edit. -->

JSON HTTP API over one Improv model. Every path is also served without the /v1 prefix (an alias kept for older clients). `x-improv-scope` names the token scope a route needs: public (no token), read, or write. A missing or unknown token gets 401, a read token on a write route 403, an over-limit body 413, a caller over its rate limit 429. With --tls-cert and --tls-key the server speaks HTTPS only; off loopback, a server with tokens refuses to start without TLS unless IMPROV_ALLOW_PLAIN=1.

The server serves this document at `/v1/openapi.json`. Point any
OpenAPI viewer at that URL, or at
[`docs/openapi.json`](https://codeberg.org/gregburd/improv/src/branch/main/docs/openapi.json), for an interactive
page. [The HTTP Server](./server.md) covers running, tokens,
TLS and limits.

| Method | Path | Scope | Summary |
| --- | --- | --- | --- |
| GET | [`/v1/capabilities`](#get-v1capabilities) | read | What the caller may do |
| POST | [`/v1/categories`](#post-v1categories) | write | Add a category |
| DELETE | [`/v1/categories/{id}`](#delete-v1categoriesid) | write | Delete a category |
| PUT | [`/v1/categories/{id}`](#put-v1categoriesid) | write | Rename a category |
| POST | [`/v1/categories/{id}/items`](#post-v1categoriesiditems) | write | Add an item |
| POST | [`/v1/cells`](#post-v1cells) | write | Edit cells |
| GET | [`/v1/changes`](#get-v1changes) | read | Changes, as Server-Sent Events |
| GET | [`/v1/health`](#get-v1health) | public | Liveness probe |
| DELETE | [`/v1/items/{id}`](#delete-v1itemsid) | write | Delete an item |
| PUT | [`/v1/items/{id}`](#put-v1itemsid) | write | Rename an item |
| GET | [`/v1/measures`](#get-v1measures) | read | List the measures |
| POST | [`/v1/measures`](#post-v1measures) | write | Define a measure |
| DELETE | [`/v1/measures/{id}`](#delete-v1measuresid) | write | Delete a measure |
| PUT | [`/v1/measures/{id}`](#put-v1measuresid) | write | Change a measure |
| POST | [`/v1/measures/{id}/cells`](#post-v1measuresidcells) | write | Set one input cell |
| POST | [`/v1/measures/{id}/eval`](#post-v1measuresideval) | read | What-if evaluation |
| GET | [`/v1/measures/{id}/values`](#get-v1measuresidvalues) | read | A measure's values |
| POST | [`/v1/measures/{id}/window`](#post-v1measuresidwindow) | read | A window of a measure's cells |
| GET | [`/v1/model`](#get-v1model) | read | The whole model |
| POST | [`/v1/nl/describe`](#post-v1nldescribe) | read | A formula to controlled English |
| POST | [`/v1/nl/parse`](#post-v1nlparse) | read | Controlled English to a formula |
| GET | [`/v1/openapi.json`](#get-v1openapijson) | public | This document |
| POST | [`/v1/redo`](#post-v1redo) | write | Redo |
| GET | [`/v1/scheduler/status`](#get-v1schedulerstatus) | read | Refresh scheduler state |
| GET | [`/v1/structure`](#get-v1structure) | read | What the caller may see of the model |
| POST | [`/v1/undo`](#post-v1undo) | write | Undo |
| POST | [`/v1/views`](#post-v1views) | write | Save a view |
| DELETE | [`/v1/views/{id}`](#delete-v1viewsid) | write | Delete a view |
| PUT | [`/v1/views/{id}`](#put-v1viewsid) | write | Change a view |

## GET /v1/capabilities

**What the caller may do** Scope: `read`.

Hints for a GUI to grey out what the caller cannot do, from the model's access policies: the model-wide actions, for each visible measure whether it may be redefined and which of its cells may be edited, and the visible items that may be renamed or deleted. Every request is still checked. Without policies, everything the token's scope allows.

| Status | Response |
| --- | --- |
| 200 | The caller's capabilities. Body: [Capabilities](#capabilities). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/categories

**Add a category** Scope: `write`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [Name](#name).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## DELETE /v1/categories/{id}

**Delete a category** Scope: `write`.

With its items. Refused while a measure uses it.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## PUT /v1/categories/{id}

**Rename a category** Scope: `write`.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [Name](#name).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/categories/{id}/items

**Add an item** Scope: `write`.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [Name](#name).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/cells

**Edit cells** Scope: `write`.

Set or clear input cells, all or none. A refusal names the failing edit (`index`); a hidden cell is refused as one that does not exist. Never a conflict: when another writer moved the model, the server reloads, checks the batch again and applies it.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [EditBatch](#editbatch).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/changes

**Changes, as Server-Sent Events** Scope: `read`.

A `text/event-stream`. The first event carries the current version, or what changed since `since`. Then one event per change the caller may see: the changed cells, filtered for the caller, or `reload`. A change the caller may see none of sends nothing. Each event's `id` is its version; reconnect with `?since=` (or `Last-Event-ID`) to resume. A stream is not bound by the request time limit and takes no in-flight slot; at most IMPROV_MAX_STREAMS (default 256) are open at once, 8 per caller, with 429 past either.

- Path parameter `since`: `{"$ref":"#/components/schemas/Version"}`.

| Status | Response |
| --- | --- |
| 200 | The event stream; each `data:` line is a ChangeEvent. |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 429 | Over the rate limit, or too many change streams open (in all, or for this caller). Body: [Failure](#failure). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/health

**Liveness probe** Scope: `public`.

| Status | Response |
| --- | --- |
| 200 | The text `ok`. |

## DELETE /v1/items/{id}

**Delete an item** Scope: `write`.

With its cells.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## PUT /v1/items/{id}

**Rename an item** Scope: `write`.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [Name](#name).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/measures

**List the measures** Scope: `read`.

| Status | Response |
| --- | --- |
| 200 | Measures, sorted by id. Body: array of [MeasureSummary](#measuresummary). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/measures

**Define a measure** Scope: `write`.

A derived measure or LAMBDA from a definition, or a new input measure. A formula error is located in the text (`diagnostic`).

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [Define](#define).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## DELETE /v1/measures/{id}

**Delete a measure** Scope: `write`.

Refused while a formula reads it.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## PUT /v1/measures/{id}

**Change a measure** Scope: `write`.

A new formula (an error located in it), name or number format.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [UpdateMeasure](#updatemeasure).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/measures/{id}/cells

**Set one input cell** Scope: `write`.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

Request body: [SetCellRequest](#setcellrequest).

Example:

```json
{"coord":{"dims":{"1":10,"2":20}},"value":90}
```

| Status | Response |
| --- | --- |
| 200 | Written. Body: `{"properties":{"ok":{"const":true}},"type":"object"}`. |
| 400 | The request was refused. Body: [Error](#error). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | The request was refused. Body: [Error](#error). |
| 404 | The request was refused. Body: [Error](#error). |
| 409 | The request was refused. Body: [Error](#error). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/measures/{id}/eval

**What-if evaluation** Scope: `read`.

Apply edits through the live engine and return derived measure `id`. A read token may evaluate; `"persist": true` writes the edits and needs a write token (403 otherwise). A persisting request is audited.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

Request body: [EvalRequest](#evalrequest).

Example:

```json
{"edits":[{"coord":[[1,10],[2,20]],"measure":101,"value":90}],"persist":false}
```

| Status | Response |
| --- | --- |
| 200 | Cells, coordinates named by category and item. Body: array of [Cell](#cell). |
| 400 | The request was refused. Body: [Error](#error). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | The request was refused. Body: [Error](#error). |
| 404 | The request was refused. Body: [Error](#error). |
| 409 | The request was refused. Body: [Error](#error). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/measures/{id}/values

**A measure's values** Scope: `read`.

The stored cells of an input measure, or the computed cells of a derived one.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

| Status | Response |
| --- | --- |
| 200 | Cells, coordinates named by category and item. Body: array of [Cell](#cell). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 404 | The request was refused. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/measures/{id}/window

**A window of a measure's cells** Scope: `read`.

The visible cells of the measure at the given items. Derived cells are computed by the server's engine over every cell, hidden ones included.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

Request body: [Window](#window).

| Status | Response |
| --- | --- |
| 200 | The window's visible cells. Body: [Cells](#cells). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/model

**The whole model** Scope: `read`. **Unstable:** not covered by the compatibility promise.

Unstable: the body is the store's internal serialization of the model (ids, expression trees, function ids as numbers) and may change in any release. Not covered by the /v1 compatibility promise; use /v1/measures and /v1/measures/{id}/values for a stable view. Refused (403) for a model with access policies, whoever asks: it is everything, unfiltered.

| Status | Response |
| --- | --- |
| 200 | The model, as the store serializes it. Body: `{"type":"object"}`. |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | The request was refused. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/nl/describe

**A formula to controlled English** Scope: `read`.

Request body:

```json
{
  "properties": {
    "formula": {
      "description": "A formula as /v1/nl/parse returns it.",
      "type": "object"
    }
  },
  "required": [
    "formula"
  ],
  "type": "object"
}
```

| Status | Response |
| --- | --- |
| 200 | The text. Body: `{"properties":{"text":{"type":"string"}},"type":"object"}`. |
| 400 | The request was refused. Body: [Error](#error). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |

## POST /v1/nl/parse

**Controlled English to a formula** Scope: `read`.

Request body:

```json
{
  "properties": {
    "text": {
      "type": "string"
    }
  },
  "required": [
    "text"
  ],
  "type": "object"
}
```

Example:

```json
{"text":"revenue is price times quantity"}
```

| Status | Response |
| --- | --- |
| 200 | The formula. Body: `{"type":"object"}`. |
| 400 | The request was refused. Body: [Error](#error). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |

## GET /v1/openapi.json

**This document** Scope: `public`.

| Status | Response |
| --- | --- |
| 200 | The OpenAPI document. Body: `{"type":"object"}`. |

## POST /v1/redo

**Redo** Scope: `write`.

The session's last undone change.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## GET /v1/scheduler/status

**Refresh scheduler state** Scope: `read`.

| Status | Response |
| --- | --- |
| 200 | `enabled` false, or the tick period and the last tick's outcome. Body: `{"properties":{"enabled":{"type":"boolean"},"last_error":{"type":["string","null"]},"last_tick_refreshed":{"items":{"type":"integer"},"type":"array"},"tick_secs":{"type":"integer"}},"required":["enabled"],"type":"object"}`. |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |

## GET /v1/structure

**What the caller may see of the model** Scope: `read`.

Categories with their visible items, visible measures with formulas when shown, LAMBDAs and views. The stable shape a GUI reads, in place of `/v1/model`.

| Status | Response |
| --- | --- |
| 200 | The visible structure. Body: [Structure](#structure). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/undo

**Undo** Scope: `write`.

The session's last change (an edit batch, or a structural change with the cells it dropped). Cells written back are checked as new edits.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## POST /v1/views

**Save a view** Scope: `write`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [ViewBody](#viewbody).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## DELETE /v1/views/{id}

**Delete a view** Scope: `write`.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## PUT /v1/views/{id}

**Change a view** Scope: `write`.

Its name, its matrices, or both.

- Path parameter `id`: `{"minimum":0,"type":"integer"}`.

- Path parameter `x-improv-session`: `{"maxLength":64,"type":"string"}`.

Request body: [ViewBody](#viewbody).

| Status | Response |
| --- | --- |
| 200 | Done. Body: [Done](#done). |
| 400 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 403 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 404 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 409 | Refused: a bad request (400), a write with a read token (403), not there or not visible (404), another writer's structural change (409). Body: [Failure](#failure). |
| 413 | The body is over IMPROV_MAX_BODY_BYTES. |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |
| 500 | Internal error; the detail is in the server log under request_id. Body: [Error](#error). |

## Schemas

### At

```json
{
  "description": "A coordinate: [category id, item id] pairs, sorted by category id.",
  "items": {
    "maxItems": 2,
    "minItems": 2,
    "prefixItems": [
      {
        "type": "integer"
      },
      {
        "type": "integer"
      }
    ],
    "type": "array"
  },
  "type": "array"
}
```

### Capabilities

```json
{
  "properties": {
    "add_measure": {
      "type": "boolean"
    },
    "admin": {
      "description": "May set the model's policies.",
      "type": "boolean"
    },
    "edit_items": {
      "description": "The visible items the caller may rename or delete.",
      "items": {
        "type": "integer"
      },
      "type": "array"
    },
    "edit_structure": {
      "description": "May add, rename or remove categories and items.",
      "type": "boolean"
    },
    "manage_views": {
      "type": "boolean"
    },
    "measures": {
      "description": "Each visible measure, in id order.",
      "items": {
        "properties": {
          "cells": {
            "description": "Which of its visible cells may be edited.",
            "enum": [
              "all",
              "some",
              "none"
            ]
          },
          "define": {
            "description": "May redefine, format or drop it.",
            "type": "boolean"
          },
          "id": {
            "type": "integer"
          }
        },
        "required": [
          "id",
          "define",
          "cells"
        ],
        "type": "object"
      },
      "type": "array"
    },
    "restricted": {
      "description": "Whether the model has access policies.",
      "type": "boolean"
    },
    "version": {
      "description": "The model's version: opaque, compare for equality only.",
      "type": "string"
    }
  },
  "required": [
    "version",
    "restricted",
    "admin",
    "add_measure",
    "edit_structure",
    "manage_views",
    "measures",
    "edit_items"
  ],
  "type": "object"
}
```

### Cell

```json
{
  "properties": {
    "coord": {
      "description": "[category name, item name] pairs, sorted.",
      "items": {
        "items": {
          "type": "string"
        },
        "maxItems": 2,
        "minItems": 2,
        "type": "array"
      },
      "type": "array"
    },
    "value": {
      "type": "number"
    }
  },
  "required": [
    "coord",
    "value"
  ],
  "type": "object"
}
```

### Cells

```json
{
  "description": "The window's visible cells, as the server computed them. A cell absent here is blank: empty, or not visible to the caller.",
  "properties": {
    "cells": {
      "items": {
        "properties": {
          "at": {
            "$ref": "#/components/schemas/At"
          },
          "value": {
            "$ref": "#/components/schemas/Value"
          }
        },
        "required": [
          "at",
          "value"
        ],
        "type": "object"
      },
      "type": "array"
    },
    "measure": {
      "type": "integer"
    },
    "version": {
      "$ref": "#/components/schemas/Version"
    }
  },
  "required": [
    "version",
    "measure",
    "cells"
  ],
  "type": "object"
}
```

### ChangeEvent

```json
{
  "properties": {
    "cells": {
      "description": "The cells that changed, and only those the subscriber may see.",
      "items": {
        "properties": {
          "at": {
            "$ref": "#/components/schemas/At"
          },
          "measure": {
            "type": "integer"
          },
          "value": {
            "oneOf": [
              {
                "$ref": "#/components/schemas/Value"
              },
              {
                "type": "null"
              }
            ]
          }
        },
        "type": "object"
      },
      "type": "array"
    },
    "reload": {
      "description": "The structure changed, or the gap since `since` is too old: re-read everything.",
      "type": "boolean"
    },
    "version": {
      "$ref": "#/components/schemas/Version"
    }
  },
  "required": [
    "version"
  ],
  "type": "object"
}
```

### Define

```json
{
  "description": "A definition (`Revenue = Price * Quantity` defines or redefines a derived measure, `Half = LAMBDA(x, x / 2)` a LAMBDA), or a name for a new input measure.",
  "properties": {
    "categories": {
      "description": "An input measure's categories (default: all).",
      "items": {
        "type": "integer"
      },
      "type": "array"
    },
    "definition": {
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "type": {
      "enum": [
        "Number",
        "Boolean",
        "Text",
        "DateTime"
      ]
    }
  },
  "type": "object"
}
```

### Done

```json
{
  "properties": {
    "id": {
      "description": "The id of what it made.",
      "type": "integer"
    },
    "message": {
      "type": "string"
    },
    "version": {
      "$ref": "#/components/schemas/Version"
    }
  },
  "required": [
    "version",
    "message"
  ],
  "type": "object"
}
```

### EditBatch

```json
{
  "properties": {
    "edits": {
      "items": {
        "properties": {
          "at": {
            "$ref": "#/components/schemas/At"
          },
          "measure": {
            "type": "integer"
          },
          "value": {
            "description": "The new value; null or absent clears the cell.",
            "oneOf": [
              {
                "$ref": "#/components/schemas/Value"
              },
              {
                "type": "null"
              }
            ]
          }
        },
        "required": [
          "measure",
          "at"
        ],
        "type": "object"
      },
      "maxItems": 10000,
      "type": "array"
    }
  },
  "required": [
    "edits"
  ],
  "type": "object"
}
```

### Error

```json
{
  "properties": {
    "error": {
      "type": "string"
    },
    "request_id": {
      "description": "On a 500: the id the server logged the detail under.",
      "type": "integer"
    }
  },
  "required": [
    "error"
  ],
  "type": "object"
}
```

### EvalRequest

```json
{
  "properties": {
    "edits": {
      "items": {
        "properties": {
          "coord": {
            "description": "[category id, item id] pairs.",
            "items": {
              "items": {
                "type": "integer"
              },
              "maxItems": 2,
              "minItems": 2,
              "type": "array"
            },
            "type": "array"
          },
          "measure": {
            "type": "integer"
          },
          "value": {
            "description": "null clears the cell.",
            "type": [
              "number",
              "null"
            ]
          }
        },
        "required": [
          "measure",
          "coord",
          "value"
        ],
        "type": "object"
      },
      "maxItems": 10000,
      "type": "array"
    },
    "persist": {
      "default": false,
      "type": "boolean"
    }
  },
  "type": "object"
}
```

### Failure

```json
{
  "description": "A refused request. A hidden thing is refused exactly as a missing one.",
  "properties": {
    "diagnostic": {
      "description": "Where in the formula text the error is (byte offsets).",
      "properties": {
        "hint": {
          "type": "string"
        },
        "message": {
          "type": "string"
        },
        "span": {
          "prefixItems": [
            {
              "type": "integer"
            },
            {
              "type": "integer"
            }
          ],
          "type": "array"
        }
      },
      "type": "object"
    },
    "error": {
      "type": "string"
    },
    "index": {
      "description": "The edit of a batch that failed.",
      "type": "integer"
    }
  },
  "required": [
    "error"
  ],
  "type": "object"
}
```

### MeasureSummary

```json
{
  "properties": {
    "categories": {
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "id": {
      "type": "integer"
    },
    "kind": {
      "enum": [
        "input",
        "derived"
      ]
    },
    "name": {
      "type": "string"
    }
  },
  "required": [
    "id",
    "name",
    "kind",
    "categories"
  ],
  "type": "object"
}
```

### Name

```json
{
  "properties": {
    "name": {
      "type": "string"
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

### NumberFormat

```json
{
  "properties": {
    "decimals": {
      "maximum": 15,
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "grouping": {
      "type": "boolean"
    }
  },
  "type": [
    "object",
    "null"
  ]
}
```

### SetCellRequest

```json
{
  "properties": {
    "coord": {
      "properties": {
        "dims": {
          "additionalProperties": {
            "type": "integer"
          },
          "description": "Category id to item id.",
          "type": "object"
        }
      },
      "required": [
        "dims"
      ],
      "type": "object"
    },
    "value": {
      "type": "number"
    }
  },
  "required": [
    "coord",
    "value"
  ],
  "type": "object"
}
```

### Structure

```json
{
  "description": "What the caller may see of the model. Hidden measures and items are absent.",
  "properties": {
    "categories": {
      "items": {
        "properties": {
          "id": {
            "type": "integer"
          },
          "items": {
            "description": "Visible items, in grid order.",
            "items": {
              "properties": {
                "id": {
                  "type": "integer"
                },
                "name": {
                  "type": "string"
                }
              },
              "type": "object"
            },
            "type": "array"
          },
          "name": {
            "type": "string"
          }
        },
        "type": "object"
      },
      "type": "array"
    },
    "lambdas": {
      "items": {
        "properties": {
          "id": {
            "type": "integer"
          },
          "name": {
            "type": "string"
          },
          "params": {
            "items": {
              "type": "string"
            },
            "type": "array"
          }
        },
        "type": "object"
      },
      "type": "array"
    },
    "measures": {
      "items": {
        "properties": {
          "categories": {
            "items": {
              "type": "integer"
            },
            "type": "array"
          },
          "editable": {
            "description": "A hint; every edit is still checked.",
            "type": "boolean"
          },
          "error": {
            "description": "Why the formula does not build.",
            "type": "string"
          },
          "format": {
            "$ref": "#/components/schemas/NumberFormat"
          },
          "formula": {
            "description": "As typed (the DSL), or controlled English; absent when the formula reads a hidden measure.",
            "type": "string"
          },
          "id": {
            "type": "integer"
          },
          "kind": {
            "enum": [
              "input",
              "derived"
            ]
          },
          "name": {
            "type": "string"
          },
          "type": {
            "enum": [
              "Number",
              "Boolean",
              "Text",
              "DateTime",
              "Enum"
            ]
          }
        },
        "type": "object"
      },
      "type": "array"
    },
    "version": {
      "$ref": "#/components/schemas/Version"
    },
    "views": {
      "items": {
        "description": "A saved view, as the model stores it, with only visible matrices, filters and pages.",
        "type": "object"
      },
      "type": "array"
    }
  },
  "required": [
    "version",
    "categories",
    "measures"
  ],
  "type": "object"
}
```

### UpdateMeasure

```json
{
  "description": "Any of a new formula, name and format, as one step.",
  "properties": {
    "format": {
      "$ref": "#/components/schemas/NumberFormat"
    },
    "formula": {
      "type": "string"
    },
    "name": {
      "type": "string"
    }
  },
  "type": "object"
}
```

### Value

```json
{
  "description": "A cell value: a number, boolean or string, or an object for a date or an error value.",
  "oneOf": [
    {
      "type": "number"
    },
    {
      "type": "boolean"
    },
    {
      "type": "string"
    },
    {
      "properties": {
        "date": {
          "format": "date-time",
          "type": "string"
        }
      },
      "required": [
        "date"
      ],
      "type": "object"
    },
    {
      "properties": {
        "error": {
          "examples": [
            "#DIV/0!"
          ],
          "type": "string"
        }
      },
      "required": [
        "error"
      ],
      "type": "object"
    }
  ]
}
```

### Version

```json
{
  "description": "The model's version: opaque, compare for equality only.",
  "type": "string"
}
```

### ViewBody

```json
{
  "properties": {
    "matrices": {
      "description": "The view's matrices, the primary first (as the model stores a view's placements).",
      "items": {
        "type": "object"
      },
      "type": "array"
    },
    "name": {
      "type": "string"
    }
  },
  "type": "object"
}
```

### Window

```json
{
  "properties": {
    "at": {
      "additionalProperties": {
        "items": {
          "type": "integer"
        },
        "type": "array"
      },
      "description": "Item ids to read, per category id; a category left out reads all its visible items.",
      "type": "object"
    }
  },
  "type": "object"
}
```

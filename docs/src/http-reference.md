# HTTP API reference

<!-- Generated from docs/openapi.json by `UPDATE_DOCS=1 cargo test -p improv_cli`. Do not edit. -->

JSON HTTP API over one Improv model. Every path is also served without the /v1 prefix (an alias kept for older clients). `x-improv-scope` names the token scope a route needs: public (no token), read, or write. A missing or unknown token gets 401, a read token on a write route 403, an over-limit body 413, a caller over its rate limit 429.

The server serves this document at `/v1/openapi.json`. Point any
OpenAPI viewer at that URL, or at
[`docs/openapi.json`](https://codeberg.org/gregburd/improv/src/branch/main/docs/openapi.json), for an interactive
page. [The HTTP Server](./server.md) covers running, tokens and
limits.

| Method | Path | Scope | Summary |
| --- | --- | --- | --- |
| GET | [`/v1/health`](#get-v1health) | public | Liveness probe |
| GET | [`/v1/measures`](#get-v1measures) | read | List the measures |
| POST | [`/v1/measures/{id}/cells`](#post-v1measuresidcells) | write | Set one input cell |
| POST | [`/v1/measures/{id}/eval`](#post-v1measuresideval) | read | What-if evaluation |
| GET | [`/v1/measures/{id}/values`](#get-v1measuresidvalues) | read | A measure's values |
| GET | [`/v1/model`](#get-v1model) | read | The whole model |
| POST | [`/v1/nl/describe`](#post-v1nldescribe) | read | A formula to controlled English |
| POST | [`/v1/nl/parse`](#post-v1nlparse) | read | Controlled English to a formula |
| GET | [`/v1/openapi.json`](#get-v1openapijson) | public | This document |
| GET | [`/v1/scheduler/status`](#get-v1schedulerstatus) | read | Refresh scheduler state |

## GET /v1/health

**Liveness probe** Scope: `public`.

| Status | Response |
| --- | --- |
| 200 | The text `ok`. |

## GET /v1/measures

**List the measures** Scope: `read`.

| Status | Response |
| --- | --- |
| 200 | Measures, sorted by id. Body: array of [MeasureSummary](#measuresummary). |
| 401 | No token, or an unknown one. Body: [Error](#error). |
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

## GET /v1/model

**The whole model** Scope: `read`. **Unstable:** not covered by the compatibility promise.

Unstable: the body is the store's internal serialization of the model (ids, expression trees, function ids as numbers) and may change in any release. Not covered by the /v1 compatibility promise; use /v1/measures and /v1/measures/{id}/values for a stable view.

| Status | Response |
| --- | --- |
| 200 | The model, as the store serializes it. Body: `{"type":"object"}`. |
| 401 | No token, or an unknown one. Body: [Error](#error). |
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

## GET /v1/scheduler/status

**Refresh scheduler state** Scope: `read`.

| Status | Response |
| --- | --- |
| 200 | `enabled` false, or the tick period and the last tick's outcome. Body: `{"properties":{"enabled":{"type":"boolean"},"last_error":{"type":["string","null"]},"last_tick_refreshed":{"items":{"type":"integer"},"type":"array"},"tick_secs":{"type":"integer"}},"required":["enabled"],"type":"object"}`. |
| 401 | No token, or an unknown one. Body: [Error](#error). |
| 429 | The caller is over IMPROV_RATE_LIMIT requests a minute. Body: [Error](#error). |

## Schemas

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

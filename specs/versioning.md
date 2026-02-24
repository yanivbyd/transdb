# Value Versioning and Operation Idempotency Specification

## Overview

Every key in the store carries a monotonically increasing version counter. `GET` returns the current version in an `ETag` response header. `PUT` and `DELETE` require an `Idempotency-Key` header; the server guarantees that replaying the same key returns the cached result — the write is applied exactly once and the version is incremented exactly once.

## Version Model

- Each key has a `u64` version starting at **1** on first write.
- Version increments by 1 on every successful `PUT` or `DELETE`.
- When a key is deleted and later re-created, the version resets to **1** (no tombstones retained).
- Versions are stored in memory alongside the value; there is no durability requirement.

## Idempotency Model

- The client generates a unique token (recommended: UUID v4) and sends it as `Idempotency-Key: <token>`.
- `Idempotency-Key` is **required** on all `PUT` and `DELETE` requests. Missing it returns `400 Bad Request`.
- On the first request with a given token the server inserts a **pending** marker into the cache before doing any work, then applies the operation and replaces the marker with the completed result.
- On a repeated request with the same token:
  - If the first request is still **in-flight**, the second request waits (coalesces) until the first completes and then returns the same cached result. The operation is not applied a second time.
  - If the first request has **completed**, the cached result is returned immediately.
- In both cases the version is incremented exactly once.
- Before returning the cached result the server verifies that `method` and `key_path` match the cached record. A mismatch returns `422 Unprocessable Entity`.
- Cached entries expire after **1 hour** and are purged by a background task. A retry arriving after expiry is treated as a new request.


## HTTP API

### GET /keys/{key}

No change to the request. The response gains an `ETag` header containing the current version as a quoted decimal string.

Response `200 OK`:
```
ETag: "3"
<raw bytes body>
```

Response table:
| Status | Meaning |
|---|---|
| `200 OK` | Key found; body is raw bytes; `ETag` carries current version |
| `404 Not Found` | Key does not exist |
| `400 Bad Request` | Key exceeds `MAX_KEY_SIZE` |
| `503 Service Unavailable` | Lock timeout |

---

### PUT /keys/{key}

`Idempotency-Key: <token>` is required. On success the response carries the (new or cached) version in `ETag`.

Response `200 OK`:
```
ETag: "<version>"
```

Response table:
| Status | Meaning |
|---|---|
| `200 OK` | Write applied (or replayed from cache); `ETag` carries the version assigned by this write |
| `422 Unprocessable Entity` | `Idempotency-Key` was already used for a different method or key path |
| `400 Bad Request` | `Idempotency-Key` header missing; or key or value exceeds size limits |
| `503 Service Unavailable` | Lock timeout |

---

### DELETE /keys/{key}

`Idempotency-Key: <token>` is required. The first call deletes the key if it exists (or is a no-op if absent); both cases return `204`. Replays return the same cached `204`.

Response table:
| Status | Meaning |
|---|---|
| `204 No Content` | Key deleted, or key was already absent; or replay of a cached delete |
| `422 Unprocessable Entity` | `Idempotency-Key` was already used for a different method or key path |
| `400 Bad Request` | `Idempotency-Key` header missing; or key exceeds `MAX_KEY_SIZE` |
| `503 Service Unavailable` | Lock timeout |

---

## Data Model Changes

**Key-value store** changes from `HashMap<String, Bytes>` to `HashMap<String, Entry>`:

```rust
struct Entry {
    value: Bytes,
    version: u64,
}
```

**Idempotency cache** tracks both in-flight and completed records:

```rust
struct IdempotencyRecord {
    method: HttpMethod,      // PUT or DELETE
    key_path: String,        // e.g. "mykey"
    status_code: u16,        // 200 or 204
    etag: Option<u64>,       // version assigned; Some for PUT, None for DELETE
    created_at: Instant,
}

enum IdempotencyEntry {
    Pending(Arc<tokio::sync::watch::Receiver<Option<IdempotencyRecord>>>),
    Completed(IdempotencyRecord),
}

type IdempotencyCache = Arc<RwLock<HashMap<String, IdempotencyEntry>>>;
```

---

## Out of Scope

- Conditional writes (`If-Match` / `If-None-Match`) — future work
- Version history / changelog
- Version-based GET (fetch a specific historical version)
- Idempotency keys inside transactions
- Idempotency cache expiry / background purge

# Global Monotonic Version & Tombstone DELETE Specification

## Overview

Replace per-key version counters and physical DELETE with a **global monotonic version counter** and **tombstone-based DELETE**. Every mutating operation — PUT or DELETE — advances a single `next_version: u64` counter inside `DbState`, protected by the existing write lock. DELETEs write a sentinel entry (`value: None`) rather than removing the key.

| Problem (current) | Solution (after) |
|---|---|
| Per-key version resets to 1 after DELETE, so `(key, version=1)` can exist multiple times | Global counter → `(key, version)` is unique across all time |
| Correctness checker needs `Vec<PutEntry>` + `max_by_key` disambiguation | Single `WriteEntry` per `(key, version)` — no ambiguity |
| DELETE has no version → separate delete index + `superseding_delete` logic | Tombstone carries a version → unified write index handles PUTs and DELETEs uniformly |
| `StaleDataReturned { latest_known_version: None }` for "read after delete" | Tombstone's version IS the `latest_known_version` → always `Some(v)` |
| Replication stream's global version separate from store versions | `next_version` doubles as a replication LSN |

---

## Server Changes (`transdb-server`)

### `DbState` and `Entry`

`DbState` gains `next_version: u64` (initialised to `0`). `Entry.value` becomes `Option<Bytes>`; tombstones carry `expires_at: Some(now + 3600)`.

```rust
pub struct DbState {
    pub store: HashMap<String, Entry>,
    pub idempotency_cache: HashMap<String, IdempotencyRecord>,
    pub next_version: u64,
}

pub struct Entry {
    pub value: Option<Bytes>,    // None = tombstone
    pub version: u64,
    pub expires_at: Option<u64>, // tombstones: now + 3600
}
```

### `handle_put`

Replaces per-key `entry.version += 1` with `db_guard.next_version += 1` before inserting. Response and idempotency behavior are unchanged.

### `handle_delete`

Writes a tombstone only when the key holds a live value. Missing keys and existing tombstones are silent no-ops returning `204 No Content` without advancing `next_version`:

```rust
match db_guard.store.get(&key) {
    None | Some(Entry { value: None, .. }) => return no_content_response(),
    _ => {}
}
db_guard.next_version += 1;
let version = db_guard.next_version;
db_guard.store.insert(key.clone(), Entry {
    value: None, version,
    expires_at: Some(clock.now_unix_secs() + 3600),
});
```

Response: `200 OK` + `ETag: "{version}"` when a tombstone is written; `204 No Content` otherwise. Idempotency record is only written for the `200` case.

### `handle_get`

Tombstones return `404` — identical to a key that never existed.

---

## HTTP API Changes

| Method | Path | Before | After |
|---|---|---|---|
| `DELETE` | `/keys/{key}` | `204 No Content` | `200 OK` + `ETag: "{version}"` (live key) or `204 No Content` (absent / already deleted) |
| `GET` | `/keys/{key}` | `404` if absent | `404` if absent **or** tombstone |
| `PUT` | `/keys/{key}` | `200 OK` + `ETag` | unchanged |

> **Breaking change**: DELETE on a live key now returns `200 OK` + `ETag` instead of `204`. Callers treating `204` as the only success response must also accept `200`.

---

## Client Changes (`transdb-client`)

`delete()` returns `Some(version)` when a tombstone was written (`200 OK` + ETag) and `None` when the key was absent or already deleted (`204 No Content`):

```rust
pub async fn delete(&self, key: &str) -> Result<Option<u64>>
```

---

## Replication Stream Changes

`DeleteOp` gains a `version` field so the replica applies the tombstone at the same global version:

```protobuf
message DeleteOp {
  string key     = 1;
  uint64 version = 2;
}
```

Primary enqueue: `repl.enqueue(DeleteOp { key: key.clone(), version })`.

Replica handler changes from `store.remove` to inserting a tombstone with a 1-hour TTL and advancing `next_version` if `op.version > db_guard.next_version`.

---

## Stress Test Changes (`transdb-stress-tests`)

### Types

`OpOutcome::DeleteOk` gains `version: u64`. `ViolationKind::StaleDataReturned.latest_known_version` changes from `Option<u64>` to `u64`.

### Unified Write Index

`PutEntry`, `DeleteEntry`, `build_delete_index`, and `superseding_delete` are removed. A single `build_write_index` maps `(key, version) → WriteEntry { write_value: Data(Vec<u8>) | Tombstone, write_start_ts, write_ack_ts }`. Global uniqueness of `(key, version)` eliminates disambiguation.

### `classify_get` (6 steps → 4)

1. `(key, version)` not in write index → `VersionNotFound`
2. Write started after GET acked → `ReadBeforeWriteStart`
3. Write acked before GET started: value mismatch (3a); newer write already acked → `StaleDataReturned` (3b). Tombstone entries count as newer writes.
4. Write/GET overlap → no violation

### `worker.rs`

`Ok(Some(v))` → `OpOutcome::DeleteOk { version: v }`; `Ok(None)` → `OpOutcome::NotFound`.

### `main.rs`

No change needed — `StaleDataReturned { .. }` pattern already compiles with `u64`.

---

## Removed Code

- `PutEntry` and `DeleteEntry` structs
- `build_delete_index()` and `superseding_delete()` functions
- `delete_index` parameter from `classify_get`

---

## Testing

### `transdb-server/tests/unit_server.rs`

- `handle_put` increments the global counter; two PUTs to different keys each advance `next_version` by 1.
- `handle_delete` on a live key writes a tombstone (`200 OK` + ETag) with `expires_at = now + 3600`.
- `handle_delete` on a non-existent key returns `204 No Content`; store and `next_version` unchanged.
- `handle_delete` on an already-tombstoned key returns `204 No Content`; tombstone and `next_version` unchanged.
- `handle_get` on a tombstoned key returns `404`.
- Idempotency replay of DELETE (live key) returns the same `200 OK` + ETag.

### `transdb-stress-tests/tests/unit_history.rs`

Update all existing tests to `DeleteOk { version }` and `StaleDataReturned { latest_known_version: u64 }`. New scenarios:

- Stale read after tombstone → `StaleDataReturned { latest_known_version: <tombstone_version> }`.
- GET overlapping a tombstone write → no violation.
- DELETE then re-PUT: old version is `VersionNotFound`; new version is clean.

### `transdb-integration-tests/tests/integration_test.rs`

- DELETE on a live key returns `200 OK` + ETag.
- DELETE on a non-existent key returns `204 No Content`.
- Second DELETE on the same key (after the first succeeded) returns `204 No Content`.
- GET after DELETE returns `404`.
- PUT after DELETE has a higher version than the tombstone.

---

## Known Limitations (Out of Scope)

- **Tombstone accumulation**: Tombstones expire after 1 hour via the existing TTL mechanism. Background compaction of expired entries is deferred.
- **version=0 is reserved**: The first write produces version 1; version 0 is never visible externally.

---

## Commit Plan

1. `feat: global monotonic version counter and tombstone DELETE`
   Replace per-key version increment with `DbState.next_version`; change `Entry.value` to `Option<Bytes>`; rewrite `handle_put`, `handle_delete` (204 for absent/already-deleted keys, 200+ETag for live keys, 1-hour TTL on tombstones), and `handle_get`; update idempotency cache for DELETE; add `version` field to `DeleteOp` in the replication proto and update the primary enqueue call and replica handler; change `client::delete()` from `Result<()>` to `Result<Option<u64>>`; add unit tests in `unit_server.rs`.

2. `feat(stress): simplify history checker using unified write index`
   Update `OpOutcome::DeleteOk`, `ViolationKind::StaleDataReturned`, and `ViolationKind::ReadBeforeWriteStart`; replace dual indexes with unified `build_write_index`; remove `PutEntry`, `DeleteEntry`, `superseding_delete`, `build_delete_index`; simplify `classify_get` from 6 steps to 4; update `worker.rs` and `main.rs`; update all unit tests in `unit_history.rs`.

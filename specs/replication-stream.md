# Replication Stream Specification

## Overview

Phase 2 replication connects a running primary to a replica using an **internal gRPC channel**. After every successful mutating operation (PUT or DELETE), the primary enqueues the mutation into an in-memory **pending queue** and returns the HTTP response immediately — delivery to the replica is asynchronous.

A background task drains the queue by sending `ReplicateBatch` RPCs to the replica. Mutations are retained until ACKed; failed calls are retried. The queue is in-memory only: unACKed mutations are lost on primary restart.

The replica applies batches in sequence order and continues to reject external key operations with HTTP 405 (unchanged from Phase 1).

---

## Server Layout

All replication code lives inside `transdb-server`, which gains `tonic`, `prost`, and `tonic-build` dependencies and a `build.rs`. New files:

```
transdb-server/src/
  proto/replication.proto
  replication/mod.rs      # re-exports; includes tonic-generated code
  replication/client.rs   # ReplicationClient (primary)
  replication/service.rs  # ReplicationService (replica)
```

---

## Proto Schema

```protobuf
syntax = "proto3";
package transdb.replication;

message ReplicateBatchRequest { repeated MutationEntry entries = 1; }

message MutationEntry {
  uint64 seq = 1;
  oneof operation { PutOp put = 2; DeleteOp delete = 3; }
}

message PutOp {
  string key     = 1;
  bytes  value   = 2;
  uint64 version = 3;
  optional uint64 expires_at = 4; // absent means no TTL
}

message DeleteOp {
  string key        = 1;
  uint64 version    = 2; // tombstone version
  uint64 expires_at = 3; // tombstone TTL (absolute Unix epoch seconds)
}

message ReplicateBatchResponse { uint64 applied_through = 1; }

service Replication {
  rpc ReplicateBatch(ReplicateBatchRequest) returns (ReplicateBatchResponse);
}
```

---

## Topology Changes

`transdb-common`'s `Topology` gains `replica_grpc_addr: Option<String>`. When `None`, the primary skips replication (single-node mode). The binary gains `--replica-grpc-addr <host:port>`, wired into a new `ServerConfig.grpc_addr: Option<SocketAddr>`.

---

## Primary: Replication Dispatch

`AppState` gains `replication: Option<ReplicationClient>`. The client wraps an in-memory `PendingQueue` (`VecDeque<MutationEntry>`) and a `Notify`. `Server::run` calls `ReplicationClient::connect(replica_grpc_addr)` before starting the HTTP listener when configured.

Each mutation's `seq` equals the global `next_version` assigned under the store's write lock — no separate counter is needed in the queue.

`enqueue(seq, op)` appends the entry and wakes the background sender, which:

1. Waits for `Notify`.
2. Snapshots the queue.
3. Calls `ReplicateBatch`.
4. On success: drops all entries with `seq ≤ applied_through`.
5. On failure: logs `WARN`, sleeps 100 ms, retries.

After a successful write, handlers call `enqueue` if `replication` is `Some`. `handle_put` passes `PutOp { key, value, version, expires_at }`; `handle_delete` passes `DeleteOp { key, version, expires_at: now + TOMBSTONE_TTL_SECS }`. Idempotency replays do not enqueue.

---

## Replica: gRPC Server

`ReplicationService` holds `db: Db` and implements the `Replication` service. The handler iterates entries in ascending `seq` order and for each acquires a write lock and applies:

- **PutOp**: Write `Entry { value: Some(value), version, expires_at }` unconditionally.
- **DeleteOp**: Write tombstone `Entry { value: None, version, expires_at: Some(expires_at) }` unconditionally.

Returns `applied_through` = seq of the last entry processed, or `0` for an empty batch. An unrecognised `operation` variant returns `Status::invalid_argument`; preceding entries are not rolled back.

`Server::run` binds the gRPC listener when `role == Replica && grpc_addr.is_some()`, spawns `ReplicationService`, then starts the HTTP listener and sends `ready_tx`.

---

## Delivery Semantics

| Property | Behaviour |
|---|---|
| **Ordering** | `seq == next_version` (assigned under primary write lock); replica applies in seq order. |
| **Durability** | Queue retained until ACKed; lost on primary restart. |
| **Consistency** | Eventual. |
| **ACK** | `applied_through` confirms all entries up to that seq; lower entries are safe to drop. |

---

## Error Handling

| Scenario | Primary | Replica |
|---|---|---|
| Replica unreachable or RPC error | Log `WARN`; retain queue; retry after 100 ms | — |
| Unknown `operation` variant | — | `Status::invalid_argument`; preceding entries already applied |
| Write-lock timeout | — | `Status::resource_exhausted`; primary retries |
| Empty batch | — | `applied_through: 0`; no-op |

---

## Testing

### `transdb-server/tests/unit_replication.rs`

**`ReplicationService`:** single PutOp inserts and returns correct `applied_through`; PutOp overwrites unconditionally; DeleteOp writes tombstone for a present key; DeleteOp writes tombstone when key is absent; mixed batch applies in order; empty batch returns `applied_through: 0`; missing `operation` returns `invalid_argument`.

**`ReplicationClient`:** `enqueue(seq, op)` appends with given seq; ascending seqs produce ordered entries; after ACK of N, entries with seq ≤ N are removed and seq > N remain.

### `transdb-server/tests/unit_server.rs` additions

`handle_put` enqueues when `replication` is `Some`; does not enqueue on idempotency replay; `handle_delete` enqueues a tombstone mutation; both handlers succeed when `replication` is `None`.

### `transdb-integration-tests/tests/integration_test.rs` additions

PUT on primary → GET on replica eventually returns same value; DELETE on primary → GET on replica eventually returns 404; replica returns 405 for external PUT/DELETE.

---

## Commit Plan

1. `feat(common): add replica_grpc_addr to Topology`
   Add `replica_grpc_addr: Option<String>` to `Topology`; update topology unit tests for serialisation round-trips with and without the field.

2. `feat(server): add replication proto schema and build.rs`
   Add `build.rs` and `src/proto/replication.proto` with all message and service definitions; no runtime code yet.

3. `feat(server): implement ReplicationService (replica gRPC server)`
   Add `replication/service.rs` implementing `replicate_batch` (PutOp upsert and DeleteOp tombstone); wire `ServerConfig.grpc_addr`, CLI flag `--replica-grpc-addr`, and gRPC listener startup in `Server::run`; add replica-side unit tests in `unit_replication.rs`.

4. `feat(server): implement ReplicationClient (primary dispatch)`
   Add `replication/client.rs` with `PendingQueue` and background sender; add `replication: Option<ReplicationClient>` to `AppState`; call `enqueue` in `handle_put` and `handle_delete`; extend `unit_replication.rs` with client tests, extend `unit_server.rs` with handler enqueue tests, and add integration tests.

---

## Out of Scope

- **Primary-restart durability**: Unacked queue is lost on restart.
- **Backpressure**: No throttling if the replica falls behind.
- **Full-sync on replica startup**: No initial bulk-sync; replica starts empty.
- **TLS**: Plain-text `h2c` only.
- **Idempotency cache replication**: Only key-value data is replicated.
- **Multiple replicas**: One replica; fan-out deferred.
- **Linearizability**: Replica reads may be stale.

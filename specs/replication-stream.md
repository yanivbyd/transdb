# Replication Stream Specification

## Overview

Phase 2 replication connects a running primary to a replica using an **internal gRPC channel**. After every successful mutating operation (PUT or DELETE), the primary enqueues the mutation into an in-memory **pending queue** and returns the HTTP response to the external client immediately — the client's ACK does **not** imply the replica has been updated.

A background task on the primary continuously drains the pending queue by sending `ReplicateBatch` RPCs to the replica. Mutations remain in the queue until the replica ACKs them; failed calls are retried automatically. Because the queue is in-memory only, any mutations pending at the time of a primary restart are lost.

The replica receives batches, applies each mutation in sequence order, and returns the sequence number of the last applied entry. It continues to reject external key operations via HTTP 405 (Phase 1 behaviour is unchanged).

---

## Server Layout

All replication code lives inside `transdb-server`. The crate gains three new dependencies and a `build.rs` to run code generation:

| Dependency | Kind | Purpose |
|---|---|---|
| `prost` | runtime | Protobuf encoding/decoding; provides the generated message structs. |
| `tonic` | runtime | gRPC runtime built on `tokio`; provides the client channel, server builder, and `async_trait` service interface. |
| `tonic-build` | build | Code generator run by `build.rs`; compiles `.proto` files into Rust using `prost` and `tonic`. |

```
transdb-server/
├── Cargo.toml          # gains tonic, prost, tonic-build deps
├── build.rs            # runs tonic-build on the proto file
└── src/
    ├── lib.rs          # existing server code (unchanged structure)
    ├── proto/
    │   └── replication.proto
    └── replication/
        ├── mod.rs      # re-exports; include!(tonic-generated code)
        ├── client.rs   # ReplicationClient (primary uses)
        └── service.rs  # ReplicationService (replica uses)
```

---

## Proto Schema

**`transdb-server/src/proto/replication.proto`**

```protobuf
syntax = "proto3";

package transdb.replication;

message ReplicateBatchRequest {
  repeated MutationEntry entries = 1;
}

// A single mutation tagged with a monotonically increasing sequence number.
message MutationEntry {
  uint64 seq = 1;
  oneof operation {
    PutOp    put    = 2;
    DeleteOp delete = 3;
  }
}

message PutOp {
  string key       = 1;
  bytes  value     = 2;
  uint64 version   = 3;           // version assigned by the primary
  optional uint64 expires_at = 4; // absolute Unix epoch seconds; absent means no TTL
}

message DeleteOp {
  string key = 1;
}

// The replica confirms it has applied all entries up to and including `applied_through`.
// Entries with seq <= applied_through may be removed from the primary's pending queue.
message ReplicateBatchResponse {
  uint64 applied_through = 1;
}

service Replication {
  rpc ReplicateBatch(ReplicateBatchRequest) returns (ReplicateBatchResponse);
}
```

---

## Topology Changes

`transdb-common` adds a `replica_grpc_addr` field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Topology {
    pub primary_addr:      String,         // HTTP address of the primary
    pub replica_addr:      Option<String>, // HTTP address of the replica
    pub replica_grpc_addr: Option<String>, // gRPC address of the replica (host:port)
}
```

When `replica_grpc_addr` is `None`, the primary skips replication entirely (single-node mode).

The `transdb-server` binary gains a new optional CLI flag:

```
--replica-grpc-addr <host:port>   gRPC listener address for the replica
                                  (required when --role replica and replication is enabled)
```

---

## Primary: Replication Dispatch

### Pending Queue

The primary holds an in-memory queue of mutations that have not yet been ACKed by the replica:

```rust
struct PendingQueue {
    next_seq: u64,                  // auto-increments on each enqueue
    entries:  VecDeque<MutationEntry>,
}
```

Sequence numbers are assigned inside `enqueue` while holding the queue lock. Because HTTP handler writes are already serialised by the store's `RwLock`, sequence numbers reflect the true mutation order on the primary.

### `ReplicationClient` (`transdb-server/src/replication/client.rs`)

```rust
#[derive(Clone)]
pub struct ReplicationClient {
    queue:  Arc<Mutex<PendingQueue>>,
    notify: Arc<tokio::sync::Notify>,
}

impl ReplicationClient {
    /// Connects to `grpc_addr` and spawns the background sender task.
    pub async fn connect(grpc_addr: &str) -> Result<Self, tonic::transport::Error>;

    /// Assigns the next sequence number, appends to the queue, and wakes the sender.
    /// Returns immediately without waiting for delivery.
    pub fn enqueue(&self, op: impl Into<mutation_entry::Operation>);
}
```

The background sender task (spawned by `connect`):

1. Waits for a `Notify` signal.
2. Snapshots all current queue entries.
3. Calls `ReplicateBatch` with the snapshot.
4. On success: removes all entries with `seq ≤ applied_through` from the queue.
5. On failure: logs `WARN`; sleeps briefly (100 ms); re-signals `Notify` to retry.

### Integration into `AppState`

```rust
#[derive(Clone)]
pub struct AppState {
    pub db:          Db,
    pub clock:       Arc<dyn Clock>,
    pub role:        NodeRole,
    pub replication: Option<ReplicationClient>, // None on replicas and single-node primaries
}
```

`Server::run` constructs `ReplicationClient::connect(replica_grpc_addr)` before starting the HTTP listener when the node is a primary with a configured `replica_grpc_addr`.

### Handler Changes

After a successful write, both `handle_put` and `handle_delete` call `state.replication.enqueue(...)` if `replication` is `Some`:

**`handle_put`** — after inserting into the store and before returning the HTTP response:

```rust
if let Some(ref repl) = state.replication {
    repl.enqueue(PutOp {
        key:        key.clone(),
        value:      entry.value.to_vec(),
        version:    entry.version,
        expires_at: entry.expires_at,
    });
}
```

**`handle_delete`** — after removing the key from the store:

```rust
if let Some(ref repl) = state.replication {
    repl.enqueue(DeleteOp { key: key.clone() });
}
```

Idempotency replays (repeated `Idempotency-Key`) do **not** enqueue a second mutation — the store is unchanged.

---

## Replica: gRPC Server

### `ReplicationService` (`transdb-server/src/replication/service.rs`)

```rust
pub struct ReplicationService {
    db: Db,
}

#[tonic::async_trait]
impl Replication for ReplicationService {
    async fn replicate_batch(
        &self,
        request: Request<ReplicateBatchRequest>,
    ) -> Result<Response<ReplicateBatchResponse>, Status>;
}
```

The handler iterates entries **in the order they appear in the request** (ascending `seq`). For each entry it acquires a write lock and applies:

- **PutOp**: Upsert `Entry { value, version, expires_at }` unconditionally.
- **DeleteOp**: Remove the key if present (no-op if absent).

After processing all entries, it returns:

```rust
ReplicateBatchResponse { applied_through: last_seq }
```

where `last_seq` is the `seq` of the final entry processed. If the batch is empty, `applied_through` is `0`.

If an entry contains an unrecognised `operation` variant, the handler returns `Status::invalid_argument` and logs `ERROR`. Entries processed before the bad entry are not rolled back.

### Replica Server Startup

`Server::run` checks the node's role. When `role == NodeRole::Replica` and `config.grpc_addr` is set, it binds the gRPC listener before signalling readiness:

```rust
let grpc_listener = tokio::net::TcpListener::bind(config.grpc_addr).await?;
let replication_svc = ReplicationService::new(Arc::clone(&state.db));
tokio::spawn(async move {
    tonic::transport::Server::builder()
        .add_service(ReplicationServer::new(replication_svc))
        .serve_with_incoming(TcpListenerStream::new(grpc_listener))
        .await
        .expect("gRPC server error");
});
// then start HTTP server and send ready_tx
```

The `ready_tx` signal is sent after both listeners are bound, so callers know both endpoints are available.

---

## Delivery Semantics

| Property | Behaviour |
|---|---|
| **Ordering** | Mutations are assigned sequence numbers under the primary's write lock, guaranteeing global order. The batch is sent in ascending sequence order and applied by the replica in the same order — per-key ordering is therefore preserved. |
| **Durability** | Mutations are retained in the pending queue until ACKed. A failed batch call is retried. Unacked mutations are lost if the primary restarts (queue is in-memory only). |
| **Consistency** | Eventual. After a successful primary write, the replica will converge to the same state provided the primary does not restart before the batch is ACKed. |
| **ACK semantics** | `applied_through` in `ReplicateBatchResponse` confirms the replica has processed all entries up to that sequence number. Entries with seq ≤ `applied_through` are safe to drop from the queue. |

---

## Error Handling

| Scenario | Primary behaviour | Replica behaviour |
|---|---|---|
| Replica gRPC server unreachable | Log `WARN`; retain queue; retry after 100 ms | — |
| Replica gRPC call returns error status | Log `WARN`; retain queue; retry after 100 ms | — |
| Unknown `operation` variant in an entry | — | Return `Status::invalid_argument`; log `ERROR`; preceding entries in the batch are already applied |
| Write-lock timeout on replica | Return `Status::resource_exhausted`; log `ERROR` | Primary logs `WARN`; retains queue; retries |
| Empty batch sent | Return `applied_through: 0`; no-op | — |

All errors are non-fatal to the primary. The HTTP client never observes replication failures.

---

## `ServerConfig` Changes

```rust
pub struct ServerConfig {
    pub address:   SocketAddr,         // HTTP bind address (unchanged)
    pub role:      NodeRole,           // unchanged
    pub topology:  Option<Topology>,   // unchanged
    pub grpc_addr: Option<SocketAddr>, // NEW: gRPC bind address (replica only)
}
```

The CLI binary wires `--replica-grpc-addr` into `ServerConfig.grpc_addr` when `--role replica`.

---

## Testing

### Unit Tests (`transdb-server/tests/unit_replication.rs`)

**`ReplicationService`:**
- `replicate_batch` with a single `PutOp` inserts the entry; returns `applied_through` equal to that entry's seq.
- `replicate_batch` with a `PutOp` overwrites an existing entry unconditionally.
- `replicate_batch` with a `DeleteOp` removes a present key.
- `replicate_batch` with a `DeleteOp` on a missing key returns `Ok` (no-op).
- `replicate_batch` with a mixed batch of PutOps and DeleteOps applies them in order.
- `replicate_batch` with an empty batch returns `applied_through: 0`.
- `replicate_batch` with a missing `operation` field returns `Status::invalid_argument`.

**`ReplicationClient`:**
- `enqueue` adds an entry to the queue with the next sequence number.
- Calling `enqueue` twice assigns ascending, distinct sequence numbers.
- After a simulated ACK of `applied_through: N`, entries with seq ≤ N are removed; entries with seq > N remain.

### Unit Tests (`transdb-server/tests/unit_server.rs` additions)

- `handle_put` enqueues a replication mutation when `AppState.replication` is `Some`.
- `handle_put` does **not** enqueue a second mutation on an idempotency replay.
- `handle_delete` enqueues a replication mutation when `AppState.replication` is `Some`.
- `handle_put` / `handle_delete` succeed normally when `AppState.replication` is `None`.

### Integration Tests (`transdb-integration-tests/tests/integration_test.rs` additions)

- After a PUT to the primary, a GET on the replica eventually returns the same value (poll with short sleep, up to 1 s).
- After a DELETE on the primary, a GET on the replica eventually returns 404 (same pattern).
- Replica still returns 405 for external PUT and DELETE requests.

---

## Commit Plan

Two commits split at the shared-crate boundary, since `transdb-common` must compile cleanly before `transdb-server` can depend on the new field.

1. `feat(common): add replica_grpc_addr to Topology`
   Add `replica_grpc_addr: Option<String>` to `Topology` in `transdb-common`; update topology unit tests to cover serialisation round-trips with and without the new field.

2. `feat(server): implement gRPC replication stream`
   Add `build.rs`, proto schema, `ReplicationClient`, and `ReplicationService` inside `transdb-server`; wire `AppState.replication`, handler `enqueue` calls, `ServerConfig.grpc_addr`, CLI flag, and replica gRPC server startup; add `unit_replication.rs` and extend `unit_server.rs` and integration tests.

---

## Out of Scope

- **Primary-restart durability**: The pending queue is in-memory; mutations unACKed at restart time are lost.
- **Backpressure**: The primary does not throttle writes if the replica falls behind.
- **Full-sync on replica startup**: A freshly started replica begins with an empty store; there is no initial bulk-sync from the primary.
- **TLS**: The gRPC channel uses plain-text `h2c`; TLS is deferred.
- **Idempotency cache replication**: Only key-value data is replicated.
- **Multiple replicas**: The topology supports one replica; fan-out to N replicas is deferred.
- **Linearizability**: Replica reads may return stale data; full read-your-writes consistency is deferred.

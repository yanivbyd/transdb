# TransDB

A distributed, in-memory key-value database with transaction support, written in Rust. Built incrementally, starting with a single-node MVP and expanding toward multi-node distribution and transactions.

## Data Model

- **Keys**: UTF-8 strings, max 1KB
- **Values**: Raw byte arrays, max 4MB
- **Storage**: In-memory only (no durability in Phase 1)
- **Namespace**: Single flat key space

## HTTP API

| Method | Path | Body | Success | Error |
|---|---|---|---|---|
| `GET` | `/keys/{key}` | — | `200 OK` + raw bytes | `404 Not Found` |
| `PUT` | `/keys/{key}` | Raw bytes | `200 OK` | — |
| `DELETE` | `/keys/{key}` | — | `204 No Content` | — |

All endpoints return `503 Service Unavailable` if the internal lock cannot be acquired within 1 second.

PUT overwrites silently if the key already exists. DELETE is idempotent — deleting a non-existent key returns `204`.

## Project Structure

```
transdb-server/            HTTP server with in-memory store
transdb-client/            Rust client library
transdb-common/            Shared types and error definitions
transdb-integration-tests/ End-to-end tests
transdb-stress-tests/      Stress test harness (builds and drives a live cluster)
```

## Running

```bash
cargo build
cargo test --workspace
```

## Development

```bash
just build              # build all crates
just test               # run all tests
just integration-test   # run integration tests only
just coverage           # run tests with coverage report (opens browser)
just stress-test        # run stress tests with defaults (30 s, balanced workload)
```

Stress test options (forwarded after `--`):

```bash
just stress-test --duration 60 --workload write-heavy --key-space 500
just stress-test --max-error-rate 0.05 --max-violations 0
```

Available workload profiles: `read-heavy`, `balanced`, `write-heavy`, `put-only`.

The harness builds the server binary itself, spawns a primary + replica cluster, runs the worker loop, then prints a pass/fail report. Exit codes: 0 = pass, 1 = error rate exceeded, 2 = correctness violations, 3 = server build/startup failed.

> Requires [just](https://github.com/casey/just) (`brew install just`) and [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) (`cargo install cargo-llvm-cov`).

## Architecture

### Phase 1 (current)
- Single server process with a `tokio::sync::RwLock<HashMap>` store
- HTTP/REST protocol between client and server
- Concurrent reads, serialised writes via `RwLock`

### Future Phases
- **Transactions**: Multi-key atomic operations with 2-phase commit
- **Distribution**: Multiple nodes, key sharding, node-to-node RPC
- **Cloud**: AWS deployment via CDK
- **Observability**: Health checks, metrics, failure detection

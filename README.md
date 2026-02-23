# TranDB

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
trandb-server/            HTTP server with in-memory store
trandb-client/            Rust client library
trandb-common/            Shared types and error definitions
trandb-integration-tests/ End-to-end tests
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
```

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

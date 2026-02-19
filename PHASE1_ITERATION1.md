# Phase 1 - Iteration 1: Minimal Server/Client Implementation

## Overview

This is the first implementation iteration for TranDB Phase 1. The goal is to establish the basic server/client architecture with minimal functionality - a "walking skeleton" that can be built upon.

## Architecture Decisions

**Protocol Layers:**
- **Client ↔ Server**: REST/HTTP (simple, easy to debug, standard)
- **Node ↔ Node**: TCP/Binary Protocol (future, for efficient internal communication)

**Project Structure:**
- Monorepo with multiple Rust crates
- crates: `trandb-server`, `trandb-client`, `trandb-common`

**Configuration:**
- Server address/port is configurable
- default: `127.0.0.1:8080`

**Error Handling:**
- Simplest possible: Standard HTTP 404 status code for "key not found"

## Scope

### What's Included
- **Server Library**: Core server logic with HTTP listener
- **Client Library**: HTTP client with request logic
- **Single Operation**: GET `/keys/{key}` endpoint only
- **Behavior**: All GET requests return HTTP 404 (key not found)
- **Unit Tests**: Tests for both server and client components
- **Project Structure**: Monorepo with multiple crates

### What's Excluded
- Actual storage (no HashMap yet)
- PUT/DELETE operations
- Real key lookups
- Error handling beyond basic HTTP 404
- Advanced configuration management
- Logging/observability
- Integration tests (separate commit)

## Implementation Details

### Server
- HTTP server listening on configurable address/port
- Handle GET `/keys/{key}` endpoint
- Always respond with HTTP 404 status
- Handle concurrent connections

### Client
- HTTP client library
- Provide `get(key)` method
- Parse HTTP responses
- Return appropriate error for 404

### Testing
- **Unit tests only** (integration tests in future commit)
- Test server request handling logic
- Test client request building and response parsing
- Mock HTTP interactions where appropriate
- Unit tests reside in dedicated files (e.g. `tests/unit_foo.rs`), NOT in inline `#[cfg(test)]` modules

## Success Criteria

- `cargo build` succeeds for all crates
- `cargo test` passes all unit tests
- Server library can be instantiated with configurable address/port
- Client library can construct GET requests
- GET endpoint returns HTTP 404 status
- Clean separation between server, client, and common code in separate crates
- All code changes are covered by unit tests in dedicated test files

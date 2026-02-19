# TranDB - Distributed Memory Database Specification

## Overview

TranDB is a distributed, in-memory key-value database with transaction support, written in Rust. It prioritizes simplicity and incremental development, starting with core functionality and expanding over time.

## Core Requirements

### Data Model
- **Key-Value Store**: Simple string keys mapping to byte array values
- **In-Memory**: All data stored in RAM (no durability in Phase 1)
- **Namespace**: Single flat key space initially
- **Constraints**:
  - Maximum key size: 1KB
  - Maximum value size: 4MB

### Transaction Model (Future Phase)
- **Multi-Key Transactions**: Atomic operations across multiple keys
- **Operations**: GET, PUT, DELETE within transactions
- **Isolation**: will be referenced in future specs
- **Atomicity**: All-or-nothing commit/abort semantics
- **API**: Begin transaction, execute operations, commit/rollback
- **Note**: Transaction support is NOT included in Phase 1

### Distribution Model
- **Two Deployment Modes**:
  1. **Local Mode**: Multiple processes on single machine (development/testing)
  2. **Cloud Mode**: AWS deployment via CDK (production) - not in Phase 1
- **Node Communication**: Internal RPC protocol for node-to-node communication
- **Sharding**: Not in Phase 1. All data will be in a single node initially, but internal architecture should be designed with future sharding in mind

## Architecture

### Components

#### 1. Storage Engine
- In-memory hash map per node
- Thread-safe concurrent access
- Designed with future sharding in mind (single shard in Phase 1)

#### 2. Transaction Coordinator (Future Phase)
- Distributed transaction protocol (2-phase commit)
- Transaction ID generation and tracking
- Lock management per key

#### 3. Network Layer
- **Protocol**: Protocol Buffers (protobuf) over TCP
- **Error Handling**: Simple error responses (start with basic error messages)
- Node discovery and membership (will use a simple     plane for node discovery)
- Request routing based on key ownership
- Health checking and failure detection (not in phase 1)

#### 4. Client Interface
- Rust client library
- Protobuf-based protocol over TCP
- Connection pooling

### System Flow
```
Client → [Local Node] → Determine Key Ownership → [Coordinate with Owner Nodes] → Execute Transaction → Return Result
```

## Phase 1: Minimum Viable Product

### In Scope
1. **Single-Node Mode**
   - Local in-memory key-value store
   - Single server process
   - Basic GET/PUT/DELETE operations
   - Simple synchronous operations

2. **Client Library**
   - Connect to single node
   - Simple key-value API (no transactions yet)
   - Synchronous operations

3. **Data Structures**
   - HashMap for storage
   - RwLock or Mutex for thread-safe access
   - Architecture designed to accommodate future sharding

### Out of Scope (Future Phases)
- Transactions
- Durability/persistence
- Replication
- Multi-node distribution
- Advanced isolation levels
- Query language beyond key-value
- Authentication/authorization
- Monitoring/observability

use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::Request;
use transdb_server::{
    replication::{
        proto::{
            mutation_entry::Operation, replication_server::Replication, DeleteOp, MutationEntry,
            PutOp, ReplicateBatchRequest,
        },
        service::ReplicationReceiver,
        is_valid_replicate_batch_request,
    },
    DbState,
};

fn make_service() -> ReplicationReceiver {
    ReplicationReceiver::new(Arc::new(RwLock::new(DbState {
        store: std::collections::HashMap::new(),
        idempotency_cache: std::collections::HashMap::new(),
        next_version: 0,
    })))
}

fn put_entry(seq: u64, key: &str, value: &[u8], version: u64) -> MutationEntry {
    MutationEntry {
        seq,
        operation: Some(Operation::Put(PutOp {
            key: key.to_string(),
            value: value.to_vec(),
            version,
            expires_at: None,
        })),
    }
}

fn delete_entry(seq: u64, key: &str, version: u64, expires_at: u64) -> MutationEntry {
    MutationEntry {
        seq,
        operation: Some(Operation::Delete(DeleteOp {
            key: key.to_string(),
            version,
            expires_at,
        })),
    }
}

#[tokio::test]
async fn test_replication_service() {
    // single PutOp inserts and returns correct last_seq
    let svc = make_service();
    let resp = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![put_entry(1, "k", b"v", 1)],
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.last_seq, 1);
    {
        let db = svc.db.read().await;
        let entry = db.store.get("k").unwrap();
        assert_eq!(entry.value.as_deref().unwrap(), b"v");
        assert_eq!(entry.version, 1);
        assert!(entry.expires_at.is_none());
    }

    // PutOp overwrites unconditionally
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![put_entry(2, "k", b"second", 2)],
    }))
    .await
    .unwrap();
    {
        let db = svc.db.read().await;
        let entry = db.store.get("k").unwrap();
        assert_eq!(entry.value.as_deref().unwrap(), b"second");
        assert_eq!(entry.version, 2);
    }

    // DeleteOp writes tombstone for a present key
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![delete_entry(3, "k", 3, 9999)],
    }))
    .await
    .unwrap();
    {
        let db = svc.db.read().await;
        let entry = db.store.get("k").unwrap();
        assert!(entry.value.is_none());
        assert_eq!(entry.version, 3);
        assert_eq!(entry.expires_at, Some(9999));
    }

    // DeleteOp writes tombstone when key is absent
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![delete_entry(4, "missing", 5, 8888)],
    }))
    .await
    .unwrap();
    {
        let db = svc.db.read().await;
        let entry = db.store.get("missing").unwrap();
        assert!(entry.value.is_none());
        assert_eq!(entry.version, 5);
        assert_eq!(entry.expires_at, Some(8888));
    }
}

#[tokio::test]
async fn test_version_checks() {
    let svc = make_service();

    // seed "k" at version 5
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![put_entry(5, "k", b"v", 5)],
    }))
    .await
    .unwrap();

    // stale put (incoming version < existing) returns failed_precondition
    let err = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![put_entry(6, "k", b"new", 4)],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);

    // same version, same content: idempotent — succeeds and store is unchanged
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![put_entry(5, "k", b"v", 5)],
    }))
    .await
    .unwrap();
    assert_eq!(svc.db.read().await.store.get("k").unwrap().version, 5);

    // same version, different content: returns failed_precondition
    let err = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![put_entry(5, "k", b"other", 5)],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);

    // seed tombstone for "d" at version 3
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![delete_entry(3, "d", 3, 9999)],
    }))
    .await
    .unwrap();

    // stale delete (incoming version < existing) returns failed_precondition
    let err = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![delete_entry(4, "d", 2, 9999)],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);

    // same version, same tombstone content: idempotent — succeeds
    svc.replicate_batch(Request::new(ReplicateBatchRequest {
        entries: vec![delete_entry(3, "d", 3, 9999)],
    }))
    .await
    .unwrap();

    // same version, different expires_at: returns failed_precondition
    let err = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![delete_entry(3, "d", 3, 1111)],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn test_mixed_batch_applies_in_order() {
    let svc = make_service();
    let resp = svc
        .replicate_batch(Request::new(ReplicateBatchRequest {
            entries: vec![
                put_entry(1, "k", b"v", 1),
                delete_entry(2, "k", 2, 7777),
            ],
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.last_seq, 2);
    let db = svc.db.read().await;
    let entry = db.store.get("k").unwrap();
    assert!(entry.value.is_none());
    assert_eq!(entry.version, 2);
}

#[tokio::test]
async fn test_empty_batch_returns_zero() {
    let svc = make_service();
    let resp = svc
        .replicate_batch(Request::new(ReplicateBatchRequest { entries: vec![] }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.last_seq, 0);
}

#[test]
fn test_is_valid_replicate_batch_request() {
    // empty batch is valid
    assert!(is_valid_replicate_batch_request(&ReplicateBatchRequest { entries: vec![] }));

    // all entries have operations and per-key seqs are increasing
    assert!(is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![put_entry(1, "k", b"v", 1), delete_entry(2, "k", 2, 9999)],
    }));

    // different keys with non-globally-ordered seqs are valid (per-key check only)
    assert!(is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![put_entry(3, "a", b"v", 1), put_entry(1, "b", b"v", 1)],
    }));

    // single None operation is invalid
    assert!(!is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![MutationEntry { seq: 1, operation: None }],
    }));

    // None after valid entries is invalid
    assert!(!is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![
            put_entry(1, "k", b"v", 1),
            MutationEntry { seq: 2, operation: None },
        ],
    }));

    // duplicate seq for the same key is invalid
    assert!(!is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![put_entry(1, "k", b"v", 1), put_entry(1, "k", b"v2", 2)],
    }));

    // decreasing seq for the same key is invalid
    assert!(!is_valid_replicate_batch_request(&ReplicateBatchRequest {
        entries: vec![put_entry(2, "k", b"v", 1), put_entry(1, "k", b"v2", 2)],
    }));
}

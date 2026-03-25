use axum::body::Bytes;
use std::cmp::Ordering;
use tonic::{Request, Response, Status};

use crate::config::LOCK_TIMEOUT;
use crate::replication::{
    is_valid_replicate_batch_request,
    proto::{
        mutation_entry::Operation, replication_server::Replication, ReplicateBatchRequest,
        ReplicateBatchResponse,
    },
};
use crate::{Db, Entry};

/// Compares `incoming_version` against `existing_version`.
/// - `Less`: existing is older, safe to overwrite — returns `Ok(true)`.
/// - `Equal`: same version, caller must verify content matches — returns `Ok(false)`.
/// - `Greater`: existing is newer, incoming is stale — returns `Err`.
fn check_version(existing_version: u64, incoming_version: u64) -> Result<bool, Status> {
    match existing_version.cmp(&incoming_version) {
        Ordering::Less => Ok(true),
        Ordering::Equal => Ok(false),
        Ordering::Greater => Err(Status::failed_precondition(
            "existing entry has a newer version than the incoming operation",
        )),
    }
}

pub struct ReplicationReceiver {
    pub db: Db,
}

impl ReplicationReceiver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[tonic::async_trait]
impl Replication for ReplicationReceiver {
    async fn replicate_batch(
        &self,
        request: Request<ReplicateBatchRequest>,
    ) -> Result<Response<ReplicateBatchResponse>, Status> {
        let inner = request.into_inner();

        if !is_valid_replicate_batch_request(&inner) {
            return Err(Status::invalid_argument("Missing operation in MutationEntry"));
        }

        if inner.entries.is_empty() {
            return Ok(Response::new(ReplicateBatchResponse { last_seq: 0 }));
        }

        let entries = inner.entries;

        let mut db = tokio::time::timeout(LOCK_TIMEOUT, self.db.write())
            .await
            .map_err(|_| Status::unavailable("Lock acquisition timed out"))?;

        let mut last_seq: u64 = 0;
        for entry in entries {
            let seq = entry.seq;
            match entry.operation {
                Some(Operation::Put(op)) => {
                    let should_insert = match db.store.get(&op.key) {
                        None => true,
                        Some(existing) => {
                            let overwrite = check_version(existing.version, op.version)?;
                            if !overwrite {
                                // same version: contents must match
                                if existing.value.as_deref() != Some(op.value.as_slice())
                                    || existing.expires_at != op.expires_at
                                {
                                    return Err(Status::failed_precondition(
                                        "version conflict: same version but different content",
                                    ));
                                }
                            }
                            overwrite
                        }
                    };
                    if should_insert {
                        db.store.insert(
                            op.key,
                            Entry {
                                value: Some(Bytes::from(op.value)),
                                version: op.version,
                                expires_at: op.expires_at,
                            },
                        );
                    }
                    last_seq = seq;
                }
                Some(Operation::Delete(op)) => {
                    let should_insert = match db.store.get(&op.key) {
                        None => true,
                        Some(existing) => {
                            let overwrite = check_version(existing.version, op.version)?;
                            if !overwrite {
                                // same version: must already be a tombstone with matching expires_at
                                if existing.value.is_some()
                                    || existing.expires_at != Some(op.expires_at)
                                {
                                    return Err(Status::failed_precondition(
                                        "version conflict: same version but different content",
                                    ));
                                }
                            }
                            overwrite
                        }
                    };
                    if should_insert {
                        db.store.insert(
                            op.key,
                            Entry {
                                value: None,
                                version: op.version,
                                expires_at: Some(op.expires_at),
                            },
                        );
                    }
                    last_seq = seq;
                }
                None => unreachable!("validated via is_valid_replicate_batch_request"),
            }
        }

        Ok(Response::new(ReplicateBatchResponse { last_seq }))
    }
}

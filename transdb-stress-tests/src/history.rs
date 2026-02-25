use std::collections::HashMap;
use std::time::Instant;

pub enum OpKind {
    Put,
    Get,
    GetAllowingExpired,
    Delete,
}

pub enum OpOutcome {
    /// The PUT succeeded. `value` is what was written (needed for correctness checking).
    PutOk { version: u64, value: Vec<u8> },
    GetOk { version: u64, value: Vec<u8> },
    NotFound,
    DeleteOk,
    /// 5xx or network failure.
    Error,
}

pub struct OpRecord {
    /// When the client sent the request.
    pub client_start_ts: Instant,
    /// When the client received the response (the ACK).
    pub client_ack_ts: Instant,
    pub key: String,
    pub kind: OpKind,
    pub outcome: OpOutcome,
}

pub struct History(pub Vec<OpRecord>);

pub enum ViolationKind {
    /// GET returned a version for which no PUT was ever recorded.
    VersionNotFound { actual: Vec<u8> },
    /// GET fully completed before the corresponding PUT even started — the server
    /// could not have had that data yet.
    ReadBeforeWriteStart { put_start_ts: Instant, get_ack_ts: Instant },
    /// GET returned the correct version but the bytes differ from what was PUT.
    ValueMismatch { expected: Vec<u8>, actual: Vec<u8> },
    /// GET returned stale data. Not counted as an error by default (eventual consistency).
    /// `latest_known_version` is `Some(v)` when a newer version was already ACKed,
    /// or `None` when the key was deleted before the GET started.
    StaleDataReturned { latest_known_version: Option<u64> },
}

pub struct Violation {
    pub key: String,
    pub version: u64,
    pub kind: ViolationKind,
}

/// Entry in the write index.
struct PutEntry {
    value: Vec<u8>,
    put_start_ts: Instant,
    put_ack_ts: Instant,
}

impl History {
    /// Check every successful GET against the write and delete indexes.
    /// Returns one [`Violation`] per inconsistent GET, with [`ViolationKind::StaleDataReturned`]
    /// reported separately (informational only — not counted as an error by default).
    pub fn check_correctness(&self) -> Vec<Violation> {
        // Step 1a: build write index — (key, version) → PutEntry.
        let mut write_index: HashMap<(String, u64), PutEntry> = HashMap::new();
        for record in &self.0 {
            if let OpOutcome::PutOk { version, value } = &record.outcome {
                write_index.insert(
                    (record.key.clone(), *version),
                    PutEntry {
                        value: value.clone(),
                        put_start_ts: record.client_start_ts,
                        put_ack_ts: record.client_ack_ts,
                    },
                );
            }
        }

        // Step 1b: build delete index — key → all delete_client_ack_ts values.
        let mut delete_index: HashMap<String, Vec<Instant>> = HashMap::new();
        for record in &self.0 {
            if matches!(record.outcome, OpOutcome::DeleteOk) {
                delete_index.entry(record.key.clone()).or_default().push(record.client_ack_ts);
            }
        }

        // Step 2: check every GetOk against both indexes.
        let mut violations = Vec::new();
        for record in &self.0 {
            if let OpOutcome::GetOk { version, value } = &record.outcome {
                match write_index.get(&(record.key.clone(), *version)) {
                    None => {
                        violations.push(Violation {
                            key: record.key.clone(),
                            version: *version,
                            kind: ViolationKind::VersionNotFound { actual: value.clone() },
                        });
                    }
                    Some(entry) => {
                        if record.client_ack_ts < entry.put_start_ts {
                            // GET fully completed before the PUT even started.
                            violations.push(Violation {
                                key: record.key.clone(),
                                version: *version,
                                kind: ViolationKind::ReadBeforeWriteStart {
                                    put_start_ts: entry.put_start_ts,
                                    get_ack_ts: record.client_ack_ts,
                                },
                            });
                        } else if &entry.value != value {
                            violations.push(Violation {
                                key: record.key.clone(),
                                version: *version,
                                kind: ViolationKind::ValueMismatch {
                                    expected: entry.value.clone(),
                                    actual: value.clone(),
                                },
                            });
                        } else if superseding_delete(
                            &delete_index,
                            &record.key,
                            entry.put_start_ts,
                            record.client_start_ts,
                        ).is_some() {
                            // Key was deleted after this version was written and before GET started.
                            violations.push(Violation {
                                key: record.key.clone(),
                                version: *version,
                                kind: ViolationKind::StaleDataReturned {
                                    latest_known_version: None,
                                },
                            });
                        } else if let Some(latest) = latest_known_version(
                            &write_index,
                            &record.key,
                            *version,
                            record.client_start_ts,
                        ) {
                            violations.push(Violation {
                                key: record.key.clone(),
                                version: *version,
                                kind: ViolationKind::StaleDataReturned {
                                    latest_known_version: Some(latest),
                                },
                            });
                        }
                    }
                }
            }
        }

        violations
    }
}

/// Returns the `client_ack_ts` of the first delete for `key` that was ACKed
/// strictly after `put_start_ts` and strictly before `get_start_ts`.
/// Returns `None` if no such delete exists.
fn superseding_delete(
    delete_index: &HashMap<String, Vec<Instant>>,
    key: &str,
    put_start_ts: Instant,
    get_start_ts: Instant,
) -> Option<Instant> {
    delete_index
        .get(key)?
        .iter()
        .find(|&&del_ack| del_ack > put_start_ts && del_ack < get_start_ts)
        .copied()
}

/// Returns the highest version for `key` (greater than `returned_version`) whose PUT was
/// ACKed before `get_start_ts`. `None` if the returned version is already the latest known.
fn latest_known_version(
    write_index: &HashMap<(String, u64), PutEntry>,
    key: &str,
    returned_version: u64,
    get_start_ts: Instant,
) -> Option<u64> {
    write_index
        .iter()
        .filter(|((k, v), entry)| {
            k == key && *v > returned_version && entry.put_ack_ts < get_start_ts
        })
        .map(|((_, v), _)| *v)
        .max()
}

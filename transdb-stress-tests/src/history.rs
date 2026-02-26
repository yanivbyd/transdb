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
    DeleteOk { version: u64 },
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
    /// GET fully completed before the corresponding write even started — the server
    /// could not have had that data yet.
    ReadBeforeWriteStart { put_start_ts: Instant, get_ack_ts: Instant },
    /// GET returned the correct version but the bytes differ from what was PUT.
    ValueMismatch { expected: Vec<u8>, actual: Vec<u8> },
    /// GET returned stale data. Not counted as an error by default (eventual consistency).
    /// `latest_known_version` is the highest write version (PUT or tombstone) that was
    /// already ACKed before the GET started.
    StaleDataReturned { latest_known_version: u64 },
}

pub struct Violation {
    pub key: String,
    pub version: u64,
    pub kind: ViolationKind,
}

/// The data payload of a write operation.
enum WriteValue {
    Data(Vec<u8>),
    Tombstone,
}

/// Entry in the unified write index.
struct WriteEntry {
    write_value: WriteValue,
    write_start_ts: Instant,
    write_ack_ts: Instant,
}

impl History {
    /// Check every successful GET against the unified write index.
    /// Returns one [`Violation`] per inconsistent GET, with [`ViolationKind::StaleDataReturned`]
    /// reported separately (informational only — not counted as an error by default).
    pub fn check_correctness(&self) -> Vec<Violation> {
        let write_index = build_write_index(&self.0);

        self.0
            .iter()
            .filter_map(|r| {
                if let OpOutcome::GetOk { version, value } = &r.outcome {
                    classify_get(
                        &r.key, *version, value,
                        r.client_start_ts, r.client_ack_ts,
                        &write_index,
                    )
                    .map(|kind| Violation { key: r.key.clone(), version: *version, kind })
                } else {
                    None
                }
            })
            .collect()
    }
}

// --- Index builder ---

/// (key, version) → the write (PUT or DELETE/tombstone) that produced that version.
///
/// With a global monotonic version counter, each `(key, version)` pair is unique across
/// all time, so each entry maps to exactly one `WriteEntry`.
fn build_write_index(records: &[OpRecord]) -> HashMap<(String, u64), WriteEntry> {
    let mut index: HashMap<(String, u64), WriteEntry> = HashMap::new();
    for r in records {
        match &r.outcome {
            OpOutcome::PutOk { version, value } => {
                index.insert(
                    (r.key.clone(), *version),
                    WriteEntry {
                        write_value: WriteValue::Data(value.clone()),
                        write_start_ts: r.client_start_ts,
                        write_ack_ts: r.client_ack_ts,
                    },
                );
            }
            OpOutcome::DeleteOk { version } => {
                index.insert(
                    (r.key.clone(), *version),
                    WriteEntry {
                        write_value: WriteValue::Tombstone,
                        write_start_ts: r.client_start_ts,
                        write_ack_ts: r.client_ack_ts,
                    },
                );
            }
            _ => {}
        }
    }
    index
}

// --- Per-GET classification ---

/// Returns the violation kind for a single GET result, or `None` if it is consistent.
fn classify_get(
    key: &str,
    version: u64,
    value: &[u8],
    get_start: Instant,
    get_ack: Instant,
    write_index: &HashMap<(String, u64), WriteEntry>,
) -> Option<ViolationKind> {
    // 1. No write (PUT or DELETE) ever produced this (key, version).
    let Some(entry) = write_index.get(&(key.to_owned(), version)) else {
        return Some(ViolationKind::VersionNotFound { actual: value.to_vec() });
    };

    // 2. Write started after GET was fully acked — server could not have had the data yet.
    if entry.write_start_ts > get_ack {
        return Some(ViolationKind::ReadBeforeWriteStart {
            put_start_ts: entry.write_start_ts,
            get_ack_ts: get_ack,
        });
    }

    // 3. Write was acked before GET started — definitive observation window.
    if entry.write_ack_ts <= get_start {
        // 3a. Value mismatch (only applicable to data writes; tombstone versions should
        //     never appear as a GetOk in a correctly-behaving system).
        match &entry.write_value {
            WriteValue::Data(expected) if expected != value => {
                return Some(ViolationKind::ValueMismatch {
                    expected: expected.clone(),
                    actual: value.to_vec(),
                });
            }
            WriteValue::Tombstone => {
                // A tombstone version was returned as data — no data write ever
                // produced this version.
                return Some(ViolationKind::VersionNotFound { actual: value.to_vec() });
            }
            _ => {}
        }

        // 3b. A newer write (PUT or tombstone) was already ACKed before GET started.
        if let Some(latest) = newer_write_acked(write_index, key, version, get_start) {
            return Some(ViolationKind::StaleDataReturned { latest_known_version: latest });
        }
    }

    // 4. Write/GET overlap — ambiguous, no violation.
    None
}

// --- Helper ---

/// Returns the highest version for `key` greater than `returned_version` for which the
/// write (PUT or DELETE/tombstone) was ACKed before `get_start_ts`, or `None` if
/// `returned_version` is already the latest known.
fn newer_write_acked(
    write_index: &HashMap<(String, u64), WriteEntry>,
    key: &str,
    returned_version: u64,
    get_start_ts: Instant,
) -> Option<u64> {
    write_index
        .iter()
        .filter(|((k, v), entry)| {
            k == key && *v > returned_version && entry.write_ack_ts < get_start_ts
        })
        .map(|((_, v), _)| *v)
        .max()
}

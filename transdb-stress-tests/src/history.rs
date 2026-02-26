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

/// Entry in the delete index.
struct DeleteEntry {
    del_start_ts: Instant,
    del_ack_ts: Instant,
}

impl History {
    /// Check every successful GET against the write and delete indexes.
    /// Returns one [`Violation`] per inconsistent GET, with [`ViolationKind::StaleDataReturned`]
    /// reported separately (informational only — not counted as an error by default).
    pub fn check_correctness(&self) -> Vec<Violation> {
        let write_index = build_write_index(&self.0);
        let delete_index = build_delete_index(&self.0);

        self.0
            .iter()
            .filter_map(|r| {
                if let OpOutcome::GetOk { version, value } = &r.outcome {
                    classify_get(
                        &r.key, *version, value,
                        r.client_start_ts, r.client_ack_ts,
                        &write_index, &delete_index,
                    )
                    .map(|kind| Violation { key: r.key.clone(), version: *version, kind })
                } else {
                    None
                }
            })
            .collect()
    }
}

// --- Index builders ---

/// (key, version) → every PUT that produced that version.
///
/// Per-key version counters reset to 1 after a DELETE, so the same (key, version) pair
/// can appear more than once.  Collecting all of them lets `classify_get` use timestamps
/// to find the specific PUT the GET actually observed.
fn build_write_index(records: &[OpRecord]) -> HashMap<(String, u64), Vec<PutEntry>> {
    let mut index: HashMap<(String, u64), Vec<PutEntry>> = HashMap::new();
    for r in records {
        if let OpOutcome::PutOk { version, value } = &r.outcome {
            index
                .entry((r.key.clone(), *version))
                .or_default()
                .push(PutEntry {
                    value: value.clone(),
                    put_start_ts: r.client_start_ts,
                    put_ack_ts: r.client_ack_ts,
                });
        }
    }
    index
}

/// key → start/ack timestamps of every successful DELETE.
fn build_delete_index(records: &[OpRecord]) -> HashMap<String, Vec<DeleteEntry>> {
    let mut index: HashMap<String, Vec<DeleteEntry>> = HashMap::new();
    for r in records {
        if matches!(r.outcome, OpOutcome::DeleteOk { .. }) {
            index.entry(r.key.clone()).or_default().push(DeleteEntry {
                del_start_ts: r.client_start_ts,
                del_ack_ts: r.client_ack_ts,
            });
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
    write_index: &HashMap<(String, u64), Vec<PutEntry>>,
    delete_index: &HashMap<String, Vec<DeleteEntry>>,
) -> Option<ViolationKind> {
    // 1. No PUT ever produced this (key, version).
    let Some(entries) = write_index.get(&(key.to_owned(), version)) else {
        return Some(ViolationKind::VersionNotFound { actual: value.to_vec() });
    };

    // 2. Find the most recently *started* PUT that was acked before this GET started.
    //    Using put_start_ts as the sort key — not put_ack_ts — because start time is a
    //    better proxy for server execution order: ack time reflects return-path network
    //    delay, which can make an earlier write appear to ack later than a subsequent one.
    let entry = entries
        .iter()
        .filter(|e| e.put_ack_ts <= get_start)
        .max_by_key(|e| e.put_start_ts);

    // 3. No PUT was acked before GET started.
    let entry = match entry {
        Some(e) => e,
        None => {
            // If every PUT for this version also started after the GET was fully acked,
            // the server could not have had this data yet — definite violation.
            let earliest = entries.iter().min_by_key(|e| e.put_start_ts).unwrap();
            if earliest.put_start_ts > get_ack {
                return Some(ViolationKind::ReadBeforeWriteStart {
                    put_start_ts: earliest.put_start_ts,
                    get_ack_ts: get_ack,
                });
            }
            // GET and PUT windows overlapped — ambiguous, not a violation.
            return None;
        }
    };

    // 4. Value returned by GET differs from what was PUT.
    if entry.value != value {
        return Some(ViolationKind::ValueMismatch {
            expected: entry.value.clone(),
            actual: value.to_vec(),
        });
    }

    // 5. A DELETE definitively started after this PUT finished and before the GET started.
    if superseding_delete(delete_index, key, entry.put_ack_ts, get_start).is_some() {
        return Some(ViolationKind::StaleDataReturned { latest_known_version: None });
    }

    // 6. A newer version was already ACKed before the GET started.
    if let Some(latest) = latest_known_version(write_index, key, version, get_start) {
        return Some(ViolationKind::StaleDataReturned { latest_known_version: Some(latest) });
    }

    None
}

// --- Helpers ---

/// Returns `Some` if there is a DELETE for `key` that definitively started after the PUT
/// finished (`del_start > put_ack_ts`) and was ACKed before the GET started
/// (`del_ack < get_start_ts`).  Both conditions are required to rule out overlap with
/// either the PUT or the GET.
fn superseding_delete(
    delete_index: &HashMap<String, Vec<DeleteEntry>>,
    key: &str,
    put_ack_ts: Instant,
    get_start_ts: Instant,
) -> Option<()> {
    delete_index
        .get(key)?
        .iter()
        .find(|e| e.del_start_ts >= put_ack_ts && e.del_ack_ts < get_start_ts)
        .map(|_| ())
}

/// Returns the highest version for `key` greater than `returned_version` for which at
/// least one PUT was ACKed before `get_start_ts`, or `None` if `returned_version` is
/// already the latest known.
fn latest_known_version(
    write_index: &HashMap<(String, u64), Vec<PutEntry>>,
    key: &str,
    returned_version: u64,
    get_start_ts: Instant,
) -> Option<u64> {
    write_index
        .iter()
        .filter(|((k, v), entries)| {
            k == key
                && *v > returned_version
                && entries.iter().any(|e| e.put_ack_ts < get_start_ts)
        })
        .map(|((_, v), _)| *v)
        .max()
}

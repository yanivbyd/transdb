use std::time::{Duration, Instant};
use transdb_stress_tests::history::{History, OpKind, OpOutcome, OpRecord, ViolationKind};

fn put(key: &str, version: u64, value: &[u8], start: Instant, ack: Instant) -> OpRecord {
    OpRecord {
        client_start_ts: start,
        client_ack_ts: ack,
        key: key.to_string(),
        kind: OpKind::Put,
        outcome: OpOutcome::PutOk { version, value: value.to_vec() },
    }
}

fn get(key: &str, version: u64, value: &[u8], start: Instant, ack: Instant) -> OpRecord {
    OpRecord {
        client_start_ts: start,
        client_ack_ts: ack,
        key: key.to_string(),
        kind: OpKind::Get,
        outcome: OpOutcome::GetOk { version, value: value.to_vec() },
    }
}

fn delete(key: &str, start: Instant, ack: Instant) -> OpRecord {
    OpRecord {
        client_start_ts: start,
        client_ack_ts: ack,
        key: key.to_string(),
        kind: OpKind::Delete,
        outcome: OpOutcome::DeleteOk,
    }
}

fn after(t: Instant) -> Instant {
    t + Duration::from_millis(1)
}

fn ts6() -> (Instant, Instant, Instant, Instant, Instant, Instant) {
    let t0 = Instant::now();
    (t0, after(t0), after(after(t0)), after(after(after(t0))),
     after(after(after(after(t0)))), after(after(after(after(after(t0))))))
}

fn ts7() -> (Instant, Instant, Instant, Instant, Instant, Instant, Instant) {
    let (t0, t1, t2, t3, t4, t5) = ts6();
    (t0, t1, t2, t3, t4, t5, after(t5))
}

// --- Basic ---

#[test]
fn test_no_violations_when_gets_match_puts() {
    let (t0, t1, t2, t3, _, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        get("k", 1, b"hello", t2, t3),
    ]);
    assert!(h.check_correctness().is_empty());
}

#[test]
fn test_empty_history_has_no_violations() {
    assert!(History(vec![]).check_correctness().is_empty());
}

// --- VersionNotFound ---

#[test]
fn test_violation_when_version_not_in_write_index() {
    let (t0, t1, ..) = ts6();
    let h = History(vec![get("k", 99, b"ghost", t0, t1)]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].version, 99);
    assert!(matches!(&v[0].kind, ViolationKind::VersionNotFound { actual } if actual == b"ghost"));
}

// --- ReadBeforeWriteStart ---

#[test]
fn test_violation_when_get_acks_before_put_starts() {
    // Timeline: GET_start → GET_ack → PUT_start → PUT_ack
    let (t0, t1, t2, t3, _, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t2, t3),
        get("k", 1, b"hello", t0, t1),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(&v[0].kind, ViolationKind::ReadBeforeWriteStart { .. }));

    let h = History(vec![
        get("k", 1, b"hello", t0, t1),
        put("k", 1, b"hello", t2, t3),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(&v[0].kind, ViolationKind::ReadBeforeWriteStart { .. }));
}

#[test]
fn test_no_violation_when_get_overlaps_with_put() {
    // Timeline: GET_start → PUT_start → GET_ack → PUT_ack
    // Verified with both record orderings to ensure the check is order-independent.
    let (t0, t1, t2, t3, _, _) = ts6();

    let h = History(vec![
        put("k", 1, b"hello", t1, t3),
        get("k", 1, b"hello", t0, t2),
    ]);
    assert!(h.check_correctness().is_empty());

    let h = History(vec![
        get("k", 1, b"hello", t0, t2),
        put("k", 1, b"hello", t1, t3),
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- ReadAfterDelete ---

#[test]
fn test_stale_data_after_delete_acked() {
    // Timeline: PUT → DELETE_ack → GET_start → GET returns the deleted value.
    // Classified as stale (eventual consistency), not a hard error.
    let (t0, t1, t2, t3, t4, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        delete("k", t1, t2),
        get("k", 1, b"hello", t3, t4),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(
        &v[0].kind,
        ViolationKind::StaleDataReturned { latest_known_version: None }
    ));
}

#[test]
fn test_no_violation_get_overlaps_with_delete() {
    // GET started before DELETE acked — overlap is acceptable.
    // Timeline: GET_start → DELETE_ack → GET_ack
    let (t0, t1, t2, t3, t4, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        delete("k", t1, t3),
        get("k", 1, b"hello", t2, t4),
    ]);
    assert!(h.check_correctness().is_empty());
}

#[test]
fn test_no_violation_after_delete_then_reput() {
    // DELETE then re-PUT with same version (version resets). The re-PUT's start_ts
    // is after the DELETE ack, so the delete does not supersede it.
    let (t0, t1, t2, t3, t4, t5, t6) = ts7();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        delete("k", t1, t2),
        put("k", 1, b"second", t3, t4), // overwrites write_index entry for (k,1)
        get("k", 1, b"second", t5, t6),
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- ValueMismatch ---

#[test]
fn test_violation_on_value_mismatch() {
    let (t0, t1, t2, t3, _, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        get("k", 1, b"world", t2, t3),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(
        &v[0].kind,
        ViolationKind::ValueMismatch { expected, actual }
            if expected == b"hello" && actual == b"world"
    ));
}

// --- StaleDataReturned ---

#[test]
fn test_stale_data_returned_when_newer_version_was_acked() {
    // PUT v1 then PUT v2 (both acked). GET returns v1 — stale.
    let (t0, t1, t2, t3, t4, t5) = ts6();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        put("k", 2, b"second", t2, t3),
        get("k", 1, b"first", t4, t5),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(
        &v[0].kind,
        ViolationKind::StaleDataReturned { latest_known_version: Some(2) }
    ));
}

#[test]
fn test_no_stale_violation_when_newer_put_not_yet_acked() {
    // PUT v2 started but not yet ACKed when GET started — not stale from client's view.
    let (t0, t1, t2, t3, t4, t5) = ts6();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        put("k", 2, b"second", t2, t5),
        get("k", 1, b"first", t3, t4),
    ]);
    assert!(h.check_correctness().is_empty());

    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        put("k", 2, b"second", t2, t5),
        get("k", 2, b"second", t3, t4),
    ]);
    assert!(h.check_correctness().is_empty());
}

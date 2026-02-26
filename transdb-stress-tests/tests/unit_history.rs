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

fn delete(key: &str, version: u64, start: Instant, ack: Instant) -> OpRecord {
    OpRecord {
        client_start_ts: start,
        client_ack_ts: ack,
        key: key.to_string(),
        kind: OpKind::Delete,
        outcome: OpOutcome::DeleteOk { version },
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

fn ts8() -> (Instant, Instant, Instant, Instant, Instant, Instant, Instant, Instant) {
    let (t0, t1, t2, t3, t4, t5, t6) = ts7();
    (t0, t1, t2, t3, t4, t5, t6, after(t6))
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
    // The tombstone's version (2) IS the latest_known_version.
    let (t0, t1, t2, t3, t4, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        delete("k", 2, t1, t2),
        get("k", 1, b"hello", t3, t4),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(
        &v[0].kind,
        ViolationKind::StaleDataReturned { latest_known_version: 2 }
    ));
}

#[test]
fn test_no_violation_get_overlaps_with_delete() {
    // GET started before DELETE acked — overlap is acceptable.
    // Timeline: GET_start → DELETE_ack → GET_ack
    let (t0, t1, t2, t3, t4, _) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t1),
        delete("k", 2, t1, t3),
        get("k", 1, b"hello", t2, t4),
    ]);
    assert!(h.check_correctness().is_empty());
}

#[test]
fn test_no_violation_after_delete_then_reput() {
    // DELETE then re-PUT with global versions (v=1, v=2 tombstone, v=3 re-PUT).
    // Case A: GET reads the re-PUT's value.
    let (t0, t1, t2, t3, t4, t5, t6) = ts7();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        delete("k", 2, t1, t2),
        put("k", 3, b"second", t3, t4),
        get("k", 3, b"second", t5, t6),
    ]);
    assert!(h.check_correctness().is_empty());

    // Case B: GET reads the first PUT's value before DELETE and re-PUT start.
    // No newer write was acked before GET started.
    let (t0, t1, t2, t3, t4, t5, t6, t7) = ts8();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        get("k", 1, b"first", t2, t3),
        delete("k", 2, t4, t5),
        put("k", 3, b"second", t6, t7),
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- Stale after tombstone and re-PUT ---

#[test]
fn test_stale_after_tombstone_and_reput() {
    // PUT v=1, DELETE v=2 (tombstone), PUT v=3 — all acked.
    // GET v=1 after all done → stale with latest_known_version = 3 (the re-PUT, not the tombstone).
    let (t0, t1, t2, t3, t4, t5, t6, t7) = ts8();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        delete("k", 2, t1, t2),
        put("k", 3, b"second", t3, t4),
        get("k", 1, b"first", t5, t6),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    // latest_known_version is 3 (the re-PUT, which acked before GET started).
    // Tombstone v=2 is also newer, but v=3 is the maximum.
    assert!(matches!(
        &v[0].kind,
        ViolationKind::StaleDataReturned { latest_known_version: 3 }
    ));
    let _ = t7; // unused
}

// --- Ack order ≠ server execution order ---

#[test]
fn test_no_violation_when_later_ack_does_not_mean_later_write() {
    // With global versions each write gets a unique version, so there is no
    // disambiguation ambiguity. This test verifies that a GET correctly reads
    // the latest PUT even when an earlier PUT acked after the later ones.
    //
    // Timeline (client-side):
    //   t0: PUT v=1 starts        (slow path — acks at t5)
    //   t1: DELETE v=2 starts
    //   t2: DELETE v=2 acks
    //   t3: PUT v=3 starts        (fast path — acks at t4)
    //   t4: PUT v=3 acks
    //   t5: PUT v=1 acks          (delayed, but server assigned v=1 first)
    //   t6: GET v=3 starts
    //   t7: GET v=3 acks  →  returns (v=3, b"second")  [correct: v=3 is the latest]
    let (t0, t1, t2, t3, t4, t5, t6, t7) = ts8();
    let h = History(vec![
        put("k", 1, b"first",  t0, t5),  // PUT v=1: early start, late ack
        delete("k", 2,          t1, t2),
        put("k", 3, b"second", t3, t4),  // PUT v=3: after DELETE, acks before PUT v=1
        get("k", 3, b"second", t6, t7),  // GET: correctly reads v=3
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- DELETE overlapping PUT → now a stale violation ---

#[test]
fn test_stale_when_delete_acks_before_get_starts() {
    // Timeline: PUT_start(t0) → DELETE_start(t1) → PUT_ack(t2) → DELETE_ack(t3) → GET_start(t4)
    // The DELETE (v=2) was fully acked before the GET started, so the server had
    // definitely advanced past v=1 by the time the client issued the GET.
    // GET returning v=1 is therefore stale.
    let (t0, t1, t2, t3, t4, t5) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t2),  // PUT: t0..t2
        delete("k", 2, t1, t3),          // DELETE: t1..t3  (starts during PUT)
        get("k", 1, b"hello", t4, t5),   // GET: after DELETE acked
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(
        &v[0].kind,
        ViolationKind::StaleDataReturned { latest_known_version: 2 }
    ));
}

// --- GET returning data for a tombstone version ---

#[test]
fn test_version_not_found_when_get_returns_data_for_tombstone_version() {
    // A GET claims it read data at the same version as a tombstone (DELETE).
    // This cannot happen in a correct system (server returns 404 for tombstones),
    // but the checker must still report VersionNotFound.
    let (t0, t1, t2, t3, _, _) = ts6();
    let h = History(vec![
        delete("k", 1, t0, t1),
        get("k", 1, b"phantom", t2, t3),
    ]);
    let v = h.check_correctness();
    assert_eq!(v.len(), 1);
    assert!(matches!(&v[0].kind, ViolationKind::VersionNotFound { actual } if actual == b"phantom"));
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
        ViolationKind::StaleDataReturned { latest_known_version: 2 }
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

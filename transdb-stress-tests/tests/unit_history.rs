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
        delete("k", 2, t1, t3),
        get("k", 1, b"hello", t2, t4),
    ]);
    assert!(h.check_correctness().is_empty());
}

#[test]
fn test_no_violation_after_delete_then_reput() {
    // DELETE then re-PUT with same version (version resets).
    // Case A: GET reads the second PUT's value.
    let (t0, t1, t2, t3, t4, t5, t6) = ts7();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        delete("k", 2, t1, t2),
        put("k", 1, b"second", t3, t4),
        get("k", 1, b"second", t5, t6),
    ]);
    assert!(h.check_correctness().is_empty());

    // Case B: GET reads the *first* PUT's value, then DELETE and re-PUT happen later.
    // The write_index must not confuse the two (key, version=1) entries: the GET correctly
    // observed the first PUT, even though a second PUT with the same version came later.
    let (t0, t1, t2, t3, t4, t5, t6, t7) = ts8();
    let h = History(vec![
        put("k", 1, b"first", t0, t1),
        get("k", 1, b"first", t2, t3),
        delete("k", 2, t4, t5),
        put("k", 1, b"second", t6, t7),
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- Ack order ≠ server execution order ---

#[test]
fn test_no_violation_when_later_ack_does_not_mean_later_write() {
    // Network-delay scenario: PUT_A starts first but acks last because of a slow network.
    // Meanwhile DELETE and PUT_B complete in between, so the server's final value is b"second".
    //
    // Timeline (client-side):
    //   t0: PUT_A starts        (slow path — acks at t5)
    //   t1: DELETE starts
    //   t2: DELETE acks
    //   t3: PUT_B starts        (fast path — acks at t4)
    //   t4: PUT_B acks
    //   t5: PUT_A acks          (delayed; PUT_A was processed *first* on the server)
    //   t6: GET starts
    //   t7: GET acks  →  returns (v=1, b"second")  [correct: PUT_B is the last write]
    //
    // Bug: max_by_key(put_ack_ts) picks PUT_A (ack=t5 > t4) and compares its value
    // b"first" against the GET's b"second", producing a false ValueMismatch.
    let (t0, t1, t2, t3, t4, t5, t6, t7) = ts8();
    let h = History(vec![
        put("k", 1, b"first",  t0, t5),  // PUT_A: early start, late ack
        delete("k", 2,          t1, t2),
        put("k", 1, b"second", t3, t4),  // PUT_B: after DELETE, acks before PUT_A
        get("k", 1, b"second", t6, t7),  // GET: correctly reads PUT_B
    ]);
    assert!(h.check_correctness().is_empty());
}

// --- DELETE overlapping PUT ---

#[test]
fn test_no_stale_when_delete_overlaps_with_put() {
    // Timeline: PUT_start(t0) → DELETE_start(t1) → PUT_ack(t2) → DELETE_ack(t3) → GET_start(t4)
    // The DELETE started while the PUT was still in-flight, so it did not definitively
    // supersede the PUT — the server may have processed them in either order.
    // A GET returning the PUT value is therefore not a stale violation.
    let (t0, t1, t2, t3, t4, t5) = ts6();
    let h = History(vec![
        put("k", 1, b"hello", t0, t2),  // PUT: t0..t2
        delete("k", 2, t1, t3),          // DELETE: t1..t3  (starts during PUT)
        get("k", 1, b"hello", t4, t5),   // GET: after DELETE acked
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

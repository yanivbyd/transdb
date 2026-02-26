use rand::{rngs::StdRng, SeedableRng};
use transdb_stress_tests::history::OpOutcome;
use transdb_stress_tests::worker::{generate_value, is_error};

// `worker::run` requires a live HTTP server and is inherently integration-level.
// The two helpers exposed by worker.rs cover all of the pure, testable logic.

#[test]
fn test_generate_value_and_is_error() {
    let mut rng = StdRng::seed_from_u64(42);

    // generate_value: length must always be in 8..=64 and content must be non-trivially varied.
    let mut all_same = true;
    let mut prev: Option<Vec<u8>> = None;
    for _ in 0..50 {
        let v = generate_value(&mut rng);
        assert!(v.len() >= 8, "value too short: {}", v.len());
        assert!(v.len() <= 64, "value too long: {}", v.len());
        if let Some(ref p) = prev {
            if p != &v {
                all_same = false;
            }
        }
        prev = Some(v);
    }
    assert!(!all_same, "generate_value returned identical bytes every time");

    // is_error: only OpOutcome::Error should return true.
    assert!(is_error(&OpOutcome::Error));
    assert!(!is_error(&OpOutcome::NotFound));
    assert!(!is_error(&OpOutcome::DeleteOk));
    assert!(!is_error(&OpOutcome::GetOk { version: 1, value: vec![1] }));
    assert!(!is_error(&OpOutcome::PutOk { version: 1, value: vec![1] }));
}

use transdb_stress_tests::metrics::Metrics;

fn make(latency_ns: Vec<u64>, errors_5xx: u64, requests_total: u64, elapsed_secs: f64) -> Metrics {
    Metrics { requests_total, errors_5xx, latency_ns, elapsed_secs }
}

#[test]
fn test_percentiles_sorted_input() {
    // [100..1000] in steps of 100, n=10
    // p50: index floor(0.50 * 10) = 5 → 600
    // p99: index floor(0.99 * 10) = 9 → 1000
    let m = make(vec![100, 200, 300, 400, 500, 600, 700, 800, 900, 1000], 0, 10, 1.0);
    assert_eq!(m.p50_ns(), 600);
    assert_eq!(m.p99_ns(), 1000);
}

#[test]
fn test_percentiles_unsorted_input() {
    // sorted: [100, 200, 300, 400, 500], n=5
    // p50: index floor(0.50 * 5) = 2 → 300
    // p99: index floor(0.99 * 5) = 4 → 500
    let m = make(vec![500, 100, 300, 200, 400], 0, 5, 1.0);
    assert_eq!(m.p50_ns(), 300);
    assert_eq!(m.p99_ns(), 500);
}

#[test]
fn test_percentiles_empty_returns_zero() {
    let m = make(vec![], 0, 0, 1.0);
    assert_eq!(m.p50_ns(), 0);
    assert_eq!(m.p99_ns(), 0);
}

#[test]
fn test_error_rate_and_throughput() {
    let m = make(vec![], 1, 10, 2.0);
    assert_eq!(m.error_rate(), 0.1);
    assert_eq!(m.throughput_rps(), 5.0);
}

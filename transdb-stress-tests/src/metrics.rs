pub struct Metrics {
    pub requests_total: u64,
    pub errors_5xx: u64,
    /// One entry per completed operation, in insertion order (unsorted).
    pub latency_ns: Vec<u64>,
    pub elapsed_secs: f64,
}

impl Metrics {
    pub fn p50_ns(&self) -> u64 {
        percentile(&self.latency_ns, 0.50)
    }

    pub fn p99_ns(&self) -> u64 {
        percentile(&self.latency_ns, 0.99)
    }

    pub fn error_rate(&self) -> f64 {
        self.errors_5xx as f64 / self.requests_total as f64
    }

    pub fn throughput_rps(&self) -> f64 {
        self.requests_total as f64 / self.elapsed_secs
    }
}

/// Sort `data` ascending and return the element at index `floor(p * n)`.
/// Returns 0 for an empty slice.
fn percentile(data: &[u64], p: f64) -> u64 {
    if data.is_empty() {
        return 0;
    }
    let mut sorted = data.to_vec();
    sorted.sort_unstable();
    let idx = (p * sorted.len() as f64).floor() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

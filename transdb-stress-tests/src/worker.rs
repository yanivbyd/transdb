use rand::Rng;
use std::time::{Duration, Instant};
use transdb_client::{Client, ClientConfig};
use transdb_common::{TransDbError, Topology};

use crate::history::{History, OpKind, OpOutcome, OpRecord};
use crate::metrics::Metrics;
use crate::workload::{Op, WorkloadProfile};

/// Drive the primary with `profile` for `duration`, recording every operation.
/// Returns raw metrics and the full operation history for post-run correctness checking.
pub async fn run(
    topology: Topology,
    profile: WorkloadProfile,
    key_space: usize,
    duration: Duration,
) -> (Metrics, History) {
    let client = Client::new(ClientConfig { topology });
    let mut rng = rand::thread_rng();
    let mut records: Vec<OpRecord> = Vec::new();
    let mut requests_total: u64 = 0;
    let mut errors_5xx: u64 = 0;
    let mut latency_ns: Vec<u64> = Vec::new();

    let run_start = Instant::now();

    while run_start.elapsed() < duration {
        let op = profile.sample(&mut rng);
        let key_idx = rng.gen_range(0..key_space);
        let key = format!("key_{key_idx}");

        let op_start = Instant::now();
        let (kind, outcome) = execute_op(&client, op, &key, &mut rng).await;
        let op_end = Instant::now();

        if is_error(&outcome) {
            errors_5xx += 1;
        }

        requests_total += 1;
        latency_ns.push((op_end - op_start).as_nanos() as u64);
        records.push(OpRecord {
            client_start_ts: op_start,
            client_ack_ts: op_end,
            key,
            kind,
            outcome,
        });
    }

    let elapsed_secs = run_start.elapsed().as_secs_f64();
    let metrics = Metrics { requests_total, errors_5xx, latency_ns, elapsed_secs };
    (metrics, History(records))
}

async fn execute_op(
    client: &Client,
    op: Op,
    key: &str,
    rng: &mut impl Rng,
) -> (OpKind, OpOutcome) {
    match op {
        Op::Get => {
            let outcome = match client.get(key).await {
                Ok(r) => OpOutcome::GetOk { version: r.version, value: r.value },
                Err(TransDbError::KeyNotFound(_)) => OpOutcome::NotFound,
                Err(_) => OpOutcome::Error,
            };
            (OpKind::Get, outcome)
        }
        Op::Put => {
            let value = generate_value(rng);
            let outcome = match client.put(key, &value).await {
                Ok(version) => OpOutcome::PutOk { version, value },
                Err(_) => OpOutcome::Error,
            };
            (OpKind::Put, outcome)
        }
        Op::Delete => {
            let outcome = match client.delete(key).await {
                Ok(Some(version)) => OpOutcome::DeleteOk { version },
                Ok(None) => OpOutcome::NotFound,
                Err(_) => OpOutcome::Error,
            };
            (OpKind::Delete, outcome)
        }
    }
}

/// Generate a random byte payload for use in PUT operations (8–64 bytes).
pub fn generate_value(rng: &mut impl Rng) -> Vec<u8> {
    let len: usize = rng.gen_range(8..=1024);
    (0..len).map(|_| rng.gen::<u8>()).collect()
}

/// Returns `true` if `outcome` represents a server-side error (5xx or network failure).
pub fn is_error(outcome: &OpOutcome) -> bool {
    matches!(outcome, OpOutcome::Error)
}

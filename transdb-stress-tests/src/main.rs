use clap::Parser;
use std::io::Write;
use std::process;
use std::time::Duration;
use transdb_stress_tests::history::ViolationKind;
use transdb_stress_tests::server::Cluster;
use transdb_stress_tests::workload::WorkloadProfile;
use transdb_stress_tests::worker;

#[derive(Parser)]
#[command(name = "transdb-stress", about = "TransDB stress test harness")]
struct Args {
    /// How long to run (seconds)
    #[arg(long, default_value_t = 5)]
    duration: u64,

    /// Workload profile: read-heavy | balanced | write-heavy | put-only
    #[arg(long, default_value = "balanced")]
    workload: String,

    /// Number of distinct keys in the key space
    #[arg(long, default_value_t = 1000)]
    key_space: usize,

    /// Fail if the 5xx error rate exceeds this fraction
    #[arg(long, default_value_t = 0.01)]
    max_error_rate: f64,

    /// Fail if correctness violations exceed this count
    #[arg(long, default_value_t = 0)]
    max_violations: u64,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let profile = WorkloadProfile::from_name(&args.workload).unwrap_or_else(|| {
        eprintln!(
            "Unknown workload {:?}. Valid values: read-heavy, balanced, write-heavy, put-only",
            args.workload
        );
        process::exit(3);
    });

    let cluster = Cluster::build_and_spawn().unwrap_or_else(|e| {
        eprintln!("Failed to start cluster: {e}");
        process::exit(3);
    });

    println!(
        "Cluster ready:  primary {}  |  replica {}",
        cluster.primary.addr,
        cluster.replica.addr,
    );

    let topology = cluster.topology.clone();
    let duration = Duration::from_secs(args.duration);

    print!("Running {}s {} workload ", args.duration, profile.as_name());
    std::io::stdout().flush().ok();

    let dot_handle = tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.tick().await; // consume the immediate first tick
        loop {
            interval.tick().await;
            print!(".");
            std::io::stdout().flush().ok();
        }
    });

    let (metrics, history) = worker::run(topology, profile, args.key_space, duration).await;

    dot_handle.abort();
    println!();

    drop(cluster);

    let violations = history.check_correctness();
    let hard_violation_count: u64 = violations
        .iter()
        .filter(|v| !matches!(v.kind, ViolationKind::StaleDataReturned { .. }))
        .count() as u64;

    print_report(&args, &metrics, hard_violation_count, profile);

    for v in &violations {
        if matches!(v.kind, ViolationKind::StaleDataReturned { .. }) {
            continue;
        }
        let detail = match &v.kind {
            ViolationKind::VersionNotFound { actual } => {
                format!("VersionNotFound: got {} bytes for unrecorded version", actual.len())
            }
            ViolationKind::ReadBeforeWriteStart { .. } => {
                "ReadBeforeWriteStart: GET completed before PUT even started".to_string()
            }
            ViolationKind::ValueMismatch { expected, actual } => {
                format!(
                    "ValueMismatch: expected {} bytes, got {} bytes",
                    expected.len(),
                    actual.len()
                )
            }
            ViolationKind::StaleDataReturned { .. } => unreachable!(),
        };
        eprintln!("VIOLATION key={} version={} {}", v.key, v.version, detail);
    }

    let error_rate_exceeded = metrics.requests_total > 0
        && metrics.error_rate() > args.max_error_rate;
    let violations_exceeded = hard_violation_count > args.max_violations;

    let exit_code = if error_rate_exceeded {
        1
    } else if violations_exceeded {
        2
    } else {
        0
    };

    process::exit(exit_code);
}

fn print_report(args: &Args, metrics: &transdb_stress_tests::metrics::Metrics, violation_count: u64, profile: WorkloadProfile) {
    let pass_fail = |exceeded: bool| if exceeded { "✗" } else { "✓" };

    let error_rate_exceeded = metrics.requests_total > 0
        && metrics.error_rate() > args.max_error_rate;
    let violations_exceeded = violation_count > args.max_violations;
    let overall_pass = !error_rate_exceeded && !violations_exceeded;

    println!("TransDB Stress Test Results");
    println!("===========================");
    println!("Duration:              {:.1} s", args.duration as f64);
    println!("Workload:              {}", profile.as_name());
    println!("Key space:             {}", args.key_space);
    println!("Nodes:                 primary + replica");
    println!();
    println!("Requests:              {}", format_thousands(metrics.requests_total));
    println!("Throughput:            {:.1} rps", metrics.throughput_rps());
    println!("P50 latency:           {:.1} ms", ns_to_ms(metrics.p50_ns()));
    println!("P99 latency:           {:.1} ms", ns_to_ms(metrics.p99_ns()));
    println!();
    println!("5xx errors:            {}", format_thousands(metrics.errors_5xx));
    println!(
        "Error rate:            {:.3}%    [threshold: {:.3}%]  {}",
        metrics.error_rate() * 100.0,
        args.max_error_rate * 100.0,
        pass_fail(error_rate_exceeded),
    );
    println!();
    println!(
        "Correctness violations: {}        [threshold: {}]        {}",
        violation_count,
        args.max_violations,
        pass_fail(violations_exceeded),
    );
    println!();
    println!("Result: {}", if overall_pass { "PASS" } else { "FAIL" });
}

fn format_thousands(n: u64) -> String {
    if n >= 1_000_000 {
        format!("~{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("~{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

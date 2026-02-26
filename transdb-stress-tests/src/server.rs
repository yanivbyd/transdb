use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use transdb_common::Topology;

pub struct ServerProcess {
    child: Child,
    pub addr: SocketAddr,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.child.kill().ok();
    }
}

pub struct Cluster {
    pub primary: ServerProcess,
    pub replica: ServerProcess,
    pub topology: Topology,
    // Kept alive so the topology file remains on disk until both processes exit.
    _tmpfile: NamedTempFile,
}

/// Reserve `count` free TCP ports by binding to port 0 for each, then
/// releasing them all at once.  Holding all listeners alive until the ports
/// are collected prevents the same port from being issued twice and reduces
/// the TOCTOU window between releasing and the caller binding.
pub fn pick_free_ports(count: usize) -> Vec<u16> {
    let listeners: Vec<std::net::TcpListener> = (0..count)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let ports = listeners.iter().map(|l| l.local_addr().unwrap().port()).collect();
    drop(listeners);
    ports
}

/// Return the path to the `transdb-server` binary that sits alongside this
/// executable in `target/debug/` (or `target/debug/deps/` when run as a test).
fn server_binary_path() -> PathBuf {
    let mut path = std::env::current_exe().expect("cannot determine own executable path");
    path.pop(); // remove own filename
    if path.file_name().map(|n| n == "deps").unwrap_or(false) {
        path.pop(); // step out of target/debug/deps → target/debug/
    }
    path.push("transdb-server");
    path
}

const READY_TIMEOUT: Duration = Duration::from_secs(30);

impl Cluster {
    /// Build the `transdb-server` binary, spawn a primary and replica, wait
    /// until both are ready to serve HTTP, and return the live `Cluster`.
    ///
    /// Returns `Err` if the build fails, a process cannot be spawned, or the
    /// readiness deadline elapses.  The caller should map this error to exit
    /// code 3 as documented in the CLI spec.
    pub fn build_and_spawn() -> Result<Self, String> {
        // 1. Build the server binary.
        let status = Command::new("cargo")
            .args(["build", "-p", "transdb-server"])
            .status()
            .map_err(|e| format!("Failed to invoke cargo build: {e}"))?;
        if !status.success() {
            return Err(format!("cargo build -p transdb-server failed: {status}"));
        }

        let ports = pick_free_ports(2);
        let (port1, port2) = (ports[0], ports[1]);
        let primary_addr: SocketAddr = format!("127.0.0.1:{port1}").parse().unwrap();
        let replica_addr: SocketAddr = format!("127.0.0.1:{port2}").parse().unwrap();

        // 3. Write topology JSON to a temp file; the file stays alive inside Cluster.
        let topology = Topology {
            primary_addr: primary_addr.to_string(),
            replica_addr: Some(replica_addr.to_string()),
        };
        let tmpfile =
            NamedTempFile::new().map_err(|e| format!("Failed to create topology tmpfile: {e}"))?;
        serde_json::to_writer(&tmpfile, &topology)
            .map_err(|e| format!("Failed to write topology JSON: {e}"))?;

        let server_bin = server_binary_path();
        let topo_path = tmpfile.path().to_str().unwrap().to_string();

        // 4. Spawn primary.
        let primary_child = Command::new(&server_bin)
            .args(["--role", "primary", "--topology", &topo_path])
            .spawn()
            .map_err(|e| format!("Failed to spawn primary: {e}"))?;
        let primary = ServerProcess { child: primary_child, addr: primary_addr };

        // 5. Spawn replica.
        let replica_child = Command::new(&server_bin)
            .args(["--role", "replica", "--topology", &topo_path])
            .spawn()
            .map_err(|e| format!("Failed to spawn replica: {e}"))?;
        let replica = ServerProcess { child: replica_child, addr: replica_addr };

        // 6. Poll both nodes for HTTP readiness concurrently.
        //    If either poll fails, `primary` and `replica` drop here, killing both processes.
        let deadline = Instant::now() + READY_TIMEOUT;
        let primary_addr = primary.addr;
        let replica_addr = replica.addr;

        let t1 = std::thread::spawn(move || poll_until_ready(primary_addr, deadline));
        let t2 = std::thread::spawn(move || poll_until_ready(replica_addr, deadline));

        t1.join()
            .map_err(|_| "Primary readiness thread panicked".to_string())?
            .map_err(|e| format!("Primary not ready within timeout: {e}"))?;
        t2.join()
            .map_err(|_| "Replica readiness thread panicked".to_string())?
            .map_err(|e| format!("Replica not ready within timeout: {e}"))?;

        Ok(Cluster { primary, replica, topology, _tmpfile: tmpfile })
    }
}

/// Poll `addr` with a TCP connect attempt until the connection succeeds
/// (server is accepting connections) or `deadline` is reached.
///
/// A successful TCP connection is sufficient to confirm the HTTP server is
/// ready: our axum-based server starts accepting the moment it binds, so
/// a successful `connect` implies it will also answer HTTP requests.
fn poll_until_ready(addr: SocketAddr, deadline: Instant) -> Result<(), String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("timed out waiting for {addr}"));
        }
        let probe = Duration::min(remaining, Duration::from_millis(200));
        if TcpStream::connect_timeout(&addr, probe).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

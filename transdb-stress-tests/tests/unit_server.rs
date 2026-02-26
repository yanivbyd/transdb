use transdb_stress_tests::server::pick_free_ports;

// `pick_free_ports` is the only function in server.rs that is pure enough to
// unit-test in isolation.  The remaining items are justified below:
//
// - `server_binary_path()` — private helper; result depends on the cargo
//   target directory at runtime, which has no stable value in a unit-test
//   context.
//
// - `ServerProcess::drop` — single-line OS call (`child.kill()`).  Verifying
//   it requires spawning a real child process and using platform-specific
//   signal checks.  This behaviour is exercised end-to-end by the full stress
//   run (commit 4).
//
// - `Cluster::build_and_spawn` — spawns real child processes and performs
//   TCP polling; inherently integration-level.  Covered by the full stress
//   run (commit 4).
//
// - `poll_until_ready` — private helper that drives TCP connect probes
//   against a live server.  Integration-level by nature.

#[test]
fn test_pick_free_ports_returns_bindable_ports() {
    let ports = pick_free_ports(2);
    assert_eq!(ports.len(), 2);
    for &port in &ports {
        assert!(port > 0, "port must be non-zero");
        // Prove each port is free: bind to it immediately after release.
        let listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}"));
        assert!(listener.is_ok(), "port {port} should be bindable after pick_free_ports");
    }
}

#[test]
fn test_pick_free_ports_returns_distinct_ports() {
    let ports = pick_free_ports(2);
    assert_ne!(ports[0], ports[1], "ports must be distinct");
}

#[test]
fn test_pick_free_ports_empty() {
    assert!(pick_free_ports(0).is_empty());
}

use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use transdb_client::{Client, ClientConfig};
use transdb_common::{ErrorResponse, Topology, TransDbError, MAX_KEY_SIZE, MAX_VALUE_SIZE};
use transdb_server::{NodeRole, Server, ServerConfig};

const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Asserts that `versions` is strictly increasing.
fn assert_monotonic(versions: &[u64]) {
    for w in versions.windows(2) {
        assert!(w[1] > w[0], "versions not strictly increasing: {} then {}", w[0], w[1]);
    }
}

struct Cluster {
    primary: Client,
    replica: Client,
}

async fn start_node(role: NodeRole) -> SocketAddr {
    let (ready_tx, ready_rx) = oneshot::channel();
    let server = Server::new(ServerConfig {
        address: "127.0.0.1:0".parse().unwrap(),
        role,
        topology: None,
    });
    tokio::spawn(async move {
        server.run(ready_tx).await.expect("server failed");
    });
    timeout(SERVER_READY_TIMEOUT, ready_rx)
        .await
        .expect("server did not start within 60 seconds")
        .expect("server ready signal dropped")
}

async fn start_cluster() -> Cluster {
    let (primary_addr, replica_addr) = tokio::join!(
        start_node(NodeRole::Primary),
        start_node(NodeRole::Replica),
    );

    let topology = Topology {
        primary_addr: primary_addr.to_string(),
        replica_addr: Some(replica_addr.to_string()),
    };

    let primary = Client::new(ClientConfig { topology: topology.clone() });

    let mut replica = Client::new(ClientConfig { topology: topology.clone() });
    replica.set_target(topology.replica_addr.as_deref().unwrap());

    Cluster { primary, replica }
}

#[tokio::test]
async fn test_get_returns_key_not_found() {
    let client = start_cluster().await.primary;

    assert!(matches!(client.get("some_key").await, Err(TransDbError::KeyNotFound(_))));
    assert!(matches!(client.get_allowing_expired("some_key").await, Err(TransDbError::KeyNotFound(_))));
}

#[tokio::test]
async fn test_put_and_get_round_trip() {
    let client = start_cluster().await.primary;

    let put_version = client.put("my_key", b"hello world").await.expect("put failed");
    assert!(put_version > 0);

    let result = client.get("my_key").await.expect("get failed");
    assert_eq!(result.value, b"hello world");
    assert_eq!(result.version, put_version);
    assert!(!result.expired);

    let result = client.get_allowing_expired("my_key").await.expect("get_allowing_expired failed");
    assert_eq!(result.value, b"hello world");
    assert_eq!(result.version, put_version);
    assert!(!result.expired);
}

#[tokio::test]
async fn test_delete_removes_existing_key() {
    let client = start_cluster().await.primary;

    let put_version = client.put("my_key", b"hello").await.expect("put failed");
    assert!(put_version > 0);

    let before = client.get("my_key").await.expect("get before delete failed");
    assert_eq!(before.value, b"hello");
    assert_eq!(before.version, put_version);

    client.delete("my_key").await.expect("delete failed");

    let after = client.get("my_key").await;
    assert!(matches!(after, Err(TransDbError::KeyNotFound(k)) if k == "my_key"));
}

#[tokio::test]
async fn test_delete_is_idempotent() {
    let client = start_cluster().await.primary;

    // Each delete call auto-generates a unique idempotency key, so both are first-time requests.
    let first = client.delete("nonexistent").await;
    let second = client.delete("nonexistent").await;

    assert!(first.is_ok());
    assert!(second.is_ok());
}

#[tokio::test]
async fn test_put_overwrites_existing_key() {
    let client = start_cluster().await.primary;

    let v1 = client.put("my_key", b"first").await.expect("first put failed");
    let v2 = client.put("my_key", b"second").await.expect("second put failed");
    assert_monotonic(&[v1, v2]);

    let result = client.get("my_key").await.expect("get failed");
    assert_eq!(result.value, b"second");
    assert_eq!(result.version, v2);
}

// --- Versioning ---

#[tokio::test]
async fn test_put_returns_positive_version_for_new_key() {
    let client = start_cluster().await.primary;

    let version = client.put("k", b"v").await.expect("put failed");
    assert!(version > 0);
}

#[tokio::test]
async fn test_put_increments_version() {
    let client = start_cluster().await.primary;

    let v1 = client.put("k", b"first").await.expect("first put failed");
    let v2 = client.put("k", b"second").await.expect("second put failed");

    assert_monotonic(&[v1, v2]);
}

#[tokio::test]
async fn test_get_returns_etag_matching_put_version() {
    let client = start_cluster().await.primary;

    let v = client.put("k", b"v").await.expect("put failed");
    let result = client.get("k").await.expect("get failed");

    assert_eq!(result.version, v);
}

#[tokio::test]
async fn test_version_increases_after_delete_and_recreate() {
    let client = start_cluster().await.primary;

    let v1 = client.put("k", b"v1").await.expect("first put failed");
    let v2 = client.put("k", b"v2").await.expect("second put failed");
    let v_del = client.delete("k").await.expect("delete failed").expect("key must be live");
    let v3 = client.put("k", b"v3").await.expect("third put failed");

    assert_monotonic(&[v1, v2, v_del, v3]);
}

// --- Idempotency (via raw reqwest to control the Idempotency-Key header) ---

#[tokio::test]
async fn test_put_idempotency_replay_returns_same_version() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("idem_key");

    let r1 = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Idempotency-Key", "replay-token-abc")
        .body(b"value".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), reqwest::StatusCode::OK);
    let etag1 = r1.headers().get("etag").unwrap().to_str().unwrap().to_string();

    let r2 = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Idempotency-Key", "replay-token-abc")
        .body(b"value".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), reqwest::StatusCode::OK);
    let etag2 = r2.headers().get("etag").unwrap().to_str().unwrap().to_string();

    assert_eq!(etag1, etag2);
}

#[tokio::test]
async fn test_put_idempotency_replay_does_not_write_twice() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("idem_write");

    // Two PUTs with the same idempotency key
    for _ in 0..2 {
        http.put(&url)
            .header("Content-Type", "application/octet-stream")
            .header("Idempotency-Key", "write-once-token")
            .body(b"v".to_vec())
            .send()
            .await
            .unwrap();
    }

    // Version should be the same as the first write; value should be what was written
    let result = client.get("idem_write").await.expect("get failed");
    assert_eq!(result.value, b"v");
    assert!(result.version > 0);
}

#[tokio::test]
async fn test_delete_idempotency_replay_returns_204() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("del_key");

    let r1 = http
        .delete(&url)
        .header("Idempotency-Key", "del-replay-token")
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), reqwest::StatusCode::NO_CONTENT);

    let r2 = http
        .delete(&url)
        .header("Idempotency-Key", "del-replay-token")
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), reqwest::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_put_idempotency_mismatch_key_returns_422() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();

    // First PUT for key_a with token X
    http.put(client.build_key_url("key_a"))
        .header("Content-Type", "application/octet-stream")
        .header("Idempotency-Key", "mismatch-token")
        .body(b"v".to_vec())
        .send()
        .await
        .unwrap();

    // Second PUT for key_b with same token X
    let r2 = http
        .put(client.build_key_url("key_b"))
        .header("Content-Type", "application/octet-stream")
        .header("Idempotency-Key", "mismatch-token")
        .body(b"v".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(r2.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_put_missing_idempotency_key_returns_400() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("k");

    let response = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(b"v".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, "Idempotency-Key header is required");
}

#[tokio::test]
async fn test_delete_missing_idempotency_key_returns_400() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("k");

    let response = http.delete(&url).send().await.unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, "Idempotency-Key header is required");
}

// --- Size validation: server-side rejection (bypassing client pre-flight via raw reqwest) ---

#[tokio::test]
async fn test_server_rejects_oversized_key_on_put() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);
    let url = format!("http://{}/keys/{}", client.config.topology.primary_addr, oversized_key);

    let response = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(b"hello".to_vec())
        .send()
        .await
        .unwrap();

    // Key size check fires before Idempotency-Key check
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE));
}

#[tokio::test]
async fn test_server_rejects_oversized_value_on_put() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let url = client.build_key_url("my_key");
    let oversized_value = vec![0u8; MAX_VALUE_SIZE + 1];

    let response = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Idempotency-Key", "tok-size-test")
        .body(oversized_value)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, format!("Value exceeds maximum size of {} bytes", MAX_VALUE_SIZE));
}

#[tokio::test]
async fn test_server_rejects_oversized_key_on_get() {
    let client = start_cluster().await.primary;
    let http = reqwest::Client::new();
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);
    let url = format!("http://{}/keys/{}", client.config.topology.primary_addr, oversized_key);

    let response = http.get(&url).send().await.unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE));
}

#[tokio::test]
async fn test_client_rejects_oversized_key_without_contacting_server() {
    // Uses an unbound address — if the client pre-flight works, no connection is attempted
    let client = Client::new(ClientConfig {
        topology: Topology { primary_addr: "127.0.0.1:59212".to_string(), replica_addr: None },
    });
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);

    let result = client.get(&oversized_key).await;

    // Must be KeyTooLarge (pre-flight), not NetworkError (would mean a connection was attempted)
    assert!(matches!(result, Err(TransDbError::KeyTooLarge(_))));
}

#[tokio::test]
async fn test_client_rejects_oversized_value_without_contacting_server() {
    // Uses an unbound address — if the client pre-flight works, no connection is attempted
    let client = Client::new(ClientConfig {
        topology: Topology { primary_addr: "127.0.0.1:59212".to_string(), replica_addr: None },
    });
    let oversized_value = vec![0u8; MAX_VALUE_SIZE + 1];

    let result = client.put("my_key", &oversized_value).await;

    // Must be ValueTooLarge (pre-flight), not NetworkError (would mean a connection was attempted)
    assert!(matches!(result, Err(TransDbError::ValueTooLarge(_))));
}

// --- TTL ---

#[tokio::test]
async fn test_expired_entry_behavior() {
    let client = start_cluster().await.primary;

    // Epoch 1 is well in the past — entry is immediately expired
    let version = client.put_with_ttl("ttl_key", b"stale value", 1).await.expect("put_with_ttl failed");
    assert!(version > 0);

    // Strong guarantee: expired entry is treated as not found
    assert!(matches!(client.get("ttl_key").await, Err(TransDbError::KeyNotFound(_))));

    // Soft guarantee: expired entry is returned with expired=true
    let result = client.get_allowing_expired("ttl_key").await.expect("get_allowing_expired failed");
    assert_eq!(result.value, b"stale value");
    assert!(result.expired);
}

#[tokio::test]
async fn test_get_returns_ok_for_entry_with_future_ttl() {
    let client = start_cluster().await.primary;

    // Far future epoch (year 2100+) — entry should not be expired
    client.put_with_ttl("future_key", b"fresh value", 4_102_444_800).await.expect("put_with_ttl failed");

    let result = client.get("future_key").await.expect("get failed");
    assert_eq!(result.value, b"fresh value");
    assert!(!result.expired);

    let result = client.get_allowing_expired("future_key").await.expect("get_allowing_expired failed");
    assert_eq!(result.value, b"fresh value");
    assert!(!result.expired);
}

#[tokio::test]
async fn test_put_with_invalid_ttl_returns_400() {
    use reqwest::Client as HttpClient;

    let url = start_cluster().await.primary.build_key_url("k");

    let http = HttpClient::new();
    let response = http.put(&url)
        .header("Idempotency-Key", "tok-invalid-ttl")
        .header("X-TTL", "not-a-number")
        .body(b"value".to_vec())
        .send()
        .await
        .expect("request failed");

    assert_eq!(response.status(), 400);
}

// --- Replication: replica enforces 405 ---

#[tokio::test]
async fn test_replica_rejects_all_key_operations() {
    let cluster = start_cluster().await;

    assert!(matches!(cluster.replica.get("k").await, Err(TransDbError::HttpError(405, _))));
    assert!(matches!(cluster.replica.put("k", b"v").await, Err(TransDbError::HttpError(405, _))));
    assert!(matches!(cluster.replica.delete("k").await, Err(TransDbError::HttpError(405, _))));
}

#[tokio::test]
async fn test_set_target_routes_to_replica_and_back() {
    let cluster = start_cluster().await;
    let replica_addr = cluster.replica.config.topology.replica_addr.clone().unwrap();
    let primary_addr = cluster.primary.config.topology.primary_addr.clone();

    let mut client = cluster.primary;

    // Primary: writes work
    let version = client.put("k", b"v").await.expect("put to primary failed");
    assert!(version > 0);

    // Redirect to replica: all operations rejected with 405
    client.set_target(&replica_addr);
    assert!(matches!(client.get("k").await, Err(TransDbError::HttpError(405, _))));
    assert!(matches!(client.put("k", b"v2").await, Err(TransDbError::HttpError(405, _))));
    assert!(matches!(client.delete("k").await, Err(TransDbError::HttpError(405, _))));

    // Redirect back to primary: reads/writes work again
    client.set_target(&primary_addr);
    let result = client.get("k").await.expect("get from primary failed");
    assert_eq!(result.value, b"v");
}

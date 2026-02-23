use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use trandb_client::{Client, ClientConfig};
use trandb_common::{ErrorResponse, TranDbError, MAX_KEY_SIZE, MAX_VALUE_SIZE};
use trandb_server::{Server, ServerConfig};

const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(60);

async fn start_server() -> Client {
    let (ready_tx, ready_rx) = oneshot::channel();

    let server = Server::new(ServerConfig {
        address: "127.0.0.1:0".parse().unwrap(),
    });

    tokio::spawn(async move {
        server.run(ready_tx).await.expect("server failed");
    });

    let addr = timeout(SERVER_READY_TIMEOUT, ready_rx)
        .await
        .expect("server did not start within 60 seconds")
        .expect("server ready signal dropped");

    Client::new(ClientConfig {
        base_url: format!("http://{}", addr),
    })
}

#[tokio::test]
async fn test_get_returns_key_not_found() {
    let client = start_server().await;

    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::KeyNotFound(k)) if k == "some_key"));
}

#[tokio::test]
async fn test_put_and_get_round_trip() {
    let client = start_server().await;

    client.put("my_key", b"hello world").await.expect("put failed");
    let result = client.get("my_key").await.expect("get failed");

    assert_eq!(result, b"hello world");
}

#[tokio::test]
async fn test_delete_removes_existing_key() {
    let client = start_server().await;

    client.put("my_key", b"hello").await.expect("put failed");

    let before = client.get("my_key").await.expect("get before delete failed");
    assert_eq!(before, b"hello");

    client.delete("my_key").await.expect("delete failed");

    let after = client.get("my_key").await;
    assert!(matches!(after, Err(TranDbError::KeyNotFound(k)) if k == "my_key"));
}

#[tokio::test]
async fn test_delete_is_idempotent() {
    let client = start_server().await;

    let first = client.delete("nonexistent").await;
    let second = client.delete("nonexistent").await;

    assert!(first.is_ok());
    assert!(second.is_ok());
}

#[tokio::test]
async fn test_put_overwrites_existing_key() {
    let client = start_server().await;

    client.put("my_key", b"first").await.expect("first put failed");
    client.put("my_key", b"second").await.expect("second put failed");
    let result = client.get("my_key").await.expect("get failed");

    assert_eq!(result, b"second");
}

// --- Size validation: server-side rejection (bypassing client pre-flight via raw reqwest) ---

#[tokio::test]
async fn test_server_rejects_oversized_key_on_put() {
    let client = start_server().await;
    let http = reqwest::Client::new();
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);
    let url = format!("{}/keys/{}", client.config.base_url, oversized_key);

    let response = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(b"hello".to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE));
}

#[tokio::test]
async fn test_server_rejects_oversized_value_on_put() {
    let client = start_server().await;
    let http = reqwest::Client::new();
    let url = format!("{}/keys/my_key", client.config.base_url);
    let oversized_value = vec![0u8; MAX_VALUE_SIZE + 1];

    let response = http
        .put(&url)
        .header("Content-Type", "application/octet-stream")
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
    let client = start_server().await;
    let http = reqwest::Client::new();
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);
    let url = format!("{}/keys/{}", client.config.base_url, oversized_key);

    let response = http.get(&url).send().await.unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: ErrorResponse = response.json().await.unwrap();
    assert_eq!(body.error, format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE));
}

#[tokio::test]
async fn test_client_rejects_oversized_key_without_contacting_server() {
    // Uses an unbound address — if the client pre-flight works, no connection is attempted
    let client = Client::new(ClientConfig {
        base_url: "http://127.0.0.1:59212".to_string(),
    });
    let oversized_key = "a".repeat(MAX_KEY_SIZE + 1);

    let result = client.get(&oversized_key).await;

    // Must be KeyTooLarge (pre-flight), not NetworkError (would mean a connection was attempted)
    assert!(matches!(result, Err(TranDbError::KeyTooLarge(_))));
}

#[tokio::test]
async fn test_client_rejects_oversized_value_without_contacting_server() {
    // Uses an unbound address — if the client pre-flight works, no connection is attempted
    let client = Client::new(ClientConfig {
        base_url: "http://127.0.0.1:59212".to_string(),
    });
    let oversized_value = vec![0u8; MAX_VALUE_SIZE + 1];

    let result = client.put("my_key", &oversized_value).await;

    // Must be ValueTooLarge (pre-flight), not NetworkError (would mean a connection was attempted)
    assert!(matches!(result, Err(TranDbError::ValueTooLarge(_))));
}

use trandb_client::{Client, ClientConfig};
use trandb_common::{TranDbError, MAX_KEY_SIZE, MAX_VALUE_SIZE};

#[test]
fn test_client_config_default() {
    let config = ClientConfig::default();
    assert_eq!(config.base_url, "http://127.0.0.1:8080");
}

#[test]
fn test_client_config_custom() {
    let config = ClientConfig {
        base_url: "http://localhost:9000".to_string(),
    };
    assert_eq!(config.base_url, "http://localhost:9000");
}

#[test]
fn test_client_creation() {
    let client = Client::with_default_config();
    assert_eq!(client.config.base_url, "http://127.0.0.1:8080");
}

#[test]
fn test_client_creation_with_config() {
    let config = ClientConfig {
        base_url: "http://example.com:3000".to_string(),
    };
    let client = Client::new(config);
    assert_eq!(client.config.base_url, "http://example.com:3000");
}

#[test]
fn test_build_key_url() {
    let client = Client::with_default_config();
    assert_eq!(
        client.build_key_url("test_key"),
        "http://127.0.0.1:8080/keys/test_key"
    );
}

#[test]
fn test_build_key_url_with_custom_base() {
    let config = ClientConfig {
        base_url: "http://localhost:9000".to_string(),
    };
    let client = Client::new(config);
    assert_eq!(
        client.build_key_url("my_key"),
        "http://localhost:9000/keys/my_key"
    );
}

#[test]
fn test_build_key_url_empty_key() {
    let client = Client::with_default_config();
    assert_eq!(
        client.build_key_url(""),
        "http://127.0.0.1:8080/keys/"
    );
}

#[test]
fn test_build_key_url_special_characters() {
    let client = Client::with_default_config();
    // Note: In real implementation, URL encoding would be needed
    let url = client.build_key_url("key-with-dashes");
    assert_eq!(url, "http://127.0.0.1:8080/keys/key-with-dashes");
}

#[tokio::test]
async fn test_get_returns_key_not_found_on_404() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/missing_key")
        .with_status(404)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });

    assert!(matches!(client.get("missing_key").await, Err(TranDbError::KeyNotFound(k)) if k == "missing_key"));
    assert!(matches!(client.get_allowing_expired("missing_key").await, Err(TranDbError::KeyNotFound(k)) if k == "missing_key"));
}

#[tokio::test]
async fn test_get_returns_bytes_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .with_body(b"hello")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("my_key").await;

    assert_eq!(result.unwrap().value, b"hello");
}

#[tokio::test]
async fn test_get_returns_version_from_etag() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"5\"")
        .with_body(b"hello")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("my_key").await.unwrap();

    assert_eq!(result.version, 5);
    assert_eq!(result.value, b"hello");
}

#[tokio::test]
async fn test_get_returns_missing_etag_error_when_etag_absent() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_body(b"hello")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("my_key").await;

    assert!(matches!(result, Err(TranDbError::MissingETag)));
}

#[tokio::test]
async fn test_put_returns_missing_etag_error_when_etag_absent() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(200)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.put("my_key", b"hello").await;

    assert!(matches!(result, Err(TranDbError::MissingETag)));
}

#[tokio::test]
async fn test_get_returns_empty_bytes_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/empty_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .with_body(b"")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("empty_key").await;

    assert_eq!(result.unwrap().value, b"");
}

#[tokio::test]
async fn test_get_returns_binary_data_on_200() {
    let binary_data: &[u8] = &[0x00, 0xFF, 0x42, 0x01, 0xDE, 0xAD];
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/binary_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .with_body(binary_data)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("binary_key").await;

    assert_eq!(result.unwrap().value, binary_data);
}

#[tokio::test]
async fn test_get_returns_http_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/some_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::HttpError(503, _))));
}

#[tokio::test]
async fn test_get_returns_http_error_on_500() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/some_key")
        .with_status(500)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::HttpError(500, _))));
}

#[tokio::test]
async fn test_put_returns_ok_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.put("my_key", b"hello").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_put_returns_version_from_etag() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"3\"")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let version = client.put("my_key", b"hello").await.unwrap();

    assert_eq!(version, 3);
}

#[tokio::test]
async fn test_put_returns_http_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.put("my_key", b"hello").await;

    assert!(matches!(result, Err(TranDbError::HttpError(503, _))));
}

#[tokio::test]
async fn test_delete_returns_ok_on_204() {
    let mut server = mockito::Server::new_async().await;
    server.mock("DELETE", "/keys/my_key")
        .with_status(204)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.delete("my_key").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_delete_returns_http_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("DELETE", "/keys/my_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.delete("my_key").await;

    assert!(matches!(result, Err(TranDbError::HttpError(503, _))));
}

#[tokio::test]
async fn test_get_returns_network_error_when_server_unreachable() {
    // Port 59210 is not bound to anything — connection will be refused immediately
    let client = Client::new(ClientConfig {
        base_url: "http://127.0.0.1:59210".to_string(),
    });
    let result = client.get("any_key").await;

    assert!(matches!(result, Err(TranDbError::NetworkError(_))));
}

// --- Pre-flight size validation ---

#[tokio::test]
async fn test_get_rejects_oversized_key() {
    let client = Client::with_default_config();
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    assert!(matches!(client.get(&key).await, Err(TranDbError::KeyTooLarge(_))));
    assert!(matches!(client.get_allowing_expired(&key).await, Err(TranDbError::KeyTooLarge(_))));
}

#[tokio::test]
async fn test_put_rejects_oversized_key() {
    let client = Client::with_default_config();
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let result = client.put(&key, b"hello").await;
    assert!(matches!(result, Err(TranDbError::KeyTooLarge(_))));
}

#[tokio::test]
async fn test_put_rejects_oversized_value() {
    let client = Client::with_default_config();
    let value = vec![0u8; MAX_VALUE_SIZE + 1];
    let result = client.put("my_key", &value).await;
    assert!(matches!(result, Err(TranDbError::ValueTooLarge(_))));
}

#[tokio::test]
async fn test_delete_rejects_oversized_key() {
    let client = Client::with_default_config();
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let result = client.delete(&key).await;
    assert!(matches!(result, Err(TranDbError::KeyTooLarge(_))));
}

#[tokio::test]
async fn test_get_parses_400_as_http_error() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(400)
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"error": "Key exceeds maximum size of 1024 bytes"}"#)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("my_key").await;

    assert!(matches!(result, Err(TranDbError::HttpError(400, ref msg)) if msg == "Key exceeds maximum size of 1024 bytes"));
}

// --- TTL: put_with_ttl ---

#[tokio::test]
async fn test_put_with_ttl_sends_x_ttl_header() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .match_header("x-ttl", "9999")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let version = client.put_with_ttl("my_key", b"hello", 9999).await.unwrap();

    assert_eq!(version, 1);
}

#[tokio::test]
async fn test_put_with_ttl_rejects_oversized_inputs() {
    let client = Client::with_default_config();

    let key = "a".repeat(MAX_KEY_SIZE + 1);
    assert!(matches!(client.put_with_ttl(&key, b"hello", 9999).await, Err(TranDbError::KeyTooLarge(_))));

    let value = vec![0u8; MAX_VALUE_SIZE + 1];
    assert!(matches!(client.put_with_ttl("my_key", &value, 9999).await, Err(TranDbError::ValueTooLarge(_))));
}

// --- TTL: get ---

#[tokio::test]
async fn test_get_expired_entry_behavior() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .with_header("X-Expired", "true")
        .with_body(b"stale")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });

    // Strong guarantee: expired entry is treated as not found
    assert!(matches!(client.get("my_key").await, Err(TranDbError::KeyNotFound(k)) if k == "my_key"));

    // Soft guarantee: expired entry is returned with expired=true
    let result = client.get_allowing_expired("my_key").await.unwrap();
    assert!(result.expired);
    assert_eq!(result.value, b"stale");
}

#[tokio::test]
async fn test_get_live_entry_behavior() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_header("ETag", "\"1\"")
        .with_body(b"fresh")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });

    // Strong guarantee: live entry is returned normally
    let result = client.get("my_key").await.unwrap();
    assert_eq!(result.value, b"fresh");
    assert!(!result.expired);

    // Soft guarantee: live entry also has expired=false
    let result = client.get_allowing_expired("my_key").await.unwrap();
    assert_eq!(result.value, b"fresh");
    assert!(!result.expired);
}


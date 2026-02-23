use trandb_client::{Client, ClientConfig};
use trandb_common::TranDbError;

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
    let result = client.get("missing_key").await;

    assert!(matches!(result, Err(TranDbError::KeyNotFound(k)) if k == "missing_key"));
}

#[tokio::test]
async fn test_get_returns_bytes_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/my_key")
        .with_status(200)
        .with_body(b"hello")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("my_key").await;

    assert_eq!(result.unwrap(), b"hello");
}

#[tokio::test]
async fn test_get_returns_empty_bytes_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/empty_key")
        .with_status(200)
        .with_body(b"")
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("empty_key").await;

    assert_eq!(result.unwrap(), b"");
}

#[tokio::test]
async fn test_get_returns_binary_data_on_200() {
    let binary_data: &[u8] = &[0x00, 0xFF, 0x42, 0x01, 0xDE, 0xAD];
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/binary_key")
        .with_status(200)
        .with_body(binary_data)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("binary_key").await;

    assert_eq!(result.unwrap(), binary_data);
}

#[tokio::test]
async fn test_get_returns_server_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/some_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::ServerError(_))));
}

#[tokio::test]
async fn test_get_returns_server_error_on_500() {
    let mut server = mockito::Server::new_async().await;
    server.mock("GET", "/keys/some_key")
        .with_status(500)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::ServerError(_))));
}

#[tokio::test]
async fn test_put_returns_ok_on_200() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(200)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.put("my_key", b"hello").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_put_returns_server_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("PUT", "/keys/my_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.put("my_key", b"hello").await;

    assert!(matches!(result, Err(TranDbError::ServerError(_))));
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
async fn test_delete_returns_server_error_on_503() {
    let mut server = mockito::Server::new_async().await;
    server.mock("DELETE", "/keys/my_key")
        .with_status(503)
        .create_async()
        .await;

    let client = Client::new(ClientConfig { base_url: server.url() });
    let result = client.delete("my_key").await;

    assert!(matches!(result, Err(TranDbError::ServerError(_))));
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

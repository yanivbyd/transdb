use axum::extract::Path;
use axum::http::StatusCode;
use std::net::SocketAddr;
use trandb_server::{handle_get, Server, ServerConfig};

#[test]
fn test_server_config_default() {
    let config = ServerConfig::default();
    assert_eq!(config.address.to_string(), "127.0.0.1:8080");
}

#[test]
fn test_server_config_custom() {
    let addr: SocketAddr = "0.0.0.0:9000".parse().unwrap();
    let config = ServerConfig { address: addr };
    assert_eq!(config.address.to_string(), "0.0.0.0:9000");
}

#[test]
fn test_server_creation() {
    let server = Server::with_default_config();
    assert_eq!(server.address().to_string(), "127.0.0.1:8080");
}

#[test]
fn test_server_creation_with_config() {
    let addr: SocketAddr = "0.0.0.0:9000".parse().unwrap();
    let config = ServerConfig { address: addr };
    let server = Server::new(config);
    assert_eq!(server.address().to_string(), "0.0.0.0:9000");
}

#[test]
fn test_router_creation() {
    let router = Server::create_router();
    assert!(std::mem::size_of_val(&router) > 0);
}

#[tokio::test]
async fn test_handle_get_returns_404() {
    let key = "test_key".to_string();
    let response = handle_get(Path(key)).await;

    let status = response.status();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_get_with_different_keys() {
    let keys = vec!["key1", "key2", "another_key", ""];

    for key in keys {
        let response = handle_get(Path(key.to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

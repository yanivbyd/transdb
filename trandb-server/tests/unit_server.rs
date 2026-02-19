use axum::extract::{Path, State};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use trandb_server::{handle_get, handle_put, Server, ServerConfig, Store};

fn empty_store() -> Store {
    Arc::new(RwLock::new(HashMap::new()))
}

async fn store_with(key: &str, value: &[u8]) -> Store {
    let store = empty_store();
    store.write().await.insert(key.to_string(), value.to_vec());
    store
}

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
    let router = Server::create_router(empty_store());
    assert!(std::mem::size_of_val(&router) > 0);
}

#[tokio::test]
async fn test_handle_get_returns_404_for_missing_key() {
    let response = handle_get(State(empty_store()), Path("missing".to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_get_returns_200_with_value_after_put() {
    let store = store_with("my_key", b"hello").await;
    let response = handle_get(State(store), Path("my_key".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_put_stores_value() {
    let store = empty_store();
    let body = axum::body::Bytes::from("hello");
    let response = handle_put(State(store.clone()), Path("my_key".to_string()), body).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(store.read().await.get("my_key").unwrap(), b"hello");
}

#[tokio::test]
async fn test_handle_put_overwrites_existing_key() {
    let store = store_with("my_key", b"old_value").await;
    let body = axum::body::Bytes::from("new_value");
    handle_put(State(store.clone()), Path("my_key".to_string()), body).await;

    assert_eq!(store.read().await.get("my_key").unwrap(), b"new_value");
}

#[tokio::test]
async fn test_handle_get_returns_404_with_different_keys() {
    let keys = vec!["key1", "key2", "another_key"];
    for key in keys {
        let response = handle_get(State(empty_store()), Path(key.to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

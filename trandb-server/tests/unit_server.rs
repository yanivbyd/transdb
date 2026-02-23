use axum::extract::{Path, State};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use trandb_common::{MAX_KEY_SIZE, MAX_VALUE_SIZE};
use trandb_server::{handle_delete, handle_get, handle_put, Server, ServerConfig, Store};

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

#[tokio::test]
async fn test_handle_delete_returns_204_for_existing_key() {
    let store = store_with("my_key", b"hello").await;
    let response = handle_delete(State(store.clone()), Path("my_key".to_string())).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(store.read().await.get("my_key").is_none());
}

#[tokio::test]
async fn test_handle_delete_returns_204_for_missing_key() {
    let response = handle_delete(State(empty_store()), Path("missing".to_string())).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_handle_delete_removes_only_specified_key() {
    let store = store_with("key_a", b"aaa").await;
    store.write().await.insert("key_b".to_string(), b"bbb".to_vec());

    handle_delete(State(store.clone()), Path("key_a".to_string())).await;

    assert!(store.read().await.get("key_a").is_none());
    assert_eq!(store.read().await.get("key_b").unwrap(), b"bbb");
}

// --- Key size validation ---

#[tokio::test]
async fn test_handle_get_rejects_key_over_limit() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let response = handle_get(State(empty_store()), Path(key)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_get_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let response = handle_get(State(empty_store()), Path(key)).await;
    // Key doesn't exist but size is valid — expect 404, not 400
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_put_rejects_key_over_limit() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let body = axum::body::Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path(key), body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let body = axum::body::Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path(key), body).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_put_rejects_value_over_limit() {
    let body = axum::body::Bytes::from(vec![0u8; MAX_VALUE_SIZE + 1]);
    let response = handle_put(State(empty_store()), Path("my_key".to_string()), body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_value_at_limit() {
    let body = axum::body::Bytes::from(vec![0u8; MAX_VALUE_SIZE]);
    let response = handle_put(State(empty_store()), Path("my_key".to_string()), body).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_delete_rejects_key_over_limit() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let response = handle_delete(State(empty_store()), Path(key)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_delete_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let response = handle_delete(State(empty_store()), Path(key)).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

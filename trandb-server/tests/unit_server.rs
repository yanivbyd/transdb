use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use trandb_common::{MAX_KEY_SIZE, MAX_VALUE_SIZE};
use trandb_server::{
    handle_delete, handle_get, handle_put, AppState, Clock, Entry, Server, ServerConfig,
};

// --- Test helpers ---

const NOW: u64 = 10_000;

struct MockClock(AtomicU64);

impl MockClock {
    fn new(now: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(now)))
    }
}

impl Clock for MockClock {
    fn unix_now_secs(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

fn empty_store() -> AppState {
    AppState::with_clock(MockClock::new(NOW) as Arc<dyn Clock>)
}

async fn store_with(key: &str, value: &[u8]) -> AppState {
    let state = AppState::with_clock(MockClock::new(NOW) as Arc<dyn Clock>);
    state.db.write().await.store.insert(
        key.to_string(),
        Entry { value: Bytes::from(value.to_vec()), version: 1, expires_at: None },
    );
    state
}

fn headers_with_idempotency_key(key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-key", key.parse().unwrap());
    headers
}

fn headers_with_idempotency_key_and_ttl(idempotency_key: &str, ttl: u64) -> HeaderMap {
    let mut headers = headers_with_idempotency_key(idempotency_key);
    headers.insert("x-ttl", ttl.to_string().parse().unwrap());
    headers
}

/// Assert the result of GET /keys/:key.
/// Pass `None` to assert the key does not exist.
/// Pass `Some((value, version))` to assert the key exists with the given value and version.
async fn assert_get(state: &AppState, key: &str, expected: Option<(&[u8], u64)>) {
    let response = handle_get(State(state.clone()), Path(key.to_string())).await;
    match expected {
        None => assert_eq!(response.status(), StatusCode::NOT_FOUND),
        Some((value, version)) => {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers().get(header::ETAG).unwrap(), &format!("\"{}\"", version));
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body.as_ref(), value);
        }
    }
}

#[test]
fn test_server_config_default() {
    let config = ServerConfig::default();
    assert_eq!(config.address.to_string(), "127.0.0.1:8080");
}

#[test]
fn test_server_config_custom() {
    use std::net::SocketAddr;
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
    use std::net::SocketAddr;
    let addr: SocketAddr = "0.0.0.0:9000".parse().unwrap();
    let config = ServerConfig { address: addr };
    let server = Server::new(config);
    assert_eq!(server.address().to_string(), "0.0.0.0:9000");
}

#[test]
fn test_router_creation() {
    let router = Server::create_router(AppState::new());
    assert!(std::mem::size_of_val(&router) > 0);
}

#[tokio::test]
async fn test_handle_get_returns_404_for_missing_key() {
    let response = handle_get(State(empty_store()), Path("missing".to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_get_returns_200_with_value_after_put() {
    let state = store_with("my_key", b"hello").await;
    let response = handle_get(State(state), Path("my_key".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_get_returns_etag_header() {
    let state = store_with("my_key", b"hello").await;
    let response = handle_get(State(state), Path("my_key".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::ETAG).unwrap(), "\"1\"");
}

#[tokio::test]
async fn test_handle_put_stores_value() {
    let state = empty_store();
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from("hello");
    let response = handle_put(State(state.clone()), Path("my_key".to_string()), headers, body).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db.read().await.store.get("my_key").unwrap().value.as_ref(), b"hello");
}

#[tokio::test]
async fn test_handle_put_returns_etag_version_1_for_new_key() {
    let state = empty_store();
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from("hello");
    let response = handle_put(State(state.clone()), Path("my_key".to_string()), headers, body).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::ETAG).unwrap(), "\"1\"");
}

#[tokio::test]
async fn test_handle_put_increments_version() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key("tok-1");
    let r1 = handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v1")).await;
    assert_eq!(r1.headers().get(header::ETAG).unwrap(), "\"1\"");

    let h2 = headers_with_idempotency_key("tok-2");
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v2")).await;
    assert_eq!(r2.headers().get(header::ETAG).unwrap(), "\"2\"");
}

#[tokio::test]
async fn test_handle_put_version_resets_after_delete() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key("tok-1");
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v1")).await;

    let hd = headers_with_idempotency_key("tok-del");
    handle_delete(State(state.clone()), Path("k".to_string()), hd).await;

    let h2 = headers_with_idempotency_key("tok-2");
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v2")).await;
    assert_eq!(r2.headers().get(header::ETAG).unwrap(), "\"1\"");
}

#[tokio::test]
async fn test_handle_get_reflects_put_version() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key("tok-1");
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("a")).await;
    let h2 = headers_with_idempotency_key("tok-2");
    handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("b")).await;

    let response = handle_get(State(state.clone()), Path("k".to_string())).await;
    assert_eq!(response.headers().get(header::ETAG).unwrap(), "\"2\"");
}

#[tokio::test]
async fn test_handle_put_overwrites_existing_key() {
    let state = store_with("my_key", b"old_value").await;
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from("new_value");
    handle_put(State(state.clone()), Path("my_key".to_string()), headers, body).await;

    assert_eq!(state.db.read().await.store.get("my_key").unwrap().value.as_ref(), b"new_value");
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
    let state = store_with("my_key", b"hello").await;
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_delete(State(state.clone()), Path("my_key".to_string()), headers).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(state.db.read().await.store.get("my_key").is_none());
}

#[tokio::test]
async fn test_handle_delete_returns_204_for_missing_key() {
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_delete(State(empty_store()), Path("missing".to_string()), headers).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_handle_delete_removes_only_specified_key() {
    let state = store_with("key_a", b"aaa").await;
    state.db.write().await.store.insert(
        "key_b".to_string(),
        Entry { value: Bytes::from(b"bbb".as_ref()), version: 1, expires_at: None },
    );

    let headers = headers_with_idempotency_key("tok-1");
    handle_delete(State(state.clone()), Path("key_a".to_string()), headers).await;

    assert!(state.db.read().await.store.get("key_a").is_none());
    assert_eq!(state.db.read().await.store.get("key_b").unwrap().value.as_ref(), b"bbb");
}

// --- Idempotency-Key validation ---

#[tokio::test]
async fn test_handle_put_missing_idempotency_key_returns_400() {
    let headers = HeaderMap::new();
    let body = Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path("k".to_string()), headers, body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_delete_missing_idempotency_key_returns_400() {
    let headers = HeaderMap::new();
    let response = handle_delete(State(empty_store()), Path("k".to_string()), headers).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- Idempotency replay ---

#[tokio::test]
async fn test_handle_put_idempotency_replay_returns_same_version() {
    let state = empty_store();
    let h1 = headers_with_idempotency_key("replay-tok");
    let r1 = handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;
    assert_eq!(r1.status(), StatusCode::OK);
    let etag1 = r1.headers().get(header::ETAG).unwrap().clone();

    let h2 = headers_with_idempotency_key("replay-tok");
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(r2.status(), StatusCode::OK);
    assert_eq!(r2.headers().get(header::ETAG).unwrap(), &etag1);
}

#[tokio::test]
async fn test_handle_put_idempotency_replay_does_not_increment_version() {
    let state = empty_store();
    let h1 = headers_with_idempotency_key("replay-tok");
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;

    let h2 = headers_with_idempotency_key("replay-tok");
    handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;

    // Version should still be 1 (replayed, not incremented again)
    assert_eq!(state.db.read().await.store.get("k").unwrap().version, 1);
}

#[tokio::test]
async fn test_handle_delete_idempotency_replay_returns_204() {
    let state = empty_store();
    let h1 = headers_with_idempotency_key("del-tok");
    let r1 = handle_delete(State(state.clone()), Path("k".to_string()), h1).await;
    assert_eq!(r1.status(), StatusCode::NO_CONTENT);

    let h2 = headers_with_idempotency_key("del-tok");
    let r2 = handle_delete(State(state.clone()), Path("k".to_string()), h2).await;
    assert_eq!(r2.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_handle_delete_idempotency_replay_does_not_delete_recreated_key() {
    let state = empty_store();

    // PUT k1
    let h1 = headers_with_idempotency_key("put-tok-1");
    handle_put(State(state.clone()), Path("k1".to_string()), h1, Bytes::from("v1")).await;
    assert_get(&state, "k1", Some((b"v1", 1))).await;

    // DELETE k1
    let hd = headers_with_idempotency_key("del-tok");
    handle_delete(State(state.clone()), Path("k1".to_string()), hd).await;
    assert_get(&state, "k1", None).await;

    // PUT k1 again (recreate)
    let h2 = headers_with_idempotency_key("put-tok-2");
    handle_put(State(state.clone()), Path("k1".to_string()), h2, Bytes::from("v2")).await;
    assert_get(&state, "k1", Some((b"v2", 1))).await;

    // Replay the original DELETE — must return 204 but must NOT delete the key again
    let hd_replay = headers_with_idempotency_key("del-tok");
    let replay = handle_delete(State(state.clone()), Path("k1".to_string()), hd_replay).await;
    assert_eq!(replay.status(), StatusCode::NO_CONTENT);

    assert_get(&state, "k1", Some((b"v2", 1))).await;
}

// --- Idempotency mismatch (422) ---

#[tokio::test]
async fn test_handle_put_idempotency_mismatch_different_key_returns_422() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key("shared-tok");
    handle_put(State(state.clone()), Path("key_a".to_string()), h1, Bytes::from("v")).await;

    // Same idempotency token, different key path
    let h2 = headers_with_idempotency_key("shared-tok");
    let r2 = handle_put(State(state.clone()), Path("key_b".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_handle_put_idempotency_mismatch_method_returns_422() {
    let state = empty_store();

    // First use: DELETE with token X
    let h1 = headers_with_idempotency_key("mixed-tok");
    handle_delete(State(state.clone()), Path("k".to_string()), h1).await;

    // Second use: PUT with same token X
    let h2 = headers_with_idempotency_key("mixed-tok");
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_handle_delete_idempotency_mismatch_method_returns_422() {
    let state = empty_store();

    // First use: PUT with token X
    let h1 = headers_with_idempotency_key("mixed-tok");
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;

    // Second use: DELETE with same token X
    let h2 = headers_with_idempotency_key("mixed-tok");
    let r2 = handle_delete(State(state.clone()), Path("k".to_string()), h2).await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_handle_delete_idempotency_mismatch_key_returns_422() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key("shared-tok");
    handle_delete(State(state.clone()), Path("key_a".to_string()), h1).await;

    // Same idempotency token, different key path
    let h2 = headers_with_idempotency_key("shared-tok");
    let r2 = handle_delete(State(state.clone()), Path("key_b".to_string()), h2).await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
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
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path(key), headers, body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path(key), headers, body).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_put_rejects_value_over_limit() {
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from(vec![0u8; MAX_VALUE_SIZE + 1]);
    let response = handle_put(State(empty_store()), Path("my_key".to_string()), headers, body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_value_at_limit() {
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from(vec![0u8; MAX_VALUE_SIZE]);
    let response = handle_put(State(empty_store()), Path("my_key".to_string()), headers, body).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_delete_rejects_key_over_limit() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_delete(State(empty_store()), Path(key), headers).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_delete_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_delete(State(empty_store()), Path(key), headers).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// Key size check must fire before Idempotency-Key check
#[tokio::test]
async fn test_handle_put_key_size_checked_before_idempotency_key() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let headers = HeaderMap::new(); // no Idempotency-Key
    let body = Bytes::from("hello");
    let response = handle_put(State(empty_store()), Path(key), headers, body).await;
    // Should be 400 for key size, not 400 for missing header (same code but ordering matters for clarity)
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_delete_key_size_checked_before_idempotency_key() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let headers = HeaderMap::new(); // no Idempotency-Key
    let response = handle_delete(State(empty_store()), Path(key), headers).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- AppState::default ---

#[test]
fn test_app_state_default() {
    let state = AppState::default();
    let _ = state.db;
}

// --- Entry::is_expired ---

#[test]
fn test_entry_is_expired() {
    let clock = MockClock::new(NOW);
    assert!(!Entry { value: Bytes::new(), version: 1, expires_at: None }.is_expired(clock.as_ref()));
    assert!(!Entry { value: Bytes::new(), version: 1, expires_at: Some(NOW + 1) }.is_expired(clock.as_ref()));
    assert!(Entry { value: Bytes::new(), version: 1, expires_at: Some(NOW) }.is_expired(clock.as_ref())); // boundary: now == ttl
    assert!(Entry { value: Bytes::new(), version: 1, expires_at: Some(NOW - 1) }.is_expired(clock.as_ref()));   // past
}

// --- PUT with X-TTL ---

#[tokio::test]
async fn test_handle_put_stores_expires_at() {
    let state = empty_store();

    // Future TTL is stored
    let h1 = headers_with_idempotency_key_and_ttl("tok-1", NOW + 1_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 1_000));

    // Past TTL is also accepted and stored (DDB behavior — no rejection at write time)
    let h2 = headers_with_idempotency_key_and_ttl("tok-2", NOW - 1_000);
    let response = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW - 1_000));
}

#[tokio::test]
async fn test_handle_put_with_invalid_ttl_returns_400() {
    let state = empty_store();

    // Non-integer value
    let mut h1 = headers_with_idempotency_key("tok-1");
    h1.insert("x-ttl", "not-a-number".parse().unwrap());
    assert_eq!(handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await.status(), StatusCode::BAD_REQUEST);

    // Negative value (u64 cannot represent it)
    let mut h2 = headers_with_idempotency_key("tok-2");
    h2.insert("x-ttl", "-1".parse().unwrap());
    assert_eq!(handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_without_ttl_clears_previous_expires_at() {
    let state = empty_store();

    // First write: set TTL
    let h1 = headers_with_idempotency_key_and_ttl("tok-1", NOW + 9_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v1")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));

    // Second write: different TTL — overwrites expires_at
    let h2 = headers_with_idempotency_key_and_ttl("tok-2", NOW + 5_000);
    handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v2")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 5_000));

    // Third write: no TTL — clears expires_at
    let h3 = headers_with_idempotency_key("tok-3");
    handle_put(State(state.clone()), Path("k".to_string()), h3, Bytes::from("v3")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, None);
}

#[tokio::test]
async fn test_handle_put_idempotency_replay_does_not_modify_ttl() {
    let state = empty_store();

    // First write: set TTL
    let h1 = headers_with_idempotency_key_and_ttl("replay-tok", NOW + 9_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));

    // Replay with a different TTL — expires_at must not change
    let h2 = headers_with_idempotency_key_and_ttl("replay-tok", NOW - 1_000);
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(r2.status(), StatusCode::OK);
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));
}

// --- GET with X-Expired ---

#[tokio::test]
async fn test_handle_get_expired_entry() {
    // Past TTL (expires_at < NOW)
    let state = empty_store();
    state.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Bytes::from(b"stale".to_vec()), version: 1, expires_at: Some(NOW - 1_000) },
    );
    let response = handle_get(State(state), Path("k".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-expired").unwrap().to_str().unwrap(), "true");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), b"stale");

    // Boundary: expires_at == NOW is also expired
    let state2 = empty_store();
    state2.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Bytes::new(), version: 1, expires_at: Some(NOW) },
    );
    let response2 = handle_get(State(state2), Path("k".to_string())).await;
    assert_eq!(response2.headers().get("x-expired").unwrap().to_str().unwrap(), "true");
}

#[tokio::test]
async fn test_handle_get_no_x_expired_for_live_entry() {
    // Entry with future TTL
    let state = empty_store();
    state.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Bytes::from(b"fresh".to_vec()), version: 1, expires_at: Some(NOW + 1_000) },
    );
    let response = handle_get(State(state), Path("k".to_string())).await;
    assert!(response.headers().get("x-expired").is_none());

    // Entry with no TTL
    let state2 = store_with("k", b"hello").await;
    let response2 = handle_get(State(state2), Path("k".to_string())).await;
    assert!(response2.headers().get("x-expired").is_none());
}

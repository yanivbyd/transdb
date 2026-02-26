use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use transdb_common::{MAX_KEY_SIZE, MAX_VALUE_SIZE};
use transdb_server::{
    config::TOMBSTONE_TTL_SECS, handle_delete, handle_get, handle_put, AppState, Clock, Entry,
    NodeRole, Server, ServerConfig,
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
    AppState::new(MockClock::new(NOW) as Arc<dyn Clock>, NodeRole::Primary)
}

fn replica_store() -> AppState {
    AppState::new(MockClock::new(NOW) as Arc<dyn Clock>, NodeRole::Replica)
}

async fn store_with(key: &str, value: &[u8]) -> AppState {
    let state = AppState::new(MockClock::new(NOW) as Arc<dyn Clock>, NodeRole::Primary);
    state.db.write().await.store.insert(
        key.to_string(),
        Entry { value: Some(Bytes::from(value.to_vec())), version: 1, expires_at: None },
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

/// Consume a response body into bytes.
async fn response_body(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
}

/// Extract the version number from a response's ETag header.
fn response_version(response: &Response) -> u64 {
    let etag = response.headers().get(header::ETAG).unwrap().to_str().unwrap();
    etag.trim_matches('"').parse().unwrap()
}

/// Issue a PUT and return the stored version.
async fn put_key(state: &AppState, key: &str, value: &[u8], tok: &str) -> u64 {
    let headers = headers_with_idempotency_key(tok);
    let response =
        handle_put(State(state.clone()), Path(key.to_string()), headers, Bytes::from(value.to_vec()))
            .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_version(&response)
}

/// Issue a DELETE and return `Some(version)` for a live-key tombstone or `None` for a no-op.
async fn delete_key(state: &AppState, key: &str, tok: &str) -> Option<u64> {
    let headers = headers_with_idempotency_key(tok);
    let response = handle_delete(State(state.clone()), Path(key.to_string()), headers).await;
    match response.status() {
        StatusCode::OK => Some(response_version(&response)),
        StatusCode::NO_CONTENT => None,
        s => panic!("unexpected DELETE status: {s}"),
    }
}

/// Assert the result of GET /keys/:key.
/// `None` asserts 404; `Some(value)` asserts 200 + matching body.
async fn assert_get(state: &AppState, key: &str, expected: Option<&[u8]>) {
    let response = handle_get(State(state.clone()), Path(key.to_string())).await;
    match expected {
        None => assert_eq!(response.status(), StatusCode::NOT_FOUND),
        Some(value) => {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_body(response).await, value);
        }
    }
}

// --- Server struct ---

#[test]
fn test_server_config_custom() {
    use std::net::SocketAddr;
    let addr: SocketAddr = "0.0.0.0:9000".parse().unwrap();
    let config = ServerConfig { address: addr, role: NodeRole::Primary, topology: None };
    assert_eq!(config.address.to_string(), "0.0.0.0:9000");
}

#[test]
fn test_server_creation_with_config() {
    use std::net::SocketAddr;
    let addr: SocketAddr = "0.0.0.0:9000".parse().unwrap();
    let config = ServerConfig { address: addr, role: NodeRole::Primary, topology: None };
    let server = Server::new(config);
    assert_eq!(server.address().to_string(), "0.0.0.0:9000");
}

#[test]
fn test_router_creation() {
    let router =
        Server::create_router(AppState::new(MockClock::new(NOW) as Arc<dyn Clock>, NodeRole::Primary));
    assert!(std::mem::size_of_val(&router) > 0);
}

// --- GET ---

#[tokio::test]
async fn test_handle_get_returns_404_for_missing_key() {
    let response = handle_get(State(empty_store()), Path("missing".to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_get_returns_value_and_etag() {
    let state = store_with("k", b"hello").await;
    let response = handle_get(State(state), Path("k".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::ETAG).is_some());
    assert_eq!(response_body(response).await, b"hello");
}

// --- PUT ---

#[tokio::test]
async fn test_handle_put_stores_value() {
    let state = empty_store();
    let v = put_key(&state, "k", b"hello", "tok-1").await;
    assert!(v > 0, "ETag must be a positive version");
    assert_eq!(
        state.db.read().await.store.get("k").unwrap().value.as_deref().unwrap(),
        b"hello"
    );
}

/// Two successive PUTs to the same key must produce strictly increasing versions,
/// and GET must reflect the latest one.
#[tokio::test]
async fn test_handle_put_version_is_monotonic() {
    let state = empty_store();
    let v1 = put_key(&state, "k", b"v1", "tok-1").await;
    let v2 = put_key(&state, "k", b"v2", "tok-2").await;
    assert!(v2 > v1, "second PUT must produce a higher version");

    let response = handle_get(State(state.clone()), Path("k".to_string())).await;
    assert_eq!(response_version(&response), v2, "GET must reflect the latest version");
}

/// PUTs to different keys must each consume a unique, monotonically increasing version
/// from the global counter.
#[tokio::test]
async fn test_handle_put_versions_are_globally_unique() {
    let state = empty_store();
    let va = put_key(&state, "a", b"1", "tok-a").await;
    let vb = put_key(&state, "b", b"2", "tok-b").await;
    assert_ne!(va, vb, "versions across different keys must be unique");
    assert!(vb > va, "later PUT must have higher version regardless of key");
}

// --- DELETE ---

/// DELETE on a live key writes a tombstone: returns 200+ETag, value=None, expires_at=now+3600.
#[tokio::test]
async fn test_handle_delete_live_key_writes_tombstone() {
    let state = empty_store();
    let v_put = put_key(&state, "k", b"v", "tok-1").await;
    let v_del = delete_key(&state, "k", "tok-del")
        .await
        .expect("DELETE on live key must return 200 + ETag");

    assert!(v_del > v_put, "tombstone version must be higher than the preceding PUT");

    let entry = state.db.read().await.store.get("k").cloned().unwrap();
    assert_eq!(entry.value, None, "tombstone value must be None");
    assert_eq!(entry.expires_at, Some(NOW + TOMBSTONE_TTL_SECS), "tombstone must expire in 1 hour");

    // GET on tombstoned key returns 404.
    assert_get(&state, "k", None).await;
}

/// DELETE on a missing key is a no-op: returns 204, store and next_version unchanged.
#[tokio::test]
async fn test_handle_delete_absent_key_is_noop() {
    let state = empty_store();
    let result = delete_key(&state, "missing", "tok-del").await;
    assert!(result.is_none(), "DELETE on absent key must return 204 No Content");
    assert!(state.db.read().await.store.get("missing").is_none());
    assert_eq!(state.db.read().await.next_version, 0, "next_version must not advance");
}

/// DELETE on an already-tombstoned key is a no-op: returns 204, tombstone unchanged.
#[tokio::test]
async fn test_handle_delete_tombstoned_key_is_noop() {
    let state = empty_store();
    put_key(&state, "k", b"v", "tok-put").await;
    let v_del = delete_key(&state, "k", "tok-del1").await.unwrap();

    let result = delete_key(&state, "k", "tok-del2").await;
    assert!(result.is_none(), "DELETE on tombstone must return 204 No Content");

    let entry = state.db.read().await.store.get("k").cloned().unwrap();
    assert_eq!(entry.version, v_del, "tombstone version must be unchanged");
    assert_eq!(state.db.read().await.next_version, v_del, "next_version must not advance");
}

/// PUT after DELETE must produce a version strictly greater than the tombstone.
#[tokio::test]
async fn test_handle_put_after_delete_has_higher_version() {
    let state = empty_store();
    put_key(&state, "k", b"v1", "tok-1").await;
    let v_del = delete_key(&state, "k", "tok-del").await.unwrap();
    let v_put2 = put_key(&state, "k", b"v2", "tok-2").await;
    assert!(v_put2 > v_del, "re-PUT after DELETE must have a higher version than the tombstone");
}

/// DELETE must only affect the specified key; unrelated keys are untouched.
#[tokio::test]
async fn test_handle_delete_affects_only_specified_key() {
    let state = empty_store();
    put_key(&state, "a", b"aaa", "tok-a").await;
    put_key(&state, "b", b"bbb", "tok-b").await;
    delete_key(&state, "a", "tok-del").await;

    assert_get(&state, "a", None).await; // tombstoned → 404
    assert_get(&state, "b", Some(b"bbb")).await; // untouched
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

/// Replaying a PUT returns the same ETag and does not advance next_version.
#[tokio::test]
async fn test_handle_put_idempotency_replay() {
    let state = empty_store();
    let v1 = put_key(&state, "k", b"v", "replay-tok").await;
    let version_before_replay = state.db.read().await.next_version;

    let v2 = put_key(&state, "k", b"v", "replay-tok").await;

    assert_eq!(v1, v2, "replayed PUT must return same ETag");
    assert_eq!(
        state.db.read().await.next_version,
        version_before_replay,
        "replay must not advance next_version"
    );
}

/// Replaying a live-key DELETE returns the same 200 + ETag.
#[tokio::test]
async fn test_handle_delete_live_key_idempotency_replay() {
    let state = empty_store();
    put_key(&state, "k", b"v", "tok-put").await;
    let v_del = delete_key(&state, "k", "tok-del").await.unwrap();

    let replay =
        handle_delete(State(state.clone()), Path("k".to_string()), headers_with_idempotency_key("tok-del"))
            .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_version(&replay), v_del, "replay must return the same ETag");
}

/// Replaying a live-key DELETE after the key has been re-PUT returns the cached response
/// but does NOT affect the current live entry.
#[tokio::test]
async fn test_handle_delete_idempotency_replay_does_not_affect_recreated_key() {
    let state = empty_store();
    put_key(&state, "k", b"v1", "tok-put-1").await;
    delete_key(&state, "k", "tok-del").await.unwrap();

    // Recreate the key.
    let v_new = put_key(&state, "k", b"v2", "tok-put-2").await;

    // Replay the original DELETE — must return its cached 200 + ETag but NOT re-delete.
    let replay =
        handle_delete(State(state.clone()), Path("k".to_string()), headers_with_idempotency_key("tok-del"))
            .await;
    assert_eq!(replay.status(), StatusCode::OK);

    // The re-PUT key must still be live.
    assert_get(&state, "k", Some(b"v2")).await;
    let current_v = state.db.read().await.store.get("k").unwrap().version;
    assert_eq!(current_v, v_new, "re-PUT version must be unchanged after idempotency replay");
}

// --- Idempotency mismatch (422) ---

#[tokio::test]
async fn test_handle_put_idempotency_mismatch_different_key_returns_422() {
    let state = empty_store();
    put_key(&state, "key_a", b"v", "shared-tok").await;

    let r2 = handle_put(
        State(state.clone()),
        Path("key_b".to_string()),
        headers_with_idempotency_key("shared-tok"),
        Bytes::from("v"),
    )
    .await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// PUT with a token previously used for a DELETE (live key) must return 422.
#[tokio::test]
async fn test_handle_put_idempotency_mismatch_method_returns_422() {
    let state = empty_store();
    // First: DELETE a live key with "mixed-tok" → 200 + ETag (idempotency record written).
    put_key(&state, "k", b"v", "put-tok").await;
    delete_key(&state, "k", "mixed-tok").await.unwrap();

    let r2 = handle_put(
        State(state.clone()),
        Path("k".to_string()),
        headers_with_idempotency_key("mixed-tok"),
        Bytes::from("v"),
    )
    .await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// DELETE with a token previously used for a PUT must return 422.
#[tokio::test]
async fn test_handle_delete_idempotency_mismatch_method_returns_422() {
    let state = empty_store();
    put_key(&state, "k", b"v", "mixed-tok").await;

    let r2 =
        handle_delete(State(state.clone()), Path("k".to_string()), headers_with_idempotency_key("mixed-tok"))
            .await;
    assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// DELETE with a token previously used for a different key's DELETE must return 422.
#[tokio::test]
async fn test_handle_delete_idempotency_mismatch_key_returns_422() {
    let state = empty_store();
    // Delete a live key to ensure the idempotency record is written (200 path).
    put_key(&state, "key_a", b"v", "put-tok").await;
    delete_key(&state, "key_a", "shared-tok").await.unwrap();

    let r2 = handle_delete(
        State(state.clone()),
        Path("key_b".to_string()),
        headers_with_idempotency_key("shared-tok"),
    )
    .await;
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
    // Key doesn't exist but size is valid — expect 404, not 400.
    let response = handle_get(State(empty_store()), Path(key)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_handle_put_rejects_key_over_limit() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_put(State(empty_store()), Path(key), headers, Bytes::from("hello")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_key_at_limit() {
    let key = "a".repeat(MAX_KEY_SIZE);
    let headers = headers_with_idempotency_key("tok-1");
    let response = handle_put(State(empty_store()), Path(key), headers, Bytes::from("hello")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_handle_put_rejects_value_over_limit() {
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from(vec![0u8; MAX_VALUE_SIZE + 1]);
    let response = handle_put(State(empty_store()), Path("k".to_string()), headers, body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_put_accepts_value_at_limit() {
    let headers = headers_with_idempotency_key("tok-1");
    let body = Bytes::from(vec![0u8; MAX_VALUE_SIZE]);
    let response = handle_put(State(empty_store()), Path("k".to_string()), headers, body).await;
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
    // Absent key → 204 No Content.
    let response = handle_delete(State(empty_store()), Path(key), headers).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// Key size check must fire before Idempotency-Key check.
#[tokio::test]
async fn test_handle_put_key_size_checked_before_idempotency_key() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let response =
        handle_put(State(empty_store()), Path(key), HeaderMap::new(), Bytes::from("hello")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_handle_delete_key_size_checked_before_idempotency_key() {
    let key = "a".repeat(MAX_KEY_SIZE + 1);
    let response = handle_delete(State(empty_store()), Path(key), HeaderMap::new()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- Entry::is_expired ---

#[test]
fn test_entry_is_expired() {
    let clock = MockClock::new(NOW);
    assert!(!Entry { value: None, version: 1, expires_at: None }.is_expired(clock.as_ref()));
    assert!(!Entry { value: None, version: 1, expires_at: Some(NOW + 1) }.is_expired(clock.as_ref()));
    assert!(Entry { value: None, version: 1, expires_at: Some(NOW) }.is_expired(clock.as_ref())); // boundary: now == ttl
    assert!(Entry { value: None, version: 1, expires_at: Some(NOW - 1) }.is_expired(clock.as_ref())); // past
}

// --- PUT with X-TTL ---

#[tokio::test]
async fn test_handle_put_stores_expires_at() {
    let state = empty_store();

    // Future TTL is stored.
    let h1 = headers_with_idempotency_key_and_ttl("tok-1", NOW + 1_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 1_000));

    // Past TTL is accepted and stored (no rejection at write time).
    let h2 = headers_with_idempotency_key_and_ttl("tok-2", NOW - 1_000);
    let response = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW - 1_000));
}

#[tokio::test]
async fn test_handle_put_with_invalid_ttl_returns_400() {
    let state = empty_store();

    let mut h1 = headers_with_idempotency_key("tok-1");
    h1.insert("x-ttl", "not-a-number".parse().unwrap());
    assert_eq!(
        handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await.status(),
        StatusCode::BAD_REQUEST
    );

    let mut h2 = headers_with_idempotency_key("tok-2");
    h2.insert("x-ttl", "-1".parse().unwrap());
    assert_eq!(
        handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn test_handle_put_without_ttl_clears_previous_expires_at() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key_and_ttl("tok-1", NOW + 9_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v1")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));

    let h2 = headers_with_idempotency_key_and_ttl("tok-2", NOW + 5_000);
    handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v2")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 5_000));

    let h3 = headers_with_idempotency_key("tok-3");
    handle_put(State(state.clone()), Path("k".to_string()), h3, Bytes::from("v3")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, None);
}

#[tokio::test]
async fn test_handle_put_idempotency_replay_does_not_modify_ttl() {
    let state = empty_store();

    let h1 = headers_with_idempotency_key_and_ttl("replay-tok", NOW + 9_000);
    handle_put(State(state.clone()), Path("k".to_string()), h1, Bytes::from("v")).await;
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));

    let h2 = headers_with_idempotency_key_and_ttl("replay-tok", NOW - 1_000);
    let r2 = handle_put(State(state.clone()), Path("k".to_string()), h2, Bytes::from("v")).await;
    assert_eq!(r2.status(), StatusCode::OK);
    assert_eq!(state.db.read().await.store.get("k").unwrap().expires_at, Some(NOW + 9_000));
}

// --- GET with X-Expired ---

#[tokio::test]
async fn test_handle_get_expired_entry() {
    // Past TTL (expires_at < NOW) and boundary (expires_at == NOW) both return x-expired: true.
    let state = empty_store();
    state.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Some(Bytes::from(b"stale".to_vec())), version: 1, expires_at: Some(NOW - 1_000) },
    );
    let response = handle_get(State(state), Path("k".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-expired").unwrap().to_str().unwrap(), "true");
    assert_eq!(response_body(response).await, b"stale");

    let state2 = empty_store();
    state2.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Some(Bytes::new()), version: 1, expires_at: Some(NOW) },
    );
    let response2 = handle_get(State(state2), Path("k".to_string())).await;
    assert_eq!(response2.headers().get("x-expired").unwrap().to_str().unwrap(), "true");
}

#[tokio::test]
async fn test_handle_get_no_x_expired_for_live_entry() {
    // Future TTL → no x-expired header.
    let state = empty_store();
    state.db.write().await.store.insert(
        "k".to_string(),
        Entry { value: Some(Bytes::from(b"fresh".to_vec())), version: 1, expires_at: Some(NOW + 1_000) },
    );
    let response = handle_get(State(state), Path("k".to_string())).await;
    assert!(response.headers().get("x-expired").is_none());

    // No TTL → no x-expired header.
    let state2 = store_with("k", b"hello").await;
    let response2 = handle_get(State(state2), Path("k".to_string())).await;
    assert!(response2.headers().get("x-expired").is_none());
}

// --- Replica role enforcement ---

#[tokio::test]
async fn test_replica_rejects_all_key_operations_with_405() {
    let state = replica_store();
    let headers = headers_with_idempotency_key("tok-1");

    let get_resp = handle_get(State(state.clone()), Path("k".to_string())).await;
    assert_eq!(get_resp.status(), StatusCode::METHOD_NOT_ALLOWED);

    let put_resp =
        handle_put(State(state.clone()), Path("k".to_string()), headers.clone(), Bytes::from("v")).await;
    assert_eq!(put_resp.status(), StatusCode::METHOD_NOT_ALLOWED);

    let del_resp = handle_delete(State(state.clone()), Path("k".to_string()), headers).await;
    assert_eq!(del_resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

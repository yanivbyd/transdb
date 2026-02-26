use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::time::timeout;
use transdb_common::{ErrorResponse, Topology, MAX_KEY_SIZE, MAX_VALUE_SIZE};

pub mod config;
use config::{LOCK_TIMEOUT, TOMBSTONE_TTL_SECS};

/// Abstraction over current time for testability.
pub trait Clock: Send + Sync {
    fn unix_now_secs(&self) -> u64;
}

/// Production clock backed by `SystemTime`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Role this process plays in the cluster.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeRole {
    Primary,
    Replica,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub value: Option<Bytes>, // None = tombstone
    pub version: u64,
    pub expires_at: Option<u64>,
}

impl Entry {
    /// Returns `true` if the entry has a TTL and the current time is at or past it.
    pub fn is_expired(&self, clock: &dyn Clock) -> bool {
        match self.expires_at {
            None => false,
            Some(ts) => clock.unix_now_secs() >= ts,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HttpMethod {
    Put,
    Delete,
}

#[derive(Clone, Debug)]
pub struct IdempotencyRecord {
    pub method: HttpMethod,
    pub key_path: String,
    pub status_code: u16,
    pub etag: Option<u64>,
    pub created_at: Instant,
}

pub struct DbState {
    pub store: HashMap<String, Entry>,
    pub idempotency_cache: HashMap<String, IdempotencyRecord>,
    pub next_version: u64,
}

pub type Db = Arc<RwLock<DbState>>;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub role: NodeRole,
}

impl AppState {
    pub fn new(clock: Arc<dyn Clock>, role: NodeRole) -> Self {
        Self {
            db: Arc::new(RwLock::new(DbState {
                store: HashMap::new(),
                idempotency_cache: HashMap::new(),
                next_version: 0,
            })),
            clock,
            role,
        }
    }
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub address: SocketAddr,
    pub role: NodeRole,
    pub topology: Option<Topology>,
}

/// TransDB Server
pub struct Server {
    config: ServerConfig,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    /// Get the server's configured address
    pub fn address(&self) -> SocketAddr {
        self.config.address
    }

    /// Create the application router with the given state
    pub fn create_router(state: AppState) -> Router {
        Router::new()
            .route("/keys/:key", get(handle_get).put(handle_put).delete(handle_delete))
            // Allow bodies up to MAX_VALUE_SIZE + 1 so our handler can validate and return 400;
            // axum's default 2MB limit would otherwise return 413 for oversized values.
            .layer(DefaultBodyLimit::max(MAX_VALUE_SIZE + 1))
            .with_state(state)
    }

    /// Run the server, signalling `ready_tx` with the bound address once accepting connections
    pub async fn run(self, ready_tx: tokio::sync::oneshot::Sender<SocketAddr>) -> Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new(Arc::new(SystemClock), self.config.role.clone());
        let app = Self::create_router(state);
        let listener = tokio::net::TcpListener::bind(self.config.address).await?;
        let local_addr = listener.local_addr()?;
        ready_tx.send(local_addr).ok();
        axum::serve(listener, app).await?;
        Ok(())
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ErrorResponse { error: message.into() })).into_response()
}

fn etag_value(version: u64) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{}\"", version)).expect("valid ETag header value")
}

fn extract_idempotency_key(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "Idempotency-Key header is required"))
}

fn verify_and_build_cached_put(record: &IdempotencyRecord, key: &str) -> Response {
    if record.method != HttpMethod::Put || record.key_path != key {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Idempotency-Key was already used for a different method or key path",
        );
    }
    let mut response = StatusCode::OK.into_response();
    if let Some(etag) = record.etag {
        response.headers_mut().insert(header::ETAG, etag_value(etag));
    }
    response
}

fn verify_and_build_cached_delete(record: &IdempotencyRecord, key: &str) -> Response {
    if record.method != HttpMethod::Delete || record.key_path != key {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Idempotency-Key was already used for a different method or key path",
        );
    }
    // Idempotency records for DELETE are only written when a tombstone is written (200 path),
    // so etag is always Some here.
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(header::ETAG, etag_value(record.etag.unwrap()));
    response
}

/// Handler for GET /keys/:key — returns the value and ETag (version) if found, 404 if not.
/// If the entry has an expired TTL, adds `X-Expired: true` to the response.
pub async fn handle_get(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    if state.role == NodeRole::Replica {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "Replica does not accept key operations");
    }

    if key.len() > MAX_KEY_SIZE {
        return error_response(
            StatusCode::BAD_REQUEST,
            format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE),
        );
    }

    let db_guard = match timeout(LOCK_TIMEOUT, state.db.read()).await {
        Ok(guard) => guard,
        Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Server error: Lock acquisition timed out"),
    };

    match db_guard.store.get(&key) {
        None | Some(Entry { value: None, .. }) => {
            error_response(StatusCode::NOT_FOUND, format!("Key not found: {}", key))
        }
        Some(entry) => {
            let expired = entry.is_expired(state.clock.as_ref());
            let value = entry.value.clone().unwrap();
            let mut response = (StatusCode::OK, value).into_response();
            response.headers_mut().insert(header::ETAG, etag_value(entry.version));
            if expired {
                response.headers_mut().insert("x-expired", HeaderValue::from_static("true"));
            }
            response
        }
    }
}

/// Handler for PUT /keys/:key — stores the request body; requires Idempotency-Key header.
/// Accepts an optional `X-TTL` header containing an absolute Unix epoch timestamp (u64).
pub async fn handle_put(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if state.role == NodeRole::Replica {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "Replica does not accept key operations");
    }

    if key.len() > MAX_KEY_SIZE {
        return error_response(
            StatusCode::BAD_REQUEST,
            format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE),
        );
    }
    if body.len() > MAX_VALUE_SIZE {
        return error_response(
            StatusCode::BAD_REQUEST,
            format!("Value exceeds maximum size of {} bytes", MAX_VALUE_SIZE),
        );
    }

    let expires_at = match headers.get("x-ttl") {
        None => None,
        Some(v) => match v.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
            Some(ts) => Some(ts),
            None => return error_response(StatusCode::BAD_REQUEST, "X-TTL must be a non-negative integer"),
        },
    };

    let idempotency_key = match extract_idempotency_key(&headers) {
        Ok(k) => k,
        Err(r) => return r,
    };

    let mut db_guard = match timeout(LOCK_TIMEOUT, state.db.write()).await {
        Ok(guard) => guard,
        Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Server error: Lock acquisition timed out"),
    };

    if let Some(record) = db_guard.idempotency_cache.get(&idempotency_key) {
        return verify_and_build_cached_put(record, &key);
    }

    db_guard.next_version += 1;
    let version = db_guard.next_version;
    db_guard.store.insert(key.clone(), Entry { value: Some(body), version, expires_at });

    let record = IdempotencyRecord {
        method: HttpMethod::Put,
        key_path: key,
        status_code: 200,
        etag: Some(version),
        created_at: Instant::now(),
    };
    db_guard.idempotency_cache.insert(idempotency_key, record);

    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(header::ETAG, etag_value(version));
    response
}

/// Handler for DELETE /keys/:key — removes the key (no-op if absent); requires Idempotency-Key header
pub async fn handle_delete(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    if state.role == NodeRole::Replica {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "Replica does not accept key operations");
    }

    if key.len() > MAX_KEY_SIZE {
        return error_response(
            StatusCode::BAD_REQUEST,
            format!("Key exceeds maximum size of {} bytes", MAX_KEY_SIZE),
        );
    }

    let idempotency_key = match extract_idempotency_key(&headers) {
        Ok(k) => k,
        Err(r) => return r,
    };

    let mut db_guard = match timeout(LOCK_TIMEOUT, state.db.write()).await {
        Ok(guard) => guard,
        Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Server error: Lock acquisition timed out"),
    };

    if let Some(record) = db_guard.idempotency_cache.get(&idempotency_key) {
        return verify_and_build_cached_delete(record, &key);
    }

    match db_guard.store.get(&key) {
        None | Some(Entry { value: None, .. }) => return StatusCode::NO_CONTENT.into_response(),
        _ => {}
    }

    db_guard.next_version += 1;
    let version = db_guard.next_version;
    let now = state.clock.unix_now_secs();
    db_guard.store.insert(key.clone(), Entry { value: None, version, expires_at: Some(now + TOMBSTONE_TTL_SECS) });

    let record = IdempotencyRecord {
        method: HttpMethod::Delete,
        key_path: key,
        status_code: 200,
        etag: Some(version),
        created_at: Instant::now(),
    };
    db_guard.idempotency_cache.insert(idempotency_key, record);

    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(header::ETAG, etag_value(version));
    response
}

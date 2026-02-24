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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::time::timeout;
use trandb_common::{ErrorResponse, MAX_KEY_SIZE, MAX_VALUE_SIZE};

const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

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

#[derive(Clone, Debug)]
pub struct Entry {
    pub value: Bytes,
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
}

pub type Db = Arc<RwLock<DbState>>;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            db: Arc::new(RwLock::new(DbState {
                store: HashMap::new(),
                idempotency_cache: HashMap::new(),
            })),
            clock,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub address: SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8080".parse().unwrap(),
        }
    }
}

/// TranDB Server
pub struct Server {
    config: ServerConfig,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    /// Create a new server with default configuration
    pub fn with_default_config() -> Self {
        Self::new(ServerConfig::default())
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
        let state = AppState::new();
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
    StatusCode::NO_CONTENT.into_response()
}

/// Handler for GET /keys/:key — returns the value and ETag (version) if found, 404 if not.
/// If the entry has an expired TTL, adds `X-Expired: true` to the response.
pub async fn handle_get(State(state): State<AppState>, Path(key): Path<String>) -> Response {
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
        Some(entry) => {
            let expired = entry.is_expired(state.clock.as_ref());
            let mut response = (StatusCode::OK, entry.value.clone()).into_response();
            response.headers_mut().insert(header::ETAG, etag_value(entry.version));
            if expired {
                response.headers_mut().insert("x-expired", HeaderValue::from_static("true"));
            }
            response
        }
        None => error_response(StatusCode::NOT_FOUND, format!("Key not found: {}", key)),
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

    let entry = db_guard.store.entry(key.clone()).or_insert_with(|| Entry {
        value: Bytes::new(),
        version: 0,
        expires_at: None,
    });
    entry.version += 1;
    entry.value = body;
    entry.expires_at = expires_at;
    let version = entry.version;

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

    db_guard.store.remove(&key);

    let record = IdempotencyRecord {
        method: HttpMethod::Delete,
        key_path: key,
        status_code: 204,
        etag: None,
        created_at: Instant::now(),
    };
    db_guard.idempotency_cache.insert(idempotency_key, record);

    StatusCode::NO_CONTENT.into_response()
}

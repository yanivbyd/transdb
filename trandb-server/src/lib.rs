use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;
use trandb_common::TranDbError;

pub type Store = Arc<RwLock<HashMap<String, Vec<u8>>>>;

const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

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

    /// Create the application router with the given store
    pub fn create_router(store: Store) -> Router {
        Router::new()
            .route("/keys/:key", get(handle_get).put(handle_put))
            .with_state(store)
    }

    /// Run the server, signalling `ready_tx` with the bound address once accepting connections
    pub async fn run(self, ready_tx: tokio::sync::oneshot::Sender<SocketAddr>) -> Result<(), Box<dyn std::error::Error>> {
        let store: Store = Arc::new(RwLock::new(HashMap::new()));
        let app = Self::create_router(store);
        let listener = tokio::net::TcpListener::bind(self.config.address).await?;
        let local_addr = listener.local_addr()?;
        ready_tx.send(local_addr).ok();
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Handler for GET /keys/:key — returns the value if found, 404 if not
pub async fn handle_get(State(store): State<Store>, Path(key): Path<String>) -> Response {
    let store_guard = match timeout(LOCK_TIMEOUT, store.read()).await {
        Ok(guard) => guard,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, TranDbError::ServerError("Lock acquisition timed out".to_string()).to_string())
                .into_response()
        }
    };

    match store_guard.get(&key) {
        Some(value) => (StatusCode::OK, value.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, TranDbError::KeyNotFound(key).to_string()).into_response(),
    }
}

/// Handler for PUT /keys/:key — stores the request body as the value
pub async fn handle_put(State(store): State<Store>, Path(key): Path<String>, body: Bytes) -> Response {
    let mut store_guard = match timeout(LOCK_TIMEOUT, store.write()).await {
        Ok(guard) => guard,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, TranDbError::ServerError("Lock acquisition timed out".to_string()).to_string())
                .into_response()
        }
    };

    store_guard.insert(key, body.to_vec());
    StatusCode::OK.into_response()
}

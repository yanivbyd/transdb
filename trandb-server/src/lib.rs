use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::net::SocketAddr;
use trandb_common::TranDbError;

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

    /// Create the application router
    pub fn create_router() -> Router {
        Router::new().route("/keys/:key", get(handle_get))
    }

    /// Run the server
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let app = Self::create_router();
        let listener = tokio::net::TcpListener::bind(self.config.address).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Handler for GET /keys/:key endpoint
/// Always returns 404 (key not found)
pub async fn handle_get(Path(key): Path<String>) -> Response {
    // For now, always return key not found
    let error = TranDbError::KeyNotFound(key);
    (StatusCode::NOT_FOUND, error.to_string()).into_response()
}

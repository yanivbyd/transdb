use transdb_common::{ErrorResponse, Result, Topology, TransDbError, MAX_KEY_SIZE, MAX_VALUE_SIZE};
use uuid::Uuid;

/// TransDB client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub topology: Topology,
}

/// Result returned by a successful GET
#[derive(Debug, Clone, PartialEq)]
pub struct GetResult {
    pub value: Vec<u8>,
    pub version: u64,
    /// `true` when the server returned `X-Expired: true` (entry exists but TTL has elapsed).
    pub expired: bool,
}

/// TransDB Client
pub struct Client {
    pub config: ClientConfig,
    /// Current target address (`host:port`); all requests go to this node.
    /// Defaults to `config.topology.primary_addr`.
    target: String,
    http_client: reqwest::Client,
}

impl Client {
    /// Create a new client with the given configuration
    pub fn new(config: ClientConfig) -> Self {
        let target = config.topology.primary_addr.clone();
        Self {
            config,
            target,
            http_client: reqwest::Client::new(),
        }
    }

    /// Override the target node for all subsequent requests.
    /// Pass a bare `host:port` address matching an entry in the topology.
    pub fn set_target(&mut self, addr: &str) {
        self.target = addr.to_string();
    }

    /// Build the URL for a key operation against the current target.
    pub fn build_key_url(&self, key: &str) -> String {
        format!("http://{}/keys/{}", self.target, key)
    }

    /// Get a value by key (strong guarantee).
    /// Returns `KeyNotFound` if the key does not exist **or** if it exists but has expired.
    pub async fn get(&self, key: &str) -> Result<GetResult> {
        let result = self.get_allowing_expired(key).await?;
        if result.expired {
            return Err(TransDbError::KeyNotFound(key.to_string()));
        }
        Ok(result)
    }

    /// Get a value by key, returning it even if its TTL has elapsed (soft guarantee).
    /// Check `GetResult::expired` to determine whether the value is stale.
    pub async fn get_allowing_expired(&self, key: &str) -> Result<GetResult> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TransDbError::KeyTooLarge(MAX_KEY_SIZE));
        }

        let url = self.build_key_url(key);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| TransDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        let version = parse_etag(&response).ok_or(TransDbError::MissingETag)?;
        let expired = response
            .headers()
            .get("x-expired")
            .and_then(|v| v.to_str().ok())
            == Some("true");

        let bytes = response
            .bytes()
            .await
            .map_err(|e| TransDbError::NetworkError(e.to_string()))?;

        Ok(GetResult { value: bytes.to_vec(), version, expired })
    }

    /// Store a value under the given key; returns the version assigned by this write.
    pub async fn put(&self, key: &str, value: &[u8]) -> Result<u64> {
        self.put_impl(key, value, None).await
    }

    /// Store a value under the given key with an absolute Unix epoch TTL (seconds).
    /// Returns the version assigned by this write.
    pub async fn put_with_ttl(&self, key: &str, value: &[u8], ttl: u64) -> Result<u64> {
        self.put_impl(key, value, Some(ttl)).await
    }

    async fn put_impl(&self, key: &str, value: &[u8], ttl: Option<u64>) -> Result<u64> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TransDbError::KeyTooLarge(MAX_KEY_SIZE));
        }
        if value.len() > MAX_VALUE_SIZE {
            return Err(TransDbError::ValueTooLarge(MAX_VALUE_SIZE));
        }

        let url = self.build_key_url(key);

        let mut request = self
            .http_client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .body(value.to_vec());

        if let Some(ts) = ttl {
            request = request.header("X-TTL", ts.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| TransDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        parse_etag(&response).ok_or(TransDbError::MissingETag)
    }

    /// Delete the value stored under the given key (idempotent)
    pub async fn delete(&self, key: &str) -> Result<()> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TransDbError::KeyTooLarge(MAX_KEY_SIZE));
        }

        let url = self.build_key_url(key);

        let response = self
            .http_client
            .delete(&url)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .send()
            .await
            .map_err(|e| TransDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        Ok(())
    }
}

/// Parse the ETag header as a `u64` version; returns `None` if absent or unparseable.
fn parse_etag(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"'))
        .and_then(|s| s.parse::<u64>().ok())
}

async fn parse_error_response(
    status: reqwest::StatusCode,
    key: &str,
    response: reqwest::Response,
) -> TransDbError {
    if status == reqwest::StatusCode::NOT_FOUND {
        return TransDbError::KeyNotFound(key.to_string());
    }

    let error_msg = response
        .json::<ErrorResponse>()
        .await
        .map(|r| r.error)
        .unwrap_or_else(|_| format!("Server returned status: {}", status));

    TransDbError::HttpError(status.as_u16(), error_msg)
}

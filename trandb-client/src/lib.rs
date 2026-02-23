use trandb_common::{ErrorResponse, Result, TranDbError, MAX_KEY_SIZE, MAX_VALUE_SIZE};

/// TranDB client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".to_string(),
        }
    }
}

/// TranDB Client
pub struct Client {
    pub config: ClientConfig,
    http_client: reqwest::Client,
}

impl Client {
    /// Create a new client with the given configuration
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }

    /// Create a new client with default configuration
    pub fn with_default_config() -> Self {
        Self::new(ClientConfig::default())
    }

    /// Build the URL for a key operation
    pub fn build_key_url(&self, key: &str) -> String {
        format!("{}/keys/{}", self.config.base_url, key)
    }

    /// Get a value by key
    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TranDbError::KeyTooLarge(MAX_KEY_SIZE));
        }

        let url = self.build_key_url(key);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// Store a value under the given key
    pub async fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TranDbError::KeyTooLarge(MAX_KEY_SIZE));
        }
        if value.len() > MAX_VALUE_SIZE {
            return Err(TranDbError::ValueTooLarge(MAX_VALUE_SIZE));
        }

        let url = self.build_key_url(key);

        let response = self
            .http_client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .body(value.to_vec())
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        Ok(())
    }

    /// Delete the value stored under the given key (idempotent)
    pub async fn delete(&self, key: &str) -> Result<()> {
        if key.len() > MAX_KEY_SIZE {
            return Err(TranDbError::KeyTooLarge(MAX_KEY_SIZE));
        }

        let url = self.build_key_url(key);

        let response = self
            .http_client
            .delete(&url)
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(parse_error_response(status, key, response).await);
        }

        Ok(())
    }
}

async fn parse_error_response(
    status: reqwest::StatusCode,
    key: &str,
    response: reqwest::Response,
) -> TranDbError {
    if status == reqwest::StatusCode::NOT_FOUND {
        return TranDbError::KeyNotFound(key.to_string());
    }

    let error_msg = response
        .json::<ErrorResponse>()
        .await
        .map(|r| r.error)
        .unwrap_or_else(|_| format!("Server returned status: {}", status));

    TranDbError::HttpError(status.as_u16(), error_msg)
}

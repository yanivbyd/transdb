use trandb_common::{Result, TranDbError};

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
        let url = self.build_key_url(key);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(TranDbError::KeyNotFound(key.to_string()));
        }

        if !response.status().is_success() {
            return Err(TranDbError::ServerError(format!(
                "Server returned status: {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// Store a value under the given key
    pub async fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        let url = self.build_key_url(key);

        let response = self
            .http_client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .body(value.to_vec())
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TranDbError::ServerError(format!(
                "Server returned status: {}",
                response.status()
            )));
        }

        Ok(())
    }

    /// Delete the value stored under the given key (idempotent)
    pub async fn delete(&self, key: &str) -> Result<()> {
        let url = self.build_key_url(key);

        let response = self
            .http_client
            .delete(&url)
            .send()
            .await
            .map_err(|e| TranDbError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TranDbError::ServerError(format!(
                "Server returned status: {}",
                response.status()
            )));
        }

        Ok(())
    }
}

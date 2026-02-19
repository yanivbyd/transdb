use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use trandb_client::{Client, ClientConfig};
use trandb_common::TranDbError;
use trandb_server::{Server, ServerConfig};

const INTEGRATION_TEST_PORT: u16 = 18080;
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn test_get_returns_key_not_found() {
    let (ready_tx, ready_rx) = oneshot::channel();

    let addr = format!("127.0.0.1:{}", INTEGRATION_TEST_PORT).parse().unwrap();
    let server = Server::new(ServerConfig { address: addr });

    tokio::spawn(async move {
        server.run(ready_tx).await.expect("server failed");
    });

    timeout(SERVER_READY_TIMEOUT, ready_rx)
        .await
        .expect("server did not start within 60 seconds")
        .expect("server ready signal dropped");

    let client = Client::new(ClientConfig {
        base_url: format!("http://127.0.0.1:{}", INTEGRATION_TEST_PORT),
    });

    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::KeyNotFound(k)) if k == "some_key"));
}

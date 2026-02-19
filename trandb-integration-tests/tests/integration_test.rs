use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use trandb_client::{Client, ClientConfig};
use trandb_common::TranDbError;
use trandb_server::{Server, ServerConfig};

const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(60);

async fn start_server() -> Client {
    let (ready_tx, ready_rx) = oneshot::channel();

    let server = Server::new(ServerConfig {
        address: "127.0.0.1:0".parse().unwrap(),
    });

    tokio::spawn(async move {
        server.run(ready_tx).await.expect("server failed");
    });

    let addr = timeout(SERVER_READY_TIMEOUT, ready_rx)
        .await
        .expect("server did not start within 60 seconds")
        .expect("server ready signal dropped");

    Client::new(ClientConfig {
        base_url: format!("http://{}", addr),
    })
}

#[tokio::test]
async fn test_get_returns_key_not_found() {
    let client = start_server().await;

    let result = client.get("some_key").await;

    assert!(matches!(result, Err(TranDbError::KeyNotFound(k)) if k == "some_key"));
}

#[tokio::test]
async fn test_put_and_get_round_trip() {
    let client = start_server().await;

    client.put("my_key", b"hello world").await.expect("put failed");
    let result = client.get("my_key").await.expect("get failed");

    assert_eq!(result, b"hello world");
}

#[tokio::test]
async fn test_put_overwrites_existing_key() {
    let client = start_server().await;

    client.put("my_key", b"first").await.expect("first put failed");
    client.put("my_key", b"second").await.expect("second put failed");
    let result = client.get("my_key").await.expect("get failed");

    assert_eq!(result, b"second");
}

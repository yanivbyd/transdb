use trandb_common::TranDbError;

#[test]
fn test_error_display() {
    let err = TranDbError::KeyNotFound("test_key".to_string());
    assert_eq!(err.to_string(), "Key not found: test_key");
}

#[test]
fn test_error_equality() {
    let err1 = TranDbError::KeyNotFound("key1".to_string());
    let err2 = TranDbError::KeyNotFound("key1".to_string());
    let err3 = TranDbError::KeyNotFound("key2".to_string());

    assert_eq!(err1, err2);
    assert_ne!(err1, err3);
}

#[test]
fn test_network_error() {
    let err = TranDbError::NetworkError("connection failed".to_string());
    assert_eq!(err.to_string(), "Network error: connection failed");
}

#[test]
fn test_http_error_5xx() {
    let err = TranDbError::HttpError(503, "Server error: Lock acquisition timed out".to_string());
    assert_eq!(err.to_string(), "HTTP 503: Server error: Lock acquisition timed out");
}

#[test]
fn test_key_too_large() {
    let err = TranDbError::KeyTooLarge(1024);
    assert_eq!(err.to_string(), "Key exceeds maximum size of 1024 bytes");
}

#[test]
fn test_value_too_large() {
    let err = TranDbError::ValueTooLarge(4194304);
    assert_eq!(err.to_string(), "Value exceeds maximum size of 4194304 bytes");
}

#[test]
fn test_http_error() {
    let err = TranDbError::HttpError(400, "Key exceeds maximum size of 1024 bytes".to_string());
    assert_eq!(err.to_string(), "HTTP 400: Key exceeds maximum size of 1024 bytes");
}

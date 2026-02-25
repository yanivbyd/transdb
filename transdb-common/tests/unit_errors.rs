use transdb_common::TransDbError;

#[test]
fn test_error_display() {
    let err = TransDbError::KeyNotFound("test_key".to_string());
    assert_eq!(err.to_string(), "Key not found: test_key");
}

#[test]
fn test_error_equality() {
    let err1 = TransDbError::KeyNotFound("key1".to_string());
    let err2 = TransDbError::KeyNotFound("key1".to_string());
    let err3 = TransDbError::KeyNotFound("key2".to_string());

    assert_eq!(err1, err2);
    assert_ne!(err1, err3);
}

#[test]
fn test_network_error() {
    let err = TransDbError::NetworkError("connection failed".to_string());
    assert_eq!(err.to_string(), "Network error: connection failed");
}

#[test]
fn test_http_error_5xx() {
    let err = TransDbError::HttpError(503, "Server error: Lock acquisition timed out".to_string());
    assert_eq!(err.to_string(), "HTTP 503: Server error: Lock acquisition timed out");
}

#[test]
fn test_key_too_large() {
    let err = TransDbError::KeyTooLarge(1024);
    assert_eq!(err.to_string(), "Key exceeds maximum size of 1024 bytes");
}

#[test]
fn test_value_too_large() {
    let err = TransDbError::ValueTooLarge(4194304);
    assert_eq!(err.to_string(), "Value exceeds maximum size of 4194304 bytes");
}

#[test]
fn test_http_error() {
    let err = TransDbError::HttpError(400, "Key exceeds maximum size of 1024 bytes".to_string());
    assert_eq!(err.to_string(), "HTTP 400: Key exceeds maximum size of 1024 bytes");
}

#[test]
fn test_missing_etag() {
    let err = TransDbError::MissingETag;
    assert_eq!(err.to_string(), "Server response missing ETag header");
}

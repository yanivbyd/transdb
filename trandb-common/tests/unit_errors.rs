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
fn test_server_error() {
    let err = TranDbError::ServerError("internal error".to_string());
    assert_eq!(err.to_string(), "Server error: internal error");
}

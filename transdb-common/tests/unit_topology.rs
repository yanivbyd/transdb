use transdb_common::Topology;

#[test]
fn test_topology_single_node() {
    let t = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: None,
        replica_grpc_addr: None,
    };
    assert_eq!(t.primary_addr, "127.0.0.1:3000");
    assert!(t.replica_addr.is_none());
    assert!(t.replica_grpc_addr.is_none());
}

#[test]
fn test_topology_with_and_without_replica() {
    // With both replica fields set
    let t = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: Some("127.0.0.1:3001".to_string()),
        replica_grpc_addr: Some("127.0.0.1:4001".to_string()),
    };
    assert_eq!(t.replica_addr.as_deref(), Some("127.0.0.1:3001"));
    assert_eq!(t.replica_grpc_addr.as_deref(), Some("127.0.0.1:4001"));

    // Omitting optional fields from JSON produces None for both
    let json = r#"{"primary_addr":"127.0.0.1:3000"}"#;
    let parsed: Topology = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.primary_addr, "127.0.0.1:3000");
    assert!(parsed.replica_addr.is_none());
    assert!(parsed.replica_grpc_addr.is_none());

    // replica_grpc_addr present but replica_addr absent
    let json = r#"{"primary_addr":"127.0.0.1:3000","replica_grpc_addr":"127.0.0.1:4001"}"#;
    let parsed: Topology = serde_json::from_str(json).unwrap();
    assert!(parsed.replica_addr.is_none());
    assert_eq!(parsed.replica_grpc_addr.as_deref(), Some("127.0.0.1:4001"));
}

#[test]
fn test_topology_equality() {
    let a = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: None,
        replica_grpc_addr: None,
    };
    let b = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: None,
        replica_grpc_addr: None,
    };
    let c = Topology {
        primary_addr: "10.0.0.1:3000".to_string(),
        replica_addr: None,
        replica_grpc_addr: None,
    };
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn test_topology_roundtrip_json() {
    // Without replica_grpc_addr
    let original = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: Some("127.0.0.1:3001".to_string()),
        replica_grpc_addr: None,
    };
    let json = serde_json::to_string(&original).unwrap();
    let decoded: Topology = serde_json::from_str(&json).unwrap();
    assert_eq!(original, decoded);

    // With replica_grpc_addr
    let original = Topology {
        primary_addr: "127.0.0.1:3000".to_string(),
        replica_addr: Some("127.0.0.1:3001".to_string()),
        replica_grpc_addr: Some("127.0.0.1:4001".to_string()),
    };
    let json = serde_json::to_string(&original).unwrap();
    let decoded: Topology = serde_json::from_str(&json).unwrap();
    assert_eq!(original, decoded);
}

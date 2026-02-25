use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use transdb_common::Topology;
use transdb_server::{NodeRole, Server, ServerConfig};

#[derive(Debug, Clone, ValueEnum)]
enum Role {
    Primary,
    Replica,
}

#[derive(Parser, Debug)]
#[command(name = "transdb-server")]
struct Args {
    /// Role this node plays in the cluster.
    #[arg(long)]
    role: Role,

    /// Path to a JSON file containing the cluster Topology.
    #[arg(long)]
    topology: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let topology: Topology = serde_json::from_str(&std::fs::read_to_string(&args.topology)?)?;

    let role = match args.role {
        Role::Primary => NodeRole::Primary,
        Role::Replica => NodeRole::Replica,
    };

    let address: SocketAddr = match role {
        NodeRole::Primary => topology.primary_addr.parse()?,
        NodeRole::Replica => topology
            .replica_addr
            .as_deref()
            .ok_or("replica_addr missing from topology")?
            .parse()?,
    };

    let config = ServerConfig {
        address,
        role,
        topology: Some(topology),
    };

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    // Print "Listening on <addr>" once the server signals it is bound.
    tokio::spawn(async move {
        if let Ok(addr) = ready_rx.await {
            println!("Listening on {}", addr);
        }
    });

    Server::new(config).run(ready_tx).await?;
    Ok(())
}

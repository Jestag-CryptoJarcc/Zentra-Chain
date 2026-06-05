//! P2P networking layer using libp2p.

use libp2p::{
    gossipsub, identify, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use tokio::sync::mpsc;
use tracing::{info, warn, error};

/// Combined network behaviour for Zentra P2P.
#[derive(NetworkBehaviour)]
pub struct ZentraBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
}

/// GossipSub topics for block and transaction propagation.
pub const BLOCKS_TOPIC: &str = "zentra/blocks/1";
pub const TXS_TOPIC: &str = "zentra/txs/1";

/// P2P network manager.
pub struct P2pManager {
    pub local_peer_id: PeerId,
    pub listen_port: u16,
}

impl P2pManager {
    /// Create a new P2P manager (does not start listening yet).
    pub fn new(port: u16) -> anyhow::Result<Self> {
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        info!(peer_id = %local_peer_id, "P2P identity generated");

        Ok(P2pManager {
            local_peer_id,
            listen_port: port,
        })
    }

    /// Get the local peer ID.
    pub fn peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }
}

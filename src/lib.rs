// Re-export subcrates under original module paths for integration test compatibility.

pub mod pb {
    pub use keel_proto::pb::*;
}

pub mod index {
    pub use keel_store::index::*;
}

pub mod eviction {
    pub use keel_store::eviction::*;
}

pub mod store {
    pub use keel_store::store::*;
}

pub mod registry {
    pub use keel_store::registry::*;
}

pub mod signal {
    pub use keel_signal::*;
}

pub mod cluster {
    pub use keel_cluster::cluster::*;
}

pub mod router {
    pub use keel_cluster::router::*;
}

pub mod config;
pub mod error;
pub mod server;

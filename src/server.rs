// gRPC server: tonic KeelMemory service implementation
//
// Phase 2: when a ClusterManager is present, writes and searches are forwarded
// to the owning node based on session_id consistent-hash routing.

use crate::config::Config;
use crate::error::{KeelError, Result};
use keel_cluster::cluster::ClusterManager;
use keel_proto::pb::keel_memory_server::{KeelMemory, KeelMemoryServer};
use keel_proto::pb::{
    EvictRequest, EvictResponse, HealthRequest, HealthResponse, ReadRequest, ReadResponse,
    SearchRequest, SearchResponse, WriteRequest, WriteResponse,
};
use keel_signal::{now_ms, SignalEvent, SignalEventType, SignalExporter};
use keel_store::registry::{bytes_to_f32, MemoryRegistry};
use std::sync::Arc;
use tonic::{Request, Response, Status};

fn to_status<E: std::fmt::Display>(e: E) -> Status {
    Status::internal(e.to_string())
}

pub struct KeelService {
    registry: Arc<MemoryRegistry>,
    signal: Arc<SignalExporter>,
    cluster: Option<Arc<ClusterManager>>,
}

#[tonic::async_trait]
impl KeelMemory for KeelService {
    async fn write(
        &self,
        request: Request<WriteRequest>,
    ) -> std::result::Result<Response<WriteResponse>, Status> {
        let chunk = request
            .into_inner()
            .chunk
            .ok_or_else(|| Status::invalid_argument("missing chunk"))?;

        let session_id = chunk.session_id.clone();

        if let Some(cluster) = &self.cluster {
            if !cluster.is_local_session(&session_id) {
                let owner = cluster.node_for_session(&session_id);
                tracing::debug!(
                    "Forwarding write for session {} to peer {}",
                    session_id,
                    owner
                );
                if let Some(mut client) = cluster.get_client(&owner).await {
                    return client
                        .write(Request::new(WriteRequest { chunk: Some(chunk) }))
                        .await
                        .map_err(|e| Status::unavailable(format!("Peer {}: {}", owner, e)));
                }
                tracing::warn!(
                    "Peer {} unavailable for session {}; handling locally",
                    owner,
                    session_id
                );
            }
        }

        let id = self.registry.write(chunk).await.map_err(to_status)?;

        self.signal.emit(SignalEvent {
            event_type: SignalEventType::Write,
            session_id,
            chunk_id: Some(id.clone()),
            query_embedding: None,
            timestamp_ms: now_ms(),
        });

        Ok(Response::new(WriteResponse { id, ok: true }))
    }

    async fn read(
        &self,
        request: Request<ReadRequest>,
    ) -> std::result::Result<Response<ReadResponse>, Status> {
        let id = request.into_inner().id;
        let result = self.registry.read(&id).await.map_err(to_status)?;

        let (found, session_id, event_type) = match &result {
            Some(chunk) => (true, chunk.session_id.clone(), SignalEventType::Hit),
            None => (false, String::new(), SignalEventType::Miss),
        };

        self.signal.emit(SignalEvent {
            event_type,
            session_id,
            chunk_id: Some(id),
            query_embedding: None,
            timestamp_ms: now_ms(),
        });

        Ok(Response::new(ReadResponse { chunk: result, found }))
    }

    async fn semantic_search(
        &self,
        request: Request<SearchRequest>,
    ) -> std::result::Result<Response<SearchResponse>, Status> {
        let req = request.into_inner();
        let query = bytes_to_f32(&req.query_embedding).map_err(to_status)?;
        let top_k = req.top_k as usize;
        let min_score = req.min_score;

        let results = self
            .registry
            .search(&query, top_k, min_score)
            .await
            .map_err(to_status)?;

        let event_type = if results.is_empty() {
            SignalEventType::Miss
        } else {
            SignalEventType::Hit
        };

        self.signal.emit(SignalEvent {
            event_type,
            session_id: String::new(),
            chunk_id: None,
            query_embedding: Some(query),
            timestamp_ms: now_ms(),
        });

        Ok(Response::new(SearchResponse { results }))
    }

    async fn evict(
        &self,
        request: Request<EvictRequest>,
    ) -> std::result::Result<Response<EvictResponse>, Status> {
        let session_id = request.into_inner().session_id;
        let evicted_count = self
            .registry
            .evict_session(&session_id)
            .await
            .map_err(to_status)?;

        self.signal.emit(SignalEvent {
            event_type: SignalEventType::Eviction,
            session_id,
            chunk_id: None,
            query_embedding: None,
            timestamp_ms: now_ms(),
        });

        Ok(Response::new(EvictResponse { evicted_count }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> std::result::Result<Response<HealthResponse>, Status> {
        let chunks_stored = self.registry.count().await.map_err(to_status)?;
        let peer_count = self
            .cluster
            .as_ref()
            .map(|c| c.peer_count())
            .unwrap_or(0);
        Ok(Response::new(HealthResponse {
            status: format!("ok (peers: {})", peer_count),
            chunks_stored,
        }))
    }
}

pub struct KeelServer {
    config: Config,
    registry: Arc<MemoryRegistry>,
    signal: Arc<SignalExporter>,
    cluster: Option<Arc<ClusterManager>>,
}

impl KeelServer {
    pub fn new(
        config: Config,
        registry: Arc<MemoryRegistry>,
        signal: Arc<SignalExporter>,
        cluster: Option<Arc<ClusterManager>>,
    ) -> Self {
        Self { config, registry, signal, cluster }
    }

    pub async fn run(self) -> Result<()> {
        let addr: std::net::SocketAddr = self.config.bind_address.parse().map_err(|e| {
            KeelError::Config(format!("Invalid bind address '{}': {}", self.config.bind_address, e))
        })?;

        let service = KeelService {
            registry: self.registry,
            signal: self.signal,
            cluster: self.cluster,
        };

        tracing::info!("Keel gRPC server listening on {}", addr);

        tonic::transport::Server::builder()
            .add_service(KeelMemoryServer::new(service))
            .serve(addr)
            .await
            .map_err(|e| KeelError::Grpc(e.to_string()))?;

        Ok(())
    }
}

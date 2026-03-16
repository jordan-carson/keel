// gRPC server: tonic KeelMemory service implementation
//
// Phase 2 routing:
//   Write  — forwarded to the session-owning peer; falls back to local on peer failure.
//   Search — fan-out to all live peers, results merged with local, deduped, top-k returned.
//   All other RPCs — local only (Read, Evict, Health).

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
use std::collections::HashSet;
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
                if !cluster.is_dead(&owner) {
                    if let Some(mut client) = cluster.get_client(&owner).await {
                        match client
                            .write(Request::new(WriteRequest { chunk: Some(chunk.clone()) }))
                            .await
                        {
                            Ok(resp) => return Ok(resp),
                            Err(e) => {
                                tracing::warn!(
                                    "Peer {} write failed: {}; marking dead, handling locally",
                                    owner,
                                    e
                                );
                                cluster.mark_dead(&owner);
                            }
                        }
                    }
                } else {
                    tracing::debug!(
                        "Peer {} is dead; handling session {} locally",
                        owner,
                        session_id
                    );
                }
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

        // Local search
        let mut all_results = self
            .registry
            .search(&query, top_k, min_score)
            .await
            .map_err(to_status)?;

        // Phase 2: fan-out to all live peers and merge results.
        if let Some(cluster) = &self.cluster {
            if cluster.peer_count() > 0 {
                let peer_results = cluster.fan_out_search(req.clone()).await;
                all_results.extend(peer_results);
            }
        }

        // Deduplicate by chunk ID, sort by score descending, truncate to top_k.
        let mut seen_ids: HashSet<String> = HashSet::new();
        all_results.retain(|sc| {
            match sc.chunk.as_ref().map(|c| c.id.as_str()) {
                Some(id) if !id.is_empty() => seen_ids.insert(id.to_string()),
                _ => true,
            }
        });
        all_results
            .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(top_k);

        let event_type = if all_results.is_empty() {
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

        Ok(Response::new(SearchResponse { results: all_results }))
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
        let (peer_count, dead_count) = self
            .cluster
            .as_ref()
            .map(|c| {
                let peers = c.peer_addrs();
                let dead = peers.iter().filter(|a| c.is_dead(a)).count();
                (peers.len(), dead)
            })
            .unwrap_or((0, 0));

        let status = if dead_count > 0 {
            format!("ok (peers: {}, dead: {})", peer_count, dead_count)
        } else {
            format!("ok (peers: {})", peer_count)
        };

        Ok(Response::new(HealthResponse { status, chunks_stored }))
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
            KeelError::Config(format!(
                "Invalid bind address '{}': {}",
                self.config.bind_address, e
            ))
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

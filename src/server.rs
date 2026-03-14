// gRPC server: tonic KeelMemory service implementation

use crate::config::Config;
use crate::error::{KeelError, Result};
use crate::pb::keel_memory_server::{KeelMemory, KeelMemoryServer};
use crate::pb::{
    EvictRequest, EvictResponse, HealthRequest, HealthResponse, ReadRequest, ReadResponse,
    SearchRequest, SearchResponse, WriteRequest, WriteResponse,
};
use crate::registry::{bytes_to_f32, MemoryRegistry};
use crate::signal::{now_ms, SignalEvent, SignalEventType, SignalExporter};
use std::sync::Arc;
use tonic::{Request, Response, Status};

fn to_status(e: KeelError) -> Status {
    Status::internal(e.to_string())
}

/// Implements the KeelMemory gRPC service, delegating to MemoryRegistry.
pub struct KeelService {
    registry: Arc<MemoryRegistry>,
    signal: Arc<SignalExporter>,
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
        Ok(Response::new(HealthResponse {
            status: "ok".to_string(),
            chunks_stored,
        }))
    }
}

/// Owns the tonic server and binds it to the configured address.
pub struct KeelServer {
    config: Config,
    registry: Arc<MemoryRegistry>,
    signal: Arc<SignalExporter>,
}

impl KeelServer {
    pub fn new(config: Config, registry: Arc<MemoryRegistry>, signal: Arc<SignalExporter>) -> Self {
        Self { config, registry, signal }
    }

    pub async fn run(self) -> Result<()> {
        let addr: std::net::SocketAddr = self.config.bind_address.parse().map_err(|e| {
            KeelError::Config(format!("Invalid bind address '{}': {}", self.config.bind_address, e))
        })?;

        let service = KeelService {
            registry: self.registry,
            signal: self.signal,
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

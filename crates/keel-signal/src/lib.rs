// SignalExporter: async training signal emitter with JSONL writer

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalEventType {
    Write,
    Hit,
    Miss,
    Eviction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    pub event_type: SignalEventType,
    pub session_id: String,
    pub chunk_id: Option<String>,
    pub query_embedding: Option<Vec<f32>>,
    pub timestamp_ms: u64,
}

pub struct SignalExporter {
    tx: mpsc::Sender<SignalEvent>,
}

impl SignalExporter {
    pub fn new(output_path: &str) -> Self {
        let (tx, mut rx) = mpsc::channel::<SignalEvent>(1024);
        let path = output_path.to_string();

        tokio::spawn(async move {
            let mut file = match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("SignalExporter: failed to open {}: {}", path, e);
                    return;
                }
            };

            while let Some(event) = rx.recv().await {
                if let Ok(line) = serde_json::to_string(&event) {
                    let entry = format!("{}\n", line);
                    if let Err(e) = file.write_all(entry.as_bytes()).await {
                        tracing::warn!("SignalExporter: write error: {}", e);
                    }
                }
            }
        });

        Self { tx }
    }

    /// Non-blocking fire-and-forget emit. Drops the event if the channel is full.
    pub fn emit(&self, event: SignalEvent) {
        let _ = self.tx.try_send(event);
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

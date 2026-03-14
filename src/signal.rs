// SignalExporter: Async training signal/log exporter

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSignal {
    pub step: u64,
    pub loss: f32,
    pub learning_rate: f32,
    pub timestamp: u64,
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

pub struct SignalExporter {
    signals: Arc<RwLock<Vec<TrainingSignal>>>,
    export_interval_secs: u64,
}

impl SignalExporter {
    pub fn new(export_interval_secs: u64) -> Self {
        Self {
            signals: Arc::new(RwLock::new(Vec::new())),
            export_interval_secs,
        }
    }

    pub async fn emit(&self, signal: TrainingSignal) {
        let mut signals = self.signals.write().await;
        signals.push(signal);
    }

    pub async fn get_signals(&self) -> Vec<TrainingSignal> {
        let signals = self.signals.read().await;
        signals.clone()
    }

    pub async fn clear(&self) {
        let mut signals = self.signals.write().await;
        signals.clear();
    }

    pub async fn export_and_clear(&self) -> Vec<TrainingSignal> {
        let mut signals = self.signals.write().await;
        let exported = signals.clone();
        signals.clear();
        exported
    }

    /// Start background export task
    pub async fn start_background_export<F>(&self, callback: F)
    where
        F: Fn(Vec<TrainingSignal>) + Send + Sync + 'static,
    {
        let signals = Arc::clone(&self.signals);
        let interval_secs = self.export_interval_secs;

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            
            loop {
                ticker.tick().await;
                
                let to_export = {
                    let mut s = signals.write().await;
                    let exported = s.clone();
                    s.clear();
                    exported
                };

                if !to_export.is_empty() {
                    callback(to_export);
                }
            }
        });
    }
}

impl Default for SignalExporter {
    fn default() -> Self {
        Self::new(60) // Default 60 second export interval
    }
}

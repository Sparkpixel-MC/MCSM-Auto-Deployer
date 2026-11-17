//! In-process log fan-out for the SSE console.

use parking_lot::RwLock;
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// Bounded history plus a broadcast channel. The history is replayed when an
/// SSE client connects, then the live channel takes over.
pub struct LogBus {
    sender: broadcast::Sender<LogEntry>,
    history: RwLock<VecDeque<LogEntry>>,
    capacity: usize,
    next_seq: AtomicU64,
}

impl LogBus {
    pub fn new(capacity: usize) -> Arc<Self> {
        let capacity = capacity.clamp(50, 5_000);
        let (sender, _) = broadcast::channel(1_024);
        Arc::new(Self {
            sender,
            history: RwLock::new(VecDeque::with_capacity(capacity)),
            capacity,
            next_seq: AtomicU64::new(1),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.sender.subscribe()
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.history.read().iter().cloned().collect()
    }

    pub fn push(
        &self,
        level: &str,
        target: &str,
        message: impl Into<String>,
        task_id: Option<String>,
    ) -> LogEntry {
        let entry = LogEntry {
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            timestamp_ms: now_ms(),
            level: level.to_string(),
            target: target.to_string(),
            message: message.into(),
            task_id,
        };
        {
            let mut history = self.history.write();
            if history.len() >= self.capacity {
                history.pop_front();
            }
            history.push_back(entry.clone());
        }
        // A missing receiver is perfectly fine: the dashboard is optional.
        let _ = self.sender.send(entry.clone());
        entry
    }
}

/// tracing layer that mirrors every application event into the SSE log bus.
pub struct LogLayer {
    bus: Arc<LogBus>,
}

impl LogLayer {
    pub fn new(bus: Arc<LogBus>) -> Self {
        Self { bus }
    }
}

impl<S> Layer<S> for LogLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let message = visitor
            .message
            .unwrap_or_else(|| metadata.name().to_string());
        self.bus.push(
            metadata.level().as_str(),
            metadata.target(),
            message,
            visitor.task_id,
        );
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    task_id: Option<String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "task_id" => self.task_id = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let rendered = format!("{value:?}");
        match field.name() {
            "message" => self.message = Some(rendered),
            "task_id" => {
                self.task_id = Some(rendered.trim_matches('"').to_string());
            }
            _ => {}
        }
    }
}

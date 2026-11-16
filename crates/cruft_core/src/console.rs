//! A bounded, thread-safe tracing sink. Install with LogPlugin::custom_layer.
use bevy::{
    log::{
        BoxedLayer,
        tracing::{self, field::Visit},
        tracing_subscriber::{Layer, layer::Context},
    },
    prelude::*,
};
use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex},
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

struct Buffer {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    dropped: u64,
}

#[derive(Resource, Clone)]
pub struct ConsoleLog(Arc<Mutex<Buffer>>);

impl Default for ConsoleLog {
    fn default() -> Self {
        Self::new(2_000)
    }
}

impl ConsoleLog {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(Buffer {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            dropped: 0,
        })))
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .cloned()
            .collect()
    }

    pub fn dropped(&self) -> u64 {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).dropped
    }

    pub fn clear(&self) {
        let mut buffer = self.0.lock().unwrap_or_else(|e| e.into_inner());
        buffer.entries.clear();
        buffer.dropped = 0;
    }

    fn push(&self, entry: LogEntry) {
        let mut buffer = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if buffer.entries.len() == buffer.capacity {
            buffer.entries.pop_front();
            buffer.dropped += 1;
        }
        buffer.entries.push_back(entry);
    }
}

pub struct ConsoleLayer(pub ConsoleLog);

impl<S: tracing::Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.push(LogEntry {
            timestamp: SystemTime::now(),
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: fields.0,
        });
    }
}

struct Fields(String);
impl Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        use fmt::Write;
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
        // Bound each entry as well as the entry count, without splitting UTF-8.
        if self.0.len() > 16_384 {
            let mut end = 16_384;
            while !self.0.is_char_boundary(end) {
                end -= 1;
            }
            self.0.truncate(end);
        }
    }
}

/// Pass this function to `DefaultPlugins.set(LogPlugin { custom_layer, ..default() })`.
pub fn capture_layer(app: &mut App) -> Option<BoxedLayer> {
    app.init_resource::<ConsoleLog>();
    Some(Box::new(ConsoleLayer(
        app.world().resource::<ConsoleLog>().clone(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::tracing_subscriber::{Registry, prelude::*};

    #[test]
    fn captures_fields_across_threads_and_evicts_oldest() {
        let logs = ConsoleLog::new(2);
        let dispatch = tracing::Dispatch::new(Registry::default().with(ConsoleLayer(logs.clone())));
        tracing::dispatcher::with_default(&dispatch, || tracing::info!("old"));
        std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                tracing::warn!(target: "game", count = 7, "new");
                tracing::error!("last");
            })
        })
        .join()
        .unwrap();
        let entries = logs.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].target, "game");
        assert!(entries[0].message.contains("count=7"));
        assert_eq!(entries[1].level, tracing::Level::ERROR);
        assert_eq!(logs.dropped(), 1);
        logs.clear();
        assert!(logs.entries().is_empty());
    }
}

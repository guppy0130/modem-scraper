use log::Level;
use serde::{self, Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
struct LokiStream {
    /// k/v label pairs
    stream: HashMap<String, String>,
    /// log entries (timestamp, log line)
    // suspect this may need a custom deserializer
    values: Vec<(String, String)>,
}

/// https://grafana.com/docs/loki/latest/api/#push-log-entries-to-loki
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct LokiStreams {
    streams: Vec<LokiStream>,
}

pub fn construct_loki_streams(
    labels: HashMap<String, String>,
    logs: Vec<(Level, u128, String)>,
) -> LokiStreams {
    let mut bucket_logs: HashMap<Level, Vec<(String, String)>> = HashMap::new();

    for log_entry in logs {
        match bucket_logs.get_mut(&log_entry.0) {
            Some(current_logs) => current_logs.push((log_entry.1.to_string(), log_entry.2)),
            None => {
                bucket_logs.insert(log_entry.0, vec![(log_entry.1.to_string(), log_entry.2)]);
            }
        }
    }

    let streams: Vec<LokiStream> = bucket_logs
        .iter()
        .map(|(key, value)| {
            let log_level_str = match key {
                Level::Trace => "trace",
                Level::Debug => "debug",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Error => "error",
            };
            let mut local_labels = labels.clone();
            local_labels.insert("level".to_owned(), log_level_str.to_owned());
            LokiStream {
                stream: local_labels,
                values: value.to_owned(),
            }
        })
        .collect();

    LokiStreams { streams }
}

use log::Level;
use serde::{self, Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;

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

/// provides for O(1) lookup and removal. Backed by a HashSet and a VecDeque. Guaranteed to not
/// exceed capacity.
#[derive(Debug, Clone)]
pub struct FixedSizeSortedHashSet<T> {
    hash_set: HashSet<T>,
    deque: VecDeque<T>,
    capacity: usize,
}

impl<T> FixedSizeSortedHashSet<T>
where
    T: Eq + Hash + Clone + Debug,
{
    pub fn contains(&self, value: &T) -> bool {
        self.hash_set.contains(value)
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if let Some(i) = self.deque.pop_front() {
            if !self.hash_set.remove(&i) {
                // the hash set and the deque are out of sync (deque contained something that the
                // hash map didn't have). This might be an issue, might not (e.g., if you only
                // care that the element is gone from both, then this doesn't matter)
                panic!("Removed {:?} from deque but didn't find it in hashmap", i)
            }
            Some(i)
        } else {
            None
        }
    }

    pub fn with_capacity(capacity: usize) -> FixedSizeSortedHashSet<T> {
        FixedSizeSortedHashSet {
            hash_set: HashSet::with_capacity(capacity),
            deque: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// if we're at capacity, you'll have to pop_front() and return that to the caller
    pub fn insert(&mut self, value: T) -> Option<T> {
        let mut popped_value = None;
        if self.deque.len() + 1 >= self.capacity {
            popped_value = self.pop_front();
        }
        // suspicious; this might double memory usage because of the clone. I don't want to deal
        // with pointers here, though.
        self.deque.push_back(value.clone());
        self.hash_set.insert(value);
        popped_value
    }
}

pub fn construct_loki_streams(
    labels: HashMap<String, String>,
    logs: Vec<Option<(Level, u128, String)>>,
) -> LokiStreams {
    let mut bucket_logs: HashMap<Level, Vec<(String, String)>> = HashMap::new();

    for entry in logs.into_iter().flatten() {
        match bucket_logs.get_mut(&entry.0) {
            Some(current_logs) => current_logs.push((entry.1.to_string(), entry.2)),
            None => {
                bucket_logs.insert(entry.0, vec![(entry.1.to_string(), entry.2)]);
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

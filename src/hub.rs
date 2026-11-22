//! Fan-out of parsed traffic entries to SSE subscribers, with a bounded
//! history so a freshly opened (or reloaded) console shows recent traffic.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use taphttp_types::TrafficEntry;
use tokio::sync::broadcast;

/// One entry, pre-serialized for SSE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// SSE event id. `None` if the entry id is not safe to put on an
    /// SSE `id:` line (it came from a log file, so it is untrusted).
    pub id: Option<String>,
    pub data: String,
}

impl Published {
    pub fn from_entry(entry: &TrafficEntry) -> Result<Self, serde_json::Error> {
        let id = Some(entry.id.clone())
            .filter(|id| !id.is_empty() && !id.chars().any(|c| matches!(c, '\n' | '\r' | '\0')));
        Ok(Self {
            id,
            data: serde_json::to_string(entry)?,
        })
    }
}

pub struct Hub {
    /// Hosts seen in everything published (#39).
    pub hosts: crate::hosts::Hosts,
    history: Mutex<VecDeque<Arc<Published>>>,
    capacity: usize,
    tx: broadcast::Sender<Arc<Published>>,
}

impl Hub {
    pub fn new(history_capacity: usize) -> Self {
        // Live channel buffer: a subscriber that falls this far behind
        // skips events rather than stalling the tailer.
        let (tx, _) = broadcast::channel(1024);
        Self {
            hosts: crate::hosts::Hosts::default(),
            history: Mutex::new(VecDeque::with_capacity(history_capacity.min(4096))),
            capacity: history_capacity,
            tx,
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Arc<Published>>> {
        // A panic while holding this lock cannot leave the deque in a
        // broken state, so recover from poisoning instead of propagating it.
        self.history.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn publish(&self, item: Published) {
        let item = Arc::new(item);
        let mut history = self.lock();
        if self.capacity > 0 {
            if history.len() == self.capacity {
                history.pop_front();
            }
            history.push_back(item.clone());
        }
        // Sent under the lock so `subscribe` sees each item exactly once:
        // either in the snapshot or on the channel, never both or neither.
        // An error only means there are no subscribers right now.
        let _ = self.tx.send(item);
    }

    /// Snapshot of recent history plus a receiver for everything after it.
    pub fn subscribe(&self) -> (Vec<Arc<Published>>, broadcast::Receiver<Arc<Published>>) {
        let history = self.lock();
        let rx = self.tx.subscribe();
        (history.iter().cloned().collect(), rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u32) -> Published {
        Published {
            id: Some(n.to_string()),
            data: format!("{{\"n\":{n}}}"),
        }
    }

    fn ids(items: &[Arc<Published>]) -> Vec<String> {
        items.iter().filter_map(|i| i.id.clone()).collect()
    }

    #[tokio::test]
    async fn subscriber_receives_items_published_after_subscribing() {
        let hub = Hub::new(10);
        let (history, mut rx) = hub.subscribe();
        assert!(history.is_empty());
        hub.publish(p(1));
        assert_eq!(*rx.recv().await.unwrap(), p(1));
    }

    #[test]
    fn late_subscriber_gets_history_snapshot() {
        let hub = Hub::new(10);
        hub.publish(p(1));
        hub.publish(p(2));
        let (history, _rx) = hub.subscribe();
        assert_eq!(ids(&history), vec!["1", "2"]);
    }

    #[test]
    fn history_is_bounded_and_keeps_newest() {
        let hub = Hub::new(3);
        for n in 1..=5 {
            hub.publish(p(n));
        }
        let (history, _rx) = hub.subscribe();
        assert_eq!(ids(&history), vec!["3", "4", "5"]);
    }

    #[test]
    fn zero_capacity_keeps_no_history() {
        let hub = Hub::new(0);
        hub.publish(p(1));
        assert!(hub.subscribe().0.is_empty());
    }

    #[test]
    fn publish_without_subscribers_does_not_fail() {
        let hub = Hub::new(1);
        hub.publish(p(1));
        hub.publish(p(2));
    }

    #[test]
    fn unsafe_entry_ids_are_not_used_as_sse_ids() {
        let mut e = serde_json::from_str::<TrafficEntry>(
            r#"{"id":"ok-id","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/"}"#,
        )
        .unwrap();
        assert_eq!(
            Published::from_entry(&e).unwrap().id.as_deref(),
            Some("ok-id")
        );
        for bad in ["a\nb", "a\rb", "a\0b", ""] {
            e.id = bad.to_string();
            assert_eq!(Published::from_entry(&e).unwrap().id, None, "{bad:?}");
        }
    }

    #[test]
    fn published_data_is_single_line_json_of_the_entry() {
        let e = serde_json::from_str::<TrafficEntry>(
            r#"{"id":"x","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/","res_body":"line1\nline2"}"#,
        )
        .unwrap();
        let data = Published::from_entry(&e).unwrap().data;
        assert!(!data.contains('\n'));
        assert_eq!(serde_json::from_str::<TrafficEntry>(&data).unwrap(), e);
    }
}

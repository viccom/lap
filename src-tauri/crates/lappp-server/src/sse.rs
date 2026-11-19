//! SSE event fan-out (plan §3.4): the `EventSink` implementation lapd
//! installs at boot, plus `GET /api/events`.
//!
//! Contract pinned by the phase-1 `http.js` transport and §3.4:
//! * default-message frames whose `data` is `{"event","id","payload"}` —
//!   event names and payloads are byte-identical to the Tauri emissions;
//! * `Last-Event-ID` resume against a 30-second ring buffer;
//! * a 15-second heartbeat;
//! * progress-class events coalesce at ≥100 ms to keep the network calm.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event as SseFrame, KeepAlive, Sse};
use futures::stream::Stream;
use lappp_core::t_events::EventSink;

const REPLAY_WINDOW: Duration = Duration::from_secs(30);
const HEARTBEAT: Duration = Duration::from_secs(15);
/// Progress events inside this window replace the pending one instead of
/// being queued (§3.4: "按 ≥100ms 合并").
const COALESCE_WINDOW: Duration = Duration::from_millis(100);

/// Progress-class event names that get coalesced; everything else passes
/// through untouched. Names match `t_events::names` verbatim (the codebase
/// mixes kebab/snake — that is historical, §3.4 keeps them byte-identical).
const COALESCED: &[&str] = &[
    "dedup-scan-progress",
    "image_search_model_download_progress",
    "face_index_progress",
    "cluster_progress",
    "similar-scan-progress",
    "index_progress",
    "vl-analysis-progress",
    "import-organize-progress",
];

#[derive(Clone)]
pub(crate) struct Stamped {
    pub(crate) id: u64,
    pub(crate) event: Arc<str>,
    pub(crate) payload: Arc<serde_json::Value>,
    pub(crate) at: tokio::time::Instant,
}

/// Shared fan-out state: every live connection reads the ring buffer and
/// waits on the condition variable; the sink is the only writer.
pub struct EventBus {
    seq: AtomicU64,
    buffer: Mutex<VecDeque<Stamped>>,
    /// The newest coalescable event, replaced in place within the window.
    pending: Mutex<Option<Stamped>>,
    signal: tokio::sync::Notify,
}

impl EventBus {
    pub fn new() -> Arc<EventBus> {
        Arc::new(EventBus {
            seq: AtomicU64::new(0),
            buffer: Mutex::new(VecDeque::new()),
            pending: Mutex::new(None),
            signal: tokio::sync::Notify::new(),
        })
    }

    fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn publish(&self, stamped: Stamped) {
        {
            let mut buffer = self.buffer.lock().unwrap();
            buffer.push_back(stamped.clone());
            let cutoff = tokio::time::Instant::now() - REPLAY_WINDOW;
            while buffer.front().is_some_and(|front| front.at < cutoff) {
                buffer.pop_front();
            }
        }
        self.signal.notify_waiters();
    }

    /// Coalescing replaces the buffered event's payload in place so clients
    /// always converge on the newest progress state under the original id.
    fn refresh(&self, id: u64, payload: Arc<serde_json::Value>) {
        let mut buffer = self.buffer.lock().unwrap();
        if let Some(stamped) = buffer.iter_mut().find(|stamped| stamped.id == id) {
            stamped.payload = payload;
            stamped.at = tokio::time::Instant::now();
        }
        drop(buffer);
        self.signal.notify_waiters();
    }

    /// Events newer than `last_seen` (0 = everything retained), oldest first.
    pub fn since(&self, last_seen: u64) -> Vec<Stamped> {
        self.buffer
            .lock()
            .unwrap()
            .iter()
            .filter(|stamped| stamped.id > last_seen)
            .cloned()
            .collect()
    }

    /// Block until an event newer than `last_seen` exists or `since` grows.
    pub async fn wait_for(&self, last_seen: u64) {
        if !self.since(last_seen).is_empty() {
            return;
        }
        self.signal.notified().await;
    }
}

/// The `EventSink` lapd installs; core emissions land here.
pub struct SseSink {
    bus: Arc<EventBus>,
}

impl SseSink {
    pub fn new(bus: Arc<EventBus>) -> Arc<SseSink> {
        Arc::new(SseSink { bus })
    }
}

impl EventSink for SseSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        let event: Arc<str> = Arc::from(name);
        let now = tokio::time::Instant::now();
        let payload = Arc::new(payload);
        if COALESCED.contains(&name) {
            // Same progress stream inside the window: keep the id, take the
            // newest payload, refresh the eviction clock.
            let coalesce_id = {
                let pending = self.bus.pending.lock().unwrap();
                match pending.as_ref() {
                    Some(existing)
                        if *existing.event == *event
                            && now.duration_since(existing.at) < COALESCE_WINDOW =>
                    {
                        Some(existing.id)
                    }
                    _ => None,
                }
            };
            if let Some(id) = coalesce_id {
                let mut pending = self.bus.pending.lock().unwrap();
                if let Some(existing) = pending.as_mut() {
                    existing.payload = payload.clone();
                    existing.at = now;
                }
                drop(pending);
                self.bus.refresh(id, payload);
                return;
            }
            let stamped = Stamped {
                id: self.bus.next_id(),
                event,
                payload,
                at: now,
            };
            *self.bus.pending.lock().unwrap() = Some(stamped.clone());
            self.bus.publish(stamped);
        } else {
            let stamped = Stamped {
                id: self.bus.next_id(),
                event,
                payload,
                at: now,
            };
            self.bus.publish(stamped);
        }
    }
}

/// GET /api/events — the per-connection stream.
pub async fn events_handler(
    State(bus): State<Arc<EventBus>>,
    headers: axum::http::HeaderMap,
) -> Sse<impl Stream<Item = Result<SseFrame, std::convert::Infallible>>> {
    // EventSource sends the last seen id on reconnect.
    let last_seen: u64 = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    // `since()` returns oldest→newest, but the unfold below consumes the
    // replay with `pop()` — from the BACK. Reverse first so the replay goes
    // out oldest first and `last_seen` advances monotonically; otherwise a
    // reconnecting client receives the newest frames first and, once the
    // reversed buffer drains, the same events again from the live branch.
    let mut replay = bus.since(last_seen);
    replay.reverse();
    let stream = futures::stream::unfold(
        (bus, last_seen, replay, None::<()>),
        |(bus, mut last_seen, mut replay, _)| async move {
            loop {
                if let Some(stamped) = replay.pop() {
                    last_seen = stamped.id;
                    // The `id:` field is what makes the browser resend
                    // `Last-Event-ID` on reconnect; without it the EventSource
                    // cannot resume and the server replays the whole retained
                    // buffer. The JSON envelope also carries `id` (phase-1
                    // contract), so both stay in sync.
                    let frame = SseFrame::default()
                        .event("message")
                        .id(stamped.id.to_string())
                        .data(render_frame(&stamped));
                    return Some((Ok(frame), (bus, last_seen, replay, None)));
                }
                bus.wait_for(last_seen).await;
                replay = bus.since(last_seen);
                replay.reverse();
            }
        },
    );

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(HEARTBEAT)
            .text("hb"),
    )
}

fn render_frame(stamped: &Stamped) -> String {
    serde_json::json!({
        "event": &*stamped.event,
        "id": stamped.id.to_string(),
        "payload": &*stamped.payload,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lappp_core::t_events::names;

    fn drain(bus: &Arc<EventBus>, last_seen: u64) -> Vec<(u64, String, serde_json::Value)> {
        bus.since(last_seen)
            .into_iter()
            .map(|s| (s.id, s.event.to_string(), (*s.payload).clone()))
            .collect()
    }

    #[tokio::test]
    async fn non_progress_events_pass_through_one_by_one() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::THUMBNAIL_READY, serde_json::json!({ "fileId": 7 }));
        sink.emit(names::TAGS_CHANGED, serde_json::json!({}));
        let events = drain(&bus, 0);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1, "thumbnail_ready");
        assert_eq!(events[0].2["fileId"], 7);
    }

    #[tokio::test]
    async fn progress_events_within_window_coalesce_keeping_the_first_id() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::INDEX_PROGRESS, serde_json::json!({ "current": 1 }));
        sink.emit(names::INDEX_PROGRESS, serde_json::json!({ "current": 2 }));
        sink.emit(names::INDEX_PROGRESS, serde_json::json!({ "current": 3 }));
        let events = drain(&bus, 0);
        // All three arrive inside the coalesce window in a test run, so the
        // ring buffer must hold exactly one event with the first id and the
        // newest payload.
        assert_eq!(events.len(), 1, "got {events:?}");
        assert_eq!(events[0].2["current"], 3);
    }

    #[tokio::test]
    async fn ids_are_monotonic_across_kinds() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::INDEX_PROGRESS, serde_json::json!({}));
        sink.emit(names::THUMBNAIL_READY, serde_json::json!({}));
        sink.emit(names::INDEX_PROGRESS, serde_json::json!({}));
        let events = drain(&bus, 0);
        let ids: Vec<u64> = events.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids: {ids:?}");
    }

    #[tokio::test]
    async fn replay_resumes_from_last_event_id() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        for n in 0..5 {
            sink.emit(names::VL_TASKS_CHANGED, serde_json::json!({ "n": n }));
        }
        let all = drain(&bus, 0);
        // Replay order is oldest→newest: a client reapplying frames in
        // arrival order must never see the newest event first (regression
        // guard for the reversed-replay bug pinned over live HTTP in
        // http_integration.rs).
        let ids: Vec<u64> = all.iter().map(|(id, _, _)| *id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "replay must be oldest first: {ids:?}");
        let last_id = all.last().unwrap().0;
        // A client that saw everything gets nothing; one two behind gets two.
        assert!(drain(&bus, last_id).is_empty());
        let resumed = drain(&bus, last_id - 2);
        assert_eq!(resumed.len(), 2);
        let resumed_ids: Vec<u64> = resumed.iter().map(|(id, _, _)| *id).collect();
        assert!(resumed_ids.windows(2).all(|w| w[0] < w[1]), "resume must be oldest first: {resumed_ids:?}");
    }

    #[tokio::test]
    async fn ring_buffer_drops_events_older_than_thirty_seconds() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::THUMBNAIL_READY, serde_json::json!({}));
        // Artificially age the stored event past the replay window.
        let cutoff = tokio::time::Instant::now() - REPLAY_WINDOW - Duration::from_millis(1);
        {
            let mut buffer = bus.buffer.lock().unwrap();
            for stamped in buffer.iter_mut() {
                stamped.at = cutoff;
            }
        }
        sink.emit(names::TAGS_CHANGED, serde_json::json!({}));
        let events = drain(&bus, 0);
        assert_eq!(events.len(), 1, "aged event must be evicted, got {events:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn coalesced_event_flushes_after_the_window() {
        // With paused time, a later emission after the window must start a
        // new coalesce slot instead of replacing forever.
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::DEDUP_SCAN_PROGRESS, serde_json::json!({ "n": 1 }));
        tokio::time::advance(COALESCE_WINDOW + Duration::from_millis(1)).await;
        sink.emit(names::DEDUP_SCAN_PROGRESS, serde_json::json!({ "n": 2 }));
        let events = drain(&bus, 0);
        assert_eq!(events.len(), 2, "post-window emission must not coalesce");
        assert_eq!(events[1].2["n"], 2);
    }

    #[test]
    fn frame_envelope_matches_the_http_transport_contract() {
        let bus = EventBus::new();
        let sink = SseSink::new(bus.clone());
        sink.emit(names::THUMBNAIL_READY, serde_json::json!({ "pct": 50 }));
        let stamped = bus.since(0).remove(0);
        let rendered: serde_json::Value = serde_json::from_str(&render_frame(&stamped)).unwrap();
        assert_eq!(rendered["event"], "thumbnail_ready");
        assert_eq!(rendered["id"], stamped.id.to_string());
        assert_eq!(rendered["payload"]["pct"], 50);
    }
}

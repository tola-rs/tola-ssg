use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::EVENT_DETAIL_CAPACITY;
use super::boundary::ClassifiedPath;
use super::producer::ProducerSet;

#[derive(Debug)]
pub(super) enum WatchMessage {
    Event {
        event: notify::Event,
        revision: u64,
        classified_paths: Vec<(PathBuf, ClassifiedPath)>,
        producers: ProducerSet,
        invalidated: ProducerSet,
    },
    Error {
        producers: ProducerSet,
        error: Arc<notify::Error>,
    },
}

impl WatchMessage {
    fn producers(&self) -> ProducerSet {
        match self {
            Self::Event { producers, .. } | Self::Error { producers, .. } => *producers,
        }
    }
}

pub(super) trait WatchMessageSink {
    /// Enqueue a message; return producers when overflow is coalesced.
    fn send_message(&self, msg: WatchMessage) -> Option<ProducerSet>;

    fn mark_overflow_ready(&self, _rev: u64) {}
}

#[derive(Debug)]
struct EventOverflow {
    producers: ProducerSet,
    revision: u64,
    failure: Option<Arc<notify::Error>>,
}

impl EventOverflow {
    fn include(&mut self, msg: &WatchMessage) {
        match msg {
            WatchMessage::Event {
                producers,
                revision,
                ..
            } => {
                self.producers.insert(*producers);
                self.revision = self.revision.max(*revision);
            }
            WatchMessage::Error { producers, error } => {
                self.producers.insert(*producers);
                if self.failure.is_none() {
                    self.failure = Some(Arc::clone(error));
                }
            }
        }
    }

    fn into_message(self) -> WatchMessage {
        if let Some(err) = self.failure {
            WatchMessage::Error {
                producers: self.producers,
                error: err,
            }
        } else {
            WatchMessage::Event {
                revision: self.revision,
                event: notify::Event::new(notify::EventKind::Other)
                    .set_flag(notify::event::Flag::Rescan),
                classified_paths: Vec::new(),
                producers: self.producers,
                invalidated: self.producers,
            }
        }
    }
}

#[derive(Default)]
struct WatchEventQueueState {
    messages: VecDeque<WatchMessage>,
    queued_paths: usize,
    overflow: Option<EventOverflow>,
    overflow_ready: bool,
    closed: bool,
}

#[derive(Clone)]
pub(super) struct WatchEventQueue {
    state: Arc<Mutex<WatchEventQueueState>>,
    wake: Arc<tokio::sync::Notify>,
}

pub(super) struct WatchEventReceiver {
    queue: WatchEventQueue,
}

impl WatchEventQueue {
    fn lock(&self) -> MutexGuard<'_, WatchEventQueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn channel() -> (Self, WatchEventReceiver) {
        let queue = Self {
            state: Arc::new(Mutex::new(WatchEventQueueState::default())),
            wake: Arc::new(tokio::sync::Notify::new()),
        };
        (
            queue.clone(),
            WatchEventReceiver {
                queue: queue.clone(),
            },
        )
    }

    pub(super) fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        if state.overflow.is_some() {
            state.overflow_ready = true;
        }
        drop(state);
        self.wake.notify_waiters();
    }

    fn mark_overflow_ready_inner(&self, rev: u64) {
        let mut state = self.lock();
        if let Some(overflow) = state.overflow.as_mut() {
            overflow.revision = rev;
            state.overflow_ready = true;
        }
        drop(state);
        self.wake.notify_waiters();
    }
}

impl WatchMessageSink for WatchEventQueue {
    fn send_message(&self, msg: WatchMessage) -> Option<ProducerSet> {
        let mut state = self.lock();
        if state.closed {
            return None;
        }
        if let Some(overflow) = state.overflow.as_mut() {
            let producers = msg.producers();
            overflow.include(&msg);
            drop(state);
            self.wake.notify_one();
            return Some(producers);
        }
        let path_count = match &msg {
            WatchMessage::Event { event, .. } => event.paths.len(),
            WatchMessage::Error { .. } => 0,
        };
        if state.messages.len() < EVENT_DETAIL_CAPACITY
            && state.queued_paths + path_count <= EVENT_DETAIL_CAPACITY
        {
            state.queued_paths += path_count;
            state.messages.push_back(msg);
            drop(state);
            self.wake.notify_one();
            return None;
        }

        tracing::debug!(
            target: "tola::watch",
            queued_events = state.messages.len(),
            queued_paths = state.queued_paths,
            incoming_paths = path_count,
            "filesystem event queue overflowed"
        );
        let mut overflow = EventOverflow {
            producers: ProducerSet::default(),
            revision: 0,
            failure: None,
        };
        for pending in state.messages.drain(..) {
            overflow.include(&pending);
        }
        state.queued_paths = 0;
        overflow.include(&msg);
        let producers = overflow.producers;
        state.overflow = Some(overflow);
        state.overflow_ready = false;
        drop(state);
        self.wake.notify_one();
        Some(producers)
    }

    fn mark_overflow_ready(&self, rev: u64) {
        self.mark_overflow_ready_inner(rev);
    }
}

impl WatchEventReceiver {
    pub(super) async fn recv(&mut self) -> Option<WatchMessage> {
        loop {
            let notified = {
                let mut state = self.queue.lock();
                if state.overflow_ready {
                    state.overflow_ready = false;
                    return state.overflow.take().map(EventOverflow::into_message);
                }
                if let Some(msg) = state.messages.pop_front() {
                    if let WatchMessage::Event { event, .. } = &msg {
                        state.queued_paths -= event.paths.len();
                    }
                    return Some(msg);
                }
                if state.closed {
                    return None;
                }
                self.queue.wake.notified()
            };
            notified.await;
        }
    }
}

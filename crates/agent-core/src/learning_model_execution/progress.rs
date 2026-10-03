//! Ephemeral, bounded notifications. Channel EOF without a terminal event means discard.
use personal_ai_domain::UserId;
use personal_ai_llm::stream::TextDeltaSink;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub const PROGRESS_CAPACITY: usize = 16;
const MAX_BYTES: usize = 24 * 1024;
const MAX_DELTAS: usize = 16_384;

pub enum ReviewProgressKind {
    Verifying,
    Sending,
    Delta(String),
    Persisting,
    Finished(String),
    NotClaimed,
    StorageFailure,
}
/// Owner/request bindings come from the executor, never from provider text.
/// Not serializable: this is not a public HTTP protocol or a durable event log.
pub struct ReviewProgressEvent {
    pub owner: UserId,
    pub request_id: String,
    pub sequence: u64,
    pub kind: ReviewProgressKind,
}
struct State {
    sender: Option<mpsc::Sender<ReviewProgressEvent>>,
    sequence: u64,
    bytes: usize,
    deltas: usize,
}
pub(super) struct Publisher {
    owner: UserId,
    request_id: String,
    state: Mutex<State>,
}
impl Publisher {
    pub(super) fn channel(
        owner: &UserId,
        request: &str,
    ) -> (Self, mpsc::Receiver<ReviewProgressEvent>) {
        let (sender, receiver) = mpsc::channel(PROGRESS_CAPACITY);
        (
            Self {
                owner: owner.clone(),
                request_id: request.into(),
                state: Mutex::new(State {
                    sender: Some(sender),
                    sequence: 0,
                    bytes: 0,
                    deltas: 0,
                }),
            },
            receiver,
        )
    }
    pub(super) fn emit(&self, kind: ReviewProgressKind) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        self.send(&mut state, kind);
    }
    fn send(&self, state: &mut State, kind: ReviewProgressKind) {
        let Some(sender) = state.sender.as_ref() else {
            return;
        };
        let terminal = matches!(
            kind,
            ReviewProgressKind::Finished(_)
                | ReviewProgressKind::NotClaimed
                | ReviewProgressKind::StorageFailure
        );
        let event = ReviewProgressEvent {
            owner: self.owner.clone(),
            request_id: self.request_id.clone(),
            sequence: state.sequence,
            kind,
        };
        if sender.try_send(event).is_err() || terminal {
            // Full or disconnected: permanently close this notification channel.
            // The caller keeps the original execution and durable send marker.
            state.sender = None;
        }
        state.sequence += 1;
    }
}
impl TextDeltaSink for Publisher {
    fn delta(&self, text: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.sender.as_ref().is_none_or(mpsc::Sender::is_closed) {
            state.sender = None;
            return;
        }
        if text.len() > MAX_BYTES - state.bytes || state.deltas >= MAX_DELTAS {
            state.sender = None;
            return;
        }
        state.bytes += text.len();
        state.deltas += 1;
        self.send(&mut state, ReviewProgressKind::Delta(text.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn slow_or_disconnected_consumers_cannot_block_or_resume_notifications() {
        let (publisher, mut rx) = Publisher::channel(&UserId::new("owner"), "request");
        for _ in 0..=PROGRESS_CAPACITY {
            publisher.delta("private");
        }
        publisher.emit(ReviewProgressKind::Finished("succeeded".into()));
        let mut count = 0;
        while let Some(event) = rx.recv().await {
            assert_eq!(event.owner.as_str(), "owner");
            assert_eq!(event.request_id, "request");
            assert_eq!(event.sequence, count);
            assert!(matches!(event.kind, ReviewProgressKind::Delta(_)));
            count += 1;
        }
        assert_eq!(count, PROGRESS_CAPACITY as u64);
        let (publisher, rx) = Publisher::channel(&UserId::new("owner"), "request");
        drop(rx);
        publisher.delta("discarded");
        assert!(publisher.state.lock().unwrap().sender.is_none());
    }
    #[tokio::test]
    async fn oversized_previews_close_without_terminal_success_and_terminal_is_unique() {
        let (publisher, mut rx) = Publisher::channel(&UserId::new("owner"), "request");
        publisher.delta(&"x".repeat(MAX_BYTES + 1));
        assert!(rx.recv().await.is_none());
        let (publisher, mut rx) = Publisher::channel(&UserId::new("owner"), "request");
        publisher.emit(ReviewProgressKind::Finished("unknown".into()));
        publisher.delta("late");
        assert!(matches!(
            rx.recv().await.unwrap().kind,
            ReviewProgressKind::Finished(_)
        ));
        assert!(rx.recv().await.is_none());
    }
}

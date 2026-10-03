//! Provider-independent text assembly. Deltas are provisional, never persisted results.

pub const MAX_TEXT_BYTES: usize = 128 * 1024;
pub const MAX_EVENTS: u64 = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamState {
    Receiving,
    Completed,
    Cancelled,
    Interrupted,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamError {
    Closed,
    InvalidSequence,
    LimitExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEvent<'a> {
    Delta(&'a str),
    Completed,
}

/// One invocation only. Sequence numbers start at zero and must be contiguous.
/// The owner must discard this object on account changes and must not retry inference.
/// Completion marks the event sequence; transport EOF, domain validation, and
/// durable persistence remain the responsibility of the caller.
pub struct TextAssembly {
    state: StreamState,
    next_sequence: u64,
    text: String,
    delivered: bool,
}
impl Default for TextAssembly {
    fn default() -> Self {
        Self {
            state: StreamState::Receiving,
            next_sequence: 0,
            text: String::new(),
            delivered: false,
        }
    }
}
impl TextAssembly {
    #[must_use]
    pub fn state(&self) -> StreamState {
        self.state
    }

    /// # Errors
    /// Rejects noncontiguous sequences, exceeded limits, and events after closure.
    /// Rejection discards buffered text; callers must not resume the invocation.
    pub fn apply(&mut self, sequence: u64, event: TextEvent<'_>) -> Result<(), StreamError> {
        if self.state != StreamState::Receiving {
            if self.state == StreamState::Completed {
                self.text.clear();
                self.state = StreamState::Rejected;
            }
            return Err(StreamError::Closed);
        }
        if sequence != self.next_sequence {
            self.reject();
            return Err(StreamError::InvalidSequence);
        }
        if self.next_sequence >= MAX_EVENTS {
            self.reject();
            return Err(StreamError::LimitExceeded);
        }
        self.next_sequence += 1;
        match event {
            TextEvent::Delta(delta) => {
                if delta.len() > MAX_TEXT_BYTES - self.text.len() {
                    self.reject();
                    return Err(StreamError::LimitExceeded);
                }
                self.text.push_str(delta);
            }
            TextEvent::Completed => self.state = StreamState::Completed,
        }
        Ok(())
    }

    /// Provisional display only; never treat this as a completed model response.
    #[must_use]
    pub fn partial_text(&self) -> Option<&str> {
        (self.state == StreamState::Receiving).then_some(self.text.as_str())
    }

    /// Consume completed text at most once. Empty completions are permitted here;
    /// domain validators decide whether the resulting output is usable.
    pub fn take_completed(&mut self) -> Option<String> {
        if self.state != StreamState::Completed || self.delivered {
            return None;
        }
        self.delivered = true;
        Some(std::mem::take(&mut self.text))
    }

    /// Cancels a receiving stream. Completed results require owner-level disposal.
    pub fn cancel(&mut self) {
        self.stop(StreamState::Cancelled);
    }
    /// Marks a receiving stream incomplete (EOF, timeout, or disconnected transport).
    pub fn interrupt(&mut self) {
        self.stop(StreamState::Interrupted);
    }
    fn reject(&mut self) {
        self.stop(StreamState::Rejected);
    }
    fn stop(&mut self, state: StreamState) {
        if self.state == StreamState::Receiving {
            self.text.clear();
            self.state = state;
        }
    }
}

#[cfg(test)]
mod tests;

/// Trusted, synchronous notification port. Implementations must return promptly,
/// must not block on consumers, and must never treat deltas as validated output.
pub trait TextDeltaSink: Send + Sync {
    fn delta(&self, text: &str);
}

pub struct IgnoreTextDeltas;
impl TextDeltaSink for IgnoreTextDeltas {
    fn delta(&self, _: &str) {}
}

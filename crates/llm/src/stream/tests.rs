use super::*;

#[test]
fn unicode_deltas_are_provisional_until_completion_and_consumed_once() {
    let mut stream = TextAssembly::default();
    stream.apply(0, TextEvent::Delta("你")).unwrap();
    stream.apply(1, TextEvent::Delta("好🌍")).unwrap();
    assert_eq!(stream.partial_text(), Some("你好🌍"));
    assert_eq!(stream.take_completed(), None);
    stream.apply(2, TextEvent::Completed).unwrap();
    assert_eq!(stream.partial_text(), None);
    assert_eq!(stream.take_completed().as_deref(), Some("你好🌍"));
    assert_eq!(stream.take_completed(), None);
}
#[test]
fn gaps_duplicates_and_terminal_conflicts_cannot_produce_results() {
    for sequence in [0, 2, u64::MAX] {
        let mut stream = TextAssembly::default();
        stream.apply(0, TextEvent::Delta("private")).unwrap();
        assert_eq!(
            stream.apply(sequence, TextEvent::Completed),
            Err(StreamError::InvalidSequence)
        );
        assert_eq!(stream.state(), StreamState::Rejected);
        assert_eq!(stream.partial_text(), None);
        assert_eq!(stream.take_completed(), None);
        assert_eq!(
            stream.apply(1, TextEvent::Completed),
            Err(StreamError::Closed)
        );
    }
    for late in [TextEvent::Completed, TextEvent::Delta("late")] {
        let mut stream = TextAssembly::default();
        stream.apply(0, TextEvent::Completed).unwrap();
        assert_eq!(stream.apply(1, late), Err(StreamError::Closed));
        assert_eq!(stream.take_completed(), None);
    }
}
#[test]
fn cancellation_and_disconnect_clear_partial_text_and_are_not_resumable() {
    for cancel in [true, false] {
        let mut stream = TextAssembly::default();
        stream.apply(0, TextEvent::Delta("private")).unwrap();
        if cancel {
            stream.cancel();
        } else {
            stream.interrupt();
        }
        let state = stream.state();
        assert_eq!(
            state,
            if cancel {
                StreamState::Cancelled
            } else {
                StreamState::Interrupted
            }
        );
        assert_eq!(stream.partial_text(), None);
        assert_eq!(stream.take_completed(), None);
        assert!(stream.apply(1, TextEvent::Completed).is_err());
        stream.cancel();
        stream.interrupt();
        assert_eq!(stream.state(), state);
    }
}
#[test]
fn byte_and_event_limits_include_empty_deltas_and_completion() {
    let mut stream = TextAssembly::default();
    stream
        .apply(0, TextEvent::Delta(&"x".repeat(MAX_TEXT_BYTES)))
        .unwrap();
    stream.apply(1, TextEvent::Completed).unwrap();
    assert_eq!(stream.take_completed().unwrap().len(), MAX_TEXT_BYTES);
    let mut stream = TextAssembly::default();
    assert_eq!(
        stream.apply(0, TextEvent::Delta(&"界".repeat(MAX_TEXT_BYTES / 3 + 1))),
        Err(StreamError::LimitExceeded)
    );
    assert_eq!(stream.take_completed(), None);
    let mut stream = TextAssembly::default();
    for sequence in 0..MAX_EVENTS {
        stream.apply(sequence, TextEvent::Delta("")).unwrap();
    }
    assert_eq!(
        stream.apply(MAX_EVENTS, TextEvent::Completed),
        Err(StreamError::LimitExceeded)
    );
    assert_eq!(stream.take_completed(), None);
}

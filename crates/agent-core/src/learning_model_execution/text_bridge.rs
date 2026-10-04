//! Best-effort forwarding; observer failure must never restart/cancel inference.
use super::progress::{ReviewProgressEvent, ReviewProgressKind};
use personal_ai_domain::UserId;
use personal_ai_storage::learning::review_text::{
    MAX_DELTA_BYTES, TextKind, TextPacket, TextPublisher,
};
use tokio::sync::mpsc::Receiver;

/// Forwards a single executor's bound sequence. No terminal packet means discard.
pub async fn relay_progress(
    mut receiver: Receiver<ReviewProgressEvent>,
    publisher: &mut dyn TextPublisher,
    owner: &UserId,
    request: &str,
) {
    let mut source_sequence = 0;
    let mut sequence = 0;
    while let Some(event) = receiver.recv().await {
        if event.owner != *owner || event.request_id != request || event.sequence != source_sequence
        {
            return;
        }
        source_sequence += 1;
        let detail = match event.kind {
            ReviewProgressKind::Delta(text) => {
                let mut remaining = text.as_str();
                while !remaining.is_empty() {
                    let mut end = remaining.len().min(MAX_DELTA_BYTES);
                    while !remaining.is_char_boundary(end) {
                        end -= 1;
                    }
                    let packet = TextPacket {
                        sequence,
                        detail: TextKind::Delta {
                            text: remaining[..end].into(),
                        },
                    };
                    if publisher.publish(packet).await.is_err() {
                        return;
                    }
                    sequence += 1;
                    remaining = &remaining[end..];
                }
                continue;
            }
            ReviewProgressKind::Persisting => TextKind::Clear,
            ReviewProgressKind::Finished(_)
            | ReviewProgressKind::NotClaimed
            | ReviewProgressKind::StorageFailure => TextKind::End,
            ReviewProgressKind::Verifying | ReviewProgressKind::Sending => continue,
        };
        let ended = matches!(detail, TextKind::End);
        if publisher
            .publish(TextPacket { sequence, detail })
            .await
            .is_err()
        {
            return;
        }
        sequence += 1;
        if ended {
            return;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use personal_ai_storage::{BoxFuture, StorageError, StorageResult};
    #[derive(Default)]
    struct Publisher {
        packets: Vec<TextPacket>,
        fail: bool,
    }
    impl TextPublisher for Publisher {
        fn publish(&mut self, packet: TextPacket) -> BoxFuture<'_, StorageResult<()>> {
            Box::pin(async move {
                if self.fail {
                    return Err(StorageError::Unavailable("fixture".into()));
                }
                self.packets.push(packet);
                Ok(())
            })
        }
    }
    #[tokio::test]
    async fn unicode_is_split_without_leaking_execution_metadata_or_business_success() {
        let owner = UserId::new("owner");
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        for (sequence, kind) in [
            ReviewProgressKind::Verifying,
            ReviewProgressKind::Sending,
            ReviewProgressKind::Delta("中".repeat(400)),
            ReviewProgressKind::Persisting,
            ReviewProgressKind::Finished("succeeded".into()),
        ]
        .into_iter()
        .enumerate()
        {
            sender
                .send(ReviewProgressEvent {
                    owner: owner.clone(),
                    request_id: "request".into(),
                    sequence: sequence as u64,
                    kind,
                })
                .await
                .unwrap();
        }
        drop(sender);
        let mut publisher = Publisher::default();
        relay_progress(receiver, &mut publisher, &owner, "request").await;
        let mut text = String::new();
        for (sequence, packet) in publisher.packets.iter().enumerate() {
            assert_eq!(packet.sequence, sequence as u64);
            if let TextKind::Delta { text: part } = &packet.detail {
                assert!(part.len() <= MAX_DELTA_BYTES);
                text.push_str(part);
            }
        }
        assert_eq!(text, "中".repeat(400));
        assert!(matches!(publisher.packets[3].detail, TextKind::Clear));
        assert!(matches!(publisher.packets[4].detail, TextKind::End));
    }
    #[tokio::test]
    async fn forwarding_failure_or_wrong_binding_closes_the_observer() {
        for wrong in [false, true] {
            let owner = UserId::new("owner");
            let (sender, receiver) = tokio::sync::mpsc::channel(2);
            sender
                .send(ReviewProgressEvent {
                    owner: owner.clone(),
                    request_id: if wrong { "foreign" } else { "request" }.into(),
                    sequence: 0,
                    kind: ReviewProgressKind::Delta("private".into()),
                })
                .await
                .unwrap();
            let mut publisher = Publisher {
                fail: !wrong,
                ..Publisher::default()
            };
            relay_progress(receiver, &mut publisher, &owner, "request").await;
            assert_eq!(publisher.packets.len(), 0);
            assert!(sender.is_closed());
        }
    }
}

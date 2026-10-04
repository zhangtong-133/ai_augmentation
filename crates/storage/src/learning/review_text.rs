//! Live, unvalidated text transport. No storage, replay, inference, or business success.
use crate::{BoxFuture, StorageError, StorageResult};
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};

pub const MAX_DELTA_BYTES: usize = 512;
pub const MAX_TEXT_BYTES: usize = 24 * 1024;
pub const MAX_PACKETS: u64 = 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextKind {
    Delta { text: String },
    Clear,
    End,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPacket {
    pub sequence: u64,
    pub detail: TextKind,
}
/// Each observer may join mid-stream; after its first packet no gaps are allowed.
#[derive(Default)]
pub struct TextWindow {
    next: Option<u64>,
    bytes: usize,
    ended: bool,
    cleared: bool,
}
impl TextWindow {
    /// # Errors
    /// Rejects gaps, repeats, oversized content and any text after clearing/ending.
    pub fn accept(&mut self, packet: &TextPacket) -> StorageResult<()> {
        if self.ended
            || packet.sequence >= MAX_PACKETS
            || self.next.is_some_and(|next| next != packet.sequence)
        {
            return Err(invalid());
        }
        match &packet.detail {
            TextKind::Delta { text } => {
                if self.cleared
                    || text.is_empty()
                    || text.len() > MAX_DELTA_BYTES
                    || text.len() > MAX_TEXT_BYTES - self.bytes
                {
                    return Err(invalid());
                }
                self.bytes += text.len();
            }
            TextKind::Clear => self.cleared = true,
            TextKind::End => self.ended = true,
        }
        self.next = Some(packet.sequence + 1);
        Ok(())
    }
}
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid temporary review text".into())
}
pub trait TextSubscription: Send {
    fn next(&mut self) -> BoxFuture<'_, StorageResult<Option<TextPacket>>>;
}
pub trait TextPublisher: Send {
    fn publish(&mut self, packet: TextPacket) -> BoxFuture<'_, StorageResult<()>>;
}
/// Trusted server-side transport; callers must authenticate and recheck sources.
pub trait ReviewTextBridge: Send + Sync {
    fn subscribe(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Box<dyn TextSubscription>>>;
    fn publisher(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Box<dyn TextPublisher>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn delta(sequence: u64, text: &str) -> TextPacket {
        TextPacket {
            sequence,
            detail: TextKind::Delta { text: text.into() },
        }
    }
    #[test]
    fn late_join_allows_only_a_contiguous_bounded_suffix() {
        let mut window = TextWindow::default();
        window.accept(&delta(19, "中文")).unwrap();
        assert!(window.accept(&delta(19, "repeat")).is_err());
        assert!(window.accept(&delta(21, "gap")).is_err());
        window
            .accept(&TextPacket {
                sequence: 20,
                detail: TextKind::Clear,
            })
            .unwrap();
        assert!(window.accept(&delta(21, "late")).is_err());
        window
            .accept(&TextPacket {
                sequence: 21,
                detail: TextKind::End,
            })
            .unwrap();
        assert!(window.accept(&delta(22, "late")).is_err());
    }
    #[test]
    fn packet_and_total_limits_reject_untrusted_input() {
        for packet in [
            delta(0, ""),
            delta(MAX_PACKETS, "x"),
            delta(0, &"中".repeat(171)),
        ] {
            assert!(TextWindow::default().accept(&packet).is_err());
        }
        let mut window = TextWindow::default();
        for sequence in 0..48 {
            window.accept(&delta(sequence, &"x".repeat(512))).unwrap();
        }
        assert!(window.accept(&delta(48, "x")).is_err());
        assert!(
            serde_json::from_str::<TextPacket>(
                r#"{"sequence":0,"detail":{"kind":"delta","text":"x","advice":true}}"#
            )
            .is_err()
        );
    }
}

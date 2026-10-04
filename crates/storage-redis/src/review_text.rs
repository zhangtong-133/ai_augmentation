//! Redis Pub/Sub only: no keys, retained body, resubscription, or message replay.
use futures_util::StreamExt;
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    learning::review_text::{
        ReviewTextBridge, TextKind, TextPacket, TextPublisher, TextSubscription, TextWindow,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct RedisReviewText {
    client: redis::Client,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: String,
    owner: String,
    request: String,
    packet: TextPacket,
}
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid review text bridge".into())
}
fn unavailable() -> StorageError {
    StorageError::Unavailable("review text bridge unavailable".into())
}
fn channel(owner: &UserId, request: &str) -> StorageResult<String> {
    if [owner.as_str(), request]
        .iter()
        .any(|v| v.is_empty() || v.len() > 128 || v.contains('\0'))
    {
        return Err(invalid());
    }
    Ok(format!(
        "learning-text:v1:{:x}:{:x}",
        Sha256::digest(owner.as_str()),
        Sha256::digest(request)
    ))
}
impl RedisReviewText {
    /// # Errors
    /// Invalid URL is rejected; connections are established separately without retries.
    pub fn new(url: &str) -> StorageResult<Self> {
        Ok(Self {
            client: redis::Client::open(url).map_err(|_| invalid())?,
        })
    }
}
struct Publisher {
    connection: redis::aio::MultiplexedConnection,
    channel: String,
    owner: String,
    request: String,
    window: TextWindow,
    failed: bool,
}
impl TextPublisher for Publisher {
    fn publish(&mut self, packet: TextPacket) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async move {
            if self.failed {
                return Err(unavailable());
            }
            self.failed = true;
            self.window.accept(&packet)?;
            let value = serde_json::to_string(&Envelope {
                version: "learning-text-bridge-v1".into(),
                owner: self.owner.clone(),
                request: self.request.clone(),
                packet,
            })
            .map_err(|_| invalid())?;
            if value.len() > 4096 {
                return Err(invalid());
            }
            tokio::time::timeout(
                Duration::from_secs(2),
                redis::cmd("PUBLISH")
                    .arg(&self.channel)
                    .arg(value)
                    .query_async::<i64>(&mut self.connection),
            )
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
            self.failed = false;
            Ok(())
        })
    }
}
struct Subscription {
    receiver: mpsc::Receiver<TextPacket>,
    failed: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl TextSubscription for Subscription {
    fn next(&mut self) -> BoxFuture<'_, StorageResult<Option<TextPacket>>> {
        Box::pin(async move {
            if self.failed.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let packet = self.receiver.recv().await;
            if self.failed.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            Ok(packet)
        })
    }
}
impl ReviewTextBridge for RedisReviewText {
    fn publisher(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Box<dyn TextPublisher>>> {
        let input =
            channel(owner, request).map(|channel| (channel, owner.to_string(), request.to_owned()));
        Box::pin(async move {
            let (channel, owner, request) = input?;
            let connection = tokio::time::timeout(
                Duration::from_secs(2),
                self.client.get_multiplexed_async_connection(),
            )
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
            Ok(Box::new(Publisher {
                connection,
                channel,
                owner,
                request,
                window: TextWindow::default(),
                failed: false,
            }) as Box<dyn TextPublisher>)
        })
    }
    fn subscribe(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Box<dyn TextSubscription>>> {
        let input =
            channel(owner, request).map(|channel| (channel, owner.to_string(), request.to_owned()));
        Box::pin(async move {
            let (channel, owner, request) = input?;
            let mut pubsub =
                tokio::time::timeout(Duration::from_secs(2), self.client.get_async_pubsub())
                    .await
                    .map_err(|_| unavailable())?
                    .map_err(|_| unavailable())?;
            tokio::time::timeout(Duration::from_secs(2), pubsub.subscribe(channel))
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
            let (sender, receiver) = mpsc::channel(16);
            let failed = Arc::new(AtomicBool::new(false));
            let failure = failed.clone();
            let task = tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                let mut messages = pubsub.on_message();
                let mut window = TextWindow::default();
                loop {
                    let message = tokio::select! {
                        () = sender.closed() => return,
                        value = tokio::time::timeout_at(deadline, messages.next()) => value,
                    };
                    let result = (|| {
                        let message = message
                            .map_err(|_| unavailable())?
                            .ok_or_else(unavailable)?;
                        let bytes = message.get_payload_bytes();
                        if bytes.len() > 4096 {
                            return Err(invalid());
                        }
                        let value: Envelope =
                            serde_json::from_slice(bytes).map_err(|_| invalid())?;
                        if value.version != "learning-text-bridge-v1"
                            || value.owner != owner
                            || value.request != request
                        {
                            return Err(invalid());
                        }
                        window.accept(&value.packet)?;
                        Ok(value.packet)
                    })();
                    if let Ok(packet) = result {
                        let ended = matches!(packet.detail, TextKind::End);
                        if sender.try_send(packet).is_err() {
                            failure.store(true, Ordering::Release);
                            return;
                        }
                        if ended {
                            return;
                        }
                    } else {
                        failure.store(true, Ordering::Release);
                        return;
                    }
                }
            });
            Ok(Box::new(Subscription {
                receiver,
                failed,
                task,
            }) as Box<dyn TextSubscription>)
        })
    }
}

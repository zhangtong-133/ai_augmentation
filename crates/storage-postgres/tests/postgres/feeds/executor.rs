use super::*;
use personal_ai_agent_core::feeds::{FeedExecutionError, FeedExecutor};
use personal_ai_feeds::transport::{FeedTransport, FetchError, FetchFuture};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};

static SERIAL: Mutex<()> = Mutex::const_new(());
enum Mode {
    Success,
    Fail(FetchError),
    Gate(Arc<Semaphore>),
    Hang,
}
struct Transport {
    mode: Mode,
    calls: AtomicUsize,
    started: Notify,
    sources: std::sync::Mutex<Vec<String>>,
}
impl Transport {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            sources: std::sync::Mutex::new(Vec::new()),
        })
    }
    async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.calls.load(Ordering::SeqCst) < count {
                self.started.notified().await;
            }
        })
        .await
        .unwrap();
    }
}
impl FeedTransport for Transport {
    fn fetch(&self, source: &str) -> FetchFuture<'_, Result<Vec<u8>, FetchError>> {
        self.sources.lock().unwrap().push(source.to_owned());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        Box::pin(async {
            match &self.mode {
                Mode::Fail(error) => return Err(*error),
                Mode::Hang => std::future::pending::<()>().await,
                Mode::Gate(gate) => {
                    let _permit = gate.acquire().await.unwrap();
                }
                Mode::Success => {}
            }
            let CollectionOutcome::Response(bytes) = response(ITEM) else {
                unreachable!()
            };
            Ok(bytes)
        })
    }
}
async fn store() -> Arc<PostgresStore> {
    Arc::new(
        PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
    )
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_requires_exact_consent_and_never_reissues_completed_or_failed_requests() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new().await;
    let store = store().await;
    let sub = f.sub().await;
    let transport = Transport::new(Mode::Success);
    let executor = FeedExecutor::new(store.clone(), transport.clone());
    let draft = f.preview(&sub).await;
    assert!(
        executor
            .execute(&f.other, &draft.plan.request_id, &draft.digest)
            .await
            .is_err()
    );
    assert!(
        executor
            .execute(&f.owner, &draft.plan.request_id, "wrong")
            .await
            .is_err()
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    let done = executor
        .execute(&f.owner, &draft.plan.request_id, &draft.digest)
        .await
        .unwrap();
    assert_eq!(done.status, CollectionStatus::Succeeded);
    assert_eq!(done.counts.inserted, 1);
    assert_eq!(
        transport.sources.lock().unwrap().as_slice(),
        std::slice::from_ref(&sub.snapshot.source_url)
    );
    assert!(
        executor
            .execute(&f.owner, &draft.plan.request_id, &draft.digest)
            .await
            .is_err()
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    for (error, status) in [
        (FetchError::Blocked, CollectionStatus::Failed),
        (FetchError::Redirect, CollectionStatus::Failed),
        (FetchError::Unavailable, CollectionStatus::Unknown),
        (FetchError::Timeout, CollectionStatus::Unknown),
    ] {
        let draft = f.preview(&sub).await;
        let transport = Transport::new(Mode::Fail(error));
        let executor = FeedExecutor::new(store.clone(), transport.clone());
        assert_eq!(
            executor
                .execute(&f.owner, &draft.plan.request_id, &draft.digest)
                .await
                .unwrap()
                .status,
            status
        );
        assert!(
            executor
                .execute(&f.owner, &draft.plan.request_id, &draft.digest)
                .await
                .is_err()
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_limit_is_process_wide_and_caller_cancellation_keeps_claimed_work_bounded() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new().await;
    let third = Fixture::new().await;
    let store = store().await;
    let mut drafts = Vec::new();
    for owner in [&f.owner, &f.other, &third.owner] {
        let sub = store
            .create_subscription(owner, &Uuid::new_v4().to_string(), &input())
            .await
            .unwrap();
        drafts.push(
            store
                .preview_collection(
                    owner,
                    &sub.snapshot.subscription_id,
                    &Uuid::new_v4().to_string(),
                )
                .await
                .unwrap(),
        );
    }
    let gate = Arc::new(Semaphore::new(0));
    let transport = Transport::new(Mode::Gate(gate.clone()));
    let mut callers = Vec::new();
    for (index, owner) in [f.owner.clone(), f.other.clone()].into_iter().enumerate() {
        let executor = FeedExecutor::new(store.clone(), transport.clone());
        let draft = drafts[index].clone();
        callers.push(tokio::spawn(async move {
            executor
                .execute(&owner, &draft.plan.request_id, &draft.digest)
                .await
        }));
        transport.wait_for(index + 1).await;
    }
    let third_executor = FeedExecutor::new(store.clone(), transport.clone());
    let result = third_executor
        .execute(&third.owner, &drafts[2].plan.request_id, &drafts[2].digest)
        .await;
    assert!(matches!(result, Err(FeedExecutionError::Busy)));
    assert_eq!(
        store
            .get_collection(&third.owner, &drafts[2].plan.request_id)
            .await
            .unwrap()
            .status,
        CollectionStatus::Draft
    );
    let first = callers.remove(0);
    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    assert!(matches!(
        third_executor
            .execute(&third.owner, &drafts[2].plan.request_id, &drafts[2].digest)
            .await,
        Err(FeedExecutionError::Busy)
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    gate.add_permits(2);
    assert_eq!(
        callers.remove(0).await.unwrap().unwrap().status,
        CollectionStatus::Succeeded
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if store
                .get_collection(&f.owner, &drafts[0].plan.request_id)
                .await
                .unwrap()
                .status
                == CollectionStatus::Succeeded
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    f.cleanup().await;
    third.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_timeout_and_writeback_failure_do_not_retry_network_or_claim() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new().await;
    let store = store().await;
    let sub = f.sub().await;
    let transport = Transport::new(Mode::Hang);
    let executor = FeedExecutor::new(store.clone(), transport.clone());
    let draft = f.preview(&sub).await;
    let saved = tokio::time::timeout(
        Duration::from_secs(12),
        executor.execute(&f.owner, &draft.plan.request_id, &draft.digest),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(saved.status, CollectionStatus::Unknown);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    let draft = f.preview(&sub).await;
    sqlx::query("INSERT INTO feed_collection_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'succeeded',1)")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&draft.plan.request_id).unwrap()).execute(&f.pool).await.unwrap();
    let transport = Transport::new(Mode::Success);
    let executor = FeedExecutor::new(store.clone(), transport.clone());
    assert!(matches!(
        executor
            .execute(&f.owner, &draft.plan.request_id, &draft.digest)
            .await,
        Err(FeedExecutionError::Storage(_))
    ));
    assert_eq!(
        store
            .get_collection(&f.owner, &draft.plan.request_id)
            .await
            .unwrap()
            .status,
        CollectionStatus::Running
    );
    assert!(
        executor
            .execute(&f.owner, &draft.plan.request_id, &draft.digest)
            .await
            .is_err()
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(
        store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduled_executor_sends_once_and_retains_unknown_transport_outcomes() {
    use personal_ai_agent_core::feeds::ScheduledFeedExecutor;
    let _serial = SERIAL.lock().await;
    let f = Fixture::new().await;
    let store = store().await;
    for mode in [Mode::Success, Mode::Fail(FetchError::Unavailable)] {
        let sub = f.sub().await;
        let schedule = super::scheduled_execution::due(&f, &sub).await;
        let expected = if matches!(&mode, Mode::Success) {
            CollectionStatus::Succeeded
        } else {
            CollectionStatus::Unknown
        };
        let transport = Transport::new(mode);
        let executor = ScheduledFeedExecutor::new(store.clone(), transport.clone());
        let result = executor
            .execute(&f.owner, &schedule.plan.input.schedule_id)
            .await
            .unwrap();
        assert_eq!(result.status, expected);
        assert!(
            executor
                .execute(&f.owner, &schedule.plan.input.schedule_id)
                .await
                .is_err()
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
    f.cleanup().await;
}

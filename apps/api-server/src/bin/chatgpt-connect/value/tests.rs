use super::*;
#[test]
fn scoring_requires_explicit_identity_digest_and_separate_usage_flags() {
    let id = Uuid::new_v4().to_string();
    let digest = "a".repeat(64);
    let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    for s in [
        format!("dir value-preview {id} {id} model {id}"),
        format!("dir value-approve {id} {id} {digest} --share-content --use-subscription"),
        format!("dir value-run personal {id} {id} --use-subscription"),
        format!("dir value-show {id} {id}"),
        format!("dir value-cancel {id} {id}"),
    ] {
        assert!(parse(&args(&s)).is_ok());
    }
    for s in [
        format!("dir value-approve {id} {id} {digest} --use-subscription"),
        format!("dir value-approve {id} {id} bad --share-content --use-subscription"),
        format!("dir value-run personal {id} {id}"),
        format!("dir value-run ../other {id} {id} --use-subscription"),
        format!("dir value-preview {id} invalid model {id}"),
        format!("dir value-show {id} {id} extra"),
        "dir value-cancel invalid invalid".into(),
    ] {
        assert!(parse(&args(&s)).is_err());
    }
}

use crate::value_runtime::tests::{Client, local, pricing};
use personal_ai_agent_core::feed_value_execution::SubscriptionValueRuntime;
use personal_ai_storage::{
    MetadataStore,
    briefs::BriefStore,
    feeds::{CollectionOutcome, FeedStore, SubscriptionInput},
};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn seed(db: &PostgresStore, owner: &UserId) {
    db.save_user(&personal_ai_domain::User {
        id: owner.clone(),
        email: format!("{}@value-local.example", owner.as_str()),
        display_name: "fixture".into(),
    })
    .await
    .unwrap();
    let sub = db
        .create_subscription(
            owner,
            &Uuid::new_v4().to_string(),
            &SubscriptionInput {
                name: "fixture".into(),
                source_url: "https://example.com/rss".into(),
                enabled: true,
            },
        )
        .await
        .unwrap();
    let preview = db
        .preview_collection(
            owner,
            &sub.snapshot.subscription_id,
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
    let claim = db
        .claim_collection(owner, &preview.plan.request_id, &preview.digest)
        .await
        .unwrap();
    db.finish_collection(&claim,CollectionOutcome::Response(b"<rss version=\"2.0\"><channel><title>News</title><description>Summary</description><link>https://example.com/</link><item><guid>one</guid><title>Rust &lt;script&gt;title&lt;/script&gt;</title><description>Learning Rust</description></item></channel></rss>".to_vec())).await.unwrap();
    db.save_brief_preferences(owner, 0, &["Rust".into()])
        .await
        .unwrap();
}
async fn command(db: &PostgresStore, tail: &str) -> Result<ValueReview> {
    let args = format!("unused {tail}")
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let directory = std::env::temp_dir().join(format!("value-no-credentials-{}", Uuid::new_v4()));
    let result = manage(db, directory.to_str().unwrap(), &parse(&args)?).await;
    assert!(!directory.exists());
    result
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn local_scoring_preview_approval_execution_and_replay_remain_explicit() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let db = PostgresStore::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    seed(&db, &owner).await;
    let (path, local) = local();
    let client = Client {
        calls: AtomicUsize::new(0),
    };
    let runtime = Runtime {
        client: &client,
        store: &local,
        label: "fixture",
    };
    let connection = Uuid::new_v4().to_string();
    let pricing = pricing(connection.clone());
    let proof = runtime.verify(&pricing).await.unwrap();
    db.save_subscription_connection(&owner, &connection, 0, &proof)
        .await
        .unwrap();
    let id = Uuid::new_v4().to_string();
    let saved = command(
        &db,
        &format!("value-preview {owner} {connection} fixture {id}"),
    )
    .await
    .unwrap();
    assert_eq!(saved.status, "draft");
    assert!(
        command(
            &db,
            &format!("value-preview {owner} {connection} fixture {id}")
        )
        .await
        .unwrap()
            == saved
    );
    let view = display(&owner, &saved).unwrap();
    assert!(
        view["shared_content"]["instructions"]
            .as_str()
            .unwrap()
            .contains("不可信数据")
    );
    let input: serde_json::Value =
        serde_json::from_str(view["shared_content"]["input"].as_str().unwrap()).unwrap();
    assert_eq!(input["items"].as_array().unwrap().len(), 1);
    assert!(input["items"][0].get("source_url").is_none());
    assert!(!view.to_string().contains("test-only"));
    assert!(!view.to_string().contains("oaiapp_"));
    assert!(
        execute_subscription_value(&db, &runtime, &owner, &id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        command(
            &db,
            &format!(
                "value-approve {owner} {id} {} --share-content --use-subscription",
                "b".repeat(64)
            )
        )
        .await
        .is_err()
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    let approved = command(
        &db,
        &format!(
            "value-approve {owner} {id} {} --share-content --use-subscription",
            saved.digest
        ),
    )
    .await
    .unwrap();
    assert_eq!(approved.status, "authorized");
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    let complete = execute_subscription_value(&db, &runtime, &owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(complete.status, "succeeded");
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    assert!(
        execute_subscription_value(&db, &runtime, &owner, &id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        command(
            &db,
            &format!("value-run nonexistent {owner} {id} --use-subscription")
        )
        .await
        .unwrap()
            == complete
    );
    assert!(
        command(&db, &format!("value-show {owner} {id}"))
            .await
            .unwrap()
            == complete
    );
    assert!(
        command(&db, &format!("value-show {} {id}", Uuid::new_v4()))
            .await
            .is_err()
    );
    let cancel_id = Uuid::new_v4().to_string();
    let cancel = command(
        &db,
        &format!("value-preview {owner} {connection} fixture {cancel_id}"),
    )
    .await
    .unwrap();
    assert_eq!(
        command(&db, &format!("value-cancel {owner} {cancel_id}"))
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    assert!(
        command(
            &db,
            &format!(
                "value-approve {owner} {cancel_id} {} --share-content --use-subscription",
                cancel.digest
            )
        )
        .await
        .is_err()
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    drop(local);
    std::fs::remove_dir_all(path).unwrap();
}

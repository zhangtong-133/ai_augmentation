use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore,
    learning::{
        LearningPlanInput, LearningStore, SkillInput, TrainingOutcome, TrainingResultInput,
    },
    learning_operations::LearningOperationsStore,
};
use personal_ai_storage_postgres::PostgresStore;
use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use std::{process::Command, str::FromStr};
use uuid::Uuid;

struct Fixture {
    url: String,
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = UserId::new(Uuid::new_v4().to_string());
        store
            .save_user(&User {
                id: owner.clone(),
                email: format!("{owner}@learning-ops.example"),
                display_name: "private-owner".into(),
            })
            .await
            .unwrap();
        Self {
            url,
            store,
            pool,
            owner,
        }
    }
    fn id(&self) -> Uuid {
        Uuid::parse_str(self.owner.as_str()).unwrap()
    }
    async fn skill(&self) -> String {
        let id = Uuid::new_v4().to_string();
        self.store
            .save_skill(
                &self.owner,
                &id,
                0,
                &SkillInput {
                    name: "private-skill".into(),
                    enabled: true,
                    prerequisite_ids: vec![],
                },
            )
            .await
            .unwrap();
        id
    }
    async fn plan(&self, skill: &str) -> (String, String) {
        let id = Uuid::new_v4().to_string();
        let revision = self
            .store
            .learning_snapshot(&self.owner)
            .await
            .unwrap()
            .revision;
        let p = self
            .store
            .create_learning_plan(
                &self.owner,
                &id,
                &LearningPlanInput {
                    expected_revision: revision,
                    budget_minutes: 30,
                    goal_skill_ids: vec![skill.into()],
                },
            )
            .await
            .unwrap();
        (id, p.plan.unwrap().tasks[0].task_id.clone())
    }
    async fn result(&self, plan: &str, task: &str) {
        self.store
            .record_training_result(
                &self.owner,
                plan,
                task,
                &TrainingResultInput {
                    request_id: Uuid::new_v4().to_string(),
                    outcome: TrainingOutcome::Completed,
                    note: "private-note".into(),
                    actual_minutes: 5,
                },
            )
            .await
            .unwrap();
    }
    async fn seed(&self, n: i32, today: bool) {
        sqlx::query("INSERT INTO learning_plans(user_id,request_id,snapshot_revision,request_digest,digest,created_ms,status) SELECT $1,gen_random_uuid(),0,repeat('a',64),repeat('b',64),CASE WHEN $3 THEN floor(extract(epoch FROM clock_timestamp())*1000)::bigint ELSE 0 END,'deleted' FROM generate_series(1,$2)")
            .bind(self.id()).bind(n).bind(today).execute(&self.pool).await.unwrap();
    }
    async fn snapshot(&self) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        for table in [
            "learning_state",
            "learning_skills",
            "learning_edges",
            "learning_assessments",
            "learning_plans",
            "learning_plan_sources",
            "learning_tasks",
            "learning_results",
        ] {
            let value = sqlx::query_scalar(&format!("SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM {table} t WHERE user_id=$1")).bind(self.id()).fetch_one(&self.pool).await.unwrap();
            values.push(value);
        }
        values
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.id())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}
fn cli(url: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_learning-operations"))
        .args(args)
        .env("DATABASE_URL", url)
        .output()
        .unwrap()
}
#[test]
fn invalid_cli_arguments_never_connect_or_expose_configuration() {
    for args in [
        vec!["audit"],
        vec!["audit", "--user", "bad"],
        vec!["audit", "--user", "00000000-0000-0000-0000-000000000000"],
        vec!["audit", "--apply"],
    ] {
        let out = cli("postgres://private-password@invalid/db", &args);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(out.stdout, [] as [u8; 0]);
        assert!(!String::from_utf8_lossy(&out.stderr).contains("private-password"));
    }
    assert_eq!(cli("invalid", &["--help"]).status.code(), Some(0));
}
#[tokio::test]
#[ignore = "需要有建角色权限的一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn metadata_reader_paginates_without_private_columns_writes_or_migrations() {
    let f = Fixture::new().await;
    let skill = f.skill().await;
    let (plan, task) = f.plan(&skill).await;
    f.result(&plan, &task).await;
    f.seed(105, false).await;
    let before = f.snapshot().await;
    let report = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(report.consistent);
    assert_eq!(report.counts["plans"], 106);
    assert_eq!(report.counts["completed_results"], 1);
    assert_eq!(report.counts["pending_tasks"], 0);
    assert_eq!(report.items.len(), 100);
    assert_eq!(report.quotas["plans_today"].remaining, 9);
    let second = f
        .store
        .audit_learning(&f.owner, report.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 6);
    assert!(second.next_cursor.is_none());
    assert_eq!(second.counts, report.counts);
    let mut ids = report
        .items
        .iter()
        .chain(&second.items)
        .map(|i| &i.request_id)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 106);
    let role = format!("learning_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'; ALTER ROLE {role} SET default_transaction_read_only=on;
      GRANT USAGE ON SCHEMA public TO {role};
      GRANT SELECT(id) ON users TO {role};
      GRANT SELECT(user_id,revision) ON learning_state TO {role};
      GRANT SELECT(user_id,id,revision,enabled,deleted) ON learning_skills TO {role};
      GRANT SELECT(user_id,skill_id,prerequisite_id) ON learning_edges TO {role};
      GRANT SELECT(user_id,skill_id,skill_revision,expected_revision,assessed_ms) ON learning_assessments TO {role};
      GRANT SELECT(user_id,request_id,snapshot_revision,created_ms,status) ON learning_plans TO {role};
      GRANT SELECT(user_id,request_id,skill_id) ON learning_plan_sources TO {role};
      GRANT SELECT(user_id,request_id,id,ordinal,status) ON learning_tasks TO {role};
      GRANT SELECT(user_id,task_id,outcome,recorded_ms) ON learning_results TO {role};")).execute(&f.pool).await.unwrap();
    let limited = PgConnectOptions::from_str(&f.url)
        .unwrap()
        .username(&role)
        .password(&password)
        .to_url_lossy()
        .to_string();
    let out = cli(&limited, &["audit", "--user", f.owner.as_str()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stderr, [] as [u8; 0]);
    let text = String::from_utf8(out.stdout).unwrap();
    for secret in [
        "private-owner",
        "private-skill",
        "private-note",
        "request_digest",
        "actual_minutes",
    ] {
        assert!(!text.contains(secret));
    }
    let reader = sqlx::PgPool::connect(&limited).await.unwrap();
    for query in [
        "SELECT name FROM learning_skills",
        "SELECT score FROM learning_assessments",
        "SELECT plan FROM learning_plans",
        "SELECT task FROM learning_tasks",
        "SELECT note FROM learning_results",
        "SELECT version FROM _sqlx_migrations",
        "UPDATE learning_state SET revision=revision+1",
    ] {
        assert!(sqlx::query(query).execute(&reader).await.is_err());
    }
    reader.close().await;
    assert_eq!(f.snapshot().await, before);
    sqlx::query("UPDATE learning_tasks SET ordinal=4 WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        cli(&limited, &["audit", "--user", f.owner.as_str()])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        cli(&limited, &["audit", "--user", &Uuid::new_v4().to_string()])
            .status
            .code(),
        Some(1)
    );
    sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
        .execute(&f.pool)
        .await
        .unwrap();
    let other = Fixture::new().await;
    let empty = f.store.audit_learning(&other.owner, None).await.unwrap();
    assert!(empty.consistent);
    assert_eq!(empty.counts["plans"], 0);
    assert!(empty.items.is_empty());
    assert_eq!(empty.revision, "0");
    other.cleanup().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn graph_versions_and_result_metadata_are_checked_without_false_tombstone_alarms() {
    let f = Fixture::new().await;
    let a = f.skill().await;
    let b = f.skill().await;
    f.store.delete_skill(&f.owner, &a, 1).await.unwrap();
    let (plan, task) = f.plan(&b).await;
    f.result(&plan, &task).await;
    // Plans legitimately include tombstones already present in their input graph.
    assert!(
        f.store
            .audit_learning(&f.owner, None)
            .await
            .unwrap()
            .consistent
    );
    sqlx::query("UPDATE learning_state SET revision=9007199254740993 WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .audit_learning(&f.owner, None)
            .await
            .unwrap()
            .revision,
        "9007199254740993"
    );
    sqlx::query(
        "INSERT INTO learning_edges(user_id,skill_id,prerequisite_id) VALUES($1,$2,$3),($1,$3,$2)",
    )
    .bind(f.id())
    .bind(Uuid::parse_str(&a).unwrap())
    .bind(Uuid::parse_str(&b).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    let report = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert_eq!(report.counts["cyclic_skills"], 2);
    assert_eq!(report.counts["edges_on_deleted_skills"], 1);
    assert_eq!(report.counts["unavailable_prerequisites"], 1);
    assert!(!report.consistent);
    sqlx::query("UPDATE learning_results SET recorded_ms=0 WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM learning_plan_sources WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(
        report.items[0]
            .issues
            .contains(&"invalid_result_time".into())
    );
    assert!(
        report.items[0]
            .issues
            .contains(&"missing_plan_sources".into())
    );
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) VALUES($1,gen_random_uuid(),$2,2,0,50,9223372036854775807)").bind(f.id()).bind(Uuid::parse_str(&b).unwrap()).execute(&f.pool).await.unwrap();
    sqlx::query("DELETE FROM learning_state WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert_eq!(report.counts["invalid_assessment_metadata"], 1);
    assert_eq!(report.counts["revision_below_mutations"], 1);
    assert!(
        report.items[0]
            .issues
            .contains(&"snapshot_ahead_of_state".into())
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn quota_exhaustion_overage_and_off_page_inconsistency_remain_distinct() {
    let f = Fixture::new().await;
    f.seed(10, true).await;
    let full = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(full.consistent);
    assert_eq!(full.quotas["plans_today"].remaining, 0);
    assert!(
        full.warnings
            .contains(&"plans_today_quota_exhausted".into())
    );
    f.seed(1, true).await;
    f.seed(990, false).await;
    let over = f
        .store
        .audit_learning(&f.owner, Some("ffffffff-ffff-ffff-ffff-ffffffffffff"))
        .await
        .unwrap();
    assert!(over.items.is_empty());
    assert_eq!(over.counts["plans"], 1001);
    assert!(over.issues.contains(&"plans_limit_exceeded".into()));
    assert!(over.issues.contains(&"plans_today_limit_exceeded".into()));
    assert_eq!(over.quotas["plans"].remaining, 0);
    sqlx::query("UPDATE learning_plans SET created_ms=9223372036854775807 WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f
        .store
        .audit_learning(&f.owner, Some("ffffffff-ffff-ffff-ffff-ffffffffffff"))
        .await
        .unwrap();
    assert_eq!(report.counts["inconsistent_plans"], 1001);
    assert!(report.items.is_empty());
    assert!(!report.consistent);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn graph_over_limit_is_reported_without_unbounded_traversal() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO learning_skills(user_id,id,revision,name,enabled) SELECT $1,gen_random_uuid(),1,'private-skill',true FROM generate_series(1,101)").bind(f.id()).execute(&f.pool).await.unwrap();
    let report = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(report.issues.contains(&"skills_limit_exceeded".into()));
    assert!(
        report
            .warnings
            .contains(&"cycle_check_skipped_over_limit".into())
    );
    assert_eq!(report.quotas["skills"].remaining, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn audit_uses_one_snapshot_across_concurrent_result_cleanup() {
    let f = Fixture::new().await;
    let skill = f.skill().await;
    let (plan, task) = f.plan(&skill).await;
    f.result(&plan, &task).await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("UPDATE learning_plans SET status='deleted',plan=NULL WHERE user_id=$1")
        .bind(f.id())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE learning_tasks SET status='deleted',task=NULL WHERE user_id=$1")
        .bind(f.id())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM learning_plan_sources WHERE user_id=$1")
        .bind(f.id())
        .execute(&mut *tx)
        .await
        .unwrap();
    let reads = async {
        for _ in 0..8 {
            let r = f.store.audit_learning(&f.owner, None).await.unwrap();
            assert!(r.consistent);
            assert_eq!(r.counts["ready_plans"], r.counts["completed_results"]);
            assert_eq!(
                r.counts["ready_plans"],
                i64::from(r.items[0].status == "ready")
            );
            assert_eq!(r.counts["ready_plans"] + r.counts["deleted_plans"], 1);
        }
    };
    let ((), committed) = tokio::join!(reads, tx.commit());
    committed.unwrap();
    assert_eq!(
        f.store.audit_learning(&f.owner, None).await.unwrap().counts["completed_results"],
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn stale_assessments_and_skill_tombstones_count_toward_quotas() {
    let f = Fixture::new().await;
    let skill = f.skill().await;
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) SELECT $1,gen_random_uuid(),$2,1,0,NULL,n FROM generate_series(1,1000) n").bind(f.id()).bind(Uuid::parse_str(&skill).unwrap()).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE learning_state SET revision=1001 WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    f.store.delete_skill(&f.owner, &skill, 1).await.unwrap();
    let r = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(r.consistent);
    assert_eq!(r.counts["historical_assessments"], 1000);
    assert_eq!(r.quotas["assessments"].remaining, 0);
    assert_eq!(r.quotas["skills"].remaining, 99);
    assert_eq!(r.counts["deleted_skills"], 1);
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) VALUES($1,gen_random_uuid(),$2,1,0,NULL,1001)").bind(f.id()).bind(Uuid::parse_str(&skill).unwrap()).execute(&f.pool).await.unwrap();
    let r = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(r.issues.contains(&"assessments_limit_exceeded".into()));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn inactive_tasks_and_deleted_plan_sources_are_reported() {
    let f = Fixture::new().await;
    let skill = f.skill().await;
    let (plan, task) = f.plan(&skill).await;
    sqlx::query("UPDATE learning_tasks SET status='invalidated',task=NULL WHERE user_id=$1")
        .bind(f.id())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO learning_results(user_id,task_id,request_id,outcome,note,actual_minutes,recorded_ms) VALUES($1,$2,gen_random_uuid(),'cancelled','private-note',0,0)").bind(f.id()).bind(Uuid::parse_str(&task).unwrap()).execute(&f.pool).await.unwrap();
    let r = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(r.items[0].issues.contains(&"task_status_mismatch".into()));
    assert!(
        r.items[0]
            .issues
            .contains(&"result_on_inactive_task".into())
    );
    sqlx::query(
        "UPDATE learning_plans SET status='deleted',plan=NULL WHERE user_id=$1 AND request_id=$2",
    )
    .bind(f.id())
    .bind(Uuid::parse_str(&plan).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    let r = f.store.audit_learning(&f.owner, None).await.unwrap();
    assert!(r.items[0].issues.contains(&"deleted_plan_sources".into()));
    f.cleanup().await;
}

#[path = "learning_operations/model_reviews.rs"]
mod model_reviews;

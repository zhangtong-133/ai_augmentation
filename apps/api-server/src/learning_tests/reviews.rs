use super::*;
fn body(verdict: &str) -> serde_json::Value {
    let dim = json!({"verdict":verdict,"reason":"用户核验理由 <script>"});
    json!({"explanation":dim,"work":dim,"verification":dim,"limitations":dim})
}
pub(super) async fn setup() -> (Fixture, String, String, Uuid) {
    let f = Fixture::new().await;
    let skill = Uuid::new_v4();
    f.call(
        "PUT",
        &format!("/api/learning/skills/{skill}"),
        json!({"revision":"0","name":"核验技能","enabled":true,"prerequisite_ids":[]}),
    )
    .await;
    let plan = Uuid::new_v4();
    let (_, saved) = f.call("POST", "/api/learning/plans", json!({"request_id":plan,"expected_revision":"1","budget_minutes":30,"goal_skill_ids":[skill]})).await;
    let task = saved["plan"]["tasks"][0]["task_id"].as_str().unwrap();
    let plan_path = format!("/api/learning/plans/{plan}");
    let task_path = format!("{plan_path}/tasks/{task}");
    assert_eq!(
        f.call(
            "POST",
            &format!("{task_path}/result"),
            json!({"request_id":Uuid::new_v4(),"outcome":"completed","note":"","actual_minutes":5})
        )
        .await
        .0,
        StatusCode::OK
    );
    (f, plan_path, format!("{task_path}/evidence"), skill)
}
pub(super) async fn evidence(f: &Fixture, path: &str) -> Uuid {
    let request = Uuid::new_v4();
    assert_eq!(f.call("POST", path, json!({"request_id":request,"body":{"explanation":"概念","work":"独立产物","verification":"验证过程","limitations":"局限"}})).await.0, StatusCode::OK);
    request
}
fn confirm_input(review: &serde_json::Value, revision: &str) -> serde_json::Value {
    json!({"request_id":Uuid::new_v4(),"review_request_id":review["request_id"],"expected_revision":revision,"score":68})
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn review_confirm_is_explicit_atomic_private_and_revoked_with_evidence() {
    let (f, plan, path, _) = setup().await;
    let review_path = format!("{path}/review");
    let confirm_path = format!("{review_path}/confirm");
    let mut input = json!({"request_id":Uuid::new_v4(),"evidence_request_id":Uuid::new_v4(),"body":body("supported")});
    assert_eq!(
        f.call("POST", &review_path, input.clone()).await.0,
        StatusCode::CONFLICT
    );
    let evidence = evidence(&f, &path).await;
    input["evidence_request_id"] = json!(evidence);
    for endpoint in [&review_path, &confirm_path] {
        for (cookie, csrf, status) in [
            (None, true, StatusCode::UNAUTHORIZED),
            (Some(f.cookie.as_str()), false, StatusCode::FORBIDDEN),
        ] {
            let response = f.send("POST", endpoint, json!({}), cookie, csrf).await;
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
    }
    assert_eq!(
        f.send(
            "POST",
            &review_path,
            input.clone(),
            Some(&f.other_cookie),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let mut bad = input.clone();
    bad["body"]["work"]["reason"] = json!(" ");
    assert_eq!(
        f.call("POST", &review_path, bad).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut bad = input.clone();
    bad["body"]["work"]["verdict"] = json!("verified_by_model");
    assert_eq!(
        f.call("POST", &review_path, bad).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut bad = input.clone();
    bad["body"]["score"] = json!(100);
    assert_eq!(
        f.call("POST", &review_path, bad).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (a, b) = tokio::join!(
        f.call("POST", &review_path, input.clone()),
        f.call("POST", &review_path, input.clone())
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(a, b);
    let pending = &a.1["results"][0]["evidence"]["review"];
    assert_eq!(pending["status"], "pending");
    assert!(pending["confirmed_score"].is_null());
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    assert_eq!(snapshot["revision"], "1");
    assert_eq!(snapshot["assessments"], json!([]));
    let mut changed = input.clone();
    changed["body"]["work"]["reason"] = json!("不同理由");
    assert_eq!(
        f.call("POST", &review_path, changed).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &confirm_path, confirm_input(&input, "0"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let confirmation = confirm_input(&input, "1");
    assert_eq!(
        f.send(
            "POST",
            &confirm_path,
            confirmation.clone(),
            Some(&f.other_cookie),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let (a, b) = tokio::join!(
        f.call("POST", &confirm_path, confirmation.clone()),
        f.call("POST", &confirm_path, confirmation.clone())
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(a, b);
    let confirmed = &a.1["results"][0]["evidence"]["review"];
    assert_eq!(confirmed["status"], "confirmed");
    assert_eq!(confirmed["confirmed_score"], 68);
    assert_eq!(confirmed["expected_revision"], "1");
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    assert_eq!(snapshot["revision"], "2");
    assert_eq!(snapshot["assessments"].as_array().unwrap().len(), 1);
    assert_eq!(
        snapshot["assessments"][0]["assessment_id"],
        confirmed["assessment_id"]
    );
    let mut changed = confirmation.clone();
    changed["score"] = json!(69);
    assert_eq!(
        f.call("POST", &confirm_path, changed).await.0,
        StatusCode::CONFLICT
    );
    let (_, erased) = f
        .call("DELETE", &path, json!({"request_id":evidence}))
        .await;
    let invalid = &erased["results"][0]["evidence"]["review"];
    assert_eq!(invalid["status"], "invalidated");
    assert!(invalid["body"].is_null());
    assert!(invalid["confirmed_score"].is_null());
    assert!(!erased.to_string().contains("用户核验理由"));
    assert_eq!(f.call("GET", &plan, json!({})).await.1, erased);
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    assert_eq!(snapshot["revision"], "3");
    assert_eq!(snapshot["assessments"], json!([]));
    assert_eq!(
        f.call("POST", &confirm_path, confirmation).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &review_path, input).await.0,
        StatusCode::CONFLICT
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn review_missing_support_or_stale_skill_cannot_confirm() {
    for verdict in ["missing", "unverified", "supported"] {
        let (f, _, path, skill) = setup().await;
        let evidence = evidence(&f, &path).await;
        let mut rubric = body("supported");
        rubric["verification"]["verdict"] = json!(verdict);
        let review =
            json!({"request_id":Uuid::new_v4(),"evidence_request_id":evidence,"body":rubric});
        assert_eq!(
            f.call("POST", &format!("{path}/review"), review.clone())
                .await
                .0,
            StatusCode::OK
        );
        let mut confirmation = confirm_input(&review, "1");
        if verdict == "supported" {
            f.call("POST","/api/learning/assessments",json!({"request_id":Uuid::new_v4(),"expected_revision":"1","skill_id":skill,"skill_revision":"1","score":30})).await;
            assert_eq!(
                f.call(
                    "POST",
                    &format!("{path}/review/confirm"),
                    confirmation.clone()
                )
                .await
                .0,
                StatusCode::CONFLICT
            );
            f.call(
                "PUT",
                &format!("/api/learning/skills/{skill}"),
                json!({"revision":"1","name":"新版本","enabled":true,"prerequisite_ids":[]}),
            )
            .await;
            confirmation["expected_revision"] = json!("3");
        }
        assert_eq!(
            f.call("POST", &format!("{path}/review/confirm"), confirmation)
                .await
                .0,
            StatusCode::CONFLICT
        );
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn review_confirmation_cannot_survive_source_deletion_races() {
    for delete_plan in [false, true] {
        let (f, plan, path, _) = setup().await;
        let evidence = evidence(&f, &path).await;
        let review = json!({"request_id":Uuid::new_v4(),"evidence_request_id":evidence,"body":body("supported")});
        f.call("POST", &format!("{path}/review"), review.clone())
            .await;
        let confirmation = confirm_input(&review, "1");
        let endpoint = format!("{path}/review/confirm");
        let (confirmed, deleted) = tokio::join!(
            f.call("POST", &endpoint, confirmation.clone()),
            f.call(
                "DELETE",
                if delete_plan { &plan } else { &path },
                if delete_plan {
                    json!({})
                } else {
                    json!({"request_id":evidence})
                }
            )
        );
        assert!(matches!(confirmed.0, StatusCode::OK | StatusCode::CONFLICT));
        assert_eq!(deleted.0, StatusCode::OK);
        assert_eq!(
            f.call("GET", "/api/learning/snapshot", json!({})).await.1["assessments"],
            json!([])
        );
        assert_eq!(
            f.call("POST", &endpoint, confirmation).await.0,
            StatusCode::CONFLICT
        );
        let body_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM learning_reviews WHERE user_id=$1 AND body IS NOT NULL",
        )
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(body_count, 0);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn confirmed_review_cleanup_preserves_independent_assessments_and_handles_owner_erasure() {
    for deletion in ["evidence", "plan", "skill", "owner"] {
        let (f, plan, path, skill) = setup().await;
        let evidence = evidence(&f, &path).await;
        f.call("POST","/api/learning/assessments",json!({"request_id":Uuid::new_v4(),"expected_revision":"1","skill_id":skill,"skill_revision":"1","score":20})).await;
        let review = json!({"request_id":Uuid::new_v4(),"evidence_request_id":evidence,"body":body("supported")});
        assert_eq!(
            f.call("POST", &format!("{path}/review"), review.clone())
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            f.call(
                "POST",
                &format!("{path}/review/confirm"),
                confirm_input(&review, "2")
            )
            .await
            .0,
            StatusCode::OK
        );
        match deletion {
            "evidence" => {
                assert_eq!(
                    f.call("DELETE", &path, json!({"request_id":evidence}))
                        .await
                        .0,
                    StatusCode::OK
                );
            }
            "plan" => {
                assert_eq!(f.call("DELETE", &plan, json!({})).await.0, StatusCode::OK);
            }
            "skill" => {
                assert_eq!(
                    f.call(
                        "DELETE",
                        &format!("/api/learning/skills/{skill}"),
                        json!({"revision":"1"})
                    )
                    .await
                    .0,
                    StatusCode::OK
                );
            }
            _ => {
                f.cleanup().await;
            }
        }
        if deletion != "owner" {
            let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
            if deletion == "skill" {
                assert_eq!(snapshot["assessments"], json!([]));
            } else {
                assert_eq!(snapshot["assessments"].as_array().unwrap().len(), 1);
                assert_eq!(snapshot["assessments"][0]["score"], 20);
            }
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn revoked_source_blocks_new_work_but_preserves_historical_plans_and_authored_records() {
    let (f, root, path, skill) = setup().await;
    let evidence_id = evidence(&f, &path).await;
    let review = json!({"request_id":Uuid::new_v4(),"evidence_request_id":evidence_id,"body":body("supported")});
    f.call("POST", &format!("{path}/review"), review.clone())
        .await;
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/review/confirm"),
            confirm_input(&review, "1")
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut children = Vec::new();
    for stage in 0..3 {
        let plan = Uuid::new_v4();
        let (status,saved)=f.call("POST","/api/learning/plans",json!({"request_id":plan,"expected_revision":"2","budget_minutes":30,"goal_skill_ids":[skill]})).await;
        assert_eq!(status, StatusCode::CREATED);
        let plan_path = format!("/api/learning/plans/{plan}");
        let task = saved["plan"]["tasks"][0]["task_id"].as_str().unwrap();
        let task_path = format!("{plan_path}/tasks/{task}");
        let mut confirmation = json!({});
        if stage > 0 {
            assert_eq!(f.call("POST",&format!("{task_path}/result"),json!({"request_id":Uuid::new_v4(),"outcome":"completed","note":"独立训练记录保留","actual_minutes":25})).await.0,StatusCode::OK);
        }
        if stage == 2 {
            let child_path = format!("{task_path}/evidence");
            let child_evidence = evidence(&f, &child_path).await;
            let child_review = json!({"request_id":Uuid::new_v4(),"evidence_request_id":child_evidence,"body":body("supported")});
            assert_eq!(
                f.call(
                    "POST",
                    &format!("{child_path}/review"),
                    child_review.clone()
                )
                .await
                .0,
                StatusCode::OK
            );
            confirmation = confirm_input(&child_review, "3");
        }
        children.push((plan_path, task_path, confirmation));
    }
    assert_eq!(
        f.call("DELETE", &path, json!({"request_id":evidence_id}))
            .await
            .0,
        StatusCode::OK
    );
    for (stage, (plan, task, confirmation)) in children.iter().enumerate() {
        let (_, saved) = f.call("GET", plan, json!({})).await;
        assert_eq!(saved["status"], "ready");
        assert!(!saved["plan"].is_null());
        assert_eq!(saved["source_assessments_available"], false);
        assert_eq!(
            f.call("GET", &format!("{task}/evidence/model-preview"), json!({}))
                .await
                .0,
            StatusCode::CONFLICT
        );
        if stage > 0 {
            assert_eq!(saved["results"][0]["note"], "独立训练记录保留");
        }
        let (endpoint, input) = match stage {
            0 => (
                format!("{task}/result"),
                json!({"request_id":Uuid::new_v4(),"outcome":"completed","note":"新训练","actual_minutes":25}),
            ),
            1 => (
                format!("{task}/evidence"),
                json!({"request_id":Uuid::new_v4(),"body":{"explanation":"概念","work":"产物","verification":"验证","limitations":"局限"}}),
            ),
            _ => (
                format!("{task}/evidence/review/confirm"),
                confirmation.clone(),
            ),
        };
        assert_eq!(
            f.call("POST", &endpoint, input).await.0,
            StatusCode::CONFLICT
        );
    }
    assert_eq!(
        f.call("GET", &root, json!({})).await.1["source_assessments_available"],
        true
    );
    let progress = f.call("GET", "/api/learning/progress", json!({})).await.1;
    assert_eq!(progress["pending_tasks"], 0);
    assert_eq!(progress["completed_tasks"], 3);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_preview_is_private_read_only_and_requires_live_evidence_and_skill() {
    let (f, plan, path, skill) = setup().await;
    let endpoint = format!("{path}/model-preview");
    assert_eq!(
        f.call("GET", &endpoint, json!({})).await.0,
        StatusCode::CONFLICT
    );
    let evidence_id = evidence(&f, &path).await;
    let before = f.call("GET", &plan, json!({})).await.1;
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    for (cookie, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.other_cookie.as_str()), StatusCode::NOT_FOUND),
    ] {
        let response = f.send("GET", &endpoint, json!({}), cookie, false).await;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let response = f
        .send("GET", &endpoint, json!({}), Some(&f.cookie), false)
        .await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let (status, preview) = decode(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["protocol_version"], "learning-model-review-v1");
    assert_eq!(preview["input"]["skill_name"], "核验技能");
    assert_eq!(preview["input"]["evidence"]["work"], "独立产物");
    assert_eq!(preview.as_object().unwrap().len(), 3);
    assert_eq!(preview["input"].as_object().unwrap().len(), 4);
    assert_eq!(preview["input"]["input_digest"].as_str().unwrap().len(), 64);
    assert!(!preview.to_string().contains(f.owner.as_str()));
    assert!(!preview.to_string().contains(&evidence_id.to_string()));
    assert_eq!(f.call("GET", &endpoint, json!({})).await.1, preview);
    assert_eq!(f.call("GET", &plan, json!({})).await.1, before);
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1,
        snapshot
    );
    assert_eq!(
        f.call("GET", &format!("{endpoint}?include_notes=true"), json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send("POST", &endpoint, json!({}), Some(&f.cookie), true)
            .await
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        f.call(
            "PUT",
            &format!("/api/learning/skills/{skill}"),
            json!({"revision":"1","name":"新版本","enabled":true,"prerequisite_ids":[]})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.call("GET", &endpoint, json!({})).await.0,
        StatusCode::CONFLICT
    );
    f.cleanup().await;
    let (f, _, path, _) = setup().await;
    let evidence_id = evidence(&f, &path).await;
    f.call("DELETE", &path, json!({"request_id":evidence_id}))
        .await;
    assert_eq!(
        f.call("GET", &format!("{path}/model-preview"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    f.cleanup().await;
}

use super::*;
use personal_ai_domain::{ConversationId, UserId};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("git-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let f = Self(path);
        f.git(&["init", "--quiet", "--initial-branch=main"]);
        f
    }
    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-C",
                self.0.to_str().unwrap(),
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn config(&self, owner: &str) -> String {
        json!([{"owner_id":owner,"repository_id":"project","path":self.0}]).to_string()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn request(value: &serde_json::Value) -> ToolRequest {
    ToolRequest {
        arguments_json: value.to_string(),
    }
}
#[tokio::test]
async fn history_is_owner_scoped_bounded_unicode_and_read_only() {
    let f = Fixture::new();
    f.git(&["commit", "--quiet", "--allow-empty", "-m", "第一条🙂"]);
    f.git(&["commit", "--quiet", "--allow-empty", "-m", "第二条"]);
    let head = std::fs::read(f.0.join(".git/refs/heads/main")).ok();
    let owner = uuid::Uuid::new_v4().to_string();
    let tool = GitLogTool::from_json(&f.config(&owner)).unwrap().unwrap();
    let mut context = ToolContext {
        user_id: UserId::new(owner),
        conversation_id: ConversationId::new("unused"),
    };
    let output = tool
        .execute(
            &context,
            &request(&json!({"repository_id":"project","limit":1})),
        )
        .await
        .unwrap();
    let output: serde_json::Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(output["commits"].as_array().unwrap().len(), 1);
    assert_eq!(output["commits"][0]["subject"], "第二条");
    assert_eq!(output["commits"][0]["commit"].as_str().unwrap().len(), 40);
    assert!(output["commits"][0]["committed_at_unix_seconds"].is_string());
    let full = tool
        .execute(&context, &request(&json!({"repository_id":"project"})))
        .await
        .unwrap();
    assert!(full.content.contains("第一条🙂"));
    assert!(!full.content.contains(f.0.to_str().unwrap()));
    assert_eq!(head, std::fs::read(f.0.join(".git/refs/heads/main")).ok());
    assert!(!f.0.join(".git/index.lock").exists());
    f.git(&["config", "remote.origin.promisor", "true"]);
    f.git(&[
        "config",
        "remote.origin.url",
        "https://invalid.example/private",
    ]);
    assert_eq!(
        tool.execute(&context, &request(&json!({"repository_id":"project"})))
            .await
            .unwrap_err(),
        failed()
    );
    assert!(!f.0.join(".git/FETCH_HEAD").exists());
    context.user_id = UserId::new(uuid::Uuid::new_v4().to_string());
    assert!(matches!(
        tool.execute(&context, &request(&json!({"repository_id":"project"})))
            .await,
        Err(ToolError::PermissionDenied(_))
    ));
}
#[test]
fn configuration_and_arguments_reject_paths_commands_and_duplicate_bindings() {
    assert!(GitLogTool::from_json("[]").unwrap().is_none());
    let f = Fixture::new();
    let owner = uuid::Uuid::new_v4().to_string();
    let config = f.config(&owner);
    let tool = GitLogTool::from_json(&config).unwrap().unwrap();
    let entries: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert!(GitLogTool::from_json(&json!([entries[0], entries[0]]).to_string()).is_err());
    for value in [
        json!({"repository_id":"../private"}),
        json!({"repository_id":"project","path":"/etc"}),
        json!({"repository_id":"project","limit":0}),
        json!({"repository_id":"project","limit":21}),
        json!({"repository_id":"project","revision":"--all"}),
        json!({"repository_id":"project","command":"push"}),
    ] {
        assert!(tool.validate(&request(&value)).is_err());
    }
    assert!(!tool.may_incur_cost());
}
#[tokio::test]
async fn oversized_or_unavailable_history_fails_without_exposing_server_details() {
    let f = Fixture::new();
    let owner = uuid::Uuid::new_v4().to_string();
    let tool = GitLogTool::from_json(&f.config(&owner)).unwrap().unwrap();
    let context = ToolContext {
        user_id: UserId::new(owner),
        conversation_id: ConversationId::new("unused"),
    };
    let query = request(&json!({"repository_id":"project"}));
    assert_eq!(tool.execute(&context, &query).await.unwrap_err(), failed());
    f.git(&[
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        &"文".repeat(1200),
    ]);
    let output = tool.execute(&context, &query).await.unwrap();
    let output: serde_json::Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(
        output["commits"][0]["subject"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1000
    );
    assert_eq!(output["commits"][0]["subject_truncated"], true);
    f.git(&[
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        &"x".repeat(70_000),
    ]);
    assert_eq!(tool.execute(&context, &query).await.unwrap_err(), failed());
}

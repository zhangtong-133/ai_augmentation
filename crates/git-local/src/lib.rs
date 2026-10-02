#![forbid(unsafe_code)]
//! 管理员配置的本地仓库提交历史；不接受路径、修订表达式或任意 Git 参数。
use personal_ai_tools::{BoxFuture, Tool, ToolContext, ToolError, ToolRequest, ToolResponse};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    owner_id: String,
    repository_id: String,
    path: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    repository_id: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    10
}
fn alias(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn invalid() -> ToolError {
    ToolError::InvalidArguments("invalid git log arguments".into())
}
fn failed() -> ToolError {
    ToolError::ExecutionFailed("git history unavailable".into())
}
fn input(request: &ToolRequest) -> Result<Input, ToolError> {
    let input: Input = serde_json::from_str(&request.arguments_json).map_err(|_| invalid())?;
    if !alias(&input.repository_id) || !(1..=20).contains(&input.limit) {
        return Err(invalid());
    }
    Ok(input)
}
pub struct GitLogTool {
    repositories: HashMap<(String, String), PathBuf>,
}
impl GitLogTool {
    /// 只接受已有普通仓库的绝对路径，冻结解析后的 Git 目录。空配置禁用工具。
    /// # Errors
    /// 非法、重复、过大配置或路径不可用时返回脱敏错误。
    pub fn from_json(value: &str) -> Result<Option<Self>, ToolError> {
        let config_error =
            || ToolError::ExecutionFailed("invalid git repository configuration".into());
        if value.len() > 65_536 {
            return Err(config_error());
        }
        let entries: Vec<Binding> = serde_json::from_str(value).map_err(|_| config_error())?;
        if entries.len() > 32 {
            return Err(config_error());
        }
        let mut repositories = HashMap::new();
        for entry in entries {
            let owner = uuid::Uuid::parse_str(&entry.owner_id).map_err(|_| config_error())?;
            if owner.is_nil() || !alias(&entry.repository_id) || !entry.path.is_absolute() {
                return Err(config_error());
            }
            let path = entry
                .path
                .join(".git")
                .canonicalize()
                .map_err(|_| config_error())?;
            if !path.is_dir() || !path.join("HEAD").is_file() || !path.join("objects").is_dir() {
                return Err(config_error());
            }
            if repositories
                .insert((owner.to_string(), entry.repository_id), path)
                .is_some()
            {
                return Err(config_error());
            }
        }
        Ok((!repositories.is_empty()).then_some(Self { repositories }))
    }
}
fn command(path: &std::path::Path) -> Command {
    let mut command = Command::new("/usr/bin/git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "log.showSignature=false",
            "-c",
            "i18n.logOutputEncoding=UTF-8",
            "--git-dir",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}
async fn history(path: &std::path::Path, limit: usize) -> Result<serde_json::Value, ToolError> {
    // Older Git versions may ignore GIT_NO_LAZY_FETCH. Reject any promisor
    // configuration before log can attempt an implicit fetch or write FETCH_HEAD.
    let config = command(path)
        .args([
            "config",
            "--get-regexp",
            "^(extensions\\.partialclone|remote\\..*\\.promisor)$",
        ])
        .stdout(Stdio::null())
        .status()
        .await
        .map_err(|_| failed())?;
    if config.code() != Some(1) {
        return Err(failed());
    }
    let mut child = command(path)
        .args([
            "log",
            "--no-show-signature",
            "--no-decorate",
            "--no-ext-diff",
            "--format=%H%x00%ct%x00%s%x00",
        ])
        .arg(format!("--max-count={limit}"))
        .args(["HEAD", "--"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| failed())?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(failed)?
        .take(65_537)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| failed())?;
    if bytes.len() > 65_536 {
        return Err(failed());
    }
    if !child.wait().await.map_err(|_| failed())?.success() {
        return Err(failed());
    }
    let text = String::from_utf8(bytes).map_err(|_| failed())?;
    let mut fields = text.split('\0');
    let mut commits = Vec::new();
    while let Some(hash) = fields.next() {
        let hash = hash.trim_start_matches('\n');
        if hash.is_empty() {
            break;
        }
        if !matches!(hash.len(), 40 | 64)
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || commits.len() >= limit
        {
            return Err(failed());
        }
        let timestamp = fields
            .next()
            .ok_or_else(failed)?
            .parse::<i64>()
            .map_err(|_| failed())?;
        let subject = fields.next().ok_or_else(failed)?;
        commits.push(json!({"commit":hash,"committed_at_unix_seconds":timestamp.to_string(),"subject":subject.chars().take(1000).collect::<String>(),"subject_truncated":subject.chars().count()>1000}));
    }
    Ok(json!({"commits":commits}))
}
impl Tool for GitLogTool {
    fn name(&self) -> &'static str {
        "git_log"
    }
    fn description(&self) -> &'static str {
        "读取管理员为当前用户授权的本地仓库最近提交；只读、无模型费用，不拉取远程内容。"
    }
    fn may_incur_cost(&self) -> bool {
        false
    }
    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"repository_id":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"},"limit":{"type":"integer","minimum":1,"maximum":20,"default":10}},"required":["repository_id"],"additionalProperties":false}"#
    }
    fn validate(&self, request: &ToolRequest) -> Result<(), ToolError> {
        input(request).map(|_| ())
    }
    fn execute(
        &self,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let input = input(request);
        let owner = context.user_id.to_string();
        Box::pin(async move {
            let input = input?;
            let path = self
                .repositories
                .get(&(owner, input.repository_id.clone()))
                .ok_or_else(|| ToolError::PermissionDenied("repository not available".into()))?;
            let mut output =
                tokio::time::timeout(Duration::from_secs(5), history(path, input.limit))
                    .await
                    .map_err(|_| failed())??;
            output["repository_id"] = json!(input.repository_id);
            Ok(ToolResponse {
                content: output.to_string(),
                is_error: false,
            })
        })
    }
}

#[cfg(test)]
mod tests;

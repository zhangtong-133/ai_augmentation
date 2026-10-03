//! Local, explicit `ChatGPT` subscription connection utility; no API server credentials.
#[cfg(unix)]
mod binding;
#[cfg(unix)]
mod callback;
#[cfg(unix)]
mod store;

#[cfg(unix)]
use personal_ai_llm_openai::chatgpt::{ChatGptClient, Error, PendingLogin, Result};

#[cfg(unix)]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(not(unix))]
fn main() {
    eprintln!("chatgpt-connect currently requires Unix owner-only credential storage");
    std::process::exit(1);
}

#[cfg(unix)]
async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "ChatGPT 本地订阅接入（不需要 API Key）\n\
用法：chatgpt-connect DIRECTORY login LABEL\n\
      chatgpt-connect DIRECTORY status\n\
      chatgpt-connect DIRECTORY logout LABEL\n\
      chatgpt-connect DIRECTORY models LABEL\n\
      chatgpt-connect DIRECTORY ask LABEL MODEL --use-subscription\n\
      chatgpt-connect DIRECTORY bind LABEL USER_UUID CONNECTION_UUID REVISION\n\
      chatgpt-connect DIRECTORY connections USER_UUID [AFTER_UUID]\n\
      chatgpt-connect DIRECTORY revoke-binding USER_UUID CONNECTION_UUID REVISION\n\
ask 从标准输入读取提示词，并发送给 OpenAI，消耗所选账户的订阅额度或设置允许的 credits。\n\
DIRECTORY 应使用项目外的私有目录；每个 LABEL 对应独立账户/工作区。\n\
管理授权与用量：https://chatgpt.com/settings/usage"
        );
        return Ok(());
    }
    if binding::is_command(&args) {
        return binding::run(&args).await;
    }
    let valid = match args.get(1).map(String::as_str) {
        Some("status") => args.len() == 2,
        Some("login" | "logout" | "models") => args.len() == 3,
        Some("ask") => args.len() == 5 && args[4] == "--use-subscription",
        _ => false,
    };
    if !valid {
        return Err(Error("invalid arguments; see --help"));
    }
    if let Some(label) = args.get(2) {
        store::check_label(label)?;
    }
    let mut store = store::Store::open(std::path::Path::new(&args[0]))?;
    if args[1] == "status" {
        for (label, r) in &store.data.accounts {
            println!(
                "{}",
                serde_json::json!({"label":label,"email":r.email,"signed_in":r.signed_in(),"plan_permission":r.plan_enabled()})
            );
        }
        println!("登录和模型列表不代表调用已验证；只有 ask 收到完整响应才验证本次可用。");
        return Ok(());
    }
    let label = &args[2];
    let client = ChatGptClient::new()?;
    if args[1] == "login" {
        return login(&client, &mut store, label).await;
    }
    if args[1] == "logout" {
        let r = store
            .data
            .accounts
            .get_mut(label)
            .ok_or(Error("unknown account label"))?;
        let result = client.sign_out(r).await;
        store.save()?;
        println!("本地凭据已清除，保留账户注册信息供再次登录。");
        return result
            .map_err(|_| Error("remote revocation unconfirmed; disconnect in ChatGPT Settings"));
    }
    inference(&client, &mut store, &args).await
}

#[cfg(unix)]
async fn inference(
    client: &ChatGptClient,
    store: &mut store::Store,
    args: &[String],
) -> Result<()> {
    use std::io::Read;
    let label = &args[2];
    let prompt = if args[1] == "ask" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(32769)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("cannot read prompt"))?;
        if bytes.len() > 32768 || bytes.is_empty() {
            return Err(Error("prompt must contain 1..32768 bytes"));
        }
        Some(String::from_utf8(bytes).map_err(|_| Error("prompt must be UTF-8"))?)
    } else {
        None
    };
    let registration = store
        .data
        .accounts
        .get_mut(label)
        .ok_or(Error("unknown account label"))?;
    if !registration.plan_enabled() {
        return Err(Error("ChatGPT plan permission not granted; sign in first"));
    }
    if registration.needs_refresh()? {
        client.refresh(registration).await?;
        store.save()?;
    }
    let registration = store
        .data
        .accounts
        .get(label)
        .ok_or(Error("unknown account label"))?;
    let models = client.models(registration).await?;
    if let Some(prompt) = prompt {
        if !models.iter().any(|m| m.slug == args[3]) {
            return Err(Error("selected model is not in this account's catalog"));
        }
        eprintln!(
            "正在使用账户 {} 的订阅额度发送一次请求。",
            serde_json::json!(label)
        );
        let text = client.ask(registration, &args[3], &prompt, true).await?;
        let safe: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        println!("{safe}");
    } else {
        for model in models {
            println!(
                "{}",
                serde_json::json!({"model":model.slug,"name":model.display_name})
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn login(client: &ChatGptClient, store: &mut store::Store, label: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| Error("loopback listener unavailable"))?;
    let port = listener
        .local_addr()
        .map_err(|_| Error("loopback listener unavailable"))?
        .port();
    let (pending, url) =
        PendingLogin::new(&store.data.host_id, port, store.data.accounts.get(label))?;
    println!(
        "Continue with ChatGPT\n在本机浏览器打开以下地址并确认是否允许使用订阅额度（5 分钟内有效）：\n{url}"
    );
    let exchange = callback::receive(listener, pending).await?;
    // Retain new issued client IDs even if code exchange fails. Existing valid credentials survive.
    if !store.data.accounts.contains_key(label) {
        if store
            .data
            .accounts
            .values()
            .any(|r| r.client_id == exchange.registration.client_id)
        {
            return Err(Error("registration already saved under another label"));
        }
        let registration = exchange.registration.clone();
        store.data.accounts.insert(label.to_owned(), registration);
        store.save()?;
    }
    let registration = client
        .exchange(exchange, store.data.accounts.get(label))
        .await?;
    let enabled = registration.plan_enabled();
    store.data.accounts.insert(label.to_owned(), registration);
    store.save()?;
    println!(
        "登录成功。订阅使用权限：{enabled}。尚未发起模型调用。\n管理用量：https://chatgpt.com/settings/usage"
    );
    Ok(())
}

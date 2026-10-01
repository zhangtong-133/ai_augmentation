use personal_ai_mcp::{bridge::Bridge, protocol::Session};
use std::io::{BufRead, Read, Write};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), &'static str> {
    let base = match std::env::var("MCP_API_URL") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "http://127.0.0.1:8080".into(),
        Err(_) => return Err("invalid MCP_API_URL"),
    };
    let token = std::env::var("MCP_SESSION_TOKEN").map_err(|_| "MCP_SESSION_TOKEN is required")?;
    let enabled = match std::env::var("MCP_ALLOW_EMBEDDING_COST") {
        Err(std::env::VarError::NotPresent) => false,
        Ok(v) if v == "0" => false,
        Ok(v) if v == "1" => true,
        _ => return Err("MCP_ALLOW_EMBEDDING_COST must be 0 or 1"),
    };
    let mut session = Session::new(Bridge::new(&base, &token, enabled)?);
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    loop {
        let mut line = Vec::new();
        let count = (&mut input)
            .take(16_385)
            .read_until(b'\n', &mut line)
            .map_err(|_| "MCP input unavailable")?;
        if count == 0 {
            break;
        }
        if count > 16_384 {
            return Err("MCP input frame exceeds 16 KiB");
        }
        if let Some(reply) = session.handle(&line).await {
            serde_json::to_writer(&mut output, &reply).map_err(|_| "MCP output unavailable")?;
            output
                .write_all(b"\n")
                .map_err(|_| "MCP output unavailable")?;
            output.flush().map_err(|_| "MCP output unavailable")?;
        }
    }
    Ok(())
}

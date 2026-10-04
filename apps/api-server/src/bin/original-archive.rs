//! 管理员原文归档桥接：凭据只从环境读取，SDK 留在适配器。
use personal_ai_storage::StorageError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const LIMIT: u64 = 5 * 1024 * 1024;

fn managed(key: &str) -> bool {
    let parts: Vec<_> = key.split('/').collect();
    parts.len() == 5
        && parts[0] == "users"
        && parts[2] == "documents"
        && [parts[1], parts[3], parts[4]]
            .iter()
            .all(|s| uuid::Uuid::parse_str(s).is_ok_and(|id| id.to_string() == *s))
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2
        || !matches!(args[0].as_str(), "read" | "check" | "create")
        || !managed(&args[1])
    {
        return Err("usage: original-archive read|check|create MANAGED_KEY".into());
    }
    let objects =
        api_server::object_storage_from_env()?.ok_or("explicit object storage required")?;
    match args[0].as_str() {
        "read" => {
            std::io::stdout().write_all(&objects.get(&args[1]).await?)?;
        }
        "check" => match objects.get(&args[1]).await {
            Ok(bytes) => println!(
                "{}",
                serde_json::json!({"exists":true,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))})
            ),
            Err(StorageError::NotFound) => println!("{}", serde_json::json!({"exists":false})),
            Err(error) => return Err(error.into()),
        },
        "create" => {
            let mut bytes = Vec::new();
            std::io::stdin().take(LIMIT + 1).read_to_end(&mut bytes)?;
            if bytes.is_empty() || bytes.len() as u64 > LIMIT {
                return Err("invalid original size".into());
            }
            objects.put_new(&args[1], &bytes, None).await?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "原文归档操作失败；请核对参数、显式配置、对象是否存在及文件大小；未输出凭据或原文"
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_canonical_application_original_keys_are_accepted() {
        let id = "00000000-0000-4000-8000-000000000001";
        assert!(managed(&format!("users/{id}/documents/{id}/{id}")));
        for key in [
            "users/u/documents/d/o",
            "../original",
            "users//documents/d/o",
            "other/key",
        ] {
            assert!(!managed(key));
        }
    }
}

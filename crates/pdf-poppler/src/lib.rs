//! 通过一次性 Poppler 子进程执行有资源限制的文本提取。
#![forbid(unsafe_code)]

use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
};

pub const MAX_PDF_BYTES: usize = 5 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
static SLOTS: Semaphore = Semaphore::const_new(2);

#[derive(Debug, PartialEq, Eq)]
pub enum PdfError {
    Invalid,
    TooLarge,
    Empty,
    Unavailable,
    Busy,
    Timeout,
}

/// 提取 UTF-8 文本，不打开链接、运行脚本或写入文件。
///
/// # Errors
/// PDF 损坏或加密、正文为空、输入输出超限或并发过载时返回错误。
/// Linux 运行环境的 PATH 中必须包含 `pdftotext` 和 `prlimit`。
pub async fn extract(bytes: &[u8]) -> Result<String, PdfError> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(PdfError::TooLarge);
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(PdfError::Invalid);
    }
    let _slot = SLOTS.try_acquire().map_err(|_| PdfError::Busy)?;
    let mut child = Command::new("prlimit")
        .args([
            "--as=536870912",
            "--cpu=5",
            "--",
            "pdftotext",
            "-enc",
            "UTF-8",
            "-nopgbrk",
            "-",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| PdfError::Unavailable)?;
    let mut stdin = child.stdin.take().ok_or(PdfError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(PdfError::Unavailable)?;
    let operation = async {
        let write = async {
            // 即使缺少可执行文件导致标准输入提前关闭，也要等待退出状态。
            let result = stdin.write_all(bytes).await;
            drop(stdin);
            Ok::<_, PdfError>(result)
        };
        let read = async {
            let mut output = Vec::new();
            stdout
                .take((MAX_TEXT_BYTES + 1) as u64)
                .read_to_end(&mut output)
                .await
                .map_err(|_| PdfError::Invalid)?;
            if output.len() > MAX_TEXT_BYTES {
                return Err(PdfError::TooLarge);
            }
            Ok(output)
        };
        let (written, output) = tokio::try_join!(write, read)?;
        let status = child.wait().await.map_err(|_| PdfError::Unavailable)?;
        if status.code() == Some(127) {
            return Err(PdfError::Unavailable);
        }
        if !status.success() {
            return Err(PdfError::Invalid);
        }
        written.map_err(|_| PdfError::Invalid)?;
        let text = String::from_utf8(output).map_err(|_| PdfError::Invalid)?;
        if text.contains('\0') {
            return Err(PdfError::Invalid);
        }
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(PdfError::Empty);
        }
        Ok(text)
    };
    let result = tokio::time::timeout(Duration::from_secs(7), operation)
        .await
        .unwrap_or(Err(PdfError::Timeout));
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rejects_invalid_and_oversized_input_before_spawning() {
        assert_eq!(extract(b"not a pdf").await, Err(PdfError::Invalid));
        assert_eq!(
            extract(&vec![0; MAX_PDF_BYTES + 1]).await,
            Err(PdfError::TooLarge)
        );
    }
}

//! Bounded text extraction through a disposable Poppler subprocess.
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

/// Extract UTF-8 text without opening links, running scripts, or writing files.
///
/// # Errors
/// Rejects malformed/locked PDFs, empty text, oversized input/output and overload.
/// Requires `pdftotext` and `prlimit` on PATH (Linux runtime).
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
            // Wait for exit status even if a missing executable closes stdin early.
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

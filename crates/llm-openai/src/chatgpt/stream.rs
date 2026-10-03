use super::{Error, Result};
use serde_json::Value;

// Byte buffering preserves UTF-8 even when transport chunks split a code point.
#[derive(Default)]
pub(super) struct TextStream {
    line: Vec<u8>,
    event: Vec<u8>,
    text: String,
    total: usize,
    previous_cr: bool,
}
impl TextStream {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<String>> {
        self.total = self.total.saturating_add(bytes.len());
        if self.total > 4 * 1024 * 1024 {
            return Err(Error("inference stream too large"));
        }
        for &byte in bytes {
            if byte == b'\n' && self.previous_cr {
                self.previous_cr = false;
                continue;
            }
            self.previous_cr = byte == b'\r';
            if byte == b'\n' || byte == b'\r' {
                if self.line.is_empty() {
                    if let Some(text) = self.dispatch()? {
                        return Ok(Some(text));
                    }
                } else if let Some(data) = self.line.strip_prefix(b"data:") {
                    let data = data.strip_prefix(b" ").unwrap_or(data);
                    self.event.extend_from_slice(data);
                    self.event.push(b'\n');
                }
                self.line.clear();
            } else {
                self.line.push(byte);
            }
            if self.line.len() + self.event.len() > 1024 * 1024 {
                return Err(Error("inference event too large"));
            }
        }
        Ok(None)
    }
    fn dispatch(&mut self) -> Result<Option<String>> {
        if self.event.is_empty() {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_slice(&self.event).map_err(|_| Error("invalid inference event"))?;
        self.event.clear();
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(Error("invalid text delta"))?;
                if self.text.len() + delta.len() > 128 * 1024 {
                    return Err(Error("model output too large"));
                }
                self.text.push_str(delta);
            }
            Some("response.completed") => {
                if value.pointer("/response/status").and_then(Value::as_str) != Some("completed") {
                    return Err(Error("invalid completion event"));
                }
                return Ok(Some(std::mem::take(&mut self.text)));
            }
            Some("response.failed" | "response.incomplete" | "error") => {
                let code = value
                    .pointer("/response/error/code")
                    .or_else(|| value.get("code"))
                    .and_then(Value::as_str);
                return Err(Error(match code {
                    Some("subscription_sharing_usage_limit_exceeded") => {
                        "ChatGPT subscription usage limit reached"
                    }
                    Some("subscription_sharing_usage_unavailable") => {
                        "ChatGPT subscription usage unavailable"
                    }
                    _ => "inference failed or incomplete; not retried",
                }));
            }
            Some(_) => (),
            None => return Err(Error("missing inference event type")),
        }
        Ok(None)
    }
}

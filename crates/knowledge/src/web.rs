use personal_ai_storage::BoxFuture;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebPage {
    pub title: String,
    pub source: String,
    pub text: String,
    pub html: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebImportError {
    InvalidUrl,
    Blocked,
    Unavailable,
    Timeout,
    TooLarge,
    Unsupported,
    Empty,
    Busy,
    Redirect,
}

/// Fetch public HTML without forwarding any application credentials.
pub trait WebImporter: Send + Sync {
    fn import(&self, url: &str) -> BoxFuture<'_, Result<WebPage, WebImportError>>;
}

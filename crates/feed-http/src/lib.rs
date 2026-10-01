//! RSS 的固定 HTTPS 单次 GET 适配器；没有运行时内网白名单或 TLS 绕过开关。
#![forbid(unsafe_code)]
use personal_ai_feeds::{
    normalize_source,
    parser::MAX_BYTES,
    transport::{FeedTransport, FetchError as Error, FetchFuture},
};
use personal_ai_public_network::public_ip;
use reqwest::{Client, ClientBuilder, Response, header, redirect::Policy};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::Semaphore;
use url::Url;

const DEADLINE: Duration = Duration::from_secs(8);
static SLOTS: Semaphore = Semaphore::const_new(2);

#[derive(Default)]
pub struct PublicFeedTransport;
impl FeedTransport for PublicFeedTransport {
    fn fetch(&self, source: &str) -> FetchFuture<'_, Result<Vec<u8>, Error>> {
        let source = source.to_owned();
        Box::pin(async move {
            let _permit = SLOTS.try_acquire().map_err(|_| Error::Busy)?;
            bounded_fetch(&source, &SystemDns).await
        })
    }
}

trait Resolver: Sync {
    fn resolve(&self, host: &str) -> FetchFuture<'_, Result<Vec<SocketAddr>, Error>>;
}
struct SystemDns;
impl Resolver for SystemDns {
    fn resolve(&self, host: &str) -> FetchFuture<'_, Result<Vec<SocketAddr>, Error>> {
        let host = host.to_owned();
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), 443))
                .await
                .map_err(|_| Error::Unavailable)?
                .take(33)
                .collect())
        })
    }
}

fn checked_address(addresses: &[SocketAddr]) -> Result<SocketAddr, Error> {
    if addresses.is_empty() {
        return Err(Error::Unavailable);
    }
    if addresses.len() > 32
        || addresses
            .iter()
            .any(|a| a.port() != 443 || !public_ip(a.ip()))
    {
        return Err(Error::Blocked);
    }
    // 所有返回值先校验，只连接第一个；不进行备用地址重试或第二次 DNS 解析。
    Ok(addresses[0])
}

fn client_builder(url: &Url, address: SocketAddr) -> Result<ClientBuilder, Error> {
    Ok(Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .http1_only()
        .connect_timeout(Duration::from_secs(3))
        .timeout(DEADLINE)
        .resolve_to_addrs(url.host_str().ok_or(Error::InvalidSource)?, &[address])
        .user_agent("PersonalAI-RSS/0.1"))
}

async fn bounded_fetch(source: &str, resolver: &dyn Resolver) -> Result<Vec<u8>, Error> {
    tokio::time::timeout(DEADLINE, fetch(source, resolver))
        .await
        .map_err(|_| Error::Timeout)?
}

async fn fetch(source: &str, resolver: &dyn Resolver) -> Result<Vec<u8>, Error> {
    let source = normalize_source(source).map_err(|_| Error::InvalidSource)?;
    let url = Url::parse(&source).map_err(|_| Error::InvalidSource)?;
    let addresses = resolver
        .resolve(url.host_str().ok_or(Error::InvalidSource)?)
        .await?;
    let address = checked_address(&addresses)?;
    let client = client_builder(&url, address)?
        .build()
        .map_err(|_| Error::Unavailable)?;
    send(&client, url).await
}

async fn send(client: &Client, url: Url) -> Result<Vec<u8>, Error> {
    let response = client
        .get(url)
        .header(
            header::ACCEPT,
            "application/rss+xml, application/xml, text/xml",
        )
        .header(header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| request_error(&e))?;
    read(response).await
}
fn request_error(error: &reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Unavailable
    }
}

fn headers_supported(headers: &header::HeaderMap) -> Result<(), Error> {
    let types: Vec<_> = headers.get_all(header::CONTENT_TYPE).iter().collect();
    if types.len() != 1 {
        return Err(Error::Unsupported);
    }
    let text = types[0].to_str().map_err(|_| Error::Unsupported)?;
    let mut parts = text.split(';');
    let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    if !matches!(
        media.as_str(),
        "application/rss+xml" | "application/xml" | "text/xml"
    ) {
        return Err(Error::Unsupported);
    }
    let mut charset = false;
    for parameter in parts {
        let (key, value) = parameter.trim().split_once('=').ok_or(Error::Unsupported)?;
        // 首版拒绝未知或重复参数以及非 UTF-8 声明。
        if !key.trim().eq_ignore_ascii_case("charset") || charset {
            return Err(Error::Unsupported);
        }
        charset = true;
        let value = value.trim();
        let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !value.eq_ignore_ascii_case("utf-8") {
            return Err(Error::Unsupported);
        }
    }
    for value in headers.get_all(header::CONTENT_ENCODING) {
        if !value
            .to_str()
            .is_ok_and(|s| s.trim().eq_ignore_ascii_case("identity"))
        {
            return Err(Error::Unsupported);
        }
    }
    if headers.contains_key(header::CONTENT_RANGE) {
        return Err(Error::Unsupported);
    }
    Ok(())
}

async fn read(mut response: Response) -> Result<Vec<u8>, Error> {
    if response.status().is_redirection() {
        return Err(Error::Redirect);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(Error::Unsupported);
    }
    headers_supported(response.headers())?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| request_error(&e))? {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    std::str::from_utf8(&bytes).map_err(|_| Error::Unsupported)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;

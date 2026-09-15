//! Public-web fetching with per-hop address validation and pinned DNS results.
#![forbid(unsafe_code)]

use personal_ai_knowledge::web::{WebImportError as Error, WebImporter, WebPage};
use personal_ai_storage::BoxFuture;
use reqwest::{Client, Response, header, redirect::Policy};
use scraper::{ElementRef, Html, Node, Selector};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use url::{Host, Url};

const MAX_HTML: usize = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(8);

pub struct PublicWebImporter {
    slots: Arc<Semaphore>,
}
impl Default for PublicWebImporter {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
        }
    }
}
impl WebImporter for PublicWebImporter {
    fn import(&self, input: &str) -> BoxFuture<'_, Result<WebPage, Error>> {
        let input = input.to_owned();
        let slots = self.slots.clone();
        Box::pin(async move {
            let url = validate_url(&input)?;
            let permit = slots.try_acquire_owned().map_err(|_| Error::Busy)?;
            tokio::time::timeout(DEADLINE, async move {
                let (url, html) = fetch_html(url).await?;
                // The permit remains held if a cancelled request leaves parsing running.
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    extract_html(&url, html)
                })
                .await
                .map_err(|_| Error::Unavailable)?
            })
            .await
            .map_err(|_| Error::Timeout)?
        })
    }
}

#[allow(clippy::case_sensitive_file_extension_comparisons)] // URL-normalized DNS names, not file extensions.
fn validate_url(input: &str) -> Result<Url, Error> {
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err(Error::InvalidUrl);
    }
    let mut url = Url::parse(input).map_err(|_| Error::InvalidUrl)?;
    let port = match url.scheme() {
        "http" => 80,
        "https" => 443,
        _ => return Err(Error::InvalidUrl),
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(port)
    {
        return Err(Error::InvalidUrl);
    }
    match url.host().ok_or(Error::InvalidUrl)? {
        Host::Ipv4(ip) if !public_ip(ip.into()) => return Err(Error::Blocked),
        Host::Ipv6(ip) if !public_ip(ip.into()) => return Err(Error::Blocked),
        Host::Domain(host) => {
            let host = host.trim_end_matches('.');
            if !host.contains('.')
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
            {
                return Err(Error::Blocked);
            }
        }
        _ => {}
    }
    url.set_fragment(None);
    if url.as_str().len() > 2048 {
        return Err(Error::InvalidUrl);
    }
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Permit ordinary global unicast only; exclude transition/special/documentation ranges.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), Error> {
    if addresses.is_empty() {
        return Err(Error::Unavailable);
    }
    if addresses.len() > 32 || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(Error::Blocked);
    }
    Ok(())
}

fn client_for(url: &Url, addresses: &[SocketAddr]) -> Result<Client, Error> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(3))
        .timeout(DEADLINE)
        .resolve_to_addrs(url.host_str().ok_or(Error::InvalidUrl)?, addresses)
        .user_agent("PersonalAI-WebImport/0.1")
        .build()
        .map_err(|_| Error::Unavailable)
}

async fn fetch_html(mut url: Url) -> Result<(Url, String), Error> {
    for hop in 0..=3 {
        let host = url.host_str().ok_or(Error::InvalidUrl)?;
        let addresses: Vec<_> = match url.host().ok_or(Error::InvalidUrl)? {
            Host::Ipv4(ip) => vec![SocketAddr::new(
                ip.into(),
                url.port_or_known_default().unwrap_or(80),
            )],
            Host::Ipv6(ip) => vec![SocketAddr::new(
                ip.into(),
                url.port_or_known_default().unwrap_or(80),
            )],
            Host::Domain(_) => {
                tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(80)))
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .take(33)
                    .collect()
            }
        };
        validate_addresses(&addresses)?;
        // No proxy, fresh client per hop, and only these checked addresses can be dialled.
        let response = client_for(&url, &addresses)?
            .get(url.clone())
            .header(header::ACCEPT, "text/html")
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|error| request_error(&error))?;
        if let Some(next) = redirect_target(&url, &response)? {
            if hop == 3 {
                return Err(Error::Redirect);
            }
            url = next;
            continue;
        }
        return Ok((url, read_html(response).await?));
    }
    Err(Error::Redirect)
}

fn redirect_target(url: &Url, response: &Response) -> Result<Option<Url>, Error> {
    if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Redirect)?;
    let next = url.join(location).map_err(|_| Error::InvalidUrl)?;
    let next = validate_url(next.as_str())?;
    if url.scheme() == "https" && next.scheme() == "http" {
        return Err(Error::Blocked);
    }
    Ok(Some(next))
}

fn request_error(error: &reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Unavailable
    }
}

async fn read_html(mut response: Response) -> Result<String, Error> {
    if !response.status().is_success() {
        return Err(Error::Unavailable);
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Unsupported)?;
    let mut parts = content_type.split(';');
    if !parts
        .next()
        .is_some_and(|part| part.trim().eq_ignore_ascii_case("text/html"))
    {
        return Err(Error::Unsupported);
    }
    for part in parts {
        if let Some((key, value)) = part.split_once('=')
            && key.trim().eq_ignore_ascii_case("charset")
            && !matches!(
                value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                "utf-8" | "utf8" | "us-ascii"
            )
        {
            return Err(Error::Unsupported);
        }
    }
    if response
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value != "identity")
    {
        return Err(Error::Unsupported);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTML as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| request_error(&error))?
    {
        if bytes.len() + chunk.len() > MAX_HTML {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| Error::Unsupported)
}

fn extract_html(url: &Url, html: String) -> Result<WebPage, Error> {
    if html.contains('\0') || html.bytes().filter(|byte| *byte == b'<').count() > 20_000 {
        return Err(Error::TooLarge);
    }
    let document = Html::parse_document(&html);
    let title_selector = Selector::parse("title").map_err(|_| Error::Unavailable)?;
    let title = document
        .select(&title_selector)
        .next()
        .map(|element| element.text().collect::<String>())
        .unwrap_or_default();
    let title: String = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    let root = ["article", "main", "body"]
        .into_iter()
        .find_map(|selector| {
            let selector = Selector::parse(selector).ok()?;
            document.select(&selector).find(|element| {
                !excluded(*element)
                    && !element
                        .ancestors()
                        .filter_map(ElementRef::wrap)
                        .any(excluded)
            })
        })
        .ok_or(Error::Empty)?;
    let mut output = String::new();
    let mut stack = vec![(*root, false)];
    while let Some((node, closing)) = stack.pop() {
        if let Some(element) = ElementRef::wrap(node) {
            if excluded(element) {
                continue;
            }
            let block = matches!(
                element.value().name(),
                "p" | "div"
                    | "section"
                    | "article"
                    | "main"
                    | "br"
                    | "li"
                    | "tr"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "pre"
                    | "blockquote"
            );
            if block {
                output.push_str("\n\n");
            }
            if !closing {
                stack.push((node, true));
                stack.extend(node.children().rev().map(|child| (child, false)));
            }
        } else if let Node::Text(text) = node.value() {
            output.push_str(text);
        }
    }
    let text = output
        .split("\n\n")
        .map(|part| part.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        return Err(Error::Empty);
    }
    if text.len() > MAX_HTML {
        return Err(Error::TooLarge);
    }
    Ok(WebPage {
        title,
        source: url.to_string(),
        text,
        html,
    })
}

fn excluded(element: ElementRef<'_>) -> bool {
    matches!(
        element.value().name(),
        "head"
            | "script"
            | "style"
            | "noscript"
            | "template"
            | "svg"
            | "canvas"
            | "iframe"
            | "object"
            | "embed"
            | "form"
            | "nav"
            | "header"
            | "footer"
            | "aside"
    ) || element.value().attr("hidden").is_some()
        || element
            .value()
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests;

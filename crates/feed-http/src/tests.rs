use super::*;
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};

const ROOT: &[u8] = include_bytes!("../tests/fixtures/root.der");
const CERT: &[u8] = include_bytes!("../tests/fixtures/cert.der");
const KEY: &[u8] = include_bytes!("../tests/fixtures/key.der");

struct Dns {
    addresses: Vec<SocketAddr>,
    calls: AtomicUsize,
}
impl Resolver for Dns {
    fn resolve(&self, _host: &str) -> FetchFuture<'_, Result<Vec<SocketAddr>, Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(self.addresses.clone()) })
    }
}
#[tokio::test]
async fn rejects_invalid_sources_before_dns_and_mixed_or_oversized_answers_before_connect() {
    let dns = Dns {
        addresses: vec!["8.8.8.8:443".parse().unwrap()],
        calls: AtomicUsize::new(0),
    };
    for url in [
        "http://example.com/",
        "https://127.1/",
        "https://user:pass@example.com/",
        "https://example.com:8443/",
        "https://example.internal/",
        "https://[::ffff:127.0.0.1]/",
    ] {
        assert_eq!(fetch(url, &dns).await, Err(Error::InvalidSource));
    }
    assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
    for addresses in [
        vec!["127.0.0.1:443".parse().unwrap()],
        vec![
            "8.8.8.8:443".parse().unwrap(),
            "10.0.0.1:443".parse().unwrap(),
        ],
        vec!["8.8.8.8:443".parse().unwrap(); 33],
        vec!["8.8.8.8:80".parse().unwrap()],
    ] {
        let dns = Dns {
            addresses,
            calls: AtomicUsize::new(0),
        };
        assert_eq!(
            fetch("https://example.com/rss", &dns).await,
            Err(Error::Blocked)
        );
        assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
    }
    assert_eq!(checked_address(&[]), Err(Error::Unavailable));
    let addresses = vec!["8.8.8.8:443".parse().unwrap(); 32];
    assert_eq!(checked_address(&addresses), Ok(addresses[0]));
}

struct HangingDns;
impl Resolver for HangingDns {
    fn resolve(&self, _: &str) -> FetchFuture<'_, Result<Vec<SocketAddr>, Error>> {
        Box::pin(pending())
    }
}
#[tokio::test(start_paused = true)]
async fn total_deadline_includes_dns() {
    assert_eq!(
        bounded_fetch("https://example.com/rss", &HangingDns).await,
        Err(Error::Timeout)
    );
}

fn acceptor() -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(CERT.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}
// 仅测试可通过私有 client_builder 注入回环端点和测试根证书，生产入口没有注入参数。
async fn fixture(
    response: Vec<u8>,
    trust: bool,
    wrong_host: bool,
) -> (Url, Client, tokio::task::JoinHandle<(String, usize)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = if wrong_host {
        "wrong.example"
    } else {
        "fixture.example"
    };
    let url = Url::parse(&format!(
        "https://{host}:{}/rss?private=yes",
        address.port()
    ))
    .unwrap();
    let mut builder = client_builder(&url, address).unwrap();
    if trust {
        builder = builder.add_root_certificate(reqwest::Certificate::from_der(ROOT).unwrap());
    }
    let client = builder.build().unwrap();
    let server = acceptor();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        if let Ok(mut stream) = server.accept(stream).await {
            loop {
                let mut buf = [0; 1024];
                let size = stream.read(&mut buf).await.unwrap_or(0);
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..size]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        }
        let extra = usize::from(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_ok(),
        );
        (String::from_utf8(request).unwrap(), 1 + extra)
    });
    (url, client, task)
}
fn response(headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes =
        format!("HTTP/1.1 200 OK\r\nConnection: close\r\n{headers}\r\n\r\n").into_bytes();
    bytes.extend(body);
    bytes
}

#[tokio::test]
async fn pinned_tls_get_preserves_host_without_credentials_or_extra_requests() {
    let (url, client, server) = fixture(
        response(
            "Content-Type: application/rss+xml; charset=\"UTF-8\"",
            b"<rss/>",
        ),
        true,
        false,
    )
    .await;
    assert_eq!(send(&client, url).await.unwrap(), b"<rss/>");
    let (request, count) = server.await.unwrap();
    let request = request.to_ascii_lowercase();
    assert!(request.starts_with("get /rss?private=yes http/1.1\r\n"));
    assert!(request.contains("host: fixture.example:"));
    assert!(request.contains("accept-encoding: identity\r\n"));
    for header in [
        "authorization:",
        "cookie:",
        "referer:",
        "if-none-match:",
        "if-modified-since:",
        "proxy-authorization:",
    ] {
        assert!(!request.contains(header));
    }
    assert_eq!(count, 1);
}
#[tokio::test]
async fn tls_rejects_untrusted_certificates_and_hostname_mismatch() {
    for (trust, wrong) in [(false, false), (true, true)] {
        let (url, client, server) =
            fixture(response("Content-Type: text/xml", b"<rss/>"), trust, wrong).await;
        assert_eq!(send(&client, url).await, Err(Error::Unavailable));
        let (request, count) = server.await.unwrap();
        assert_eq!(request, "");
        assert_eq!(count, 1);
    }
}
#[tokio::test]
async fn redirects_server_errors_and_connection_failures_are_never_retried() {
    for (status, expected) in [
        ("302 Found", Error::Redirect),
        ("304 Not Modified", Error::Redirect),
        ("503 Unavailable", Error::Unsupported),
        ("206 Partial Content", Error::Unsupported),
    ] {
        let bytes=format!("HTTP/1.1 {status}\r\nLocation: https://127.0.0.1/private\r\nContent-Type: text/xml\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
        let (url, client, server) = fixture(bytes, true, false).await;
        assert_eq!(send(&client, url).await, Err(expected));
        assert_eq!(server.await.unwrap().1, 1);
    }
    let (url, client, server) = fixture(Vec::new(), true, false).await;
    assert_eq!(send(&client, url).await, Err(Error::Unavailable));
    assert_eq!(server.await.unwrap().1, 1);
}
#[tokio::test]
async fn rejects_compression_ambiguous_headers_bad_utf8_and_oversized_bodies() {
    for (headers, body, expected) in [
        (
            "Content-Type: application/rss+xml\r\nContent-Encoding: gzip",
            b"compressed".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/xml\r\nContent-Encoding: identity\r\nContent-Encoding: br",
            b"compressed".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/html",
            b"html".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/xml; charset=UTF-16",
            b"x".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/xml; charset=UTF-8; charset=UTF-8",
            b"x".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/xml\r\nContent-Type: application/xml",
            b"x".to_vec(),
            Error::Unsupported,
        ),
        ("Content-Type: text/xml", vec![0xff], Error::Unsupported),
        (
            "Content-Type: text/xml\r\nContent-Length: 1048577",
            Vec::new(),
            Error::TooLarge,
        ),
        (
            "Content-Type: text/xml",
            vec![b'x'; MAX_BYTES + 1],
            Error::TooLarge,
        ),
    ] {
        let (url, client, server) = fixture(response(headers, &body), true, false).await;
        assert_eq!(send(&client, url).await, Err(expected));
        server.await.unwrap();
    }
}
#[tokio::test]
async fn exact_limit_is_allowed_but_chunked_and_truncated_responses_fail_closed() {
    let (url, client, server) = fixture(
        response("Content-Type: application/xml", &vec![b'x'; MAX_BYTES]),
        true,
        false,
    )
    .await;
    assert_eq!(send(&client, url).await.unwrap().len(), MAX_BYTES);
    server.await.unwrap();
    let mut chunked = b"100001\r\n".to_vec();
    chunked.extend(vec![b'x'; MAX_BYTES + 1]);
    chunked.extend(b"\r\n0\r\n\r\n");
    let (url, client, server) = fixture(
        response(
            "Content-Type: text/xml\r\nTransfer-Encoding: chunked",
            &chunked,
        ),
        true,
        false,
    )
    .await;
    assert_eq!(send(&client, url).await, Err(Error::TooLarge));
    server.await.unwrap();
    let (url, client, server) = fixture(
        response("Content-Type: text/xml\r\nContent-Length: 100", b"short"),
        true,
        false,
    )
    .await;
    assert_eq!(send(&client, url).await, Err(Error::Unavailable));
    server.await.unwrap();
}

#[tokio::test]
async fn transport_instances_share_process_capacity() {
    let _first = SLOTS.try_acquire().unwrap();
    let _second = SLOTS.try_acquire().unwrap();
    let a = PublicFeedTransport;
    let b = PublicFeedTransport;
    assert_eq!(a.fetch("https://example.com/rss").await, Err(Error::Busy));
    assert_eq!(b.fetch("https://example.com/rss").await, Err(Error::Busy));
}

use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn rejects_local_special_encoded_addresses_and_credentials() {
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/file",
        "http://user:pass@example.com/",
        "http://example.com:8080/",
        "https://example.com:80/",
        "http://localhost/",
        "http://127.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://[::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://10.0.0.1/",
        "http://169.254.169.254/latest/",
        "http://100.64.0.1/",
        "http://192.0.0.1/",
        "http://192.0.2.1/",
        "http://198.18.0.1/",
        "http://224.0.0.1/",
        "http://[2001:db8::1]/",
        "http://[2002:7f00:1::]/",
        "http://[64:ff9b::7f00:1]/",
        "http://[fc00::1]/",
        "http://example.local/",
        "http://example.com/\n",
    ] {
        assert!(validate_url(url).is_err(), "accepted {url}");
    }
    assert_eq!(
        validate_url("https://example.com/a#fragment")
            .unwrap()
            .as_str(),
        "https://example.com/a"
    );
    for ip in ["8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
        assert!(public_ip(ip.parse().unwrap()));
    }
}

#[test]
fn rejects_dns_answer_sets_containing_any_non_public_address() {
    assert_eq!(validate_addresses(&[]), Err(Error::Unavailable));
    assert_eq!(
        validate_addresses(&[
            "93.184.216.34:80".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap()
        ]),
        Err(Error::Blocked)
    );
    assert!(validate_addresses(&["93.184.216.34:80".parse().unwrap()]).is_ok());
}

#[test]
fn extracts_article_unicode_entities_and_literal_text_without_active_content() {
    let html = "<title> 页面 &amp; 标题 </title><nav>导航</nav><main><article><h1>中文</h1><p>hello <b>world</b> &amp; **literal**</p><script>alert(1)</script><style>secret</style><p hidden>隐藏</p><footer>页脚</footer></article></main>";
    let page = extract_html(&Url::parse("https://example.com/").unwrap(), html.into()).unwrap();
    assert_eq!(page.title, "页面 & 标题");
    assert_eq!(page.text, "中文\n\nhello world & **literal**");
    assert_eq!(page.html, html);
    assert_eq!(
        extract_html(
            &Url::parse("https://example.com/").unwrap(),
            "<script>onlyJS()</script>".into()
        ),
        Err(Error::Empty)
    );
}

// Unit-only transport injection: production fetch_html validates DNS before client_for.
// No runtime flag or allowlist exists to make loopback URLs importable.
async fn fixture(response: Vec<u8>) -> (Url, Client, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let url = Url::parse(&format!(
        "http://fixture.example:{}/article",
        address.port()
    ))
    .unwrap();
    let client = client_for(&url, &[address]).unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            let size = stream.read(&mut bytes).await.unwrap();
            if size == 0 {
                break;
            }
            request.extend_from_slice(&bytes[..size]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let _ = stream.write_all(&response).await;
        String::from_utf8(request).unwrap()
    });
    (url, client, task)
}

#[tokio::test]
async fn pinned_transport_reads_html_without_forwarding_credentials() {
    let (url, client, task) = fixture(b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\nConnection: close\r\n\r\n<article><p>hello</p></article>".to_vec()).await;
    let response = client.get(url).send().await.unwrap();
    assert_eq!(
        read_html(response).await.unwrap(),
        "<article><p>hello</p></article>"
    );
    let request = task.await.unwrap().to_lowercase();
    assert!(request.contains("host: fixture.example:"));
    assert!(!request.contains("authorization:"));
    assert!(!request.contains("cookie:"));
    assert!(!request.contains("referer:"));
}

#[tokio::test]
async fn redirects_are_not_followed_automatically_and_private_targets_are_blocked() {
    let (url, client, task) = fixture(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
    let response = client.get(url.clone()).send().await.unwrap();
    assert_eq!(response.status(), 302);
    assert_eq!(redirect_target(&url, &response), Err(Error::Blocked));
    task.await.unwrap();
}

#[tokio::test]
async fn rejects_non_html_bad_encoding_and_streams_exceeding_limit() {
    for (headers, body, expected) in [
        (
            "Content-Type: application/pdf",
            b"%PDF".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/html; charset=gbk",
            b"text".to_vec(),
            Error::Unsupported,
        ),
        (
            "Content-Type: text/html\r\nContent-Encoding: gzip",
            b"compressed".to_vec(),
            Error::Unsupported,
        ),
        ("Content-Type: text/html", vec![0xff], Error::Unsupported),
        (
            "Content-Type: text/html",
            vec![b'x'; MAX_HTML + 1],
            Error::TooLarge,
        ),
    ] {
        let mut response =
            format!("HTTP/1.1 200 OK\r\n{headers}\r\nConnection: close\r\n\r\n").into_bytes();
        response.extend(body);
        let (url, client, task) = fixture(response).await;
        assert_eq!(
            read_html(client.get(url).send().await.unwrap()).await,
            Err(expected)
        );
        task.await.unwrap();
    }
}

#[test]
fn does_not_choose_an_article_inside_hidden_or_navigation_ancestors() {
    let page = extract_html(&Url::parse("https://example.com/").unwrap(),
        "<div hidden><article>hidden</article></div><nav><article>navigation</article></nav><main><p>visible</p></main>".into()).unwrap();
    assert_eq!(page.text, "visible");
}

#[tokio::test]
#[ignore = "requires direct outbound public DNS and HTTPS; external site availability is not a CI dependency"]
async fn live_public_html_import() {
    let page = PublicWebImporter::default()
        .import("https://example.com/")
        .await
        .unwrap();
    assert!(!page.text.is_empty());
    assert_eq!(page.source, "https://example.com/");
    assert!(page.html.contains("<html"));
}

use super::*;

fn feed(items: &str) -> String {
    format!(
        "<rss version=\"2.0\"><channel><title>新闻</title><link>https://example.com/</link><description>摘要</description>{items}</channel></rss>"
    )
}
fn item(guid: &str, title: &str) -> String {
    format!("<item><guid>{guid}</guid><title>{title}</title></item>")
}
fn parsed(source: &str) -> Feed {
    parse_rss(source.as_bytes()).unwrap_or_else(|e| panic!("parse failed: {e:?}"))
}
fn rejects(source: &str, error: ParseError) {
    assert_eq!(parse_rss(source.as_bytes()).err(), Some(error));
}

#[test]
fn parses_utf8_entities_cdata_and_removes_active_html_without_fetching() {
    let source = format!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}",
        feed(
            "<item><guid isPermaLink=\"true\">opaque &amp; &#x4e2d;</guid><title>A &amp; B</title><description><![CDATA[<p>Hello <b>world</b></p><script>secret</script><style>hidden</style><iframe src='https://example.com/'>frame</iframe><img src='https://example.com/a'><p>next &amp; last</p>]]></description><pubDate>untrusted date</pubDate><enclosure url=\"https://example.com/file\"/></item>"
        )
    );
    let result = parsed(&source);
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert_eq!(entry.title, "A & B");
    assert_eq!(entry.summary, "Hello world next & last");
    assert_eq!(entry.published_at.as_deref(), Some("untrusted date"));
    assert!(entry.entry_key().starts_with("guid:"));
    assert_eq!(entry.link, None);
}

#[test]
fn rejects_dtd_entities_instructions_encodings_and_namespace_impostors() {
    for prefix in [
        "<!DOCTYPE rss>",
        "<!DOCTYPE rss [<!ENTITY x SYSTEM 'file:///etc/passwd'>]>",
        "<?fetch https://example.com/?>",
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>",
        "<?xml version=\"1.1\"?>",
    ] {
        rejects(&format!("{prefix}{}", feed("")), ParseError::UnsupportedXml);
    }
    rejects(&feed(&item("x", "&unknown;")), ParseError::InvalidXml);
    rejects(&feed(&item("x", "&#0;")), ParseError::InvalidXml);
    rejects(&feed(&item("x", "&#xB;")), ParseError::InvalidXml);
    rejects(
        &feed("").replace("<rss ", "<rss xmlns=\"urn:other\" "),
        ParseError::UnsupportedXml,
    );
    rejects(&feed("<evil:item/>"), ParseError::UnsupportedXml);
    assert!(parse_rss(&[0xff, 0xfe]).is_err());
}

#[test]
fn rejects_malformed_xml_and_ambiguous_fields_without_partial_results() {
    for source in [
        feed("").replace("</rss>", ""),
        feed("").replace("</channel>", "</wrong>"),
        format!("{}{}", feed(""), feed("")),
        format!("bad{}", feed("")),
        feed(&item("x", "]]>")),
        feed(&item("x", "\u{1}")),
        feed(&item("x", "<b>nested</b>")),
        feed("<item><guid>x</guid><guid>y</guid><title>Title</title></item>"),
        feed("").replace("version=\"2.0\"", "version=\"2.0\" version=\"2.0\""),
        feed("").replace("version=\"2.0\"", "version=\"<\""),
        feed("").replace("<title>新闻</title>", ""),
        feed("").replace("<channel>", "<channel>junk"),
        format!("\u{a0}{}", feed("")),
        format!("<?xml version=\"1.0\" standalone=\"maybe\"?>{}", feed("")),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" encoding=\"UTF-8\"?>{}",
            feed("")
        ),
        format!("<?xml version=\"1.0\" stray=\"value\"?>{}", feed("")),
        feed(&format!(
            "{}<item><title>No identity</title></item>",
            item("valid", "Valid")
        )),
        feed("<item><guid>x</guid><title><![CDATA[<script>only script</script>]]></title></item>"),
        "<feed xmlns=\"http://www.w3.org/2005/Atom\"/>".into(),
    ] {
        assert!(
            parse_rss(source.as_bytes()).is_err(),
            "accepted malformed fixture"
        );
    }
}

#[test]
fn enforces_byte_depth_node_item_and_field_limits() {
    assert_eq!(
        parsed(&feed(&item("x", &"文".repeat(512)))).entries.len(),
        1
    );
    rejects(&feed(&item("x", &"文".repeat(513))), ParseError::TooLarge);
    rejects(
        &feed(&format!(
            "<item><guid>x</guid><description>{}</description></item>",
            "a".repeat(8193)
        )),
        ParseError::TooLarge,
    );
    rejects(
        &feed(&item(&"a".repeat(2049), "Title")),
        ParseError::TooLarge,
    );
    rejects(
        &feed(&format!(
            "<item><guid>x</guid><title>T</title><pubDate>{}</pubDate></item>",
            "a".repeat(257)
        )),
        ParseError::TooLarge,
    );
    assert_eq!(
        parsed(&feed(&item("same", "Title").repeat(100)))
            .entries
            .len(),
        1
    );
    rejects(
        &feed(&item("same", "Title").repeat(101)),
        ParseError::TooLarge,
    );
    rejects(&" ".repeat(MAX_BYTES + 1), ParseError::TooLarge);
    assert!(
        parse_rss(feed(&format!("{}{}", "<x>".repeat(30), "</x>".repeat(30))).as_bytes()).is_ok()
    );
    rejects(
        &feed(&format!("{}{}", "<x>".repeat(31), "</x>".repeat(31))),
        ParseError::TooLarge,
    );
    rejects(&feed(&"<x/>".repeat(MAX_NODES)), ParseError::TooLarge);
}

#[test]
fn guid_is_opaque_and_typed_and_link_fallback_is_canonical() {
    let guid = parsed(&feed(&item("https://example.com/article", "Title")));
    let link = parsed(&feed(
        "<item><title>Title</title><link>https://EXAMPLE.com:443/article#section</link></item>",
    ));
    assert_ne!(guid.entries[0].entry_key(), link.entries[0].entry_key());
    let link2 = parsed(&feed(
        "<item><guid> </guid><title>Title</title><link>https://example.com/article</link></item>",
    ));
    assert_eq!(link.entries[0].entry_key(), link2.entries[0].entry_key());
    assert_eq!(
        link.entries[0].link.as_deref(),
        Some("https://example.com/article")
    );
    assert_eq!(
        parsed(&feed(&item("file:///opaque-guid", "Title")))
            .entries
            .len(),
        1
    );
    for url in [
        "http://example.com/",
        "https://127.0.0.1/",
        "/relative",
        "javascript:alert(1)",
    ] {
        rejects(
            &feed(&format!(
                "<item><title>Title</title><link>{url}</link></item>"
            )),
            ParseError::InvalidItem,
        );
    }
}

#[test]
fn deduplicates_identical_entries_and_rejects_conflicts_in_either_order() {
    let first = item("same", "First");
    let second = item("same", "Second");
    assert_eq!(parsed(&feed(&format!("{first}{first}"))).entries.len(), 1);
    rejects(
        &feed(&format!("{first}{second}")),
        ParseError::ConflictingDuplicate,
    );
    rejects(
        &feed(&format!("{second}{first}")),
        ParseError::ConflictingDuplicate,
    );
    assert_eq!(
        parsed(&feed(&format!("{first}{}", item("other", "First"))))
            .entries
            .len(),
        2
    );
}

#[test]
fn classifies_changes_by_entire_owner_subscription_identity_key() {
    let user = UserId::new("00000000-0000-4000-8000-000000000001");
    let other = UserId::new("00000000-0000-4000-8000-000000000002");
    let sub = "00000000-0000-4000-8000-000000000003";
    let other_sub = "00000000-0000-4000-8000-000000000004";
    let old = parsed(&feed(&item("stable", "Old"))).entries.remove(0);
    let new = parsed(&feed(&item("stable", "New"))).entries.remove(0);
    assert_eq!(old.entry_key(), new.entry_key());
    let existing = BTreeMap::from([(old.scoped_key(&user, sub).unwrap(), old.content_digest())]);
    assert_eq!(classify(&old, &user, sub, &existing), Ok(Change::Unchanged));
    assert_eq!(classify(&new, &user, sub, &existing), Ok(Change::Updated));
    assert_eq!(classify(&old, &other, sub, &existing), Ok(Change::New));
    assert_eq!(classify(&old, &user, other_sub, &existing), Ok(Change::New));
    assert_eq!(
        classify(&old, &user, "", &existing),
        Err(ParseError::InvalidScope)
    );
    let mut changed = old;
    changed.summary = "Edited".into();
    assert_eq!(
        classify(&changed, &user, sub, &existing),
        Ok(Change::Updated)
    );
}

#[test]
fn preserves_opaque_guid_whitespace_and_plain_text_block_boundaries() {
    let a = parsed(&feed(&item("id", "Title")));
    let b = parsed(&feed(&item(" id ", "Title")));
    assert_ne!(a.entries[0].entry_key(), b.entries[0].entry_key());
    let result = parsed(&feed(
        "<item><guid>id</guid><description><![CDATA[<p>A</p>B<br>C<div>D</div>E]]></description></item>",
    ));
    assert_eq!(result.entries[0].summary, "A B C D E");
    assert!(
        parse_rss(
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>{}",
                feed("")
            )
            .as_bytes()
        )
        .is_ok()
    );
}

#[test]
fn uses_xml10_line_endings_without_conflating_distinct_guids() {
    let lf = parsed(&feed(&item("a\nb", "Title")));
    let crlf = parsed(&feed(&item("a\r\nb", "Title")));
    let nel = parsed(&feed(&item("a\u{85}b", "Title")));
    let nel_reference = parsed(&feed(&item("a&#x85;b", "Title")));
    assert_eq!(lf.entries[0].entry_key(), crlf.entries[0].entry_key());
    assert_ne!(lf.entries[0].entry_key(), nel.entries[0].entry_key());
    assert_eq!(
        nel.entries[0].entry_key(),
        nel_reference.entries[0].entry_key()
    );
    let cdata = parsed(&feed(
        "<item><guid><![CDATA[a\u{85}b]]></guid><title>Title</title></item>",
    ));
    assert_eq!(nel.entries[0].entry_key(), cdata.entries[0].entry_key());
}

#[test]
fn bounds_attributes_and_accepts_exact_byte_limit() {
    use std::fmt::Write;
    let mut attributes = String::new();
    for i in 0..65 {
        write!(&mut attributes, " a{i}=\"v\"").unwrap();
    }
    rejects(
        &feed(&format!("<extension{attributes}/>")),
        ParseError::TooLarge,
    );
    let empty = feed("<!---->");
    let source = feed(&format!("<!--{}-->", "x".repeat(MAX_BYTES - empty.len())));
    assert_eq!(source.len(), MAX_BYTES);
    assert!(parse_rss(source.as_bytes()).is_ok());
    rejects(&format!("{source} "), ParseError::TooLarge);
}

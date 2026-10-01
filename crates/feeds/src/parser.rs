//! 离线 RSS 2.0 接收范围：UTF-8、无命名空间、无 DTD/PI，不发起任何资源请求。
use std::collections::{BTreeMap, btree_map};

use personal_ai_domain::UserId;
use quick_xml::{Reader, events::Event};
use scraper::{ElementRef, Html, Node};
use sha2::{Digest, Sha256};

use crate::{identity, normalize_source};

pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_ITEMS: usize = 100;
const MAX_NODES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    InvalidXml,
    UnsupportedXml,
    InvalidFeed,
    InvalidItem,
    ConflictingDuplicate,
    InvalidScope,
}

/// 输出仍是外部不可信资料；避免自动 Debug 输出私有正文、GUID 和 URL。
#[derive(Clone, PartialEq, Eq)]
pub struct Feed {
    pub title: String,
    pub link: String,
    pub description: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub summary: String,
    pub link: Option<String>,
    /// 来源原文元数据，不解释为可信时间。
    pub published_at: Option<String>,
    key: String,
}

impl Entry {
    #[must_use]
    pub fn entry_key(&self) -> &str {
        &self.key
    }

    /// 每次从当前内容重算，调用方修改公开字段后不会使用过期摘要。
    #[must_use]
    pub fn content_digest(&self) -> String {
        digest(&[
            "rss-content-v1",
            &self.title,
            &self.summary,
            self.link.as_deref().unwrap_or(""),
            self.published_at.as_deref().unwrap_or(""),
        ])
    }

    /// 仓储必须以整个键查询，不能仅凭 `entry_key` 跨用户或订阅查重。
    /// # Errors
    /// 用户或订阅不是非空 UUID 时拒绝。
    pub fn scoped_key(&self, user: &UserId, subscription: &str) -> Result<EntryKey, ParseError> {
        Ok(EntryKey {
            user_id: identity(user.as_str()).map_err(|_| ParseError::InvalidScope)?,
            subscription_id: identity(subscription).map_err(|_| ParseError::InvalidScope)?,
            entry_key: self.key.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryKey {
    pub user_id: String,
    pub subscription_id: String,
    pub entry_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    New,
    Unchanged,
    Updated,
}

/// existing 必须来自当前用户的仓储查询；匹配整个复合键，忽略其他用户/订阅。
/// # Errors
/// 无效用户或订阅 UUID 被拒绝。此纯函数不提供事务并发保证。
pub fn classify(
    entry: &Entry,
    user: &UserId,
    subscription: &str,
    existing: &BTreeMap<EntryKey, String>,
) -> Result<Change, ParseError> {
    let key = entry.scoped_key(user, subscription)?;
    Ok(match existing.get(&key) {
        None => Change::New,
        Some(old) if *old == entry.content_digest() => Change::Unchanged,
        Some(_) => Change::Updated,
    })
}

fn digest(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[derive(Default)]
struct XmlNode {
    name: String,
    text: String,
    children: Vec<Self>,
}

fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

fn xml_name(name: &[u8]) -> bool {
    name.first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && name
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(b))
}

fn append_text(stack: &mut [XmlNode], text: &str) -> Result<(), ParseError> {
    if !text.chars().all(xml_char) {
        return Err(ParseError::InvalidXml);
    }
    if let Some(node) = stack.last_mut() {
        node.text.push_str(text);
    } else if !text.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')) {
        return Err(ParseError::InvalidXml);
    }
    Ok(())
}

// 输入和节点总数同时受限，树深度最多 32；未知元素也执行完整 XML 校验。
#[allow(clippy::too_many_lines)]
fn xml(input: &[u8]) -> Result<XmlNode, ParseError> {
    if input.len() > MAX_BYTES {
        return Err(ParseError::TooLarge);
    }
    let source = std::str::from_utf8(input).map_err(|_| ParseError::UnsupportedXml)?;
    if !source.chars().all(xml_char) {
        return Err(ParseError::InvalidXml);
    }
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut reader = Reader::from_str(source);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_comments = true;
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root = None;
    let mut nodes = 0;
    loop {
        let position = reader.buffer_position();
        match reader.read_event().map_err(|_| ParseError::InvalidXml)? {
            Event::Start(start) => {
                nodes += 1;
                if stack.len() >= 32 || nodes > MAX_NODES {
                    return Err(ParseError::TooLarge);
                }
                let name = start.name();
                if !xml_name(name.as_ref()) {
                    return Err(ParseError::UnsupportedXml);
                }
                let mut version = None;
                for (index, attr) in start.attributes().enumerate() {
                    if index >= 64 {
                        return Err(ParseError::TooLarge);
                    }
                    let attr = attr.map_err(|_| ParseError::InvalidXml)?;
                    if !xml_name(attr.key.as_ref()) || attr.key.as_ref() == b"xmlns" {
                        return Err(ParseError::UnsupportedXml);
                    }
                    let raw =
                        std::str::from_utf8(&attr.value).map_err(|_| ParseError::InvalidXml)?;
                    if raw.contains('<') {
                        return Err(ParseError::InvalidXml);
                    }
                    let value =
                        quick_xml::escape::unescape(raw).map_err(|_| ParseError::InvalidXml)?;
                    if !value.chars().all(xml_char) {
                        return Err(ParseError::InvalidXml);
                    }
                    if attr.key.as_ref() == b"version" {
                        version = Some(value.into_owned());
                    }
                }
                if stack.is_empty()
                    && (root.is_some()
                        || name.as_ref() != b"rss"
                        || version.as_deref() != Some("2.0"))
                {
                    return Err(ParseError::InvalidFeed);
                }
                stack.push(XmlNode {
                    name: String::from_utf8(name.as_ref().to_vec())
                        .map_err(|_| ParseError::InvalidXml)?,
                    ..XmlNode::default()
                });
            }
            Event::End(_) => {
                let node = stack.pop().ok_or(ParseError::InvalidXml)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Event::Text(text) => {
                if text.as_ref().windows(3).any(|w| w == b"]]>") {
                    return Err(ParseError::InvalidXml);
                }
                append_text(
                    &mut stack,
                    &text.xml10_content().map_err(|_| ParseError::InvalidXml)?,
                )?;
            }
            Event::CData(text) => {
                if stack.is_empty() {
                    return Err(ParseError::InvalidXml);
                }
                append_text(
                    &mut stack,
                    &text.xml10_content().map_err(|_| ParseError::InvalidXml)?,
                )?;
            }
            Event::GeneralRef(reference) => {
                if stack.is_empty() {
                    return Err(ParseError::InvalidXml);
                }
                let raw = format!(
                    "&{};",
                    reference.decode().map_err(|_| ParseError::InvalidXml)?
                );
                let text = quick_xml::escape::unescape(&raw).map_err(|_| ParseError::InvalidXml)?;
                append_text(&mut stack, &text)?;
            }
            Event::Decl(decl) => {
                let declaration = std::str::from_utf8(&decl).map_err(|_| ParseError::InvalidXml)?;
                let attributes = quick_xml::events::BytesStart::from_content(declaration, 3);
                let mut stage = 0;
                for attribute in attributes.attributes() {
                    let attribute = attribute.map_err(|_| ParseError::InvalidXml)?;
                    let next = match attribute.key.as_ref() {
                        b"version" if attribute.value.as_ref() == b"1.0" => 1,
                        b"encoding" if attribute.value.eq_ignore_ascii_case(b"utf-8") => 2,
                        b"standalone" if matches!(attribute.value.as_ref(), b"yes" | b"no") => 3,
                        _ => return Err(ParseError::UnsupportedXml),
                    };
                    if next <= stage {
                        return Err(ParseError::InvalidXml);
                    }
                    stage = next;
                }

                if position != 0
                    || decl.version().map_err(|_| ParseError::InvalidXml)?.as_ref() != b"1.0"
                {
                    return Err(ParseError::UnsupportedXml);
                }
                if let Some(encoding) = decl.encoding()
                    && !encoding
                        .map_err(|_| ParseError::InvalidXml)?
                        .eq_ignore_ascii_case(b"utf-8")
                {
                    return Err(ParseError::UnsupportedXml);
                }
            }
            Event::Comment(_) => {}
            Event::Eof => break,
            Event::DocType(_) | Event::PI(_) | Event::Empty(_) => {
                return Err(ParseError::UnsupportedXml);
            }
        }
    }
    if !stack.is_empty() {
        return Err(ParseError::InvalidXml);
    }
    root.ok_or(ParseError::InvalidFeed)
}

fn field<'a>(node: &'a XmlNode, name: &str) -> Result<Option<&'a str>, ParseError> {
    let mut found = node.children.iter().filter(|n| n.name == name);
    let Some(value) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() || !value.children.is_empty() {
        return Err(ParseError::InvalidFeed);
    }
    Ok(Some(&value.text))
}

fn bounded(value: &str, max: usize) -> Result<&str, ParseError> {
    if value.chars().count() > max {
        return Err(ParseError::TooLarge);
    }
    Ok(value)
}

fn plain_text(value: &str, max: usize) -> Result<String, ParseError> {
    // 同时限制转换前后字符数，避免靠巨大隐藏 HTML 绕过字段上限。
    bounded(value, max)?;
    let html = Html::parse_fragment(value);
    let mut output = String::new();
    let mut stack = vec![(html.tree.root(), 0, false)];
    let mut visited = 0;
    while let Some((node, depth, closing)) = stack.pop() {
        visited += 1;
        if depth > 32 || visited > MAX_NODES {
            return Err(ParseError::TooLarge);
        }
        if let Some(element) = ElementRef::wrap(node) {
            if matches!(
                element.value().name(),
                "script"
                    | "style"
                    | "template"
                    | "noscript"
                    | "iframe"
                    | "object"
                    | "embed"
                    | "svg"
                    | "math"
                    | "head"
            ) {
                continue;
            }
            if matches!(
                element.value().name(),
                "p" | "div" | "br" | "li" | "tr" | "section" | "h1" | "h2"
            ) {
                output.push(' ');
            }
        } else if let Node::Text(text) = node.value() {
            output.push_str(text);
        }
        if !closing {
            if ElementRef::wrap(node).is_some() {
                stack.push((node, depth, true));
            }
            stack.extend(node.children().rev().map(|child| (child, depth + 1, false)));
        }
    }
    let output = output.split_whitespace().collect::<Vec<_>>().join(" ");
    bounded(&output, max)?;
    Ok(output)
}

fn entry(node: &XmlNode) -> Result<Entry, ParseError> {
    if !node.text.trim().is_empty() {
        return Err(ParseError::InvalidItem);
    }
    let title = plain_text(field(node, "title")?.unwrap_or(""), 512)?;
    let summary = plain_text(field(node, "description")?.unwrap_or(""), 8192)?;
    if title.is_empty() && summary.is_empty() {
        return Err(ParseError::InvalidItem);
    }
    let link = field(node, "link")?
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_source)
        .transpose()
        .map_err(|_| ParseError::InvalidItem)?;
    let guid = field(node, "guid")?.filter(|s| !s.trim().is_empty());
    let (kind, value) = if let Some(guid) = guid {
        if guid.len() > 2048 {
            return Err(ParseError::TooLarge);
        }
        ("guid", guid)
    } else {
        ("link", link.as_deref().ok_or(ParseError::InvalidItem)?)
    };
    let published_at = field(node, "pubDate")?
        .map(|s| bounded(s, 256).map(str::to_owned))
        .transpose()?;
    let entry_key = format!("{kind}:{}", digest(&["rss-entry-v1", kind, value]));
    Ok(Entry {
        title,
        summary,
        link,
        published_at,
        key: entry_key,
    })
}

/// 解析整个批次后返回；任何条目失败都不返回部分结果。
/// # Errors
/// 拒绝超限、非 UTF-8/XML 1.0、命名空间、DTD/PI、非法结构/条目和冲突重复身份。
pub fn parse_rss(input: &[u8]) -> Result<Feed, ParseError> {
    let root = xml(input)?;
    if !root.text.trim().is_empty()
        || root.children.len() != 1
        || root.children[0].name != "channel"
    {
        return Err(ParseError::InvalidFeed);
    }
    let channel = &root.children[0];
    if !channel.text.trim().is_empty() {
        return Err(ParseError::InvalidFeed);
    }
    let title = plain_text(
        field(channel, "title")?.ok_or(ParseError::InvalidFeed)?,
        512,
    )?;
    let description = plain_text(
        field(channel, "description")?.ok_or(ParseError::InvalidFeed)?,
        8192,
    )?;
    let link = normalize_source(
        field(channel, "link")?
            .ok_or(ParseError::InvalidFeed)?
            .trim(),
    )
    .map_err(|_| ParseError::InvalidFeed)?;
    if title.is_empty() || description.is_empty() {
        return Err(ParseError::InvalidFeed);
    }
    let items: Vec<_> = channel
        .children
        .iter()
        .filter(|n| n.name == "item")
        .collect();
    if items.len() > MAX_ITEMS {
        return Err(ParseError::TooLarge);
    }
    let mut entries = Vec::new();
    let mut seen = BTreeMap::new();
    for item in items {
        let entry = entry(item)?;
        let content = entry.content_digest();
        match seen.entry(entry.key.clone()) {
            btree_map::Entry::Occupied(old) if *old.get() != content => {
                return Err(ParseError::ConflictingDuplicate);
            }
            btree_map::Entry::Occupied(_) => {}
            btree_map::Entry::Vacant(slot) => {
                slot.insert(content);
                entries.push(entry);
            }
        }
    }
    Ok(Feed {
        title,
        link,
        description,
        entries,
    })
}

#[cfg(test)]
mod tests;

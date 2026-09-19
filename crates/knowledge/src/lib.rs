#![forbid(unsafe_code)]

pub mod index;
pub mod retrieval;
pub mod web;

use personal_ai_domain::DocumentId;
use pulldown_cmark::{Event, Parser, TagEnd};

/// 提取可读文本；不渲染原始 HTML 事件，也不将其纳入索引。
#[must_use]
pub fn markdown_text(markdown: &str) -> String {
    let mut text = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item,
            ) => text.push_str("\n\n"),
            _ => {}
        }
    }
    text.trim().to_owned()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceType {
    Markdown,
    Pdf,
    WebPage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentMetadata {
    pub id: DocumentId,
    pub source: String,
    pub source_type: SourceType,
    pub created_at_unix_ms: u64,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentChunk {
    pub document_id: DocumentId,
    pub ordinal: usize,
    pub content: String,
}

/// 按段落边界切分 UTF-8 文本，仅在段落超出指定字符数上限时
/// 才在段落内部强制切分。
#[must_use]
pub fn chunk_text(document_id: &DocumentId, text: &str, max_chars: usize) -> Vec<DocumentChunk> {
    if max_chars == 0 || text.trim().is_empty() {
        return Vec::new();
    }

    let mut pieces = Vec::new();
    let mut current = String::new();

    for paragraph in text
        .split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if paragraph.chars().count() > max_chars {
            flush(&mut pieces, &mut current);
            split_long_paragraph(&mut pieces, paragraph, max_chars);
            continue;
        }

        let separator_size = usize::from(!current.is_empty()) * 2;
        if current.chars().count() + separator_size + paragraph.chars().count() > max_chars {
            flush(&mut pieces, &mut current);
        }

        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    flush(&mut pieces, &mut current);

    pieces
        .into_iter()
        .enumerate()
        .map(|(ordinal, content)| DocumentChunk {
            document_id: document_id.clone(),
            ordinal,
            content,
        })
        .collect()
}

fn flush(pieces: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        pieces.push(std::mem::take(current));
    }
}

fn split_long_paragraph(pieces: &mut Vec<String>, paragraph: &str, max_chars: usize) {
    let mut part = String::new();
    let mut count = 0;
    for character in paragraph.chars() {
        part.push(character);
        count += 1;
        if count == max_chars {
            pieces.push(std::mem::take(&mut part));
            count = 0;
        }
    }
    if !part.is_empty() {
        pieces.push(part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extraction_keeps_text_and_code_but_not_html_blocks() {
        let text =
            markdown_text("# Heading\n\n**中文** and `code`\n\n<script>alert(1)</script>\n\nEnd");
        assert!(text.contains("Heading"));
        assert!(text.contains("中文 and code"));
        assert!(text.contains("End"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("**"));
    }

    #[test]
    fn chunks_at_paragraph_boundaries() {
        let chunks = chunk_text(&DocumentId::new("doc-1"), "first\n\nsecond", 8);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].content, "first");
        assert_eq!(chunks[1].content, "second");
    }

    #[test]
    fn hard_split_preserves_unicode_characters() {
        let chunks = chunk_text(&DocumentId::new("doc-1"), "个人知识库", 2);

        let contents: Vec<_> = chunks.into_iter().map(|chunk| chunk.content).collect();
        assert_eq!(contents, ["个人", "知识", "库"]);
    }
}

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    pub markdown: String,
    pub revision: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveChapter {
    pub title: String,
    pub markdown: String,
    pub expected_revision: u64,
}

pub fn normalize(markdown: &str) -> String {
    markdown.replace("\r\n", "\n").replace('\r', "\n").trim().to_string()
}
pub fn hash(markdown: &str) -> String {
    format!("{:x}", Sha256::digest(normalize(markdown).as_bytes()))
}
pub fn validate(input: &SaveChapter) -> Result<Chapter, &'static str> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 200 { return Err("Название должно содержать 1–200 символов"); }
    if input.markdown.len() > MAX_TEXT_BYTES { return Err("Глава слишком большая"); }
    let text = normalize(&input.markdown);
    if text.is_empty() { return Err("Текст главы пуст"); }
    Ok(Chapter { title: title.to_string(), content_hash: hash(&text), markdown: text, revision: input.expected_revision })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_line_endings() { assert_eq!(hash("а\r\nб"), hash("а\nб")); assert_eq!(hash("а\rб"), hash("а\nб")); }
    #[test]
    fn validates_missing_title_and_text() {
        assert!(validate(&SaveChapter{title:" ".into(), markdown:"текст".into(),expected_revision:0}).is_err());
        assert!(validate(&SaveChapter{title:"Глава".into(), markdown:" ".into(),expected_revision:0}).is_err());
    }
    #[test]
    fn long_chapters() {
        for count in [20_000, 150_000, 1_000_000] {
            let source = "Я".repeat(count);
            let out = validate(&SaveChapter { title:"Глава".into(), markdown:source.clone(), expected_revision:0 }).unwrap();
            assert_eq!(out.markdown.chars().count(),count);
            assert_eq!(hash(&out.markdown),out.content_hash);
        }
    }
}

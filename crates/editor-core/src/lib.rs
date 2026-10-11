use std::borrow::Cow;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_CHAPTER_CHARS: usize = 50_000;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const CHAPTER_LENGTH_ERROR: &str = "Текст главы не должен превышать 50 000 символов";

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

/// Canonicalize only line endings. Whitespace belongs to the author.
/// For LF-only text, this does not allocate an intermediate string.
pub fn normalize(markdown: &str) -> Cow<'_, str> {
    if !markdown.contains('\r') {
        return Cow::Borrowed(markdown);
    }
    let mut output = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(index) = rest.find('\r') {
        output.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        if rest.starts_with('\n') {
            rest = &rest[1..];
        }
        output.push('\n');
    }
    output.push_str(rest);
    Cow::Owned(output)
}

/// Hash already-normalized content to avoid a second normalization.
fn hash_normalized(markdown: &str) -> String {
    format!("{:x}", Sha256::digest(markdown.as_bytes()))
}

pub fn hash(markdown: &str) -> String {
    let normalized = normalize(markdown);
    hash_normalized(&normalized)
}
pub fn validate(input: &SaveChapter) -> Result<Chapter, &'static str> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 200 { return Err("Название должно содержать 1–200 символов"); }
    if input.markdown.len() > MAX_TEXT_BYTES { return Err("Глава слишком большая"); }
    let text = normalize(&input.markdown);
    // Reject a blank chapter, but never erase whitespace from a valid one.
    if text.trim().is_empty() { return Err("Текст главы пуст"); }
    if text.chars().count() > MAX_CHAPTER_CHARS { return Err(CHAPTER_LENGTH_ERROR); }
    let content_hash = hash_normalized(&text);
    Ok(Chapter {
        title: title.to_string(),
        content_hash,
        markdown: text.into_owned(),
        revision: input.expected_revision,
    })
}
#[cfg(test)]
mod saving_markdown_whitespace_tests {
    use super::*;
    use std::borrow::Cow;

    fn chapter(text: &str) -> Chapter {
        validate(&SaveChapter {
            title: "Пробельные символы".into(),
            markdown: text.into(),
            expected_revision: 0,
        })
        .unwrap()
    }

    #[test]
    fn retains_indented_code_and_trailing_markdown_breaks() {
        let original = "    fn main() {}\n\nТекст с жёстким переносом  \n\n";
        let saved = chapter(original);
        assert_eq!(saved.markdown, original);
        assert_eq!(saved.content_hash, hash(original));
    }

    #[test]
    fn whitespace_edits_change_identity_and_are_not_noop_saves() {
        let original = chapter("Текст\n");
        let changed = chapter("Текст\n ");
        assert_ne!(original.markdown, changed.markdown);
        assert_ne!(original.content_hash, changed.content_hash);
        assert_ne!(hash("    отступ"), hash("отступ"));
    }

    #[test]
    fn normalizes_line_endings_but_preserves_surrounding_spaces() {
        let saved = chapter("  привет  \r\n    код \rхвост  \r\n");
        assert_eq!(saved.markdown, "  привет  \n    код \nхвост  \n");
        assert_eq!(saved.content_hash, hash("  привет  \n    код \nхвост  \n"));
    }

    #[test]
    fn lf_text_is_borrowed_and_crlf_text_is_copied_only_once() {
        assert!(matches!(normalize("Привет 🌍\n"), Cow::Borrowed(_)));
        assert!(matches!(normalize("Привет 🌍\r\n"), Cow::Owned(_)));
    }

    #[test]
    fn whitespace_only_chapter_is_rejected() {
        assert_eq!(
            validate(&SaveChapter {
                title: "Глава".into(),
                markdown: "  \n   \r\n".into(),
                expected_revision: 0,
            })
            .unwrap_err(),
            "Текст главы пуст"
        );
    }

    #[test]
    fn preserves_whitespace_at_50_000_character_boundary() {
        let markdown = format!("  {}  ", "Я".repeat(49_996));
        let saved = chapter(&markdown);
        assert_eq!(saved.markdown, markdown);
        assert_eq!(saved.markdown.chars().count(), MAX_CHAPTER_CHARS);
    }
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
    fn supported_chapter_sizes() {
        for count in [5_000, 10_000, 20_000, 50_000] {
            let source = "Я".repeat(count);
            let out = validate(&SaveChapter { title:"Глава".into(), markdown:source, expected_revision:0 }).unwrap();
            assert_eq!(out.markdown.chars().count(), count);
            assert_eq!(hash(&out.markdown), out.content_hash);
        }
    }
    #[test]
    fn rejects_more_than_50_000_unicode_characters() {
        for character in ["Я", "🌍"] {
            let exact = character.repeat(MAX_CHAPTER_CHARS);
            assert!(validate(&SaveChapter {
                title:"Глава".into(), markdown:exact, expected_revision:0
            }).is_ok());

            let too_long = character.repeat(MAX_CHAPTER_CHARS + 1);
            assert_eq!(
                validate(&SaveChapter {
                    title:"Глава".into(), markdown:too_long, expected_revision:0
                }).unwrap_err(),
                CHAPTER_LENGTH_ERROR
            );
        }
    }
    #[test]
    fn normalizes_crlf_before_counting() {
        // CRLF is one line break after normalization, not two characters.
        let crlf = "a\r\n".repeat(25_000);
        let chapter = validate(&SaveChapter {
            title:"Глава".into(), markdown:crlf, expected_revision:0
        }).unwrap();
        assert!(chapter.markdown.chars().count() <= MAX_CHAPTER_CHARS);
        assert!(!chapter.markdown.contains('\r'));
    }
}

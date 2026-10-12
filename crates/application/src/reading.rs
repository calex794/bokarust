//! Public reader ports. In BKR-86 all non-public and paid content is denied.
use async_trait::async_trait;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadingError {
    Unavailable,
}

impl fmt::Display for ReadingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for ReadingError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicChapterLink {
    pub id: u64,
    pub number: i32,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicBookPage {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub chapters: Vec<PublicChapterLink>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicChapterPage {
    pub book_id: u64,
    pub book_title: String,
    pub chapter_title: String,
    pub version_no: i32,
    pub body_markdown: String,
}

/// This port deliberately has no method to read paid, draft, held or deleted text.
/// Such access will need a separate authenticated policy and entitlement check.
#[async_trait]
pub trait PublicReadingRepository: Send + Sync {
    async fn public_book(&self, id: u64) -> Result<Option<PublicBookPage>, ReadingError>;
    async fn public_chapter(&self, id: u64) -> Result<Option<PublicChapterPage>, ReadingError>;
}

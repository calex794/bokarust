//! SQL-level public book and chapter visibility; raw versions are never returned by ID.
use async_trait::async_trait;
use bokarust_application::reading::{
    PublicBookPage, PublicChapterLink, PublicChapterPage, PublicReadingRepository, ReadingError,
};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgReader {
    pool: PgPool,
}

impl PgReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_db(_: sqlx::Error) -> ReadingError {
    ReadingError::Unavailable
}

fn positive_id(value: i64) -> Result<u64, ReadingError> {
    u64::try_from(value)
        .ok()
        .filter(|id| *id > 0)
        .ok_or(ReadingError::Unavailable)
}

#[async_trait]
impl PublicReadingRepository for PgReader {
    async fn public_book(&self, id: u64) -> Result<Option<PublicBookPage>, ReadingError> {
        let Ok(id) = i64::try_from(id) else {
            return Ok(None);
        };
        if id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT id, title, target_language FROM books \
             WHERE id = $1 AND visibility = 'public' AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db)?;
        let Some(book) = row else {
            return Ok(None);
        };
        // Recheck the parent state in the chapter query, not just in the first query.
        let rows = sqlx::query(
            "SELECT c.id, c.chapter_no, c.title FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.id = p.version_id AND v.chapter_id = c.id \
             WHERE c.book_id = $1 AND b.visibility = 'public' AND NOT b.admin_hold \
               AND NOT b.is_deleted AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
             ORDER BY c.chapter_no, c.id LIMIT 200",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db)?;
        let chapters = rows
            .into_iter()
            .map(|row| {
                Ok(PublicChapterLink {
                    id: positive_id(row.try_get("id").map_err(map_db)?)?,
                    number: row.try_get("chapter_no").map_err(map_db)?,
                    title: row.try_get("title").map_err(map_db)?,
                })
            })
            .collect::<Result<Vec<_>, ReadingError>>()?;
        Ok(Some(PublicBookPage {
            id: positive_id(book.try_get("id").map_err(map_db)?)?,
            title: book.try_get("title").map_err(map_db)?,
            language: book.try_get("target_language").map_err(map_db)?,
            chapters,
        }))
    }

    async fn public_chapter(&self, id: u64) -> Result<Option<PublicChapterPage>, ReadingError> {
        let Ok(id) = i64::try_from(id) else {
            return Ok(None);
        };
        if id <= 0 {
            return Ok(None);
        }
        // Access predicates must be in the SAME SQL query that selects the text.
        // Never rely on filtering navigation links, the homepage or an earlier request.
        let row = sqlx::query(
            "SELECT b.id AS book_id, b.title AS book_title, \
                    c.title AS chapter_title, v.version_no, v.body_markdown \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.id = p.version_id AND v.chapter_id = c.id \
             WHERE c.id = $1 AND b.visibility = 'public' AND NOT b.admin_hold \
               AND NOT b.is_deleted AND c.visibility = 'public' AND c.access_level = 'free' \
               AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now())",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db)?;
        row.map(|row| {
            Ok(PublicChapterPage {
                book_id: positive_id(row.try_get("book_id").map_err(map_db)?)?,
                book_title: row.try_get("book_title").map_err(map_db)?,
                chapter_title: row.try_get("chapter_title").map_err(map_db)?,
                version_no: row.try_get("version_no").map_err(map_db)?,
                body_markdown: row.try_get("body_markdown").map_err(map_db)?,
            })
        })
        .transpose()
    }
}

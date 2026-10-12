//! Author-owned book reads and shared transaction primitives.
#![forbid(unsafe_code)]
use bokarust_application::authoring::{
    AuthorError, ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterSummary,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgAuthor {
    pub(crate) pool: PgPool,
}

impl PgAuthor {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn my_books(&self, user_id: i64) -> Result<Vec<OwnedBook>, AuthorError> {
        let rows = sqlx::query(
            "SELECT id, title FROM books WHERE owner_user_id = $1 \
             AND NOT is_deleted ORDER BY id DESC LIMIT 100",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|row| {
                Ok(OwnedBook {
                    id: row.try_get("id").map_err(db)?,
                    title: row.try_get("title").map_err(db)?,
                })
            })
            .collect()
    }

    pub async fn owned_book(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Option<OwnedBook>, AuthorError> {
        let row = sqlx::query(
            "SELECT id, title FROM books WHERE id = $1 AND owner_user_id = $2 \
             AND NOT is_deleted AND NOT admin_hold",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(OwnedBook {
                id: r.try_get("id").map_err(db)?,
                title: r.try_get("title").map_err(db)?,
            })
        })
        .transpose()
    }

    pub async fn my_chapters(
        &self,
        user_id: i64,
        book_id: i64,
    ) -> Result<Vec<OwnedChapterSummary>, AuthorError> {
        let rows = sqlx::query(
            "SELECT c.id, c.chapter_no, c.title, c.access_level FROM chapters c \
             JOIN books b ON b.id = c.book_id \
             WHERE c.book_id = $1 AND b.owner_user_id = $2 \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND NOT c.admin_hold AND NOT c.is_deleted \
             ORDER BY c.chapter_no LIMIT 200",
        )
        .bind(book_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                let access: String = r.try_get("access_level").map_err(db)?;
                let access = match access.as_str() {
                    "free" => ChapterAccess::Free,
                    "paid" => ChapterAccess::Paid,
                    _ => return Err(AuthorError::Unavailable),
                };
                Ok(OwnedChapterSummary {
                    id: r.try_get("id").map_err(db)?,
                    number: r.try_get("chapter_no").map_err(db)?,
                    title: r.try_get("title").map_err(db)?,
                    access,
                })
            })
            .collect()
    }

    pub async fn owned_chapter(
        &self,
        user_id: i64,
        book_id: i64,
        chapter_id: i64,
    ) -> Result<Option<OwnedChapter>, AuthorError> {
        let row = sqlx::query(
            "SELECT c.chapter_no, c.title, c.access_level, c.price_amount, \
                    c.edit_revision, v.body_markdown \
             FROM chapters c JOIN books b ON b.id = c.book_id \
             JOIN chapter_publications p ON p.chapter_id = c.id \
             JOIN chapter_versions v ON v.id = p.version_id AND v.chapter_id = c.id \
             WHERE c.id = $1 AND c.book_id = $2 AND b.owner_user_id = $3 \
               AND NOT b.admin_hold AND NOT b.is_deleted \
               AND NOT c.admin_hold AND NOT c.is_deleted",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|r| {
            let access: String = r.try_get("access_level").map_err(db)?;
            let access = match access.as_str() {
                "free" => ChapterAccess::Free,
                "paid" => ChapterAccess::Paid,
                _ => return Err(AuthorError::Unavailable),
            };
            Ok(OwnedChapter {
                number: r.try_get("chapter_no").map_err(db)?,
                title: r.try_get("title").map_err(db)?,
                body: r.try_get("body_markdown").map_err(db)?,
                access,
                price_amount: r.try_get("price_amount").map_err(db)?,
                revision: r.try_get("edit_revision").map_err(db)?,
            })
        })
        .transpose()
    }
}

pub(crate) fn db(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref cause) = error {
        if cause.code().as_deref() == Some("23505") {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

pub(crate) fn payload_digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Unique-row insert serializes simultaneous repeats, even across web instances.
pub(crate) async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    sql: &'static str,
    user_id: i64,
    key: &str,
    digest: &[u8; 32],
) -> Result<bool, AuthorError> {
    let created: Option<i64> = sqlx::query_scalar(sql)
        .bind(user_id)
        .bind(key)
        .bind(digest.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    Ok(created.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_frames_prevent_ambiguous_command_hashing() {
        assert_ne!(
            payload_digest(&[b"ab", b"c"]),
            payload_digest(&[b"a", b"bc"])
        );
    }
}

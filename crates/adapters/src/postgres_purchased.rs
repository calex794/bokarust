//! Read-only PostgreSQL projection of independently settled, immutable chapter purchases.
//! Neither a saved cursor nor a posted form can mint entitlements.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgPurchasedReader {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseReadError {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseStatus {
    Readable,
    Blocked,
    Revoked,
}

#[derive(Debug)]
pub struct PurchasedListItem {
    pub chapter_id: i64,
    pub book_title: String,
    pub chapter_title: String,
    pub chapter_no: i32,
    pub version_no: i32,
    pub status: PurchaseStatus,
}

#[derive(Debug)]
pub struct PurchasedChapter {
    pub book_title: String,
    pub chapter_title: String,
    pub chapter_no: i32,
    pub version_no: i32,
    pub body_markdown: String,
}

fn db(_: sqlx::Error) -> PurchaseReadError {
    PurchaseReadError::Unavailable
}

impl PgPurchasedReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Financial history stays visible when reading is blocked or refunded.
    /// No book visibility, lifecycle or current publication pointer is consulted.
    pub async fn library(&self, user_id: i64) -> Result<Vec<PurchasedListItem>, PurchaseReadError> {
        if user_id <= 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT c.id AS chapter_id, b.title AS book_title, c.title AS chapter_title, \
                    c.chapter_no, v.version_no, \
                    (r.purchase_id IS NOT NULL) AS revoked, \
                    (b.admin_hold OR c.admin_hold OR c.is_deleted) AS blocked \
             FROM library_purchased_chapters AS l \
             JOIN chapter_purchase_facts AS p ON p.id = l.purchase_id \
               AND p.user_id = l.user_id AND p.chapter_id = l.chapter_id \
               AND p.version_id = l.active_version_id \
             JOIN chapter_versions AS v ON v.id = l.active_version_id AND v.chapter_id = l.chapter_id \
             JOIN chapters AS c ON c.id = l.chapter_id \
             JOIN books AS b ON b.id = c.book_id \
             LEFT JOIN chapter_purchase_revocations AS r ON r.purchase_id = p.id \
             WHERE l.user_id = $1 \
             ORDER BY l.created_at DESC, l.chapter_id DESC LIMIT 100",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|row| {
                let revoked: bool = row.try_get("revoked").map_err(db)?;
                let blocked: bool = row.try_get("blocked").map_err(db)?;
                let status = if revoked {
                    PurchaseStatus::Revoked
                } else if blocked {
                    PurchaseStatus::Blocked
                } else {
                    PurchaseStatus::Readable
                };
                Ok(PurchasedListItem {
                    chapter_id: row.try_get("chapter_id").map_err(db)?,
                    book_title: row.try_get("book_title").map_err(db)?,
                    chapter_title: row.try_get("chapter_title").map_err(db)?,
                    chapter_no: row.try_get("chapter_no").map_err(db)?,
                    version_no: row.try_get("version_no").map_err(db)?,
                    status,
                })
            })
            .collect()
    }

    /// The pinned ChapterVersion comes only from a matching, non-revoked purchase
    /// owned by the authenticated user. Admin hold overrides every purchase.
    /// Ordinary book hiding/deletion does NOT destroy the acquired version.
    /// Chapter deletion is conservatively denied until its policy is approved.
    pub async fn owned_chapter(
        &self,
        user_id: i64,
        chapter_id: i64,
    ) -> Result<Option<PurchasedChapter>, PurchaseReadError> {
        if user_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title AS book_title, c.title AS chapter_title, c.chapter_no, \
                    v.version_no, v.body_markdown \
             FROM library_purchased_chapters AS l \
             JOIN chapter_purchase_facts AS p ON p.id = l.purchase_id \
               AND p.user_id = l.user_id AND p.chapter_id = l.chapter_id \
               AND p.version_id = l.active_version_id \
             JOIN chapter_versions AS v ON v.id = p.version_id AND v.chapter_id = p.chapter_id \
             JOIN chapters AS c ON c.id = p.chapter_id \
             JOIN books AS b ON b.id = c.book_id \
             WHERE l.user_id = $1 AND l.chapter_id = $2 \
               AND NOT EXISTS (SELECT 1 FROM chapter_purchase_revocations r \
                               WHERE r.purchase_id = p.id) \
               AND NOT b.admin_hold AND NOT c.admin_hold AND NOT c.is_deleted",
        )
        .bind(user_id)
        .bind(chapter_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|row| {
            Ok(PurchasedChapter {
                book_title: row.try_get("book_title").map_err(db)?,
                chapter_title: row.try_get("chapter_title").map_err(db)?,
                chapter_no: row.try_get("chapter_no").map_err(db)?,
                version_no: row.try_get("version_no").map_err(db)?,
                body_markdown: row.try_get("body_markdown").map_err(db)?,
            })
        })
        .transpose()
    }
}

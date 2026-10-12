//! PostgreSQL implementation of the public catalog port. Never reads drafts.
use async_trait::async_trait;
use bokarust_application::catalog::{CatalogError, PublicBookRepository, MAX_PAGE_SIZE};
use bokarust_domain::books::{Book, BookKey, Visibility};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCatalog {
    pool: PgPool,
}

impl PgCatalog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PublicBookRepository for PgCatalog {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(CatalogError::InvalidLimit);
        }
        let limit = i64::try_from(limit).map_err(|_| CatalogError::InvalidLimit)?;
        let rows = sqlx::query(
            "SELECT id, source_identity, target_language, title, visibility \
             FROM books WHERE visibility = 'public' AND NOT admin_hold AND NOT is_deleted ORDER BY id ASC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CatalogError::Unavailable)?;

        rows.into_iter()
            .map(|row| {
                let id: i64 = row.try_get("id").map_err(|_| CatalogError::Unavailable)?;
                let source_identity: String = row
                    .try_get("source_identity")
                    .map_err(|_| CatalogError::Unavailable)?;
                let target_language: String = row
                    .try_get("target_language")
                    .map_err(|_| CatalogError::Unavailable)?;
                let title: String = row
                    .try_get("title")
                    .map_err(|_| CatalogError::Unavailable)?;
                let visibility: String = row
                    .try_get("visibility")
                    .map_err(|_| CatalogError::Unavailable)?;

                // Fail closed if a malformed or private row ever reaches this port.
                if visibility != "public" {
                    return Err(CatalogError::Unavailable);
                }
                let id = u64::try_from(id).map_err(|_| CatalogError::Unavailable)?;
                let key = BookKey::new(source_identity, target_language)
                    .map_err(|_| CatalogError::Unavailable)?;
                Book::new(id, key, title, Visibility::Public).map_err(|_| CatalogError::Unavailable)
            })
            .collect()
    }
}

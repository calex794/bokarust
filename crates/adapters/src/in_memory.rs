use async_trait::async_trait;
use bokarust_application::catalog::{CatalogError, PublicBookRepository};
use bokarust_domain::books::{Book, Visibility};
use std::sync::Arc;

/// Temporary local adapter: production catalog must use PostgreSQL and real ACLs.
#[derive(Clone, Default)]
pub struct InMemoryCatalog {
    books: Arc<Vec<Book>>,
}

impl InMemoryCatalog {
    pub fn new(books: Vec<Book>) -> Self {
        Self {
            books: Arc::new(books),
        }
    }
}

#[async_trait]
impl PublicBookRepository for InMemoryCatalog {
    async fn list_public(&self, limit: usize) -> Result<Vec<Book>, CatalogError> {
        Ok(self
            .books
            .iter()
            .filter(|book| book.visibility() == Visibility::Public)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bokarust_domain::books::BookKey;

    #[tokio::test]
    async fn never_returns_drafts() {
        let draft = Book::new(
            1,
            BookKey::new("source:1", "ru").expect("key"),
            "Draft",
            Visibility::Draft,
        )
        .expect("book");
        let published = Book::new(
            2,
            BookKey::new("source:2", "ru").expect("key"),
            "Published",
            Visibility::Public,
        )
        .expect("book");
        let repo = InMemoryCatalog::new(vec![draft, published]);
        let found = repo.list_public(10).await.expect("query");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title(), "Published");
    }
}

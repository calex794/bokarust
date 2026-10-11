use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use bokarust_application::catalog::{CatalogError, ListPublicBooks, PublicBookRepository, DEFAULT_PAGE_SIZE};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    catalog: Arc<dyn PublicBookRepository>,
}

#[derive(Deserialize)]
struct CatalogQuery {
    limit: Option<usize>,
}

#[derive(Serialize)]
struct BookView {
    id: u64,
    title: String,
    target_language: String,
}

async fn health() -> &'static str {
    "ok"
}

async fn list_public_books(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<Vec<BookView>>, StatusCode> {
    let result = ListPublicBooks::new(state.catalog.as_ref())
        .execute(query.limit.unwrap_or(DEFAULT_PAGE_SIZE))
        .await
        .map_err(|error| match error {
            CatalogError::InvalidLimit => StatusCode::BAD_REQUEST,
            CatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    let response = result
        .into_iter()
        .map(|book| BookView {
            id: book.id(),
            title: book.title().to_owned(),
            target_language: book.key().target_language().to_owned(),
        })
        .collect();
    Ok(Json(response))
}

pub fn router(catalog: Arc<dyn PublicBookRepository>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/books", get(list_public_books))
        .with_state(AppState { catalog })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::{to_bytes, Body}, http::Request};
    use bokarust_adapters::in_memory::InMemoryCatalog;
    use bokarust_domain::books::{Book, BookKey, Visibility};
    use tower::ServiceExt;

    fn test_router() -> Router {
        let draft = Book::new(
            1,
            BookKey::new("source:one", "ru").expect("key"),
            "Secret",
            Visibility::Draft,
        )
        .expect("book");
        let public = Book::new(
            2,
            BookKey::new("source:two", "RU").expect("key"),
            "Visible",
            Visibility::Public,
        )
        .expect("book");
        router(Arc::new(InMemoryCatalog::new(vec![draft, public])))
    }

    #[tokio::test]
    async fn health_route_is_ready_for_local_dev() {
        let response = test_router()
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn public_route_omits_private_data() {
        let response = test_router()
            .oneshot(Request::builder().uri("/api/v1/books").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.expect("body");
        let json = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(json.contains("Visible"));
        assert!(!json.contains("Secret"));
    }

    #[tokio::test]
    async fn public_route_rejects_unbounded_page() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/books?limit=10000")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

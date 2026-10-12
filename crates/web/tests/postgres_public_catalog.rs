//! Real PostgreSQL contract, executed in CI against an isolated test database.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use bokarust_adapters::{postgres::PgCatalog, postgres_accounts::PgAccounts, MIGRATOR};
use bokarust_web::router;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tower::ServiceExt;

async fn insert_book(
    pool: &PgPool,
    identity: &str,
    language: &str,
    title: &str,
    visibility: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO books (source_identity, target_language, title, visibility) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(identity)
    .bind(language)
    .bind(title)
    .bind(visibility)
    .execute(pool)
    .await?;
    Ok(())
}

async fn request(app: axum::Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8(body.to_vec()).expect("utf-8"))
}

#[tokio::test]
async fn postgres_catalog_hides_drafts_and_enforces_identity() {
    let database_url = match std::env::var("TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("Skipping PostgreSQL integration: TEST_DATABASE_URL is unset");
            return;
        }
    };
    let pool = PgPool::connect(&database_url).await.expect("test database");
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database name");
    assert!(
        db_name == "bokarust_test",
        "refusing to modify a non-test database"
    );

    MIGRATOR.run(&pool).await.expect("first migration");
    MIGRATOR.run(&pool).await.expect("idempotent migration");
    PgAccounts::from_pool(pool.clone())
        .migrate()
        .await
        .expect("account migration entry point must use the same schema");
    for table in ["users", "sessions", "books"] {
        let table_name: Option<String> =
            sqlx::query_scalar("SELECT to_regclass($1)::text")
                .bind(format!("public.{table}"))
                .fetch_one(&pool)
                .await
                .expect("check migrated table");
        assert!(table_name.is_some(), "missing table: {table}");
    }
    sqlx::query("TRUNCATE TABLE books RESTART IDENTITY")
        .execute(&pool)
        .await
        .expect("clear isolated test database");

    insert_book(&pool, "source:one", "ru", "Скрытый черновик", "draft")
        .await
        .expect("draft");
    insert_book(&pool, "source:two", "ru", "<Visible & book>", "public")
        .await
        .expect("public");
    insert_book(&pool, "source:two", "en", "English edition", "public")
        .await
        .expect("different translation language");
    assert!(
        insert_book(&pool, "source:two", "ru", "Duplicate", "public")
            .await
            .is_err(),
        "same work and language must remain unique"
    );
    assert!(
        insert_book(&pool, "source:three", "RU", "Bad language", "public")
            .await
            .is_err(),
        "database may not store unnormalized language keys"
    );

    let app = router(Arc::new(PgCatalog::new(pool.clone())));
    let (status, json) = request(app.clone(), "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("<Visible & book>"));
    assert!(json.contains("English edition"));
    assert!(!json.contains("Скрытый черновик"));

    let (status, html) = request(app.clone(), "/catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("&lt;Visible &amp; book&gt;"));
    assert!(!html.contains("<Visible & book>"));
    assert!(!html.contains("Скрытый черновик"));
    let (status, _) = request(app.clone(), "/api/v1/books?limit=101").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let race_one = insert_book(&pool, "source:race", "ru", "First", "public");
    let race_two = insert_book(&pool, "source:race", "ru", "Second", "public");
    let (a, b) = tokio::join!(race_one, race_two);
    assert_ne!(a.is_ok(), b.is_ok(), "concurrent duplicate must be rejected");
    let row = sqlx::query(
        "SELECT COUNT(*) AS count FROM books \
         WHERE source_identity = 'source:race' AND target_language = 'ru'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(row.try_get::<i64, _>("count").expect("count column"), 1);

    pool.close().await;
    let (status, _) = request(app, "/api/v1/books").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

//! Actual PostgreSQL contract: direct URLs must never disclose private text.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use bokarust_adapters::{
    account_security::generate_session_token, postgres::PgCatalog, postgres_accounts::PgAccounts,
    postgres_reading::PgReader, MIGRATOR,
};
use bokarust_web::router_with_auth_and_reading;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("bounded body");
    (status, String::from_utf8(body.to_vec()).expect("utf8"))
}

async fn book(pool: &PgPool, source: &str, title: &str, visibility: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (source_identity,target_language,title,visibility) \
        VALUES ($1,'ru',$2,$3) RETURNING id",
    )
    .bind(source)
    .bind(title)
    .bind(visibility)
    .fetch_one(pool)
    .await
    .expect("insert book")
}

async fn chapter(
    pool: &PgPool,
    book_id: i64,
    number: i32,
    title: &str,
    visibility: &str,
    access: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapters (book_id,chapter_no,title,visibility,access_level) \
        VALUES ($1,$2,$3,$4,$5) RETURNING id",
    )
    .bind(book_id)
    .bind(number)
    .bind(title)
    .bind(visibility)
    .bind(access)
    .fetch_one(pool)
    .await
    .expect("insert chapter")
}

async fn version(pool: &PgPool, chapter_id: i64, no: i32, content: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown) \
        VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(chapter_id)
    .bind(no)
    .bind(content)
    .fetch_one(pool)
    .await
    .expect("version")
}

async fn publish(pool: &PgPool, chapter_id: i64, version_id: i64) {
    sqlx::query(
        "INSERT INTO chapter_publications (chapter_id,version_id) VALUES ($1,$2) \
        ON CONFLICT (chapter_id) DO UPDATE SET version_id=EXCLUDED.version_id",
    )
    .bind(chapter_id)
    .bind(version_id)
    .execute(pool)
    .await
    .expect("publish version");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL database via TEST_DATABASE_URL"]
async fn integration_reading_deny_private_and_serve_published() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
    let pool = PgPool::connect(&url).await.expect("connect");
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database name");
    assert!(
        matches!(name.as_str(), "bokarust_ci" | "bokarust_test"),
        "refusing non-test database"
    );
    MIGRATOR.run(&pool).await.expect("migrations");
    MIGRATOR.run(&pool).await.expect("idempotent migrations");

    let suffix = generate_session_token();
    let public_id = book(
        &pool,
        &format!("reading:{suffix}"),
        "<Book & title>",
        "public",
    )
    .await;
    let hidden_book = book(
        &pool,
        &format!("reading:{suffix}:hidden"),
        "SECRET BOOK",
        "draft",
    )
    .await;
    let live = chapter(&pool, public_id, 1, "<Welcome>", "public", "free").await;
    let first = version(
        &pool,
        live,
        1,
        "Visible <script>run()</script>\nSecond line",
    )
    .await;
    let second = version(&pool, live, 2, "NEW_PUBLISHED_TEXT").await;
    publish(&pool, live, first).await;
    // Database rejects edits to immutable historical versions.
    assert!(sqlx::query("UPDATE chapter_versions SET body_markdown='tampered' WHERE id=$1")
        .bind(first)
        .execute(&pool)
        .await
        .is_err());
    // Author and reader agree on at most 50,000 Unicode characters.
    assert!(sqlx::query(
        "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown) VALUES ($1,$2,$3)"
    )
    .bind(live)
    .bind(3_i32)
    .bind("т".repeat(50_001))
    .execute(&pool)
    .await
    .is_err());

    let draft = chapter(&pool, public_id, 2, "SECRET_DRAFT", "draft", "free").await;
    let paid = chapter(&pool, public_id, 3, "SECRET_PAID", "public", "paid").await;
    let held = chapter(&pool, public_id, 4, "SECRET_HOLD", "public", "free").await;
    let deleted = chapter(&pool, public_id, 5, "SECRET_DELETED", "public", "free").await;
    let future = chapter(&pool, public_id, 6, "SECRET_FUTURE", "public", "free").await;
    let unpub = chapter(&pool, public_id, 7, "SECRET_UNPUBLISHED", "public", "free").await;
    let hidden = chapter(
        &pool,
        hidden_book,
        1,
        "SECRET_BOOK_CHAPTER",
        "public",
        "free",
    )
    .await;
    for id in [draft, paid, held, deleted, future, hidden] {
        let v = version(&pool, id, 1, &format!("PRIVATE_TEXT_{id}")).await;
        publish(&pool, id, v).await;
    }
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(held)
        .execute(&pool)
        .await
        .expect("admin hold");
    sqlx::query("UPDATE chapters SET is_deleted=true WHERE id=$1")
        .bind(deleted)
        .execute(&pool)
        .await
        .expect("deleted");
    sqlx::query("UPDATE chapters SET reader_released_at=now()+INTERVAL '1 day' WHERE id=$1")
        .bind(future)
        .execute(&pool)
        .await
        .expect("future release");

    // A publication pointer may not refer to any other chapter's version.
    assert!(sqlx::query(
        "INSERT INTO chapter_publications (chapter_id,version_id) \
        VALUES ($1,$2) ON CONFLICT(chapter_id) DO UPDATE SET version_id=EXCLUDED.version_id"
    )
    .bind(paid)
    .bind(first)
    .execute(&pool)
    .await
    .is_err());

    let app = router_with_auth_and_reading(
        Arc::new(PgCatalog::new(pool.clone())),
        Arc::new(PgReader::new(pool.clone())),
        PgAccounts::from_pool(pool.clone()),
        false,
        "http://localhost:3000".into(),
    );
    let (status, catalog) = get(&app, "/catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert!(catalog.contains(&format!("href=\"/books/{public_id}\"")));
    assert!(!catalog.contains("SECRET BOOK"));

    let (status, detail) = get(&app, &format!("/books/{public_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail.contains("&lt;Book &amp; title&gt;"));
    assert!(detail.contains(&format!("href=\"/reader/{live}\"")));
    for name in [
        "SECRET_DRAFT",
        "SECRET_PAID",
        "SECRET_HOLD",
        "SECRET_DELETED",
        "SECRET_FUTURE",
        "SECRET_UNPUBLISHED",
    ] {
        assert!(!detail.contains(name), "leaked title {name}");
    }
    for id in [draft, paid, held, deleted, future, unpub, hidden] {
        let (status, body) = get(&app, &format!("/reader/{id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "private chapter {id}");
        assert!(!body.contains("PRIVATE_TEXT"));
    }
    let (status, _) = get(&app, &format!("/books/{hidden_book}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&app, &format!("/reader/{}", u64::MAX)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/reader/{live}"))
                .body(Body::empty())
                .expect("cache policy request"),
        )
        .await
        .expect("cache policy response");
    assert_eq!(response.headers()["cache-control"], "no-store");

    let (status, reader) = get(&app, &format!("/reader/{live}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader.contains("&lt;script&gt;run()&lt;/script&gt;"));
    assert!(!reader.contains("<script>"));
    assert!(
        !reader.contains("NEW_PUBLISHED_TEXT"),
        "unpublished version leaked"
    );
    assert!(reader.contains("Second line"));

    publish(&pool, live, second).await;
    let (status, reader) = get(&app, &format!("/reader/{live}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader.contains("NEW_PUBLISHED_TEXT"));
    assert!(!reader.contains("Second line"));

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(public_id)
        .execute(&pool)
        .await
        .expect("book hold");
    let (status, result) = get(&app, "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!result.contains("<Book & title>"));
    assert_eq!(
        get(&app, &format!("/books/{public_id}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("/reader/{live}")).await.0,
        StatusCode::NOT_FOUND
    );

    // Delete only test-owned rows; do not truncate tables shared by other tests.
    sqlx::query("DELETE FROM books WHERE id IN ($1,$2)")
        .bind(public_id)
        .bind(hidden_book)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
    assert_eq!(
        get(&app, &format!("/reader/{live}")).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

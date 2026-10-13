//! Fixed purchased versions are authorized independently of free reading progress.
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use bokarust_adapters::{
    account_security::{generate_session_token, hash_password, session_token_digest},
    postgres::PgCatalog,
    postgres_accounts::PgAccounts,
    postgres_reading::PgReader,
    MIGRATOR,
};
use bokarust_web::router_with_auth_and_reading;
use sqlx::PgPool;
use std::{num::NonZeroU32, sync::Arc};
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    url: &str,
    cookie: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder().method(method).uri(url);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 512 * 1024)
        .await
        .expect("bounded response");
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn session(accounts: &PgAccounts, email: &str) -> (i64, String) {
    let password_hash = hash_password("long testing passphrase").unwrap();
    let id = accounts
        .insert_user(&email.to_ascii_lowercase(), "Покупатель", &password_hash)
        .await
        .expect("test user");
    let token = generate_session_token();
    accounts
        .insert_session(
            id,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .expect("test session");
    (id, format!("bkr-dev-session={token}"))
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL via TEST_DATABASE_URL"]
async fn integration_purchased_version_survives_hide_and_edit_but_not_hold_or_refund() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    let pool = PgPool::connect(&url).await.expect("connect");
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        matches!(db.as_str(), "bokarust_ci" | "bokarust_test"),
        "refusing to modify non-test database"
    );
    MIGRATOR.run(&pool).await.expect("migrations");
    MIGRATOR.run(&pool).await.expect("idempotent migrations");
    let accounts = PgAccounts::from_pool(pool.clone());
    let suffix = generate_session_token();
    let (owner, owner_cookie) =
        session(&accounts, &format!("buyer-{suffix}@example.invalid")).await;
    let (_, stranger_cookie) = session(&accounts, &format!("other-{suffix}@example.invalid")).await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books (source_identity, target_language, title, visibility) \
         VALUES ($1, 'ru', '<Наша & книга>', 'public') RETURNING id",
    )
    .bind(format!("purchased:{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters (book_id,chapter_no,title,visibility,access_level,price_amount) \
         VALUES ($1,1,'<Купленная глава>','public','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let old_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown) \
         VALUES ($1,1,'# Закреплённая\n\n**Версия-1** <script>untrusted()</script>') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications (chapter_id,version_id) VALUES ($1,$2)")
        .bind(chapter)
        .bind(old_version)
        .execute(&pool)
        .await
        .unwrap();

    let app = router_with_auth_and_reading(
        Arc::new(PgCatalog::new(pool.clone())),
        Arc::new(PgReader::new(pool.clone())),
        accounts,
        false,
        "http://localhost:3000".into(),
    );
    let owned_url = format!("/me/purchases/{chapter}");
    let list_url = "/me/purchases";
    assert_eq!(
        call(&app, "GET", &owned_url, "",).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "GET", &owned_url, &owner_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &format!("/reader/{chapter}"), "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &format!("/read/{chapter}"), &owner_cookie)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", list_url, &owner_cookie).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );

    // Trusted fixture simulates a separately SETTLED payment; no public route
    // is allowed to issue a purchase or create this transaction.
    let settlement = format!("settled:{suffix}");
    let purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts \
         (user_id, chapter_id, version_id, settlement_reference, amount_minor, currency) \
         VALUES ($1,$2,$3,$4,100,'RUB') RETURNING id",
    )
    .bind(owner)
    .bind(chapter)
    .bind(old_version)
    .bind(&settlement)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (linked_user, linked_version, linked_purchase): (i64, i64, i64) = sqlx::query_as(
        "SELECT user_id, active_version_id, purchase_id \
         FROM library_purchased_chapters WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .expect("settlement and library must be one atomic fact");
    assert_eq!(
        (linked_user, linked_version, linked_purchase),
        (owner, old_version, purchase)
    );
    assert!(
        sqlx::query(
            "INSERT INTO chapter_purchase_facts \
             (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency) \
             VALUES ($1,$2,$3,$4,100,'RUB')",
        )
        .bind(owner)
        .bind(chapter)
        .bind(old_version)
        .bind(&settlement)
        .execute(&pool)
        .await
        .is_err(),
        "duplicate settlement must fail"
    );
    assert!(
        sqlx::query(
            "INSERT INTO chapter_purchase_facts \
             (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency) \
             VALUES ($1,$2,$3,$4,100,'RUB')",
        )
        .bind(owner)
        .bind(chapter)
        .bind(old_version)
        .bind(format!("settled-again:{suffix}"))
        .execute(&pool)
        .await
        .is_err(),
        "new key cannot buy the already-owned version a second time"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(owner)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "failed duplicate must leave no orphaned historical facts"
    );
    assert!(
        sqlx::query("UPDATE chapter_purchase_facts SET version_id=$2 WHERE id=$1")
            .bind(purchase)
            .bind(old_version)
            .execute(&pool)
            .await
            .is_err(),
        "the historical fact cannot be modified"
    );

    let (status, headers, html) = call(&app, "GET", &owned_url, &owner_cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(headers["x-robots-tag"]
        .to_str()
        .unwrap()
        .contains("noindex"));
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    assert!(html.contains("<h1>"));
    assert!(html.contains("<strong>"));
    assert!(html.contains("&lt;script&gt;untrusted()&lt;/script&gt;"));
    assert!(!html.contains("<script>untrusted()</script>"));
    assert!(!html.contains("Версия-2"));
    assert_eq!(
        call(&app, "GET", &owned_url, &stranger_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, _, stranger_list) = call(&app, "GET", list_url, &stranger_cookie).await;
    assert!(!stranger_list.contains("Закреплённая"));

    let new_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown) \
         VALUES ($1,2,'# Версия-2 НЕ ДЛЯ ПРОШЛОЙ ПОКУПКИ') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(new_version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET visibility='draft', is_deleted=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, old_text) = call(&app, "GET", &owned_url, &owner_cookie).await;
    assert!(old_text.contains("Версия-1"));
    assert!(!old_text.contains("Версия-2"));
    assert_eq!(
        call(&app, "GET", &format!("/reader/{chapter}"), "").await.0,
        StatusCode::NOT_FOUND
    );
    // Book admin hold is the provisional hard-block mapping for this slice.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", &owned_url, &owner_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, _, blocked) = call(&app, "GET", list_url, &owner_cookie).await;
    assert!(blocked.contains("Чтение приостановлено блокировкой"));
    assert!(!blocked.contains(&format!("href=\"{owned_url}\"")));
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO chapter_purchase_revocations (purchase_id,reversal_reference,reason) \
         VALUES ($1,$2,'refund')",
    )
    .bind(purchase)
    .bind(format!("refund:{suffix}"))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        call(&app, "GET", &owned_url, &owner_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, _, revoked) = call(&app, "GET", list_url, &owner_cookie).await;
    assert!(revoked.contains("Право чтения отозвано"));
    assert!(!revoked.contains(&format!("href=\"{owned_url}\"")));
    let stored_version: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_purchase_facts WHERE id=$1")
            .bind(purchase)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_version, old_version);
    assert!(
        sqlx::query("DELETE FROM books WHERE id=$1")
            .bind(book)
            .execute(&pool)
            .await
            .is_err(),
        "physical deletion cannot cascade into sold versions"
    );
}

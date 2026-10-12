//! End-to-end author path against a disposable real PostgreSQL database.
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
use sqlx::{PgPool, Row};
use std::{num::NonZeroU32, sync::Arc};
use tower::ServiceExt;

fn encode(raw: &str) -> String {
    raw.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'*') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    cookies: &str,
    origin: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(path);
    if !cookies.is_empty() {
        builder = builder.header(header::COOKIE, cookies);
    }
    if method == "POST" {
        builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if !origin.is_empty() {
            builder = builder.header(header::ORIGIN, origin);
        }
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded");
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf8"),
    )
}

fn field(html: &str, name: &str) -> String {
    let key = format!("name=\"{name}\" value=\"");
    html.split(&key)
        .nth(1)
        .expect("form hidden field")
        .split('"')
        .next()
        .expect("value")
        .to_owned()
}

fn cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .expect("CSRF cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn chapter_body(
    csrf: &str,
    key: &str,
    revision: &str,
    title: &str,
    text: &str,
    access: &str,
    price: &str,
) -> String {
    form(&[
        ("csrf_token", csrf),
        ("command_key", key),
        ("number", "1"),
        ("expected_revision", revision),
        ("title", title),
        ("body_markdown", text),
        ("access_level", access),
        ("price_amount", price),
    ])
}

async fn new_user(accounts: &PgAccounts, suffix: &str) -> (i64, String) {
    let email = format!("author-{suffix}@example.invalid").to_ascii_lowercase();
    let password_hash = hash_password("a sufficiently long secret!").expect("test argon2");
    let id = accounts
        .insert_user(&email, "Писатель", &password_hash)
        .await
        .expect("user");
    let token = generate_session_token();
    accounts
        .insert_session(
            id,
            &session_token_digest(&token),
            NonZeroU32::new(3600).expect("ttl"),
        )
        .await
        .expect("session");
    (id, format!("bkr-dev-session={token}"))
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL database via TEST_DATABASE_URL"]
async fn integration_author_create_save_replay_conflict_and_permissions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    let dbname: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("dbname");
    assert!(
        matches!(dbname.as_str(), "bokarust_ci" | "bokarust_test"),
        "refusing to use a non-test database"
    );
    MIGRATOR.run(&pool).await.expect("migration");
    MIGRATOR.run(&pool).await.expect("repeat migration");
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = router_with_auth_and_reading(
        Arc::new(PgCatalog::new(pool.clone())),
        Arc::new(PgReader::new(pool.clone())),
        accounts.clone(),
        false,
        "http://localhost:3000".into(),
    );
    let suffix = generate_session_token();
    let (owner, session) = new_user(&accounts, &suffix).await;
    let (foreign, foreign_session) = new_user(&accounts, &format!("{suffix}-foreign")).await;

    let (status, _, _) = call(&app, "GET", "/books/new", "", "", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (status, headers, book_html) = call(&app, "GET", "/books/new", "", &session, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let browser = format!("{session}; {csrf_cookie}");
    let csrf = field(&book_html, "csrf_token");
    let key = field(&book_html, "command_key");
    let source = format!("origin:{suffix}");
    let book_form = form(&[
        ("csrf_token", &csrf),
        ("command_key", &key),
        ("source_identity", &source),
        ("target_language", "RU"),
        ("title", "<My book & one>"),
    ]);

    let (status, _, _) = call(
        &app,
        "POST",
        "/books/new",
        &book_form,
        &browser,
        "https://evil.example",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(
        &app,
        "POST",
        "/books/new",
        &book_form,
        &csrf_cookie,
        "http://localhost:3000",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, headers, _) = call(
        &app,
        "POST",
        "/books/new",
        &book_form,
        &browser,
        "http://localhost:3000",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let manager_path = headers[header::LOCATION]
        .to_str()
        .expect("Location")
        .to_owned();
    let book_id: i64 = manager_path.rsplit('/').next().unwrap().parse().unwrap();
    assert_eq!(manager_path, format!("/me/books/{book_id}"));

    // Timeout-after-commit replay returns the original book, not a duplicate.
    assert_eq!(
        call(
            &app,
            "POST",
            "/books/new",
            &book_form,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let mut changed_book_form = form(&[
        ("csrf_token", &csrf),
        ("command_key", &key),
        ("source_identity", &source),
        ("target_language", "ru"),
        ("title", "A different book"),
    ]);
    assert_eq!(
        call(
            &app,
            "POST",
            "/books/new",
            &changed_book_form,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    changed_book_form.clear();
    let duplicate_book = form(&[
        ("csrf_token", &csrf),
        ("command_key", &generate_session_token()),
        ("source_identity", &source),
        ("target_language", "ru"),
        ("title", "A duplicate work"),
    ]);
    assert_eq!(
        call(
            &app,
            "POST",
            "/books/new",
            &duplicate_book,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "canonical source work and language may only exist once"
    );

    let (status, _, detail) = call(&app, "GET", &format!("/books/{book_id}"), "", "", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail.contains("&lt;My book &amp; one&gt;"));

    // Second user cannot read owner management or insert into this book.
    assert_eq!(
        call(&app, "GET", &manager_path, "", &foreign_session, "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, _, new_html) = call(
        &app,
        "GET",
        &format!("/books/{book_id}/chapters/new"),
        "",
        &browser,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let chapter_key = field(&new_html, "command_key");
    let chapter_csrf = field(&new_html, "csrf_token");
    assert_eq!(
        chapter_csrf, csrf,
        "parallel editor tabs keep their CSRF token"
    );
    let chapter_form = chapter_body(
        &chapter_csrf,
        &chapter_key,
        "0",
        "<Chapter one>",
        "Hello <script>bad()</script>",
        "free",
        "0",
    );
    let post_path = format!("/books/{book_id}/chapters");
    let (status, foreign_headers, foreign_form) =
        call(&app, "GET", "/books/new", "", &foreign_session, "").await;
    assert_eq!(status, StatusCode::OK);
    let foreign_browser = format!(
        "{}; {}",
        foreign_session,
        cookie(&foreign_headers, "bkr-dev-csrf"),
    );
    let foreign_csrf = field(&foreign_form, "csrf_token");
    let foreign_post = chapter_body(
        &foreign_csrf,
        &generate_session_token(),
        "0",
        "Stolen chapter",
        "Private content",
        "free",
        "0",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &post_path,
            &foreign_post,
            &foreign_browser,
            "http://localhost:3000",
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "a valid login and CSRF do not grant another book owner's rights"
    );

    assert_eq!(
        call(
            &app,
            "POST",
            &post_path,
            &chapter_form,
            &csrf_cookie,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &post_path,
            &chapter_form,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &post_path,
            &chapter_form,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let altered = chapter_body(
        &chapter_csrf,
        &chapter_key,
        "0",
        "Other",
        "different",
        "free",
        "0",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &post_path,
            &altered,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    let chapter_id: i64 = sqlx::query_scalar("SELECT id FROM chapters WHERE book_id = $1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (status, _, reader) = call(&app, "GET", &format!("/reader/{chapter_id}"), "", "", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader.contains("&lt;script&gt;bad()&lt;/script&gt;"));
    assert!(!reader.contains("<script>bad()"));
    let first_version: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let releases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(releases, 1);

    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/books/{book_id}/chapters/{chapter_id}/edit"),
        "",
        &foreign_session,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let edit_path = format!("/books/{book_id}/chapters/{chapter_id}/edit");
    let (status, _, tab1) = call(&app, "GET", &edit_path, "", &browser, "").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, tab2) = call(&app, "GET", &edit_path, "", &browser, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&tab1, "csrf_token"), field(&tab2, "csrf_token"));
    assert_eq!(field(&tab1, "expected_revision"), "1");
    let update_path = format!("/books/{book_id}/chapters/{chapter_id}");
    let first_edit = chapter_body(
        &csrf,
        &field(&tab1, "command_key"),
        "1",
        "Changed in tab 1",
        "Body one",
        "free",
        "0",
    );
    let second_edit = chapter_body(
        &csrf,
        &field(&tab2, "command_key"),
        "1",
        "Changed in tab 2",
        "Body two",
        "free",
        "0",
    );
    let (one, two) = tokio::join!(
        call(
            &app,
            "POST",
            &update_path,
            &first_edit,
            &browser,
            "http://localhost:3000"
        ),
        call(
            &app,
            "POST",
            &update_path,
            &second_edit,
            &browser,
            "http://localhost:3000"
        )
    );
    assert!(
        matches!(
            (one.0, two.0),
            (StatusCode::SEE_OTHER, StatusCode::CONFLICT)
                | (StatusCode::CONFLICT, StatusCode::SEE_OTHER)
        ),
        "only one tab may commit, statuses: {:?}, {:?}",
        one.0,
        two.0
    );
    let published: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(first_version, published);
    let old_body: String =
        sqlx::query_scalar("SELECT body_markdown FROM chapter_versions WHERE id=$1")
            .bind(first_version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_body, "Hello <script>bad()</script>");
    let (status, _, reader) = call(&app, "GET", &format!("/reader/{chapter_id}"), "", "", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader.contains(if one.0 == StatusCode::SEE_OTHER {
        "Body one"
    } else {
        "Body two"
    }));

    let (status, _, current_form) = call(&app, "GET", &edit_path, "", &browser, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&current_form, "expected_revision"), "2");
    let oversized = "я".repeat(50_001);
    let invalid = chapter_body(
        &csrf,
        &field(&current_form, "command_key"),
        "2",
        "Should not appear",
        &oversized,
        "free",
        "0",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &update_path,
            &invalid,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let revision: i64 = sqlx::query_scalar("SELECT edit_revision FROM chapters WHERE id=$1")
        .bind(chapter_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(revision, 2);

    let key_paid = generate_session_token();
    let paid = chapter_body(
        &csrf,
        &key_paid,
        "2",
        "Locked text",
        "Paid secret",
        "paid",
        "150",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &update_path,
            &paid,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "GET", &format!("/reader/{chapter_id}"), "", "", "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let row =
        sqlx::query("SELECT price_amount, access_level, edit_revision FROM chapters WHERE id=$1")
            .bind(chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.try_get::<i64, _>("price_amount").unwrap(), 150);
    assert_eq!(row.try_get::<String, _>("access_level").unwrap(), "paid");
    assert_eq!(row.try_get::<i64, _>("edit_revision").unwrap(), 3);
    let releases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_first_release_events WHERE chapter_id=$1")
            .bind(chapter_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(releases, 1);

    // A revoked session cannot continue authoring, even with valid CSRF.
    accounts
        .revoke_session(&session_token_digest(session.split('=').nth(1).unwrap()))
        .await
        .expect("revoke");
    assert_eq!(
        call(
            &app,
            "POST",
            &update_path,
            &paid,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // Clean up only this test's rows, respecting durable command and release FKs.
    sqlx::query("DELETE FROM chapter_save_commands WHERE user_id IN ($1,$2)")
        .bind(owner)
        .bind(foreign)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM book_create_commands WHERE user_id IN ($1,$2)")
        .bind(owner)
        .bind(foreign)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM chapter_first_release_events WHERE chapter_id=$1")
        .bind(chapter_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(owner)
        .bind(foreign)
        .execute(&pool)
        .await
        .unwrap();
}

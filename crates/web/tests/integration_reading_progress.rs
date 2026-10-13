//! BKR-88: browser + PostgreSQL contract for private reading positions.
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
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    cookie: &str,
    origin: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder().uri(path).method(method);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    if method == "POST" {
        request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if !origin.is_empty() {
            request = request.header(header::ORIGIN, origin);
        }
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(body.to_string()))
                .expect("valid request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 512 * 1024)
        .await
        .expect("bounded response");
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf-8"),
    )
}

fn cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|x| x.to_str().ok())
        .find(|x| x.starts_with(&format!("{name}=")))
        .expect("named cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn hidden(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .expect("hidden input")
        .split('"')
        .next()
        .expect("input value")
        .to_string()
}

fn place(csrf: &str, version_id: i64, block: i32) -> String {
    form(&[
        ("csrf_token", csrf),
        ("version_id", &version_id.to_string()),
        ("block_index", &block.to_string()),
    ])
}

fn place_offset(csrf: &str, version_id: i64, block: i32, offset: i32) -> String {
    form(&[
        ("csrf_token", csrf),
        ("version_id", &version_id.to_string()),
        ("block_index", &block.to_string()),
        ("char_offset", &offset.to_string()),
    ])
}

async fn user(accounts: &PgAccounts, name: &str) -> (i64, String) {
    let hash = hash_password("a sufficiently long test password").expect("Argon2");
    let email = name.to_ascii_lowercase();
    let id = accounts
        .insert_user(&email, "Читатель", &hash)
        .await
        .expect("user");
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

async fn chapter(pool: &PgPool, book_id: i64, no: i32, title: &str, text: &str) -> (i64, i64) {
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters (book_id, chapter_no, title, visibility, access_level) \
         VALUES ($1, $2, $3, 'public', 'free') RETURNING id",
    )
    .bind(book_id)
    .bind(no)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("chapter");
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id, version_no, body_markdown) \
         VALUES ($1, 1, $2) RETURNING id",
    )
    .bind(chapter_id)
    .bind(text)
    .fetch_one(pool)
    .await
    .expect("version");
    sqlx::query("INSERT INTO chapter_publications (chapter_id, version_id) VALUES ($1, $2)")
        .bind(chapter_id)
        .bind(version_id)
        .execute(pool)
        .await
        .expect("publication");
    (chapter_id, version_id)
}

async fn current_position(pool: &PgPool, user: i64, book: i64) -> (i64, i32) {
    sqlx::query_as(
        "SELECT chapter_id, block_index FROM reader_book_positions WHERE user_id=$1 AND book_id=$2",
    )
    .bind(user)
    .bind(book)
    .fetch_one(pool)
    .await
    .expect("position")
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL via TEST_DATABASE_URL"]
async fn integration_progress_resume_without_rollback_or_access_leak() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    let pool = PgPool::connect(&url).await.expect("database");
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        matches!(db.as_str(), "bokarust_test" | "bokarust_ci"),
        "refuses to modify a non-test database"
    );
    MIGRATOR.run(&pool).await.expect("migration");
    MIGRATOR.run(&pool).await.expect("idempotent migration");
    let accounts = PgAccounts::from_pool(pool.clone());
    let suffix = generate_session_token();
    let (one, one_cookie) = user(&accounts, &format!("reader-{suffix}@example.invalid")).await;
    let (two, two_cookie) = user(&accounts, &format!("reader-{suffix}-two@example.invalid")).await;
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (source_identity,target_language,title,visibility) \
         VALUES ($1,'ru','<Private & book>','public') RETURNING id",
    )
    .bind(format!("reading:progress:{suffix}"))
    .fetch_one(&pool)
    .await
    .expect("book");
    let (chapter_id, first_version) = chapter(
        &pool,
        book_id,
        1,
        "<First chapter>",
        "Start😀\n\nSecond\n\nThird\n\n<script>dont-run()</script>",
    )
    .await;
    let app = router_with_auth_and_reading(
        Arc::new(PgCatalog::new(pool.clone())),
        Arc::new(PgReader::new(pool.clone())),
        accounts.clone(),
        false,
        "http://localhost:3000".into(),
    );
    let read_url = format!("/read/{chapter_id}");
    let place_url = format!("/read/{chapter_id}/place");
    let autosave_url = format!("{place_url}/auto");

    let (status, headers, _) = call(&app, "GET", "/me/library", "", "", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login");
    let (status, _, _) = call(&app, "GET", &read_url, "", "", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (status, public_headers, public_html) =
        call(&app, "GET", &format!("/reader/{chapter_id}"), "", "", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public_headers[header::CACHE_CONTROL], "no-store");
    assert!(public_html.contains(&format!("href=\"/read/{chapter_id}\"")));
    assert!(!public_html.contains("name=\"csrf_token\""));

    // The public reader remains script-free; the asset contains no private data.
    assert!(!public_html.contains("/assets/reader-progress.js"));
    let (asset_status, asset_headers, asset_body) =
        call(&app, "GET", "/assets/reader-progress.js", "", "", "").await;
    assert_eq!(asset_status, StatusCode::OK);
    assert!(asset_headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert_eq!(asset_headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(asset_body.contains("sendBeacon"));

    let (status, headers, html) = call(&app, "GET", &read_url, "", &one_cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers["x-frame-options"], "DENY");
    let reader_csp = headers["content-security-policy"].to_str().unwrap();
    assert!(reader_csp.contains("script-src 'self'"));
    assert!(reader_csp.contains("connect-src 'self'"));
    assert!(!reader_csp.contains("'unsafe-inline'"));
    assert!(html.contains("<script src=\"/assets/reader-progress.js\" defer></script>"));
    assert!(html.contains("id=\"reader-save-status\""));
    assert!(html.contains("id=\"p-4\""));
    assert!(html.contains("&lt;script&gt;dont-run()&lt;/script&gt;"));
    assert!(!html.contains("<script>dont-run()"));
    let csrf = hidden(&html, "csrf_token");
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let browser = format!("{one_cookie}; {csrf_cookie}");
    let first = place(&csrf, first_version, 1);
    let last = place(&csrf, first_version, 4);
    let middle = place(&csrf, first_version, 2);

    // Auto-save is a separate 204 endpoint but has the same CSRF and session checks.
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &first,
            &one_cookie,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &first,
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
            &autosave_url,
            &first,
            &browser,
            "https://evil.example"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &place(&csrf, first_version + 1, 1),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // CSRF, origin, session and the public access gate are independent.
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &first,
            &one_cookie,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &first,
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
            &place_url,
            &first,
            &browser,
            "https://evil.example"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &place(&csrf, first_version, 999),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &place(&csrf, first_version + 9999, 1),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &first,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(current_position(&pool, one, book_id).await, (chapter_id, 1));
    let (auto_status, auto_headers, auto_body) = call(
        &app,
        "POST",
        &autosave_url,
        &first,
        &browser,
        "http://localhost:3000",
    )
    .await;
    assert_eq!(auto_status, StatusCode::NO_CONTENT);
    assert_eq!(auto_headers[header::CACHE_CONTROL], "no-store");
    assert!(!auto_headers.contains_key(header::LOCATION));
    assert!(auto_body.is_empty());

    // An emoji counts as two UTF-16 units in the browser text node.
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &place_offset(&csrf, first_version, 1, 8),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &place_offset(&csrf, first_version, 1, 7),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let offset: i32 = sqlx::query_scalar(
        "SELECT char_offset FROM reader_book_positions WHERE user_id=$1 AND book_id=$2",
    )
    .bind(one)
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offset, 7);
    // A delayed earlier offset in the same block cannot undo a later one.
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &place_offset(&csrf, first_version, 1, 4),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, _, exact_html) = call(&app, "GET", &read_url, "", &browser, "").await;
    assert!(exact_html.contains("href=\"#p-1\" data-char-offset=\"7\""));
    let offset: i32 = sqlx::query_scalar(
        "SELECT char_offset FROM reader_book_positions WHERE user_id=$1 AND book_id=$2",
    )
    .bind(one)
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offset, 7);

    // Parallel tabs are serialized by PostgreSQL's upsert condition.
    let (a, b) = tokio::join!(
        call(
            &app,
            "POST",
            &place_url,
            &middle,
            &browser,
            "http://localhost:3000"
        ),
        call(
            &app,
            "POST",
            &place_url,
            &last,
            &browser,
            "http://localhost:3000"
        )
    );
    assert_eq!(a.0, StatusCode::SEE_OTHER);
    assert_eq!(b.0, StatusCode::SEE_OTHER);
    assert_eq!(current_position(&pool, one, book_id).await, (chapter_id, 4));
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &first,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(current_position(&pool, one, book_id).await, (chapter_id, 4));
    let (status, _, library) = call(&app, "GET", "/me/library", "", &one_cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(library.contains("&lt;Private &amp; book&gt;"));
    assert!(library.contains(&format!("href=\"/read/{chapter_id}#p-4\"")));
    let (_, _, foreign_library) = call(&app, "GET", "/me/library", "", &two_cookie, "").await;
    assert!(!foreign_library.contains("Private &amp; book"));

    // A delayed autosave must be successful as a no-op and never move back.
    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &middle,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(current_position(&pool, one, book_id).await, (chapter_id, 4));

    // Edit creates a new immutable version. Old offsets cannot be re-applied.
    let second_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id, version_no, body_markdown) \
         VALUES ($1, 2, 'Brand-new section\n\nSecond') RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(&pool)
    .await
    .expect("new text");
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter_id)
        .bind(second_version)
        .execute(&pool)
        .await
        .expect("publish new");
    sqlx::query("UPDATE chapters SET edit_revision=edit_revision+1 WHERE id=$1")
        .bind(chapter_id)
        .execute(&pool)
        .await
        .expect("update revision");
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &last,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, _, library) = call(&app, "GET", "/me/library", "", &one_cookie, "").await;
    assert!(library.contains(&format!("href=\"/read/{chapter_id}\"")));
    assert!(!library.contains(&format!("href=\"/read/{chapter_id}#p-4\"")));
    let (status, _, new_html) = call(&app, "GET", &read_url, "", &browser, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(new_html.contains("Brand-new section"));
    assert!(!new_html.contains("К сохранённому месту: отрывок"));
    let position = place(&csrf, second_version, 1);
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &position,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(current_position(&pool, one, book_id).await, (chapter_id, 1));

    assert_eq!(
        call(
            &app,
            "POST",
            &autosave_url,
            &last,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // Saving a later chapter supersedes earlier progress in the same book.
    let (next_chapter, next_version) =
        chapter(&pool, book_id, 2, "Следующая глава", "one\n\ntwo").await;
    let (_, _, first_with_next) = call(&app, "GET", &read_url, "", &browser, "").await;
    assert!(first_with_next.contains(&format!("href=\"/read/{next_chapter}\"")));
    let (_, _, next_with_prev) = call(
        &app,
        "GET",
        &format!("/read/{next_chapter}"),
        "",
        &browser,
        "",
    )
    .await;
    assert!(next_with_prev.contains(&format!("href=\"/read/{chapter_id}\"")));
    let (_, _, public_first) =
        call(&app, "GET", &format!("/reader/{chapter_id}"), "", "", "").await;
    assert!(public_first.contains(&format!("href=\"/reader/{next_chapter}\"")));

    let next_url = format!("/read/{next_chapter}/place");
    assert_eq!(
        call(
            &app,
            "POST",
            &next_url,
            &place(&csrf, next_version, 2),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        current_position(&pool, one, book_id).await,
        (next_chapter, 2)
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &place_url,
            &position,
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        current_position(&pool, one, book_id).await,
        (next_chapter, 2)
    );

    // A book hold must block both direct reading and display in personal library.
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .expect("hold");
    let (_, _, hidden_library) = call(&app, "GET", "/me/library", "", &one_cookie, "").await;
    assert!(!hidden_library.contains("Private &amp; book"));
    assert_eq!(
        call(&app, "GET", &read_url, "", &browser, "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &next_url,
            &place(&csrf, next_version, 2),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .expect("release");
    let (_, _, after) = call(&app, "GET", "/me/library", "", &one_cookie, "").await;
    assert!(after.contains("Следующая глава"));

    // A public reader must never disclose a link to a blocked chapter.
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(next_chapter)
        .execute(&pool)
        .await
        .expect("chapter hold");
    let (_, _, no_next) = call(&app, "GET", &read_url, "", &browser, "").await;
    assert!(!no_next.contains(&format!("href=\"/read/{next_chapter}\"")));
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(next_chapter)
        .execute(&pool)
        .await
        .expect("remove chapter hold");

    // An active purchased/paid restriction is still enforced; cursor is not a purchase.
    sqlx::query("UPDATE chapters SET access_level='paid', price_amount=100 WHERE id=$1")
        .bind(next_chapter)
        .execute(&pool)
        .await
        .expect("paid");
    let (_, _, filtered) = call(&app, "GET", "/me/library", "", &one_cookie, "").await;
    assert!(!filtered.contains("Следующая глава"));
    assert_eq!(
        call(
            &app,
            "POST",
            &next_url,
            &place(&csrf, next_version, 2),
            &browser,
            "http://localhost:3000"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Remove only this test's records; FKs cascade bookmarks and sessions.
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .expect("delete book");
    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(one)
        .bind(two)
        .execute(&pool)
        .await
        .expect("users");
}

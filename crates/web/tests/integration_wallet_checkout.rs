//! Real HTTP/PostgreSQL checkout: auth, stale quote, replay, atomic ledger and pin.
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
    body: &str,
    cookie: &str,
    origin: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(url);
    if !cookie.is_empty() {
        builder = builder.header(header::COOKIE, cookie);
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
    let bytes = to_bytes(response.into_body(), 512 * 1024)
        .await
        .expect("bounded response");
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

fn cookie(headers: &HeaderMap, name: &str) -> String {
    headers.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .find(|h| h.starts_with(&format!("{name}=")))
        .unwrap()
        .split(';').next().unwrap().to_owned()
}

fn field(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1).expect("form value").split('"').next().unwrap().to_owned()
}

fn form(fields: &[(&str, &str)]) -> String {
    // The generated command and CSRF tokens, numeric quote fields, need no
    // percent encoding; adding an arbitrary value must use a form encoder.
    fields.iter().map(|(key, val)| format!("{key}={val}"))
        .collect::<Vec<_>>().join("&")
}

fn quote_form(html: &str) -> String {
    let values = ["csrf_token", "command_key", "expected_version_id", "expected_price_minor"];
    form(&values.map(|name| (name, field(html, name))).iter()
        .map(|(name, val)| (*name, val.as_str())).collect::<Vec<_>>())
}

async fn account(accounts: &PgAccounts, suffix: &str, label: &str) -> (i64, String) {
    let email = format!("{label}-{suffix}@example.invalid").to_ascii_lowercase();
    let hash = hash_password("long password for test account").unwrap();
    let id = accounts.insert_user(&email, "Покупатель", &hash).await.unwrap();
    let token = generate_session_token();
    accounts.insert_session(
        id,
        &session_token_digest(&token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    (id, format!("bkr-dev-session={token}"))
}

async fn paid_chapter(pool: &PgPool, book_id: i64, no: i32, price: i64) -> (i64, i64) {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters (book_id,chapter_no,title,visibility,access_level,price_amount) \
         VALUES ($1,$2,$3,'public','paid',$4) RETURNING id",
    ).bind(book_id).bind(no).bind(format!("Платная глава {no}"))
        .bind(price).fetch_one(pool).await.unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id,version_no,body_markdown) \
         VALUES ($1,1,$2) RETURNING id",
    ).bind(id).bind(format!("PAID_CONTENT_{no}"))
        .fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES ($1,$2)")
        .bind(id).bind(version).execute(pool).await.unwrap();
    (id, version)
}

// Funding never comes from the browser. Only trusted, previously verified
// payments will be permitted to credit wallet accounts in a later PR.
// This test fixture represents pre-existing verified funding.
async fn fund_fixture(pool: &PgPool, user_id: i64, amount: i64, reference: &str) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO reader_wallet_accounts(user_id,balance_minor) VALUES ($1,$2)")
        .bind(user_id).bind(amount).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries \
         (user_id,kind,delta_minor,balance_after_minor,reference) \
         VALUES ($1,'verified_funding',$2,$2,$3)",
    ).bind(user_id).bind(amount).bind(reference)
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

async fn balance(pool: &PgPool, user: i64) -> i64 {
    sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
        .bind(user).fetch_one(pool).await.unwrap()
}

async fn count(pool: &PgPool, table: &str, user: i64) -> i64 {
    let query = match table {
        "purchases" => "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1",
        "ledger_debits" => "SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1 AND kind='chapter_purchase'",
        "commands" => "SELECT count(*) FROM chapter_purchase_commands WHERE user_id=$1",
        _ => panic!("unknown bounded query"),
    };
    sqlx::query_scalar(query).bind(user).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL TEST_DATABASE_URL"]
async fn integration_checkout_wallet_posting_and_replays_are_one_transaction() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool).await.unwrap();
    assert!(matches!(db.as_str(), "bokarust_ci" | "bokarust_test"));
    MIGRATOR.run(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.expect("migration replay");
    let accounts = PgAccounts::from_pool(pool.clone());
    let suffix = generate_session_token();
    let (buyer, buyer_cookie) = account(&accounts, &suffix, "buyer").await;
    let (_, stranger_cookie) = account(&accounts, &suffix, "stranger").await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility) \
         VALUES ($1,'ru','Test paid book','public') RETURNING id",
    ).bind(format!("paidbook:{suffix}")).fetch_one(&pool).await.unwrap();
    let (chapter, first_version) = paid_chapter(&pool, book, 1, 100).await;
    let (second, second_version) = paid_chapter(&pool, book, 2, 100).await;
    let app = router_with_auth_and_reading(
        Arc::new(PgCatalog::new(pool.clone())),
        Arc::new(PgReader::new(pool.clone())),
        accounts, false, "http://localhost:3000".into(),
    );
    let buy_url = format!("/chapters/{chapter}/buy");

    // Payment page and paid text are unavailable to guests and strangers.
    assert_eq!(call(&app, "GET", &buy_url, "", "", "").await.0, StatusCode::SEE_OTHER);
    assert_eq!(call(&app, "GET", &format!("/reader/{chapter}"), "", "", "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &format!("/read/{chapter}"), "", &buyer_cookie, "").await.0, StatusCode::NOT_FOUND);
    let (_, _, book_html) = call(&app, "GET", &format!("/books/{book}"), "", "", "").await;
    assert!(book_html.contains(&format!("href=\"{buy_url}\"")));
    assert!(!book_html.contains("PAID_CONTENT_1"));

    let (status, headers, html) = call(&app, "GET", &buy_url, "", &buyer_cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers["x-robots-tag"], "noindex, noarchive, nosnippet");
    assert!(html.contains("Пополнение кошелька извне пока не подключено"));
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let browser_cookie = format!("{buyer_cookie}; {csrf_cookie}");
    let quoted = quote_form(&html);
    assert_eq!(field(&html, "expected_version_id"), first_version.to_string());
    assert_eq!(field(&html, "expected_price_minor"), "100");

    assert_eq!(
        call(&app, "POST", &buy_url, &quoted, &buyer_cookie, "http://localhost:3000").await.0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(&app, "POST", &buy_url, &quoted, &browser_cookie, "https://evil.invalid").await.0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(&app, "POST", &buy_url, &quoted, &csrf_cookie, "http://localhost:3000").await.0,
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        call(&app, "POST", &buy_url, &quoted, &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::PAYMENT_REQUIRED,
    );
    assert_eq!(count(&pool, "commands", buyer).await, 0);
    assert_eq!(count(&pool, "purchases", buyer).await, 0);
    assert_eq!(count(&pool, "ledger_debits", buyer).await, 0);

    fund_fixture(&pool, buyer, 150, &format!("verified:{suffix}")).await;
    // Stale price, version or reusing the same key for a different request
    // may neither spend nor grant any rights.
    sqlx::query("UPDATE chapters SET price_amount=120 WHERE id=$1")
        .bind(chapter).execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "POST", &buy_url, &quoted, &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(balance(&pool, buyer).await, 150);
    assert_eq!(count(&pool, "commands", buyer).await, 0);

    // Show the current revision and price. Payment uses only these server
    // authoritative fields, not the price supplied by an untrusted request.
    let (_, _, fresh) = call(&app, "GET", &buy_url, "", &browser_cookie, "").await;
    let fresh_form = quote_form(&fresh);
    assert_eq!(field(&fresh, "expected_price_minor"), "120");
    let (status, headers, _) =
        call(&app, "POST", &buy_url, &fresh_form, &browser_cookie, "http://localhost:3000").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], format!("/me/purchases/{chapter}"));
    assert_eq!(balance(&pool, buyer).await, 30);
    assert_eq!(count(&pool, "purchases", buyer).await, 1);
    assert_eq!(count(&pool, "ledger_debits", buyer).await, 1);
    assert_eq!(count(&pool, "commands", buyer).await, 1);
    let (status, _, purchased_text) = call(
        &app, "GET", &format!("/me/purchases/{chapter}"), "", &buyer_cookie, "",
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert!(purchased_text.contains("PAID_CONTENT_1"));
    assert_eq!(call(&app, "GET", &format!("/me/purchases/{chapter}"), "", &stranger_cookie, "").await.0,
        StatusCode::NOT_FOUND);

    // Unknown network result: repeating exactly the original request resolves
    // to the same completed purchase even after live price changes.
    sqlx::query("UPDATE chapters SET price_amount=130 WHERE id=$1")
        .bind(chapter).execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "POST", &buy_url, &fresh_form, &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(balance(&pool, buyer).await, 30);
    assert_eq!(count(&pool, "ledger_debits", buyer).await, 1);
    let mismatched = fresh_form.replace("expected_price_minor=120", "expected_price_minor=130");
    assert_eq!(
        call(&app, "POST", &buy_url, &mismatched, &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::CONFLICT
    );
    let (_, _, duplicate_form_html) = call(&app, "GET", &buy_url, "", &browser_cookie, "").await;
    assert_eq!(
        call(&app, "POST", &buy_url, &quote_form(&duplicate_form_html), &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::CONFLICT,
    );
    assert_eq!(balance(&pool, buyer).await, 30);

    // A second chapter whose price exceeds available funds is never acquired.
    let second_url = format!("/chapters/{second}/buy");
    let (_, _, second_html) = call(&app, "GET", &second_url, "", &browser_cookie, "").await;
    assert_eq!(field(&second_html, "expected_version_id"), second_version.to_string());
    assert_eq!(
        call(&app, "POST", &second_url, &quote_form(&second_html), &browser_cookie, "http://localhost:3000").await.0,
        StatusCode::PAYMENT_REQUIRED,
    );
    assert_eq!(balance(&pool, buyer).await, 30);

    // Ledger and balance match exactly; the sole debit points at the same
    // historical purchase and the published revision at checkout time.
    let (amount, version_id, ledger_delta): (i64, i64, i64) = sqlx::query_as(
        "SELECT p.amount_minor, p.version_id, e.delta_minor \
         FROM chapter_purchase_facts AS p \
         JOIN reader_wallet_entries AS e ON e.purchase_id = p.id \
         WHERE p.user_id=$1 AND p.chapter_id=$2",
    ).bind(buyer).bind(chapter).fetch_one(&pool).await.unwrap();
    assert_eq!((amount, version_id, ledger_delta), (120, first_version, -120));
    let ledger_total: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(delta_minor),0) FROM reader_wallet_entries WHERE user_id=$1",
    ).bind(buyer).fetch_one(&pool).await.unwrap();
    assert_eq!(ledger_total, 30);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL TEST_DATABASE_URL"]
async fn integration_checkout_parallel_commands_and_failure_rollback() {
    let pool = PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap()).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()").fetch_one(&pool).await.unwrap();
    assert!(matches!(db.as_str(), "bokarust_ci" | "bokarust_test"));
    MIGRATOR.run(&pool).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let suffix = generate_session_token();
    let (user, _) = account(&accounts, &suffix, "racer").await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility) \
         VALUES ($1,'ru','Race book','public') RETURNING id",
    ).bind(format!("race:{suffix}")).fetch_one(&pool).await.unwrap();
    let (a, version_a) = paid_chapter(&pool, book, 1, 100).await;
    let (b, version_b) = paid_chapter(&pool, book, 2, 100).await;
    fund_fixture(&pool, user, 150, &format!("verified-race:{suffix}")).await;
    let repo = bokarust_adapters::postgres_checkout::PgChapterCheckout::new(pool.clone());

    let (ra, rb) = tokio::join!(
        repo.purchase(user, a, version_a, 100, &generate_session_token()),
        repo.purchase(user, b, version_b, 100, &generate_session_token()),
    );
    assert_eq!(ra.is_ok() as u8 + rb.is_ok() as u8, 1);
    assert_eq!(balance(&pool, user).await, 50);
    assert_eq!(count(&pool, "purchases", user).await, 1);
    assert_eq!(count(&pool, "ledger_debits", user).await, 1);

    // Force a ledger UNIQUE conflict after the wallet UPDATE + entitlement
    // INSERT to verify both earlier writes roll back as one transaction.
    let another: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES ($1,'Test', $2) RETURNING id",
    )
    .bind(format!("rollback-{suffix}@example.invalid").to_ascii_lowercase())
    .bind(hash_password("long test password").unwrap())
    .fetch_one(&pool).await.unwrap();
    let command_key = generate_session_token();
    let collision = format!("wallet:{another}:{command_key}");
    fund_fixture(&pool, another, 200, &collision).await;
    let (third, third_version) = paid_chapter(&pool, book, 3, 100).await;
    assert_eq!(
        repo.purchase(another, third, third_version, 100, &command_key).await,
        Err(bokarust_adapters::postgres_checkout::CheckoutError::Unavailable)
    );
    assert_eq!(balance(&pool, another).await, 200);
    assert_eq!(count(&pool, "purchases", another).await, 0);
    assert_eq!(count(&pool, "commands", another).await, 0);
    assert_eq!(count(&pool, "ledger_debits", another).await, 0);
}

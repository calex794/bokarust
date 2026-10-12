//! Real browser flow against an isolated PostgreSQL 18; never a production DB.
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use bokarust_adapters::{
    account_security::{generate_session_token, session_token_digest},
    in_memory::InMemoryCatalog,
    postgres_accounts::PgAccounts,
};
use bokarust_web::router_with_auth;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    cookies: &str,
    origin: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder().method(method).uri(path);
    if !cookies.is_empty() {
        request = request.header(header::COOKIE, cookies);
    }
    if method == "POST" {
        request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    if let Some(value) = origin {
        request = request.header(header::ORIGIN, value);
    }
    let mut request = request.body(Body::from(body.to_owned())).expect("request");
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:58321"
            .parse::<std::net::SocketAddr>()
            .expect("test peer"),
    ));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("limited body");
    (
        status,
        headers,
        String::from_utf8(body.to_vec()).expect("utf8"),
    )
}

fn cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
        .expect("server sets the named cookie")
        .split(';')
        .next()
        .expect("cookie component")
        .to_owned()
}

fn csrf(html: &str) -> String {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("CSRF hidden input")
        .split('"')
        .next()
        .expect("token")
        .to_owned()
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'*') {
                (c as char).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect::<String>()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn accounts_and_pool() -> (PgAccounts, PgPool) {
    let url =
        std::env::var("TEST_DATABASE_URL").expect("auth integration requires TEST_DATABASE_URL");
    let accounts = PgAccounts::connect(&url)
        .await
        .expect("connect test postgres");
    let pool = accounts.pool().clone();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database name");
    assert!(
        matches!(name.as_str(), "bokarust_test" | "bokarust_ci"),
        "auth integration refuses non-test database"
    );
    accounts.migrate().await.expect("migration");
    (accounts, pool)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL via TEST_DATABASE_URL"]
async fn integration_auth_registration_to_logout_and_login() {
    let (accounts, pool) = accounts_and_pool().await;
    let app = router_with_auth(
        Arc::new(InMemoryCatalog::default()),
        accounts.clone(),
        false,
        "http://localhost:3000".to_owned(),
    );
    let suffix = generate_session_token().to_ascii_lowercase();
    let email = format!("ci-auth-{suffix}@example.invalid");
    let password = "safe-password-12345";
    let display_name = "<script>alert(1)</script>";

    let (status, headers, _) = call(&app, "GET", "/me", "", "", None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login");

    let (status, headers, html) = call(&app, "GET", "/register", "", "", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("method=\"post\""));
    assert!(html.contains("name=\"email\""));
    assert!(html.contains("name=\"display_name\""));
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let token = csrf(&html);
    let registration = form(&[
        ("email", &email),
        ("display_name", display_name),
        ("password", password),
        ("csrf_token", &token),
    ]);
    let (status, _, _) = call(
        &app,
        "POST",
        "/register",
        &registration,
        &csrf_cookie,
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let without_csrf = form(&[
        ("email", &email),
        ("display_name", display_name),
        ("password", password),
    ]);
    let (status, _, _) = call(&app, "POST", "/register", &without_csrf, &csrf_cookie, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, headers, _) =
        call(&app, "POST", "/register", &registration, &csrf_cookie, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/me");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let session_cookie = cookie(&headers, "bkr-dev-session");
    assert!(!session_cookie.contains(password));

    let (status, headers, html) = call(&app, "GET", "/me", "", &session_cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let logout_form = form(&[("csrf_token", &csrf(&html))]);

    let (status, _, _) = call(
        &app,
        "POST",
        "/logout",
        &logout_form,
        &format!("{session_cookie}; {csrf_cookie}"),
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = call(
        &app,
        "POST",
        "/logout",
        &logout_form,
        &format!("{session_cookie}; {csrf_cookie}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, headers, _) = call(&app, "GET", "/me", "", &session_cookie, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login");

    let (status, headers, html) = call(&app, "GET", "/login", "", "", None).await;
    assert_eq!(status, StatusCode::OK);
    let csrf_cookie = cookie(&headers, "bkr-dev-csrf");
    let token = csrf(&html);
    let wrong = form(&[
        ("email", &email),
        ("password", "wrong-password"),
        ("csrf_token", &token),
    ]);
    let (status, _, error) = call(&app, "POST", "/login", &wrong, &csrf_cookie, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(error.contains("Неверный адрес"));
    assert!(!error.contains(&email));

    let correct = form(&[
        ("email", &email),
        ("password", password),
        ("csrf_token", &token),
    ]);
    let (status, headers, _) = call(
        &app,
        "POST",
        "/login?next=https://evil.example",
        &correct,
        &csrf_cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/me");
    let new_session = cookie(&headers, "bkr-dev-session");
    let (status, _, _) = call(&app, "GET", "/me", "", &new_session, None).await;
    assert_eq!(status, StatusCode::OK);

    let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email_normalized = $1")
        .bind(&email)
        .fetch_one(&pool)
        .await
        .expect("created user");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL via TEST_DATABASE_URL"]
async fn integration_auth_rate_limiter_is_atomic_and_finite() {
    let (accounts, _) = accounts_and_pool().await;
    let key = session_token_digest(&format!("test-rate-{}", generate_session_token()));
    for _ in 0..8 {
        assert!(accounts.permit_auth_attempt(&key, 8, 600).await.unwrap());
    }
    assert!(!accounts.permit_auth_attempt(&key, 8, 600).await.unwrap());
    sqlx::query(
        "UPDATE auth_attempts SET period_started = now() - INTERVAL '1 hour' WHERE key_hash = $1",
    )
    .bind(key.as_slice())
    .execute(accounts.pool())
    .await
    .expect("expire window");
    assert!(accounts.permit_auth_attempt(&key, 8, 600).await.unwrap());
    accounts
        .clear_auth_attempt(&key)
        .await
        .expect("cleanup rate key");
}

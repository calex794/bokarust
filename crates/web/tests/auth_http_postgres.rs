//! End-to-end native HTML auth contract; run against an isolated PostgreSQL 18 DB.
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use bokarust_adapters::{
    account_security::{generate_session_token, session_token_digest},
    in_memory::InMemoryCatalog,
    postgres_accounts::PgAccounts,
    MIGRATOR,
};
use bokarust_web::router_with_auth;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

fn cookie(headers: &HeaderMap, prefix: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with(prefix))
        .expect("expected Set-Cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn csrf(html: &str) -> String {
    let marker = "name=\"csrf\" value=\"";
    let position = html.find(marker).expect("hidden CSRF input") + marker.len();
    html[position..].split('"').next().unwrap().to_owned()
}

async fn request(
    app: Router,
    method: Method,
    path: &str,
    cookies: Option<&str>,
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookies) = cookies {
        builder = builder.header(header::COOKIE, cookies);
    }
    if !body.is_empty() {
        builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 via TEST_DATABASE_URL=bokarust_auth_test"]
async fn registration_session_logout_and_login_roundtrip() {
    let url = std::env::var("TEST_DATABASE_URL").expect("database URL required");
    let pool = PgPool::connect(&url).await.expect("connect test PostgreSQL");
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "bokarust_auth_test", "refuse non-test database");
    MIGRATOR.run(&pool).await.expect("migrations");
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = router_with_auth(
        Arc::new(InMemoryCatalog::default()),
        accounts.clone(),
        true,
    );
    let email = format!("bkr95-{}@example.invalid", generate_session_token());
    let password = "A very long test password #2026!";
    let (status, _, _) = request(app.clone(), Method::GET, "/me", None, "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "guest cannot view /me");
    let (status, headers, form) =
        request(app.clone(), Method::GET, "/register", None, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(form.contains("action=\"/register\""));
    assert!(!form.contains("disabled"));
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&headers, "__Host-bokarust_csrf=");
    let csrf_token = csrf(&form);
    let payload = format!(
        "name=Alice%3Cscript%3E&email={email}&password=A%20very%20long%20test%20password%20%232026%21&csrf={csrf_token}"
    );
    let (status, _, _) =
        request(app.clone(), Method::POST, "/register", None, &payload).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "missing CSRF cookie");
    let (status, _, _) = request(
        app.clone(),
        Method::POST,
        "/register",
        Some(&csrf_cookie),
        &payload.replace(&csrf_token, "invalid"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "invalid form token");
    let (status, headers, _) =
        request(app.clone(), Method::POST, "/register", Some(&csrf_cookie), &payload).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/me");
    let sid = cookie(&headers, "__Host-bokarust_sid=");
    for requirement in ["Secure", "HttpOnly", "SameSite=Strict"] {
        assert!(headers.get_all(header::SET_COOKIE).iter().any(|cookie| {
            cookie.to_str().unwrap().starts_with("__Host-bokarust_sid=")
                && cookie.to_str().unwrap().contains(requirement)
        }));
    }
    let both = format!("{csrf_cookie}; {sid}");
    let (status, _, body) =
        request(app.clone(), Method::GET, "/me", Some(&both), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Alice&lt;script&gt;"));
    assert!(!body.contains("Alice<script>"));
    assert!(body.contains(&email.to_lowercase()));
    assert!(!body.contains(password));
    let profile_csrf = csrf(&body);
    let (status, _, _) = request(
        app.clone(),
        Method::POST,
        "/logout",
        Some(&sid),
        &format!("csrf={profile_csrf}"),
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "logout requires CSRF");
    let (status, headers, _) = request(
        app.clone(),
        Method::POST,
        "/logout",
        Some(&both),
        &format!("csrf={profile_csrf}"),
    ).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login");
    assert!(headers.get_all(header::SET_COOKIE).iter().any(|cookie| {
        cookie.to_str().unwrap().starts_with("__Host-bokarust_sid=;")
            && cookie.to_str().unwrap().contains("Max-Age=0")
    }));
    let digest = session_token_digest(sid.split('=').nth(1).unwrap());
    assert_eq!(accounts.active_session_user(&digest).await.unwrap(), None);
    let (status, _, _) =
        request(app.clone(), Method::GET, "/me", Some(&sid), "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (status, headers, login) =
        request(app.clone(), Method::GET, "/login", None, "").await;
    assert_eq!(status, StatusCode::OK);
    let login_cookie = cookie(&headers, "__Host-bokarust_csrf=");
    let login_csrf = csrf(&login);
    let bad = format!("email={email}&password=wrong&csrf={login_csrf}");
    let (status, _, _) =
        request(app.clone(), Method::POST, "/login", Some(&login_cookie), &bad).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let good = format!(
        "email={email}&password=A%20very%20long%20test%20password%20%232026%21&csrf={login_csrf}"
    );
    let (status, headers, _) = request(
        app.clone(), Method::POST, "/login?next=https://evil.invalid",
        Some(&login_cookie), &good
    ).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/me", "no open redirect");
    let sid2 = cookie(&headers, "__Host-bokarust_sid=");
    let (status, _, _) =
        request(app.clone(), Method::GET, "/me", Some(&sid2), "").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) =
        request(app.clone(), Method::GET, "/logout", Some(&sid2), "").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _, _) = request(
        app.clone(), Method::POST, "/register", Some(&csrf_cookie), &payload
    ).await;
    assert_eq!(status, StatusCode::CONFLICT);

    sqlx::query("DELETE FROM users WHERE email_normalized = $1")
        .bind(&email)
        .execute(&pool)
        .await
        .expect("cleanup test user");
    pool.close().await;
}

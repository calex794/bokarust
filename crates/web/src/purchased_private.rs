//! Authenticated owner-only access to purchased immutable chapter versions.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    purchased_pages,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use bokarust_adapters::{
    account_security::session_token_digest, postgres_accounts::PgAccounts,
    postgres_purchased::PgPurchasedReader,
};

#[derive(Clone)]
struct PurchasedState {
    accounts: PgAccounts,
    purchases: PgPurchasedReader,
    cookies: CookiePolicy,
}

pub(crate) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let state = PurchasedState {
        purchases: PgPurchasedReader::new(accounts.pool().clone()),
        accounts,
        cookies,
    };
    Router::new()
        .route("/me/purchases", get(purchases))
        .route("/me/purchases/{chapter_id}", get(purchased_chapter))
        .with_state(state)
}

fn private_page<T: axum::response::IntoResponse>(response: T) -> Response {
    let mut response = private_response(response, &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, noarchive, nosnippet"),
    );
    response
}

async fn session_user(state: &PurchasedState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn session_failure(status: StatusCode) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        private_page(Redirect::to("/login"))
    } else {
        private_page(status)
    }
}

async fn purchases(State(state): State<PurchasedState>, headers: HeaderMap) -> Response {
    let user = match session_user(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return session_failure(status),
    };
    match state.purchases.library(user).await {
        Ok(items) => private_page(purchased_pages::library(&items)),
        Err(_) => private_page(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn purchased_chapter(
    State(state): State<PurchasedState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user = match session_user(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return session_failure(status),
    };
    match state.purchases.owned_chapter(user, chapter_id).await {
        Ok(Some(chapter)) => private_page(purchased_pages::owned_chapter(&chapter)),
        Ok(None) => private_page(StatusCode::NOT_FOUND),
        Err(_) => private_page(StatusCode::SERVICE_UNAVAILABLE),
    }
}

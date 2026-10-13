//! Authenticated CSRF-checked purchase from an already funded internal wallet.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    checkout_pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use bokarust_adapters::{
    account_security::{generate_session_token, session_token_digest},
    postgres_accounts::PgAccounts,
    postgres_checkout::{CheckoutError, PgChapterCheckout},
};
use serde::Deserialize;

#[derive(Clone)]
struct CheckoutState {
    accounts: PgAccounts,
    checkout: PgChapterCheckout,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct CheckoutForm {
    csrf_token: String,
    command_key: String,
    expected_version_id: i64,
    expected_price_minor: i64,
}

pub(crate) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let state = CheckoutState {
        checkout: PgChapterCheckout::new(accounts.pool().clone()),
        accounts,
        cookies,
    };
    Router::new()
        .route("/chapters/{chapter_id}/buy", get(offer).post(purchase))
        .layer(DefaultBodyLimit::max(2048))
        .with_state(state)
}

fn private_page<T: IntoResponse>(result: T, cookies: &[String]) -> Response {
    let mut response = private_response(result, cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, noarchive, nosnippet"),
    );
    response
}

fn fail(error: CheckoutError) -> Response {
    match error {
        CheckoutError::Invalid => private_page(StatusCode::BAD_REQUEST, &[]),
        CheckoutError::Missing => private_page(StatusCode::NOT_FOUND, &[]),
        CheckoutError::Changed | CheckoutError::AlreadyOwned => private_page(
            (
                StatusCode::CONFLICT,
                "Цена, редакция или право изменились. Обновите страницу покупки.",
            ),
            &[],
        ),
        CheckoutError::InsufficientFunds => private_page(
            (
                StatusCode::PAYMENT_REQUIRED,
                "Недостаточно средств на внутреннем кошельке.",
            ),
            &[],
        ),
        CheckoutError::Unavailable => private_page(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

async fn session_user(state: &CheckoutState, headers: &HeaderMap) -> Result<i64, StatusCode> {
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

async fn offer(
    State(state): State<CheckoutState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user_id = match session_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(StatusCode::UNAUTHORIZED) => return private_page(Redirect::to("/login"), &[]),
        Err(error) => return private_page(error, &[]),
    };
    let (offer, balance) = match tokio::try_join!(
        state.checkout.offer(user_id, chapter_id),
        state.checkout.balance(user_id),
    ) {
        Ok((Some(offer), balance)) => (offer, balance),
        Ok((None, _)) => return private_page(StatusCode::NOT_FOUND, &[]),
        Err(error) => return fail(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    private_page(
        checkout_pages::offer(&offer, balance, &csrf, &key),
        &cookies,
    )
}

async fn purchase(
    State(state): State<CheckoutState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CheckoutForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return private_page(StatusCode::FORBIDDEN, &[]);
    }
    let user_id = match session_user(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return private_page(error, &[]),
    };
    match state
        .checkout
        .purchase(
            user_id,
            chapter_id,
            form.expected_version_id,
            form.expected_price_minor,
            &form.command_key,
        )
        .await
    {
        Ok(result) => private_page(
            Redirect::to(&format!("/me/purchases/{}", result.chapter_id)),
            &[],
        ),
        Err(error) => fail(error),
    }
}

//! Native HTTP account flows. No browser-side JavaScript or in-memory sessions.
use crate::{
    auth_cookies::{self, SESSION_SECONDS},
    auth_state::AuthState,
    pages,
    state::AppState,
};
use axum::{
    extract::{Form, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bokarust_adapters::{
    account_security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
};
use bokarust_application::auth::validate_registration;
use bokarust_domain::accounts::normalize_email;
use serde::Deserialize;
use std::num::NonZeroU32;

#[derive(Deserialize)]
pub(crate) struct Registration {
    email: String,
    name: String,
    password: String,
    csrf: String,
}

#[derive(Deserialize)]
pub(crate) struct Login {
    email: String,
    password: String,
    csrf: String,
}

#[derive(Deserialize)]
pub(crate) struct Logout {
    csrf: String,
}

fn auth_state(state: &AppState) -> Result<&AuthState, StatusCode> {
    state.auth.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

fn form_error(status: StatusCode, page: axum::response::Html<String>) -> Response {
    auth_cookies::protect((status, page).into_response())
}

fn signed_in(auth: &AuthState, token: &str) -> Response {
    let mut response = auth_cookies::redirect("/me");
    response
        .headers_mut()
        .append(header::SET_COOKIE, auth.cookies.set_session(token));
    response
}

pub(crate) async fn get_register(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    let (csrf, cookie) = auth.cookies.csrf_for_page(&headers);
    let mut response = auth_cookies::protect(pages::register_enabled(&csrf, None).into_response());
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

pub(crate) async fn post_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Registration>,
) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !auth.cookies.verify_csrf(&headers, &form.csrf) {
        return auth_cookies::error(StatusCode::FORBIDDEN);
    }
    let new_account = match validate_registration(&form.email, &form.name, &form.password) {
        Ok(value) => value,
        Err(problem) => {
            return form_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                pages::register_enabled(&form.csrf, Some(problem)),
            );
        }
    };
    let password_hash = match auth.password_job(move || hash_password(&form.password)).await {
        Ok(Ok(hash)) => hash,
        Ok(Err(_)) | Err(StatusCode::SERVICE_UNAVAILABLE) => {
            return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
        }
        Err(code) => return auth_cookies::error(code),
    };
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_SECONDS).expect("session lifetime is nonzero");
    match auth
        .accounts
        .register_with_session(
            &new_account.email_normalized,
            &new_account.display_name,
            &password_hash,
            &digest,
            ttl,
        )
        .await
    {
        Ok(_) => signed_in(auth, &token),
        Err(error) if error.as_database_error().and_then(|db| db.code()).as_deref() == Some("23505") => {
            form_error(
                StatusCode::CONFLICT,
                pages::register_enabled(&form.csrf, Some("Адрес почты уже зарегистрирован")),
            )
        }
        Err(_) => auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(crate) async fn get_login(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    let (csrf, cookie) = auth.cookies.csrf_for_page(&headers);
    let mut response = auth_cookies::protect(pages::login_enabled(&csrf, None).into_response());
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

pub(crate) async fn post_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Login>,
) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !auth.cookies.verify_csrf(&headers, &form.csrf) {
        return auth_cookies::error(StatusCode::FORBIDDEN);
    }
    let normalized = match normalize_email(&form.email) {
        Ok(email) => email,
        Err(_) => {
            return form_error(
                StatusCode::UNAUTHORIZED,
                pages::login_enabled(&form.csrf, Some("Неверная почта или пароль")),
            );
        }
    };
    if !auth.allow_attempt(&normalized) {
        return auth_cookies::error(StatusCode::TOO_MANY_REQUESTS);
    }
    let account = match auth.accounts.find_user(&normalized).await {
        Ok(value) => value,
        Err(_) => return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let password = form.password;
    let password_hash = account.as_ref().map(|user| user.password_hash.clone());
    let verified = match auth
        .password_job(move || match password_hash {
            Some(hash) => verify_password(&password, &hash),
            None => {
                // Unknown accounts must still pay the cost of one Argon2id operation.
                let _ = hash_password(&password);
                false
            }
        })
        .await
    {
        Ok(value) => value,
        Err(code) => return auth_cookies::error(code),
    };
    if !verified {
        return form_error(
            StatusCode::UNAUTHORIZED,
            pages::login_enabled(&form.csrf, Some("Неверная почта или пароль")),
        );
    }
    let user_id = account.expect("verified account exists").id;
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_SECONDS).expect("session lifetime is nonzero");
    if auth.accounts.insert_session(user_id, &digest, ttl).await.is_err() {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    }
    if let Some(old_token) = auth.cookies.session(&headers) {
        if auth.accounts.revoke_session(&session_token_digest(&old_token)).await.is_err() {
            // Newly created session must not be leaked if revocation fails.
            let _ = auth.accounts.revoke_session(&digest).await;
            return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    auth.forget_attempts(&normalized);
    signed_in(auth, &token)
}

pub(crate) async fn get_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(secret) = auth.cookies.session(&headers) else {
        return auth_cookies::redirect("/login");
    };
    let profile = match auth.accounts.active_profile(&session_token_digest(&secret)).await {
        Ok(Some(profile)) => profile,
        Ok(None) => return auth_cookies::redirect("/login"),
        Err(_) => return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookie) = auth.cookies.csrf_for_page(&headers);
    let mut response = auth_cookies::protect(
        pages::account(&profile.display_name, &profile.email_normalized, &csrf).into_response(),
    );
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

pub(crate) async fn post_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Logout>,
) -> Response {
    let Ok(auth) = auth_state(&state) else {
        return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !auth.cookies.verify_csrf(&headers, &form.csrf) {
        return auth_cookies::error(StatusCode::FORBIDDEN);
    }
    if let Some(secret) = auth.cookies.session(&headers) {
        if auth.accounts.revoke_session(&session_token_digest(&secret)).await.is_err() {
            return auth_cookies::error(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    let mut response = auth_cookies::redirect("/login");
    response.headers_mut().append(header::SET_COOKIE, auth.cookies.clear_session());
    response.headers_mut().append(header::SET_COOKIE, auth.cookies.clear_csrf());
    response
}

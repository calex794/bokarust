//! Authenticated reading positions. Public reader remains read-only.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_reader_response, private_response, CookiePolicy},
    reading_pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use bokarust_adapters::{
    account_security::session_token_digest, postgres_accounts::PgAccounts,
    postgres_progress::PgReadingProgress,
};
use bokarust_application::{
    reading::PublicReadingRepository,
    reading_progress::{reader_blocks, valid_place, ProgressError},
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
struct ReadingState {
    accounts: PgAccounts,
    progress: PgReadingProgress,
    reader: Arc<dyn PublicReadingRepository>,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct PositionForm {
    csrf_token: String,
    version_id: i64,
    block_index: i32,
}

pub(crate) fn router(
    accounts: PgAccounts,
    reader: Arc<dyn PublicReadingRepository>,
    cookies: CookiePolicy,
) -> Router {
    let state = ReadingState {
        progress: PgReadingProgress::new(accounts.pool().clone()),
        accounts,
        reader,
        cookies,
    };
    Router::new()
        .route("/me/library", get(library))
        .route("/read/{chapter_id}", get(read_chapter))
        .route("/read/{chapter_id}/place", post(save_place))
        .route("/read/{chapter_id}/place/auto", post(save_place_auto))
        .layer(DefaultBodyLimit::max(2048))
        .with_state(state)
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

fn failure(error: ProgressError) -> Response {
    deny(match error {
        ProgressError::Invalid => StatusCode::BAD_REQUEST,
        ProgressError::NotFound => StatusCode::NOT_FOUND,
        ProgressError::StaleVersion => StatusCode::CONFLICT,
        ProgressError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}

async fn user_id(state: &ReadingState, headers: &HeaderMap) -> Result<i64, StatusCode> {
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

fn page_error(error: StatusCode) -> Response {
    if error == StatusCode::UNAUTHORIZED {
        private_response(Redirect::to("/login"), &[])
    } else {
        deny(error)
    }
}

async fn library(State(state): State<ReadingState>, headers: HeaderMap) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(id) => id,
        Err(error) => return page_error(error),
    };
    match state.progress.continue_reading(user).await {
        Ok(books) => private_response(reading_pages::library(&books), &[]),
        Err(error) => failure(error),
    }
}

async fn read_chapter(
    State(state): State<ReadingState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(id) => id,
        Err(error) => return page_error(error),
    };
    let Ok(chapter_key) = u64::try_from(chapter_id) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let chapter = match state.reader.public_chapter(chapter_key).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Ok(version_id) = i64::try_from(chapter.version_id) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let saved = match state
        .progress
        .saved_block(user, chapter_id, version_id)
        .await
    {
        Ok(place) => place,
        Err(error) => return failure(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_reader_response(
        reading_pages::tracked_reader(chapter_id, &chapter, &csrf, saved),
        &cookies,
    )
}

async fn save_place(
    State(state): State<ReadingState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PositionForm>,
) -> Response {
    let block_index = form.block_index;
    match persist_place(&state, chapter_id, &headers, &form).await {
        Ok(()) => private_response(
            Redirect::to(&format!("/read/{chapter_id}#p-{block_index}")),
            &[],
        ),
        Err(response) => *response,
    }
}

/// No redirect or chapter HTML in response to background requests.
async fn save_place_auto(
    State(state): State<ReadingState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PositionForm>,
) -> Response {
    match persist_place(&state, chapter_id, &headers, &form).await {
        Ok(()) => private_response(StatusCode::NO_CONTENT, &[]),
        Err(response) => *response,
    }
}

/// Both the manual and background routes use exactly the same access checks.
async fn persist_place(
    state: &ReadingState,
    chapter_id: i64,
    headers: &HeaderMap,
    form: &PositionForm,
) -> Result<(), Box<Response>> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, &form.csrf_token) {
        return Err(Box::new(deny(StatusCode::FORBIDDEN)));
    }
    let user = user_id(state, headers)
        .await
        .map_err(|status| Box::new(deny(status)))?;
    let chapter_key = u64::try_from(chapter_id).map_err(|_| Box::new(deny(StatusCode::NOT_FOUND)))?;
    let chapter = state
        .reader
        .public_chapter(chapter_key)
        .await
        .map_err(|_| Box::new(deny(StatusCode::SERVICE_UNAVAILABLE)))?
        .ok_or_else(|| Box::new(deny(StatusCode::NOT_FOUND)))?;
    let version_id =
        i64::try_from(chapter.version_id).map_err(|_| Box::new(deny(StatusCode::SERVICE_UNAVAILABLE)))?;
    if version_id != form.version_id {
        return Err(Box::new(deny(StatusCode::CONFLICT)));
    }
    valid_place(
        form.block_index,
        reader_blocks(&chapter.body_markdown).len(),
    )
    .map_err(|error| Box::new(failure(error)))?;
    state
        .progress
        .save_position(user, chapter_id, version_id, form.block_index)
        .await
        .map_err(|error| Box::new(failure(error)))
}

//! Browser author flows: authenticated ownership, CSRF and transactional saves.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    author_pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use bokarust_adapters::{
    account_security::{generate_session_token, session_token_digest},
    postgres_accounts::PgAccounts,
    postgres_author::PgAuthor,
};
use bokarust_application::authoring::{self, AuthorError, SaveChapterRequest};
use serde::Deserialize;

#[derive(Clone)]
struct AuthorState {
    accounts: PgAccounts,
    author: PgAuthor,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct CreateBookForm {
    source_identity: String,
    target_language: String,
    title: String,
    command_key: String,
    csrf_token: String,
}

#[derive(Deserialize)]
struct ChapterForm {
    number: i32,
    expected_revision: i64,
    title: String,
    body_markdown: String,
    access_level: String,
    price_amount: i64,
    command_key: String,
    csrf_token: String,
}

pub(crate) fn router(accounts: PgAccounts, author: PgAuthor, cookies: CookiePolicy) -> Router {
    let state = AuthorState {
        accounts,
        author,
        cookies,
    };
    Router::new()
        .route("/me/books", get(my_books))
        .route("/me/books/{book_id}", get(manage_book))
        .route("/books/new", get(book_form).post(create_book))
        .route("/books/{book_id}/chapters/new", get(new_chapter_form))
        .route("/books/{book_id}/chapters", post(create_chapter))
        .route(
            "/books/{book_id}/chapters/{chapter_id}/edit",
            get(edit_chapter_form),
        )
        .route(
            "/books/{book_id}/chapters/{chapter_id}",
            post(update_chapter),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state)
}

fn fail(status: StatusCode) -> Response {
    private_response(status, &[])
}

fn map_error(error: AuthorError) -> Response {
    fail(match error {
        AuthorError::Invalid => StatusCode::BAD_REQUEST,
        AuthorError::Forbidden => StatusCode::FORBIDDEN,
        AuthorError::Missing => StatusCode::NOT_FOUND,
        AuthorError::Conflict => StatusCode::CONFLICT,
        AuthorError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}

async fn user_id(state: &AuthorState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}

async fn page_user(state: &AuthorState, headers: &HeaderMap) -> Result<i64, Response> {
    match user_id(state, headers).await {
        Ok(id) => Ok(id),
        Err(StatusCode::UNAUTHORIZED) => Err(private_response(Redirect::to("/login"), &[])),
        Err(status) => Err(fail(status)),
    }
}

fn permitted_form(state: &AuthorState, headers: &HeaderMap, csrf: &str) -> bool {
    state.cookies.same_origin(headers) && state.cookies.valid_csrf(headers, csrf)
}

fn page_with_csrf(
    state: &AuthorState,
    headers: &HeaderMap,
    render: impl FnOnce(&str, &str) -> axum::response::Html<String>,
) -> Response {
    let (csrf, cookies) = state.cookies.csrf_for_page(headers);
    let key = generate_session_token();
    private_response(render(&csrf, &key), &cookies)
}

async fn my_books(State(state): State<AuthorState>, headers: HeaderMap) -> Response {
    let user = match page_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    match state.author.my_books(user).await {
        Ok(books) => private_response(author_pages::books_index(&books), &[]),
        Err(error) => map_error(error),
    }
}

async fn manage_book(
    State(state): State<AuthorState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user = match page_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let book = match state.author.owned_book(user, book_id).await {
        Ok(Some(book)) => book,
        Ok(None) => return fail(StatusCode::NOT_FOUND),
        Err(error) => return map_error(error),
    };
    match state.author.my_chapters(user, book_id).await {
        Ok(chapters) => private_response(author_pages::manage_book(&book, &chapters), &[]),
        Err(error) => map_error(error),
    }
}

async fn book_form(State(state): State<AuthorState>, headers: HeaderMap) -> Response {
    if let Err(response) = page_user(&state, &headers).await {
        return response;
    }
    page_with_csrf(&state, &headers, author_pages::new_book)
}

async fn create_book(
    State(state): State<AuthorState>,
    headers: HeaderMap,
    Form(form): Form<CreateBookForm>,
) -> Response {
    if !permitted_form(&state, &headers, &form.csrf_token) {
        return fail(StatusCode::FORBIDDEN);
    }
    let user = match user_id(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return fail(status),
    };
    let input = match authoring::new_book(
        &form.source_identity,
        &form.target_language,
        &form.title,
        &form.command_key,
    ) {
        Ok(input) => input,
        Err(error) => return map_error(error),
    };
    match state.author.create_book(user, &input).await {
        Ok(id) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(error) => map_error(error),
    }
}

async fn new_chapter_form(
    State(state): State<AuthorState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user = match page_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    match state.author.owned_book(user, book_id).await {
        Ok(Some(_)) => page_with_csrf(&state, &headers, |csrf, key| {
            author_pages::chapter_form(book_id, None, None, csrf, key)
        }),
        Ok(None) => fail(StatusCode::NOT_FOUND),
        Err(error) => map_error(error),
    }
}

async fn edit_chapter_form(
    State(state): State<AuthorState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Response {
    let user = match page_user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    match state.author.owned_chapter(user, book_id, chapter_id).await {
        Ok(Some(chapter)) => page_with_csrf(&state, &headers, |csrf, key| {
            author_pages::chapter_form(book_id, Some(chapter_id), Some(&chapter), csrf, key)
        }),
        Ok(None) => fail(StatusCode::NOT_FOUND),
        Err(error) => map_error(error),
    }
}

async fn save(
    state: &AuthorState,
    headers: &HeaderMap,
    book_id: i64,
    chapter_id: Option<i64>,
    form: ChapterForm,
) -> Response {
    if !permitted_form(state, headers, &form.csrf_token) {
        return fail(StatusCode::FORBIDDEN);
    }
    let user = match user_id(state, headers).await {
        Ok(user) => user,
        Err(status) => return fail(status),
    };
    let input = match authoring::save_chapter(SaveChapterRequest {
        book_id: u64::try_from(book_id).unwrap_or(0),
        chapter_id: chapter_id.map(|id| u64::try_from(id).unwrap_or(0)),
        number: form.number,
        expected_revision: form.expected_revision,
        title: &form.title,
        body: &form.body_markdown,
        access: &form.access_level,
        price_amount: form.price_amount,
        command_key: &form.command_key,
    }) {
        Ok(input) => input,
        Err(error) => return map_error(error),
    };
    match state.author.save_chapter(user, &input).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{book_id}")), &[]),
        Err(error) => map_error(error),
    }
}

async fn create_chapter(
    State(state): State<AuthorState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ChapterForm>,
) -> Response {
    save(&state, &headers, book_id, None, form).await
}

async fn update_chapter(
    State(state): State<AuthorState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ChapterForm>,
) -> Response {
    save(&state, &headers, book_id, Some(chapter_id), form).await
}

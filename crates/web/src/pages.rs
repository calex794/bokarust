//! Server-rendered pages. The book data is escaped before inserting into HTML.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use std::fmt::Write;

pub(crate) struct PublicBookCard {
    pub title: String,
    pub language: String,
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn layout(title: &str, description: &str, content: &str) -> Html<String> {
    let shell = include_str!("../templates/layout.html");
    Html(
        shell
            .replace("<!--PAGE_TITLE-->", &escape_html(title))
            .replace("<!--PAGE_DESCRIPTION-->", &escape_html(description))
            .replace("<!--PAGE_CONTENT-->", content),
    )
}

fn book_cards(books: &[PublicBookCard]) -> String {
    if books.is_empty() {
        return "<p class=\"empty-state\">В каталоге пока нет опубликованных книг. Они появятся здесь после публикации.</p>".to_owned();
    }
    let mut html = String::new();
    for book in books {
        let title = escape_html(&book.title);
        let language = escape_html(&book.language);
        write!(
            html,
            "<article class=\"book-card\"><div class=\"book-card__cover\" aria-hidden=\"true\">К</div><div class=\"book-card__body\"><span class=\"book-card__label\">Книга · {language}</span><h3 class=\"book-card__title\">{title}</h3></div></article>"
        )
        .expect("writing into String cannot fail");
    }
    html
}

pub(crate) fn home(books: &[PublicBookCard]) -> Html<String> {
    let content =
        include_str!("../templates/home.html").replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Главная",
        "Пандаолейт — книги и переводы для чтения онлайн.",
        &content,
    )
}

pub(crate) fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Каталог",
        "Опубликованные книги и переводы на Пандаолейт.",
        &content,
    )
}

pub(crate) async fn login() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        layout(
            "Вход",
            "Вход в учётную запись Пандаолейт.",
            include_str!("../templates/login.html"),
        ),
    )
}

pub(crate) async fn register() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        layout(
            "Регистрация",
            "Создание учётной записи Пандаолейт.",
            include_str!("../templates/register.html"),
        ),
    )
}

/// Authenticated flows render only escaped display values and server-made CSRF secrets.
pub(crate) fn register_enabled(csrf: &str, error: Option<&str>) -> Html<String> {
    let content = include_str!("../templates/register-enabled.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace(
            "<!--AUTH_ERROR-->",
            &error
                .map(|s| {
                    format!(
                        "<p class=\"form-error\" role=\"alert\">{}</p>",
                        escape_html(s)
                    )
                })
                .unwrap_or_default(),
        );
    layout(
        "Регистрация",
        "Создание учётной записи Пандаолейт.",
        &content,
    )
}

pub(crate) fn login_enabled(csrf: &str, error: Option<&str>) -> Html<String> {
    let content = include_str!("../templates/login-enabled.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace(
            "<!--AUTH_ERROR-->",
            &error
                .map(|s| {
                    format!(
                        "<p class=\"form-error\" role=\"alert\">{}</p>",
                        escape_html(s)
                    )
                })
                .unwrap_or_default(),
        );
    layout("Вход", "Вход в учётную запись Пандаолейт.", &content)
}

pub(crate) fn account(name: &str, email: &str, csrf: &str) -> Html<String> {
    let content = include_str!("../templates/account.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--EMAIL-->", &escape_html(email))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf));
    layout(
        "Личный кабинет",
        "Личный кабинет читателя Пандаолейт.",
        &content,
    )
}

pub(crate) async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/site.css"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_book_titles_before_rendering() {
        let page = home(&[PublicBookCard {
            title: "<script>alert('x')</script> & \"hello\"".to_owned(),
            language: "ru".to_owned(),
        }]);
        assert!(page.0.contains("&lt;script&gt;"));
        assert!(page.0.contains("&#39;x&#39;"));
        assert!(page.0.contains("&amp; &quot;hello&quot;"));
        assert!(!page.0.contains("<script>"));
    }

    #[test]
    fn empty_catalog_does_not_invent_books() {
        let page = catalog(&[]);
        assert!(page.0.contains("пока нет опубликованных книг"));
        assert!(!page.0.contains("<!--PUBLIC_BOOKS-->"));
    }
}

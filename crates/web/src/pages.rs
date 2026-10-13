//! Server-rendered pages. The book data is escaped before inserting into HTML.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use bokarust_application::reading::{PublicBookPage, PublicChapterPage};
use std::fmt::Write;

pub(crate) struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
}

pub(crate) use crate::html_escape::escape_html;

pub(crate) fn layout(title: &str, description: &str, content: &str) -> Html<String> {
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
            "<article class=\"book-card\"><div class=\"book-card__cover\" aria-hidden=\"true\">К</div><div class=\"book-card__body\"><span class=\"book-card__label\">Книга · {language}</span><h3 class=\"book-card__title\"><a href=\"/books/{}\">{title}</a></h3></div></article>",
            book.id
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

pub(crate) fn book_detail(book: &PublicBookPage) -> Html<String> {
    let mut chapter_links = String::new();
    if book.chapters.is_empty() {
        chapter_links.push_str("<p class=\"empty-state\">Пока нет опубликованных глав.</p>");
    } else {
        for chapter in &book.chapters {
            write!(
                chapter_links,
                "<li><a href=\"{}{}\">Глава {}. {}</a>{}</li>",
                if chapter.is_paid {
                    "/chapters/"
                } else {
                    "/reader/"
                },
                if chapter.is_paid {
                    format!("{}/buy", chapter.id)
                } else {
                    chapter.id.to_string()
                },
                chapter.number,
                escape_html(&chapter.title),
                if chapter.is_paid {
                    " · платная"
                } else {
                    ""
                }
            )
            .expect("writing into String cannot fail");
        }
    }
    let body = include_str!("../templates/book_detail.html")
        .replace("<!--BOOK_TITLE-->", &escape_html(&book.title))
        .replace("<!--BOOK_LANGUAGE-->", &escape_html(&book.language))
        .replace("<!--CHAPTER_LIST-->", &chapter_links);
    layout(&book.title, "Опубликованные главы книги.", &body)
}

/// Readable-only chapter neighbours, already filtered by the database.
pub(crate) fn chapter_navigation(chapter: &PublicChapterPage, private: bool) -> String {
    let prefix = if private { "/read" } else { "/reader" };
    let previous = chapter
        .previous_chapter_id
        .map(|id| format!("<a rel=\"prev\" href=\"{prefix}/{id}\">← Предыдущая глава</a>"))
        .unwrap_or_default();
    let next = chapter
        .next_chapter_id
        .map(|id| format!("<a rel=\"next\" href=\"{prefix}/{id}\">Следующая глава →</a>"))
        .unwrap_or_default();
    if previous.is_empty() && next.is_empty() {
        String::new()
    } else {
        format!("<nav class=\"chapter-navigation\" aria-label=\"Переход между главами\">{previous}{next}</nav>")
    }
}

/// Render a restricted, escaped Markdown subset without embedding raw HTML.
pub(crate) fn reader(chapter: &PublicChapterPage, chapter_id: u64) -> Html<String> {
    let body = include_str!("../templates/reader.html")
        .replace("<!--CHAPTER_ID-->", &chapter_id.to_string())
        .replace(
            "<!--CHAPTER_NAVIGATION-->",
            &chapter_navigation(chapter, false),
        )
        .replace("<!--BOOK_ID-->", &chapter.book_id.to_string())
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_VERSION-->", &chapter.version_no.to_string())
        .replace(
            "<!--CHAPTER_CONTENT-->",
            &crate::safe_markdown::render(
                &bokarust_application::reading_progress::reader_blocks(&chapter.body_markdown)
                    .join("\n\n"),
            ),
        );
    layout(&chapter.chapter_title, "Опубликованный текст главы.", &body)
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

pub(crate) fn registration_form(csrf: &str) -> Html<String> {
    let body = include_str!("../templates/register_active.html").replace("<!--CSRF_TOKEN-->", csrf);
    layout("Регистрация", "Создание учётной записи Пандаолейт.", &body)
}

pub(crate) fn login_form(csrf: &str) -> Html<String> {
    let body = include_str!("../templates/login_active.html").replace("<!--CSRF_TOKEN-->", csrf);
    layout("Вход", "Вход в учётную запись Пандаолейт.", &body)
}

pub(crate) fn my_account(name: &str, csrf: &str) -> Html<String> {
    let body = include_str!("../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", csrf);
    layout("Кабинет", "Личный кабинет Пандаолейт.", &body)
}

/// Browser enhancement is a separate static asset, with a strict MIME type.
pub(crate) async fn reader_progress_script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../assets/reader-progress.js"),
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
            id: 9,
            title: "<script>alert('x')</script> & \"hello\"".to_owned(),
            language: "ru".to_owned(),
        }]);
        assert!(page.0.contains("&lt;script&gt;"));
        assert!(page.0.contains("&#39;x&#39;"));
        assert!(page.0.contains("&amp; &quot;hello&quot;"));
        assert!(!page.0.contains("<script>"));
        assert!(page.0.contains("href=\"/books/9\""));
    }

    #[test]
    fn empty_catalog_does_not_invent_books() {
        let page = catalog(&[]);
        assert!(page.0.contains("пока нет опубликованных книг"));
        assert!(!page.0.contains("<!--PUBLIC_BOOKS-->"));
    }
}

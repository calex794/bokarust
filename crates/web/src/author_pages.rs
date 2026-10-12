//! Small server-rendered author pages; every dynamic value is HTML-escaped.
use crate::pages::{escape_html, layout};
use axum::response::Html;
use bokarust_application::authoring::{ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterSummary};
use std::fmt::Write;

pub(crate) fn books_index(books: &[OwnedBook]) -> Html<String> {
    let mut list = String::new();
    for book in books {
        write!(
            list,
            "<li><a href=\"/me/books/{}\">{}</a></li>",
            book.id,
            escape_html(&book.title),
        )
        .expect("String write");
    }
    if list.is_empty() {
        list.push_str("<li>У вас пока нет книг.</li>");
    }
    layout(
        "Мои книги",
        "Ваши книги и главы.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container author-width\">\
             <h1>Мои книги</h1><p><a class=\"button\" href=\"/books/new\">Создать книгу</a></p>\
             <ul class=\"author-chapters\">{list}</ul><p><a href=\"/me\">← Кабинет</a></p>\
             </div></section>"
        ),
    )
}

pub(crate) fn new_book(csrf: &str, key: &str) -> Html<String> {
    let body = include_str!("../templates/book_new.html")
        .replace("<!--CSRF-->", &escape_html(csrf))
        .replace("<!--KEY-->", &escape_html(key));
    layout("Новая книга", "Создание книги владельцем.", &body)
}

pub(crate) fn manage_book(book: &OwnedBook, chapters: &[OwnedChapterSummary]) -> Html<String> {
    let mut entries = String::new();
    for chapter in chapters {
        let public_link = match chapter.access {
            ChapterAccess::Free => format!(
                " <a href=\"/reader/{}\">Читать</a>",
                chapter.id
            ),
            ChapterAccess::Paid => " (платная, покупка пока недоступна)".to_owned(),
        };
        write!(
            entries,
            "<li>Глава {}. {} — <a href=\"/books/{}/chapters/{}/edit\">Изменить</a>{}</li>",
            chapter.number,
            escape_html(&chapter.title),
            book.id,
            chapter.id,
            public_link,
        )
        .expect("String write");
    }
    if entries.is_empty() {
        entries.push_str("<li>Глав пока нет.</li>");
    }
    let body = include_str!("../templates/book_manage.html")
        .replace("<!--BOOK_TITLE-->", &escape_html(&book.title))
        .replace("<!--BOOK_ID-->", &book.id.to_string())
        .replace("<!--CHAPTERS-->", &entries);
    layout(&book.title, "Управление книгой и главами.", &body)
}

pub(crate) fn chapter_form(
    book_id: i64,
    chapter_id: Option<i64>,
    chapter: Option<&OwnedChapter>,
    csrf: &str,
    key: &str,
) -> Html<String> {
    let heading = if chapter_id.is_some() { "Изменить главу" } else { "Новая глава" };
    let action = match chapter_id {
        Some(id) => format!("/books/{book_id}/chapters/{id}"),
        None => format!("/books/{book_id}/chapters"),
    };
    let number = chapter.map_or(1, |c| c.number);
    let number_control = format!(
        "<input id=\"number\" type=\"number\" name=\"number\" min=\"1\" value=\"{number}\" required{}>",
        if chapter_id.is_some() { " readonly" } else { "" },
    );
    let title = chapter.map_or("", |c| c.title.as_str());
    let body = chapter.map_or("", |c| c.body.as_str());
    let revision = chapter.map_or(0, |c| c.revision);
    let access = chapter.map_or(ChapterAccess::Free, |c| c.access);
    let price = chapter.map_or(0, |c| c.price_cents);
    let html = include_str!("../templates/chapter_author_form.html")
        .replace("<!--PAGE_HEADING-->", heading)
        .replace("<!--ACTION-->", &action)
        .replace("<!--CSRF-->", &escape_html(csrf))
        .replace("<!--KEY-->", &escape_html(key))
        .replace("<!--REVISION-->", &revision.to_string())
        .replace("<!--NUMBER_CONTROL-->", &number_control)
        .replace("<!--TITLE-->", &escape_html(title))
        .replace("<!--BODY-->", &escape_html(body))
        .replace("<!--FREE_SELECTED-->", if access == ChapterAccess::Free { "selected" } else { "" })
        .replace("<!--PAID_SELECTED-->", if access == ChapterAccess::Paid { "selected" } else { "" })
        .replace("<!--PRICE-->", &price.to_string())
        .replace("<!--BOOK_ID-->", &book_id.to_string());
    layout(heading, "Сохранение и немедленная публикация главы.", &html)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn author_data_is_never_raw_html() {
        let book = OwnedBook { id: 3, title: "<script>alert(1)</script>".to_owned() };
        let html = manage_book(&book, &[]).0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        let chapter = OwnedChapter {
            number: 1, title: "<x>".into(), body: "</textarea><script>boom</script>".into(),
            access: ChapterAccess::Free, price_cents: 0, revision: 1,
        };
        let html = chapter_form(3, Some(4), Some(&chapter), "safe", "safe").0;
        assert!(html.contains("&lt;/textarea&gt;"));
        assert!(!html.contains("</textarea><script>"));
    }
}
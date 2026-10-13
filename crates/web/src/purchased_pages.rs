//! Server-rendered purchased versions are private, no JS and never indexable.
#![forbid(unsafe_code)]
use crate::{
    pages::{escape_html, layout},
    safe_markdown,
};
use axum::response::Html;
use bokarust_adapters::postgres_purchased::{PurchaseStatus, PurchasedChapter, PurchasedListItem};
use std::fmt::Write;

pub(crate) fn library(items: &[PurchasedListItem]) -> Html<String> {
    let mut entries = String::new();
    for item in items {
        let label = match item.status {
            PurchaseStatus::Readable => format!(
                "<a href=\"/me/purchases/{}\">Открыть купленную редакцию №{}</a>",
                item.chapter_id, item.version_no
            ),
            PurchaseStatus::Blocked => "Чтение приостановлено блокировкой".to_owned(),
            PurchaseStatus::Revoked => "Право чтения отозвано после возврата или отмены".to_owned(),
        };
        write!(
            entries,
            "<li><strong>{}</strong> — глава {}: {}. {label}</li>",
            escape_html(&item.book_title),
            item.chapter_no,
            escape_html(&item.chapter_title),
        )
        .expect("writing String");
    }
    if entries.is_empty() {
        entries.push_str("<li>Приобретённых глав пока нет.</li>");
    }
    let body = include_str!("../templates/purchased_library.html")
        .replace("<!--PURCHASED_ITEMS-->", &entries);
    layout(
        "Мои приобретённые главы",
        "Приобретённые редакции сохраняются независимо от места чтения.",
        &body,
    )
}

pub(crate) fn owned_chapter(chapter: &PurchasedChapter) -> Html<String> {
    let body = include_str!("../templates/purchased_reader.html")
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_NO-->", &chapter.chapter_no.to_string())
        .replace("<!--VERSION_NO-->", &chapter.version_no.to_string())
        .replace(
            "<!--CHAPTER_CONTENT-->",
            &safe_markdown::render(&chapter.body_markdown),
        );
    layout(
        &chapter.chapter_title,
        "Личная приобретённая редакция главы.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_version_pages_escape_user_titles_and_omit_blocked_links() {
        let pages = library(&[
            PurchasedListItem {
                chapter_id: 13,
                book_title: "<script>alert(1)</script>".into(),
                chapter_title: "<img>".into(),
                chapter_no: 3,
                version_no: 1,
                status: PurchaseStatus::Blocked,
            },
            PurchasedListItem {
                chapter_id: 14,
                book_title: "Сохранённая книга".into(),
                chapter_title: "Покупка".into(),
                chapter_no: 4,
                version_no: 2,
                status: PurchaseStatus::Readable,
            },
        ]);
        assert!(!pages.0.contains("<script>alert(1)</script>"));
        assert!(!pages.0.contains("href=\"/me/purchases/13\""));
        assert!(pages.0.contains("href=\"/me/purchases/14\""));
    }
}

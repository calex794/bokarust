//! Escaped, script-free chapter position controls.
#![forbid(unsafe_code)]
use crate::pages::{chapter_navigation, escape_html, layout};
use axum::response::Html;
use bokarust_application::{
    reading::PublicChapterPage,
    reading_progress::{reader_blocks, ContinueReading, ReadingPlace},
};
use std::fmt::Write;

pub(crate) fn library(items: &[ContinueReading]) -> Html<String> {
    let mut rendered = String::new();
    for item in items {
        let fragment = if item.block_index > 0 {
            format!("#p-{}", item.block_index)
        } else {
            String::new()
        };
        write!(
            rendered,
            "<li><strong>{}</strong><p>Глава {}: {} — \
             <a href=\"/read/{}{}\">Продолжить чтение</a></p></li>",
            escape_html(&item.book_title),
            item.chapter_no,
            escape_html(&item.chapter_title),
            item.chapter_id,
            fragment,
        )
        .expect("write to String");
    }
    if rendered.is_empty() {
        rendered.push_str("<li>Вы пока не сохранили место чтения. Откройте бесплатную главу и выберите «Читать с сохранением места».</li>");
    }
    let body = include_str!("../templates/reading_library.html")
        .replace("<!--LIBRARY_ITEMS-->", &rendered);
    layout(
        "Продолжить чтение",
        "Сохранённое место чтения в ваших книгах.",
        &body,
    )
}

pub(crate) fn tracked_reader(
    chapter_id: i64,
    chapter: &PublicChapterPage,
    csrf: &str,
    saved_block: Option<ReadingPlace>,
) -> Html<String> {
    let sections = reader_blocks(&chapter.body_markdown);
    let mut blocks = String::with_capacity(chapter.body_markdown.len() + sections.len() * 300);
    for (offset, block) in sections.iter().enumerate() {
        let position = offset + 1;
        write!(
            blocks,
            "<section id=\"p-{position}\" class=\"reader-piece\">\
             <pre class=\"reader-piece__text\">{}</pre>\
             <button type=\"submit\" form=\"save-position\" name=\"block_index\" \
             value=\"{position}\" class=\"reader-save-button\">Запомнить место здесь</button>\
             </section>",
            escape_html(block),
        )
        .expect("write to String");
    }
    let saved = saved_block
        .filter(|place| place.block_index > 0
            && usize::try_from(place.block_index).ok().is_some_and(|v| v <= sections.len()))
        .map(|place| {
            let position = place.block_index;
            let offset = usize::try_from(place.char_offset).unwrap_or(0);
            format!(
                "<p class=\"reader-return\"><a href=\"#p-{position}\" data-char-offset=\"{offset}\">К сохранённому месту: отрывок {position}</a></p>"
            )
        })
        .unwrap_or_default();
    let body = include_str!("../templates/reader_tracked.html")
        .replace("<!--BOOK_ID-->", &chapter.book_id.to_string())
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_VERSION-->", &chapter.version_no.to_string())
        .replace("<!--CHAPTER_ID-->", &chapter_id.to_string())
        .replace("<!--VERSION_ID-->", &chapter.version_id.to_string())
        .replace("<!--CSRF-->", &escape_html(csrf))
        .replace("<!--SAVED_PLACE-->", &saved)
        .replace(
            "<!--CHAPTER_NAVIGATION-->",
            &chapter_navigation(chapter, true),
        )
        .replace("<!--CHAPTER_BLOCKS-->", &blocks);
    layout(
        &chapter.chapter_title,
        "Личная читательская позиция.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_reader_is_escaped_and_has_bounded_anchor_list() {
        let chapter = PublicChapterPage {
            book_id: 1,
            book_title: "<script>x</script>".into(),
            chapter_title: "<b>Unsafe</b>".into(),
            version_id: 5,
            version_no: 3,
            previous_chapter_id: None,
            next_chapter_id: None,
            body_markdown: "</pre><script>alert(1)</script>".into(),
        };
        let page = tracked_reader(
            7,
            &chapter,
            "csrf",
            Some(ReadingPlace {
                block_index: 2,
                char_offset: 0,
            }),
        )
        .0;
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.contains("name=\"version_id\" value=\"5\""));
        assert!(page.contains("id=\"p-1\""));
        assert!(!page.contains("href=\"#p-2\"")); // position is out of range
    }
}

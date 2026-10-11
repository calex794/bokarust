/// Convert Markdown to safe preview HTML. The caller controls when this runs;
/// typing a character must not trigger a full parse or sanitization.
pub fn render_safe_markdown(markdown: &str) -> String {
    let parser=pulldown_cmark::Parser::new_ext(markdown,pulldown_cmark::Options::all());
    let mut html=String::new();
    pulldown_cmark::html::push_html(&mut html,parser);
    ammonia::clean(&html)
}

#[cfg(test)]
mod tests {
    use super::render_safe_markdown;

    #[test]
    fn safe_preview_keeps_unicode_markdown_and_removes_scripts() {
        let rendered=render_safe_markdown("**Привет 🌍**\n\n<script>alert('xss')</script>");
        assert!(rendered.contains("<strong>Привет 🌍</strong>"));
        assert!(!rendered.contains("<script"));
        assert!(!rendered.contains("alert("));
    }

    #[test]
    fn supports_maximum_sized_chapter_without_truncation() {
        let source="Я".repeat(50_000);
        let rendered=render_safe_markdown(&source);
        assert_eq!(rendered.matches('Я').count(),50_000);
    }
}

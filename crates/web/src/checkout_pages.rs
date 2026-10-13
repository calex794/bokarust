//! Script-free wallet checkout, with values escaped and all prices in RUB minor units.
#![forbid(unsafe_code)]
use crate::pages::{escape_html, layout};
use axum::response::Html;
use bokarust_adapters::postgres_checkout::CheckoutOffer;

fn money(amount: i64) -> String {
    format!("{}.{:02} ₽", amount / 100, amount % 100)
}

pub(crate) fn offer(
    chapter: &CheckoutOffer,
    balance_minor: i64,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    let body = include_str!("../templates/chapter_buy.html")
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_ID-->", &chapter.chapter_id.to_string())
        .replace("<!--CHAPTER_VERSION-->", &chapter.version_id.to_string())
        .replace("<!--PRICE_MINOR-->", &chapter.price_minor.to_string())
        .replace("<!--PRICE-->", &money(chapter.price_minor))
        .replace("<!--BALANCE-->", &money(balance_minor))
        .replace("<!--CSRF-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout(
        &format!("Покупка главы «{}»", chapter.chapter_title),
        "Оплата главы с внутреннего кошелька.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_escapes_titles_and_has_server_bound_version_and_price() {
        let page = offer(
            &CheckoutOffer {
                chapter_id: 17,
                book_title: "<script>x</script>".into(),
                chapter_title: "<b>Глава</b>".into(),
                version_id: 31,
                price_minor: 125,
            },
            200,
            "csrf_token",
            "key",
        )
        .0;
        assert!(!page.contains("<script>x</script>"));
        assert!(page.contains("&lt;b&gt;Глава&lt;/b&gt;"));
        assert!(page.contains("name=\"expected_version_id\" value=\"31\""));
        assert!(page.contains("name=\"expected_price_minor\" value=\"125\""));
        assert!(page.contains("1.25 ₽"));
    }
}

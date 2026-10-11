"""Deterministic cache/refresh checks for Markdown preview in Chromium."""
from __future__ import annotations


def run_preview_checks(check, context, url: str, selector: str, report: dict):
    def open_editor():
        page=context.new_page()
        page.goto(url,wait_until="domcontentloaded")
        page.wait_for_function(
            "() => document.body.innerText.includes('Глава загружена')",
            timeout=90000,
        )
        return page

    def render_count(page) -> int:
        value=page.locator("article.preview").get_attribute("data-render-count")
        assert value is not None, "Preview missing data-render-count: " + page.locator("article.preview").evaluate("(el) => el.outerHTML.slice(0,400)")
        return int(value)

    def cache_survives_mode_and_title_changes():
        page=open_editor()
        try:
            area=page.locator(selector)
            area.fill("**Первая**\n\n<script>window.preview_injected=true</script>")
            page.get_by_role("button",name="Предпросмотр",exact=True).click()
            assert render_count(page)==1
            assert page.locator("article.preview strong").inner_text()=="Первая"
            assert page.locator("article.preview script").count()==0
            assert page.evaluate("() => window.preview_injected !== true")

            # Three mode changes and an unrelated title update must not
            # re-read/re-parse unchanged Markdown.
            page.get_by_role("button",name="Текст",exact=True).click()
            page.get_by_label("Название главы").fill("Только новое название")
            page.get_by_role("button",name="Два окна",exact=True).click()
            assert render_count(page)==1
            page.get_by_role("button",name="Предпросмотр",exact=True).click()
            assert render_count(page)==1

            page.get_by_role("button",name="Текст",exact=True).click()
            area.fill("**Вторая**")
            page.get_by_role("button",name="Предпросмотр",exact=True).click()
            assert render_count(page)==2, "Text edit must invalidate preview"
            assert page.locator("article.preview strong").inner_text()=="Вторая"
            return True
        finally:
            page.close()

    check("preview_cached_across_modes_and_title_edits",
          cache_survives_mode_and_title_changes)

    def side_by_side_refreshes_after_idle():
        page=open_editor()
        try:
            area=page.locator(selector)
            page.get_by_role("button",name="Два окна",exact=True).click()
            assert render_count(page)==1
            area.fill("**Привет**")
            page.wait_for_function(
                "() => document.querySelector('article.preview').getAttribute('data-render-count') === '2'",
                timeout=30000,
            )
            assert page.locator("article.preview strong").inner_text()=="Привет"

            # Three separate actual keyboard inputs must produce one
            # debounced live-preview pass, not one parse per keystroke.
            area.focus()
            area.press_sequentially("abc",delay=10)
            page.wait_for_function(
                "() => document.querySelector('article.preview').getAttribute('data-render-count') === '3'",
                timeout=30000,
            )
            assert "Приветabc" in page.locator("article.preview").inner_text()
            assert render_count(page)==3
            return True
        finally:
            page.close()

    check("preview_live_update_in_two_panes_after_typing",
          side_by_side_refreshes_after_idle)

    def large_markdown_preview_cache():
        page=open_editor()
        try:
            area=page.locator(selector)
            sample="**Глава**\n\n" + "Я" * (50_000-len("**Глава**\n\n"))
            assert len(sample)==50_000
            area.fill(sample)
            first_ms=page.evaluate("""() => {
                const button=[...document.querySelectorAll('button')].find(
                    b=>b.textContent.trim()==='Предпросмотр'
                );
                const start=performance.now();
                button.click();
                return performance.now()-start;
            }""")
            assert render_count(page)==1
            assert page.locator("article.preview strong").inner_text()=="Глава"

            page.get_by_role("button",name="Текст",exact=True).click()
            repeat_ms=page.evaluate("""() => {
                const button=[...document.querySelectorAll('button')].find(
                    b=>b.textContent.trim()==='Предпросмотр'
                );
                const start=performance.now();
                button.click();
                return performance.now()-start;
            }""")
            assert render_count(page)==1
            assert page.locator("article.preview strong").inner_text()=="Глава"
            report["preview_metrics"]={
                "chapter_codepoints":50_000,
                "initial_preview_click_ms":round(first_ms,3),
                "cached_preview_click_ms":round(repeat_ms,3),
                "renders_before_and_after_cached_click":[1,1],
                "note":"Browser-side synchronous button click timing, single CI run; report illustrative only",
            }
            print(
                f"PREVIEW 50000 characters: initial_click={first_ms:.2f}ms "
                f"cached_click={repeat_ms:.2f}ms renders=1->1",
                flush=True
            )
            return True
        finally:
            page.close()

    check("preview_50k_cache_render_and_timing",large_markdown_preview_cache)

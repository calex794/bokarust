#!/usr/bin/env python3
"""Real browser checks for the local Rust/WASM editor. Not synthetic unit tests."""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.request
from pathlib import Path

import psutil
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
REPORT_DIR = ROOT / "browser-reports"
SELECTOR = 'textarea[aria-label="Текст главы"]'


def percentile(values: list[float], level: float) -> float:
    values = sorted(values)
    if not values:
        return 0.0
    at = (len(values) - 1) * level
    lo = int(at)
    hi = min(lo + 1, len(values) - 1)
    return round(values[lo] + (values[hi] - values[lo]) * (at - lo), 3)


def child_browser_memory() -> dict:
    """Unique memory of Chrome's processes, not total system or app server memory."""
    uss = rss = count = 0
    parent = psutil.Process(os.getpid())
    for proc in parent.children(recursive=True):
        try:
            name = proc.name().lower()
            if "chrome" not in name and "chromium" not in name and "firefox" not in name and "webkit" not in name:
                continue
            info = proc.memory_full_info()
            uss += getattr(info, "uss", 0)
            rss += info.rss
            count += 1
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
            continue
    return {
        "browser_processes": count,
        "browser_unique_mb": round(uss / (1024 * 1024), 2),
        "browser_rss_sum_mb": round(rss / (1024 * 1024), 2),
    }


def chrome_heap(page) -> dict:
    try:
        session = page.context.new_cdp_session(page)
        result = session.send("Runtime.getHeapUsage")
        session.detach()
        return {
            "js_heap_used_mb": round(result["usedSize"] / 1048576, 2),
            "js_heap_total_mb": round(result["totalSize"] / 1048576, 2),
        }
    except Exception:
        return {"js_heap_used_mb": None, "js_heap_total_mb": None}


def keyboard_probe(page, selector: str, presses: int = 18) -> dict:
    """Actual Playwright key presses, timed in the page from keydown to input and next frame."""
    field = page.locator(selector)
    field.evaluate(
        """(el) => {
          const records = [];
          const onKey = (e) => {
            if (e.key !== 'x') return;
            const started = performance.now();
            const entry = {start: started, input: null, frame: null};
            records.push(entry);
            requestAnimationFrame(() => {entry.frame = performance.now() - started;});
          };
          const onInput = () => {
            const recent = records[records.length - 1];
            if (recent && recent.input === null) {
              recent.input = performance.now() - recent.start;
            }
          };
          el.addEventListener('keydown', onKey);
          el.addEventListener('input', onInput);
          el.__stopKeyboardProbe = () => {
            el.removeEventListener('keydown', onKey);
            el.removeEventListener('input', onInput);
            return records;
          };
          el.focus();
          el.setSelectionRange(el.textLength, el.textLength);
        }"""
    )
    for _ in range(presses):
        page.keyboard.press("x")
        page.evaluate("() => new Promise(resolve => requestAnimationFrame(resolve))")
    rows = field.evaluate("(el) => el.__stopKeyboardProbe()")
    assert len(rows) == presses, f"Expected {presses} keydown events, got {len(rows)}"
    assert all(row["input"] is not None and row["frame"] is not None for row in rows), rows
    return {
        "keydown_to_input_ms": {
            "p50": percentile([row["input"] for row in rows], .50),
            "p95": percentile([row["input"] for row in rows], .95),
        },
        "keydown_to_frame_ms": {
            "p50": percentile([row["frame"] for row in rows], .50),
            "p95": percentile([row["frame"] for row in rows], .95),
            "max": round(max(row["frame"] for row in rows), 3),
        },
        "presses": presses,
    }


def plain_textarea_probe(page, content: str, matching_width: float) -> dict:
    """Browser-native baseline, without Rust, in the same browser and viewport."""
    baseline = page.context.new_page()
    try:
        baseline.set_content(
            "<style>*{box-sizing:border-box}textarea{display:block;height:70vh;"
            "min-height:440px;padding:16px;font:16px/1.65 ui-monospace,monospace;"
            "overflow:auto;overflow-wrap:anywhere;resize:vertical}</style>"
            f'<textarea style="width:{matching_width}px"></textarea>'
        )
        baseline.locator("textarea").fill(content)
        baseline.wait_for_timeout(150)
        return keyboard_probe(baseline, "textarea")
    finally:
        baseline.close()


def browser_benchmark(page, size: int, button: str, browser_name: str) -> dict:
    page.get_by_role("button", name=button, exact=True).click(timeout=90000)
    area = page.locator(SELECTOR)
    page.wait_for_function(
        "(count) => Array.from(document.querySelector('textarea[aria-label=\"Текст главы\"]').value).length === count",
        arg=size,
        timeout=90000,
    )
    actual = area.input_value(timeout=90000)
    assert len(actual) == size, f"Sample has {len(actual)} code points, expected {size}"
    assert "世界" in actual and "🌍" in actual and "Тест" in actual
    page.wait_for_timeout(150)
    memory_before = child_browser_memory()
    heap_before = chrome_heap(page) if browser_name == "chromium" else {}
    live_input = keyboard_probe(page, SELECTOR)
    assert area.input_value().endswith("x" * 18)
    memory_after = child_browser_memory()
    heap_after = chrome_heap(page) if browser_name == "chromium" else {}
    width = area.evaluate("(el) => el.getBoundingClientRect().width")
    baseline = plain_textarea_probe(page, actual, width)
    runtime_ms = live_input["keydown_to_frame_ms"]["p95"]
    baseline_ms = baseline["keydown_to_frame_ms"]["p95"]
    result = {
        "requested_codepoints": size,
        "method": "Real keyboard events from Playwright, timed in the browser from keydown to input/next frame",
        "editor_keyboard": live_input,
        "native_textarea_baseline": baseline,
        "frame_p95_ratio_to_native": round(runtime_ms / baseline_ms, 2) if baseline_ms else None,
        "memory_before": {**memory_before, **heap_before},
        "memory_after": {**memory_after, **heap_after},
    }
    print(
        f"MEASURE {browser_name} {size} chars: keyboard-to-frame p95="
        f"{runtime_ms} ms, plain textarea p95={baseline_ms} ms, "
        f"browser USS={memory_after['browser_unique_mb']} MiB", flush=True
    )
    return result


def run_suite(browser_name: str) -> dict:
    started = time.time()
    report = {
        "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "commit": os.environ.get("GITHUB_SHA", "local"),
        "os": platform.platform(),
        "browser": browser_name,
        "checks": {},
        "measurements": {},
        "limits": "CI VM values are comparative observations, not end-user performance promises.",
    }

    def check(name, fn):
        try:
            value = fn()
            report["checks"][name] = {"ok": True}
            print(f"PASS {name}", flush=True)
            return value
        except Exception as exc:
            report["checks"][name] = {
                "ok": False,
                "error": f"{type(exc).__name__}: {exc}",
            }
            print(f"FAIL {name}: {exc}", flush=True)
            traceback.print_exc()
            return None

    def check_equal(actual, expected):
        assert actual == expected, f"Expected {expected!r}, got {actual!r}"

    with tempfile.TemporaryDirectory(prefix="bokarust-browser-") as directory:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = os.environ.copy()
        env["PORT"] = str(port)
        env["CHAPTER_FILE"] = str(Path(directory) / "chapter.json")
        binary = ROOT / "target" / "release" / "editor-server"
        assert binary.is_file(), f"Missing binary: {binary} (run cargo build --release -p editor-server)"
        server_log = open(Path(directory) / "server.log", "wb")
        server = subprocess.Popen(
            [str(binary)], cwd=ROOT, env=env, stdout=server_log, stderr=subprocess.STDOUT
        )
        url = f"http://127.0.0.1:{port}"
        try:
            for _ in range(150):
                try:
                    with urllib.request.urlopen(f"{url}/api/chapter", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(.2)
            else:
                raise RuntimeError("Axum did not start in 30 seconds")
            with sync_playwright() as playwright:
                kind = getattr(playwright, browser_name)
                browser = kind.launch(headless=True, args=["--no-sandbox", "--disable-dev-shm-usage"] if browser_name == "chromium" else [])
                report["browser_version"] = browser.version
                context = browser.new_context(viewport={"width": 1400, "height": 1000}, locale="ru-RU")
                page = context.new_page()
                page_errors = []
                page.on("pageerror", lambda error: page_errors.append(str(error)))
                check("page_load", lambda: (
                    page.goto(url, wait_until="domcontentloaded"),
                    page.wait_for_function(
                        "() => document.body.innerText.includes('Глава загружена')", timeout=90000
                    ),
                    check_equal(page.locator(SELECTOR).input_value().splitlines()[0], "Первый абзац."),
                ))
                for size, label in ((20_000, "20 тыс."), (150_000, "150 тыс."), (1_000_000, "1 млн")):
                    data = check(
                        f"input_memory_{size}",
                        lambda size=size, label=label: browser_benchmark(page, size, label, browser_name),
                    )
                    if data:
                        report["measurements"][str(size)] = data
                    if size == 1_000_000:
                        check("million_chars_screenshot", lambda: page.screenshot(
                            path=str(REPORT_DIR / f"{browser_name}-million.png")
                        ))

                def check_big_save():
                    page.get_by_role("button", name="Сохранить", exact=True).click()
                    page.wait_for_function(
                        "() => document.body.innerText.includes('Сохранено: версия')", timeout=90000
                    )
                    response = page.request.get(f"{url}/api/chapter")
                    assert response.ok, response.status
                    doc = response.json()
                    assert len(doc["markdown"]) >= 1_000_000
                    assert doc["revision"] == 1
                    page.get_by_role("button", name="Сохранить", exact=True).click()
                    response_2 = page.request.get(f"{url}/api/chapter").json()
                    check_equal(response_2["revision"], 1)
                    return True

                check("million_character_save_and_noop", check_big_save)

                def unicode_selection():
                    area = page.locator(SELECTOR)
                    area.fill("Привет 🌍 мир")
                    area.evaluate("(el) => { el.focus(); el.setSelectionRange(7, 9); }")
                    page.get_by_role("button", name="Жирный", exact=True).click()
                    check_equal(area.input_value(), "Привет **🌍** мир")
                    return True

                check("utf16_emoji_selection_format", unicode_selection)

                def undo_typing():
                    area = page.locator(SELECTOR)
                    area.fill("Начало ")
                    area.press("End")
                    area.press_sequentially("Тест", delay=25)
                    before = area.input_value()
                    assert before.endswith("Тест"), before
                    area.press("Control+z")
                    after = area.input_value()
                    assert after != before, "Ctrl+Z did not undo any typed character"
                    return {"before": before, "after": after}

                check("ctrl_z_typed_text", undo_typing)

                def undo_toolbar():
                    area = page.locator(SELECTOR)
                    area.fill("Основа 🌍")
                    area.evaluate("(el) => { el.focus(); el.setSelectionRange(7, 9); }")
                    page.get_by_role("button", name="Жирный", exact=True).click()
                    check_equal(area.input_value(), "Основа **🌍**")
                    area.press("Control+z")
                    check_equal(area.input_value(), "Основа 🌍")
                    return True

                check("ctrl_z_after_formatting", undo_toolbar)

                def multi_language_typing():
                    area = page.locator(SELECTOR)
                    area.fill("")
                    text = "Русский 中文 日本語 العَرَبِيَّة 🌍"
                    area.press_sequentially(text, delay=1)
                    check_equal(area.input_value(), text)
                    return True

                check("multilingual_keyboard_input", multi_language_typing)

                def composition_simulation():
                    result = page.locator(SELECTOR).evaluate(
                        """(el) => {
                          el.value = '';
                          el.dispatchEvent(new CompositionEvent('compositionstart', {data:''}));
                          el.value = 'に';
                          el.dispatchEvent(new InputEvent('input', {bubbles:true, data:'に', isComposing:true, inputType:'insertCompositionText'}));
                          const during = el.value;
                          el.value = '日本語';
                          el.dispatchEvent(new CompositionEvent('compositionend', {data:'日本語'}));
                          el.dispatchEvent(new InputEvent('input', {bubbles:true, data:'日本語', isComposing:false, inputType:'insertText'}));
                          return {during, after:el.value};
                        }"""
                    )
                    check_equal(result, {"during": "に", "after": "日本語"})
                    return "Synthetic composition events only; no physical IME."
                check("synthetic_ime_composition", composition_simulation)

                def preview_security():
                    area = page.locator(SELECTOR)
                    area.fill("<script>window.editor_injected=true</script>\n\n**Привет**")
                    page.get_by_role("button", name="Предпросмотр", exact=True).click()
                    assert page.locator(".preview strong").count() >= 1
                    assert page.locator(".preview script").count() == 0
                    assert page.evaluate("() => window.editor_injected !== true")
                    page.get_by_role("button", name="Текст", exact=True).click()
                    return True

                check("markdown_preview_sanitizes_html", preview_security)

                def stale_revision():
                    page.get_by_role("button", name="Сохранить", exact=True).click()
                    page.wait_for_function(
                        "() => document.body.innerText.includes('Сохранено: версия 2')", timeout=90000
                    )
                    tab2 = context.new_page()
                    tab2.goto(url)
                    tab2.wait_for_function("() => document.body.innerText.includes('Глава загружена')")
                    page.locator(SELECTOR).fill("Вкладка А изменила главу.")
                    page.get_by_role("button", name="Сохранить", exact=True).click()
                    page.wait_for_function("() => document.body.innerText.includes('Сохранено: версия 3')")
                    tab2.locator(SELECTOR).fill("Вкладка Б пытается затереть А.")
                    tab2.get_by_role("button", name="Сохранить", exact=True).click()
                    tab2.wait_for_function("() => document.body.innerText.includes('Ошибка 409')", timeout=30000)
                    assert "Вкладка А" in page.request.get(f"{url}/api/chapter").json()["markdown"]
                    tab2.close()
                    return True

                check("two_tabs_stale_revision_409", stale_revision)

                def mobile_layout():
                    mobile = browser.new_context(
                        viewport={"width": 390, "height": 844},
                        device_scale_factor=1,
                        is_mobile=browser_name != "firefox",
                        has_touch=browser_name != "firefox",
                    )
                    p = mobile.new_page()
                    p.goto(url)
                    p.wait_for_function("() => document.body.innerText.includes('Глава загружена')")
                    p.get_by_role("button", name="Два окна", exact=True).click(timeout=30000)
                    sizes = p.evaluate(
                        """() => {
                          const t=document.querySelector('textarea');
                          const prev=document.querySelector('.preview');
                          const a=t.getBoundingClientRect(), b=prev.getBoundingClientRect();
                          return {width:innerWidth, documentWidth:document.documentElement.scrollWidth,
                            textarea: {x:a.x,y:a.y,width:a.width,height:a.height},
                            preview: {x:b.x,y:b.y,width:b.width,height:b.height}};
                        }"""
                    )
                    assert sizes["documentWidth"] <= sizes["width"] + 2, sizes
                    assert sizes["preview"]["y"] >= sizes["textarea"]["y"] + sizes["textarea"]["height"] - 1, sizes
                    p.screenshot(path=str(REPORT_DIR / f"{browser_name}-mobile.png"))
                    mobile.close()
                    return sizes

                check("mobile_two_pane_no_overflow", mobile_layout)
                check("no_uncaught_page_errors", lambda: (
                    None if not page_errors else (_ for _ in ()).throw(AssertionError(page_errors))
                ))
                context.close()
                browser.close()
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
            server_log.close()
            report["seconds"] = round(time.time() - started, 2)
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", choices=("chromium", "firefox", "webkit"), default="chromium")
    args = parser.parse_args()
    REPORT_DIR.mkdir(exist_ok=True)
    try:
        result = run_suite(args.browser)
    except Exception as exc:
        traceback.print_exc()
        result = {"browser": args.browser, "fatal_error": repr(exc), "checks": {"setup": {"ok": False}}}
    output = REPORT_DIR / f"{args.browser}.json"
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    failures = [name for name, item in result["checks"].items() if not item["ok"]]
    print("REPORT", str(output))
    print("FAILURES", failures)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

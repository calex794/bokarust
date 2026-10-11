#!/usr/bin/env python3
"""Matched Chromium benchmark of Booka's real EasyMDE versus BookaRust.

Fresh Chromium processes for each configuration, identical Markdown payload,
viewport and font size. Results are saved even if one editor stalls/crashes.
Reference is the open-source EasyMDE 2.20.0 package with Booka toolbar options.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import psutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "browser-reports" / "comparison"
PATTERN = "Тест текста на русском языке. 世界 🌍\n"
SIZES = ((5_000, "5 тыс."), (10_000, "10 тыс."), (20_000, "20 тыс."), (50_000, "50 тыс."), (150_000, "150 тыс."), (1_000_000, "1 млн"))
ORIGINAL_SHA = "f19a7a20174cc2651d88c51123afb5f8e9093d4b"
OLD_SCRIPT = "EasyMDE 2.20.0, with toolbar/settings based on chapter-editor-images.js"
RUST_INPUT = 'textarea[aria-label="Текст главы"]'


def p(values: list[float], q: float) -> float:
    values = sorted(values)
    if not values:
        return 0.0
    i = (len(values) - 1) * q
    lo = int(i)
    return round(values[lo] + (values[min(lo + 1, len(values)-1)] - values[lo]) * (i - lo), 3)


def sample_text(n: int) -> str:
    text = (PATTERN * (n // len(PATTERN) + 1))[:n]
    assert len(text) == n
    return text


def chrome_process_memory() -> dict:
    """USS is private physical memory of all Chromium processes belonging to this run."""
    count, uss, rss = 0, 0, 0
    for child in psutil.Process().children(recursive=True):
        try:
            name = child.name().lower()
            if not ("chrome" in name or "chromium" in name):
                continue
            info = child.memory_full_info()
            count += 1
            rss += info.rss
            uss += getattr(info, "uss", 0)
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
            continue
    return {
        "processes": count,
        "uss_mib": round(uss / 1048576, 2),
        "sum_rss_mib": round(rss / 1048576, 2),
    }


def heap(page) -> dict:
    try:
        cdps = page.context.new_cdp_session(page)
        res = cdps.send("Runtime.getHeapUsage")
        cdps.detach()
        return {
            "js_heap_used_mib": round(res.get("usedSize", 0) / 1048576, 2),
            "js_heap_total_mib": round(res.get("totalSize", 0) / 1048576, 2),
        }
    except Exception:
        return {"js_heap_used_mib": None, "js_heap_total_mib": None}


def payload_into_editor(page, kind: str, n: int, label: str) -> float:
    if kind == "rust":
        # Time includes generation of the Rust sample and updating the native textarea.
        delta = page.evaluate(
            """(label) => {
              const button = [...document.querySelectorAll('button')].find(x => x.textContent.trim() === label);
              if (!button) throw Error('Missing Rust sample button: ' + label);
              const start = performance.now();
              button.click();
              return performance.now() - start;
            }""",
            label,
        )
    else:
        # Time excludes building and transferring the identical payload; this is
        # only the legacy library's content replacement cost.
        doc = sample_text(n)
        delta = page.evaluate(
            """(text) => {
              const editor = document.querySelector('textarea[data-chapter-markdown-editor]')._chapterMarkdownEditor;
              if (!editor || !editor.codemirror) throw Error('Real EasyMDE is not initialized');
              const start = performance.now();
              editor.value(text);
              return performance.now() - start;
            }""",
            doc,
        )
    return round(delta, 3)


def prepare_typing(page, kind: str) -> None:
    page.evaluate(
        """(kind) => {
          if (window.__stopTyping) window.__stopTyping();
          const events = [];
          const root = document;
          const textarea = document.querySelector('textarea[aria-label="Текст главы"]');
          const cm = kind === 'easymde' ? textarea._chapterMarkdownEditor.codemirror : null;
          const input = kind === 'rust' ? textarea : cm.getInputField();
          const down = (e) => {
            if (e.key !== 'x') return;
            events.push({t: performance.now(), applied: null, frame: null});
          };
          const updated = () => {
            const entry = events.find(x => x.applied === null);
            if (!entry) return;
            entry.applied = performance.now() - entry.t;
            requestAnimationFrame(() => {
              entry.frame = performance.now() - entry.t;
            });
          };
          root.addEventListener('keydown', down, true);
          if (cm) {
            cm.on('change', updated);
            cm.setCursor(cm.lastLine(), cm.getLine(cm.lastLine()).length);
            cm.focus();
          } else {
            input.addEventListener('input', updated);
            input.focus();
            input.setSelectionRange(input.textLength, input.textLength);
          }
          window.__benchmarkEvents = events;
          window.__stopTyping = () => {
            root.removeEventListener('keydown', down, true);
            if (cm) cm.off('change', updated);
            else input.removeEventListener('input', updated);
          };
        }""",
        kind,
    )


def type_letters(page, kind: str, count: int = 24) -> dict:
    prepare_typing(page, kind)
    page.wait_for_timeout(220)
    for i in range(count):
        page.keyboard.press("x")
        page.wait_for_function(
            "(n) => window.__benchmarkEvents.filter(e => e.frame !== null).length >= n",
            arg=i + 1,
            timeout=20000,
        )
    report = page.evaluate(
        """(kind) => {
          window.__stopTyping();
          const arr = window.__benchmarkEvents;
          const textarea = document.querySelector('textarea[aria-label="Текст главы"]');
          const cm = kind === 'easymde' ? textarea._chapterMarkdownEditor.codemirror : null;
          const value = cm ? cm.getValue() : textarea.value;
          return {
            samples: arr.map(e => ({applied:e.applied, frame:e.frame})),
            last_24: value.slice(-24),
            final_code_units: value.length
          };
        }""",
        kind,
    )
    assert report["last_24"] == "x" * count, f"Editor failed to insert all letters: {report['last_24']!r}"
    rows = report["samples"]
    assert len(rows) == count and all(
        x["applied"] is not None and x["frame"] is not None for x in rows
    ), rows
    return {
        "event_count": count,
        "keydown_to_change_ms": {
            "p50": p([x["applied"] for x in rows], .50),
            "p95": p([x["applied"] for x in rows], .95),
            "max": round(max(x["applied"] for x in rows), 3),
        },
        "keydown_to_frame_after_change_ms": {
            "p50": p([x["frame"] for x in rows], .50),
            "p95": p([x["frame"] for x in rows], .95),
            "max": round(max(x["frame"] for x in rows), 3),
        },
    }


def measure(browser_type, base_url: str, kind: str, n: int, label: str, repeat: int) -> dict:
    browser = browser_type.launch(
        headless=True,
        args=["--no-sandbox", "--disable-dev-shm-usage"],
    )
    context = browser.new_context(
        viewport={"width": 1400, "height": 1000},
        locale="ru-RU",
        device_scale_factor=1,
    )
    page = context.new_page()
    result = {
        "editor": kind,
        "size_codepoints": n,
        "repeat": repeat,
        "browser_version": browser.version,
        "status": "running",
    }
    errors: list[str] = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    try:
        url = base_url + ("/compare/reference.html" if kind == "easymde" else "/")
        response = page.goto(url, wait_until="domcontentloaded", timeout=60000)
        assert response and response.ok, f"Reference page unavailable: {url}"
        if kind == "rust":
            page.wait_for_function(
                "() => document.body.innerText.includes('Глава загружена')",
                timeout=90000,
            )
            input_selector = RUST_INPUT
        else:
            page.wait_for_function(
                "() => !!document.querySelector('textarea[data-chapter-markdown-editor]')"
                "._chapterMarkdownEditor?.codemirror",
                timeout=90000,
            )
            input_selector = ".CodeMirror"
        page.wait_for_timeout(300)
        result["idle_uss"] = chrome_process_memory()
        result["idle_heap"] = heap(page)

        result["replace_document_ms"] = payload_into_editor(page, kind, n, label)
        # Checking through CodeMirror's own document, not its hidden textarea.
        loaded = page.evaluate(
            """(kind) => {
              const textarea = document.querySelector('textarea[aria-label="Текст главы"]');
              const cm = kind === 'easymde' ? textarea._chapterMarkdownEditor.codemirror : null;
              const value = cm ? cm.getValue() : textarea.value;
              const el = cm ? document.querySelector('.CodeMirror') : textarea;
              return {
                count: [...value].length,
                has_unicode: value.includes('世界 🌍') && value.includes('Тест'),
                width: el.getBoundingClientRect().width,
                height: el.getBoundingClientRect().height,
                line_count: cm ? cm.lineCount() : value.split('\\n').length,
              };
            }""",
            kind,
        )
        assert loaded["count"] == n, f"Wrong characters: {loaded['count']} instead of {n}"
        assert loaded["has_unicode"], f"Corrupt Unicode in {kind}"
        result["field_size"] = {
            "width_px": round(loaded["width"], 2),
            "height_px": round(loaded["height"], 2),
        }
        result["line_count"] = loaded["line_count"]
        page.wait_for_timeout(500)
        result["loaded_uss"] = chrome_process_memory()
        result["loaded_heap"] = heap(page)

        result["typing"] = type_letters(page, kind)
        result["typed_uss"] = chrome_process_memory()
        result["typed_heap"] = heap(page)
        result["loaded_minus_idle_uss_mib"] = round(
            result["loaded_uss"]["uss_mib"] - result["idle_uss"]["uss_mib"], 2
        )
        result["typed_minus_idle_uss_mib"] = round(
            result["typed_uss"]["uss_mib"] - result["idle_uss"]["uss_mib"], 2
        )
        assert not errors, f"Uncaught browser errors: {errors[:3]}"
        page.screenshot(path=str(OUTPUT / f"{kind}-{n}-repeat{repeat}.png"))
        result["status"] = "ok"
        typing = result["typing"]["keydown_to_frame_after_change_ms"]["p95"]
        print(
            f"MEASURE {kind:7s} n={n:7d} trial={repeat} "
            f"frame-p95={typing:8.2f}ms change-p95="
            f"{result['typing']['keydown_to_change_ms']['p95']:8.2f}ms "
            f"USS={result['typed_uss']['uss_mib']:7.1f}MiB "
            f"(+{result['typed_minus_idle_uss_mib']:.1f}) "
            f"load={result['replace_document_ms']:.1f}ms",
            flush=True,
        )
    except Exception as exc:
        result["status"] = "failed"
        result["error"] = f"{type(exc).__name__}: {str(exc).splitlines()[0][:500]}"
        print(f"FAILED {kind} n={n} trial={repeat}: {result['error']}", flush=True)
    finally:
        try:
            context.close()
        finally:
            browser.close()
        time.sleep(.2)
    return result


def start_server() -> tuple[subprocess.Popen, str, Path]:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    binary = ROOT / "target" / "release" / "editor-server"
    if not binary.exists():
        raise RuntimeError("Build editor-server in release mode first")
    path = Path(tempfile.mkdtemp(prefix="bookarust-compare-"))
    env = {**os.environ, "PORT": str(port), "CHAPTER_FILE": str(path / "chapter.json")}
    log = open(path / "server.log", "wb")
    proc = subprocess.Popen(
        [str(binary)], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT
    )
    log.close()
    base_url = f"http://127.0.0.1:{port}"
    for _ in range(150):
        try:
            with urllib.request.urlopen(f"{base_url}/api/chapter", timeout=1) as r:
                if r.status == 200:
                    return proc, base_url, path
        except OSError:
            time.sleep(.2)
    proc.terminate()
    raise RuntimeError("Rust editor server failed to start")


def summarize(rows: list[dict]) -> list[dict]:
    table = []
    for n, _ in SIZES:
        a = [r for r in rows if r["size_codepoints"] == n and r["editor"] == "rust" and r["status"] == "ok"]
        b = [r for r in rows if r["size_codepoints"] == n and r["editor"] == "easymde" and r["status"] == "ok"]
        if not a or not b:
            table.append({"size_codepoints": n, "status": "incomplete"})
            continue
        def median(samples: list[dict], getter):
            return round(__import__("statistics").median(getter(x) for x in samples), 2)
        rust_p95 = median(a, lambda x: x["typing"]["keydown_to_frame_after_change_ms"]["p95"])
        old_p95 = median(b, lambda x: x["typing"]["keydown_to_frame_after_change_ms"]["p95"])
        rust_uss = median(a, lambda x: x["typed_uss"]["uss_mib"])
        old_uss = median(b, lambda x: x["typed_uss"]["uss_mib"])
        table.append({
            "size_codepoints": n, "status": "ok",
            "rust_frame_p95_ms": rust_p95,
            "easymde_frame_p95_ms": old_p95,
            "rust_chromium_uss_mib": rust_uss,
            "easymde_chromium_uss_mib": old_uss,
            "rust_loaded_minus_idle_uss_mib": median(a, lambda x: x["loaded_minus_idle_uss_mib"]),
            "easymde_loaded_minus_idle_uss_mib": median(b, lambda x: x["loaded_minus_idle_uss_mib"]),
            "rust_load_ms": median(a, lambda x: x["replace_document_ms"]),
            "easymde_load_ms": median(b, lambda x: x["replace_document_ms"]),
            "rust_width_px": median(a, lambda x: x["field_size"]["width_px"]),
            "easymde_width_px": median(b, lambda x: x["field_size"]["width_px"]),
            "rust_heap_mib": median(a, lambda x: x["typed_heap"]["js_heap_used_mib"]),
            "easymde_heap_mib": median(b, lambda x: x["typed_heap"]["js_heap_used_mib"]),
        })
    return table


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--max-size", type=int, default=1_000_000)
    args = parser.parse_args()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    proc, url, tmpdir = start_server()
    rows: list[dict] = []
    try:
        with sync_playwright() as p:
            for n, label in SIZES:
                if n > args.max_size:
                    continue
                for trial in range(1, args.repeats + 1):
                    for kind in ("rust", "easymde"):
                        rows.append(measure(p.chromium, url, kind, n, label, trial))
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    result = {
        "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "environment": {
            "platform": platform.platform(),
            "test_workflow_sha": os.environ.get("GITHUB_SHA", "local"),
            "original_booka_sha": ORIGINAL_SHA,
            "old_editor_js": OLD_SCRIPT,
            "viewport": "1400x1000",
            "character_count": "Python Unicode codepoints (not UTF-16 units)",
            "measured_typing": "24 real Chromium keypresses, keydown to frame after textarea input / CodeMirror change",
            "memory": "USS across all Chromium processes spawned by Python test; each profile uses fresh browser process; unlike app-specific WASM/JS heap",
            "load_time": "Rust demo button also generates sample, while EasyMDE receives generated payload; do not interpret as strict like-for-like load speed",
            "note": "isolated official EasyMDE 2.20.0 with Booka-equivalent toolbar options, not byte-identical JavaScript or the deployed NEwBOO site",
        },
        "rows": rows,
        "summary": summarize(rows),
    }
    filename = OUTPUT / "comparison.json"
    filename.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    lines = [
        "# BookaRust / original Booka EasyMDE comparison",
        "",
        f"Old code pinned to {ORIGINAL_SHA}. Chromium headless / Linux, 1400x1000. "
        f"Independent browser process per editor and size, {args.repeats} runs per case.",
        "",
        "| Chapter | Rust p95 key→frame | EasyMDE p95 | Rust browser USS | EasyMDE browser USS |",
        "|---|---:|---:|---:|---:|",
    ]
    for item in result["summary"]:
        if item["status"] != "ok":
            lines.append(f"| {item['size_codepoints']:,} | incomplete | incomplete | – | – |")
        else:
            lines.append(
                f"| {item['size_codepoints']:,} | {item['rust_frame_p95_ms']:.2f} ms | "
                f"{item['easymde_frame_p95_ms']:.2f} ms | "
                f"{item['rust_chromium_uss_mib']:.1f} MiB | {item['easymde_chromium_uss_mib']:.1f} MiB |"
            )
    lines.extend([
        "",
        "**Memory USS** is Chromium's total unique resident memory (all processes); "
        "the incremental column in comparison.json excludes initial editor load. "
        "Results are CI observations, not forecasts for end-user devices.",
        "",
        "The old editor uses the official EasyMDE 2.20.0 bundle and mirrored Booka settings, "
        "not a mock. The fixture does not include all other NEwBOO website modules.",
        "",
        "Rust sample generation time and EasyMDE replace-document time have different setup "
        "costs; compare keypress and memory numbers first. IME/native mobile untested.",
    ])
    (OUTPUT / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    for item in result["summary"]:
        print("SUMMARY", json.dumps(item, ensure_ascii=False), flush=True)
    failed = [r for r in rows if r["status"] != "ok"]
    print(f"REPORT {filename} ERRORS={len(failed)}", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

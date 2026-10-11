#!/usr/bin/env python3
"""Stage OSS EasyMDE 2.20.0 matching Booka, without exposing private Booka code.

Booka uses EasyMDE 2.20.0; we install that exact open-source npm release and
recreate the old public editor config, not the app's private JavaScript.
"""
from pathlib import Path
import hashlib
import json
import shutil

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "reference-easymde-dist" / "package" / "dist"
DIST = ROOT / "dist" / "compare"
OLD_BLOB_HASHES = {
    "easymde.min.js": "45fadc9598012d5dd2e3b7ef76fd9414504c40b7",
    "easymde.min.css": "26f8790da41ee2ba7f805b48623bc1e00545aac7",
}
HTML = """<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>NEwBOO EasyMDE v2.20.0 — comparison fixture</title>
<link rel="stylesheet" href="/compare/easymde.min.css">
<style>
:root {
 --border-color: #dce0e7; --bg-surface:#fff; --bg-secondary:#f5f6f8;
 --text-main:#162035; --text-muted:#596578; --primary-color:#204a9d;
 --radius-md:7px; --radius-lg:12px;
}
*{box-sizing:border-box}
body{margin:0;background:#f5f6f8;color:#162035;font-family:system-ui,sans-serif}
.shell{max-width:1200px;margin:0 auto;padding:24px}
.chapter-authoring-form{background:white;border:1px solid #dce0e7;border-radius:12px;
 margin:49px 0 0;padding:20px;gap:12px;width:100%;max-width:100%;min-width:0}
.chapter-authoring-section{min-width:0;max-width:100%}
.chapter-authoring-field{display:block;width:100%;min-width:0}
.chapter-authoring-field>span{display:block;font-weight:650;margin-bottom:12px}
.EasyMDEContainer{width:100%;min-width:0;max-width:100%}
.EasyMDEContainer .editor-toolbar{display:flex;flex-wrap:wrap;gap:6px;
 padding:8px;min-height:48px;background:#fff}
.EasyMDEContainer .editor-toolbar button.chapter-editor-toolbar-button{width:auto;
 min-width:34px;height:32px;padding:0 9px;border:1px solid #dce0e7;
 border-radius:8px;font:850 12px system-ui,sans-serif}
.EasyMDEContainer .CodeMirror{height:700px;min-height:700px;
 font:16px/1.65 ui-monospace,monospace;width:100%;max-width:100%}
.EasyMDEContainer .CodeMirror-scroll{height:700px;max-width:100%}
@media(max-width:760px){.shell{padding:10px}.chapter-authoring-form{padding:12px}}
</style>
</head><body><div class="shell">
<form class="chapter-authoring-form">
<section class="chapter-authoring-section">
<label class="chapter-authoring-field">
<span>Текст главы</span>
<textarea aria-label="Текст главы" name="content_markdown"
  data-chapter-markdown-editor>Первый абзац.

Второй абзац.</textarea>
</label></section></form></div>
<script src="/compare/easymde.min.js"></script>
<script>
(function () {
  const textarea = document.querySelector('textarea[data-chapter-markdown-editor]');
  const toolbarButton = (name, text, title, action) => ({
    name, action, title, text, className:"chapter-editor-toolbar-button"
  });
  // Mirror actual chapter-editor-images.js editor options and toolbar.
  const toolbar = [
    toolbarButton("bold","B","Жирный текст",EasyMDE.toggleBold),
    toolbarButton("italic","I","Курсив",EasyMDE.toggleItalic),
    toolbarButton("heading","H","Заголовок",EasyMDE.toggleHeadingSmaller),
    "|",
    toolbarButton("quote","Цитата","Цитата",EasyMDE.toggleBlockquote),
    toolbarButton("unordered-list","• список","Маркированный список",EasyMDE.toggleUnorderedList),
    toolbarButton("ordered-list","1. список","Нумерованный список",EasyMDE.toggleOrderedList),
    "|",
    toolbarButton("image-url","Картинка по ссылке","Вставить картинку по ссылке",()=>{}),
    toolbarButton("image-upload","Загрузить картинку","Загрузить картинку с компьютера",()=>{}),
    "|",
    toolbarButton("preview","Предпросмотр","Предпросмотр",EasyMDE.togglePreview),
    toolbarButton("side-by-side","Два окна","Редактор и предпросмотр рядом",EasyMDE.toggleSideBySide),
    toolbarButton("fullscreen","На весь экран","На весь экран",EasyMDE.toggleFullScreen)
  ];
  const editor = new EasyMDE({
    element:textarea,
    autoDownloadFontAwesome:false,
    spellChecker:false,
    status:false,
    lineWrapping:true,
    forceSync:true,
    toolbar
  });
  textarea._chapterMarkdownEditor = editor;
  textarea.dataset.easymdeReady = "1";
  textarea.closest('form').addEventListener("submit",() => textarea.value=editor.value());
})();
</script>
</body></html>
"""


def main():
    DIST.mkdir(parents=True, exist_ok=True)
    source_hashes = {}
    for name, expected in OLD_BLOB_HASHES.items():
        path = SOURCE / name
        data = path.read_bytes()
        actual = hashlib.sha1(f"blob {len(data)}\0".encode("ascii") + data).hexdigest()
        source_hashes[name] = {"official_sha":actual, "booka_sha":expected, "byte_identical":actual == expected, "size_bytes":len(data)}
        shutil.copyfile(path, DIST / name)
        print(f"EasyMDE npm v2.20.0 {name}: {len(data)} bytes; matches pinned booka = {actual == expected}", flush=True)
    (DIST / "reference.html").write_text(HTML, encoding="utf-8")
    reports=ROOT/"browser-reports"/"comparison"
    reports.mkdir(parents=True,exist_ok=True)
    (reports/"source_check.json").write_text(json.dumps(source_hashes,indent=2),encoding="utf-8")
    print("Reference fixture ready; no private Booka JavaScript copied into public repo", flush=True)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Stage the *real* pinned Booka EasyMDE editor as a local fixture.

The original files are checked out by GitHub Actions, not reimplemented.
This lives under dist/compare/ only: no EasyMDE code is shipped as Rust application code.
"""
from pathlib import Path
import hashlib
import shutil

ROOT = Path(__file__).resolve().parents[2]
OLD = ROOT / "reference-booka" / "backend/app/web/static"
DIST = ROOT / "dist" / "compare"
FILES = {
    "vendor/easymde/easymde.min.js": ("easymde.min.js", "45fadc9598012d5dd2e3b7ef76fd9414504c40b7"),
    "vendor/easymde/easymde.min.css": ("easymde.min.css", "26f8790da41ee2ba7f805b48623bc1e00545aac7"),
    "js/chapter-editor-images.js": ("chapter-editor-images.js", "cbb062bb8abd542e4ebd0b75d95a27196fbe0136"),
    "css/components/chapter-authoring.css": ("chapter-authoring.css", "668614e21f6da78bef960eb37b9436bbe25cc573"),
}
HTML = """<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>NEwBOO — старый EasyMDE, эталонное испытание</title>
<link rel="stylesheet" href="/compare/easymde.min.css">
<link rel="stylesheet" href="/compare/chapter-authoring.css">
<style>
:root {
 --border-color: #dce0e7; --bg-surface:#fff; --bg-secondary:#f5f6f8;
 --text-main:#162035; --text-muted:#596578; --primary-color:#204a9d;
 --radius-md:7px; --radius-lg:12px;
}
* { box-sizing:border-box; }
body {margin:0; background:#f5f6f8; color:#162035; font-family:system-ui,sans-serif;}
.shell {max-width:1200px; margin:0 auto; padding:24px;}
.chapter-authoring-form {margin:49px 0 0; padding:20px; gap:12px; width:100%;}
.chapter-authoring-section{min-width:0; max-width:100%;}
.chapter-authoring-field{display:block;width:100%;min-width:0;}
.EasyMDEContainer{width:100%;min-width:0;max-width:100%;}
.EasyMDEContainer .CodeMirror {height:700px; min-height:700px;
    font:16px/1.65 ui-monospace,monospace; width:100%; max-width:100%;}
.EasyMDEContainer .CodeMirror-scroll {height:700px;max-width:100%;}
.chapter-authoring-form .EasyMDEContainer .editor-toolbar {min-height:48px;}
@media(max-width:760px){.shell{padding:10px}.chapter-authoring-form{padding:12px}}
</style>
</head><body><div class="shell">
<form class="chapter-authoring-form">
<section class="chapter-authoring-section">
<label class="chapter-authoring-field">
<span>Текст главы</span>
<textarea aria-label="Текст главы" name="content_markdown" data-chapter-markdown-editor></textarea>
</label></section>
</form></div>
<script src="/compare/easymde.min.js"></script>
<script src="/compare/chapter-editor-images.js"></script>
</body></html>
"""


def main():
    DIST.mkdir(parents=True, exist_ok=True)
    for src, (dest, expected) in FILES.items():
        path = OLD / src
        data = path.read_bytes()
        actual = hashlib.sha1(f"blob {len(data)}\0".encode("ascii") + data).hexdigest()
        if actual != expected:
            raise RuntimeError(f"Pinned booka source changed: {src}, {actual} != {expected}")
        shutil.copyfile(path, DIST / dest)
        print(f"OK pinned original: {src} ({len(data)} bytes)", flush=True)
    (DIST / "reference.html").write_text(HTML, encoding="utf-8")
    print("Reference fixture ready: dist/compare/reference.html", flush=True)


if __name__ == "__main__":
    main()

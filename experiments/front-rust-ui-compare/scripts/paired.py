#!/usr/bin/env python3
"""Matched-runner Rust SSR comparison.

Measures only server-side rendering. Does not test FrontRust or browser latency.
All candidate binaries are built on one runner from the same Cargo.lock,
tested against the same scene fixture, then measured in alternating order.
"""
import json
import os
from html.parser import HTMLParser
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "results" / "paired"
OUT.mkdir(parents=True, exist_ok=True)
manifest = ROOT / "Cargo.toml"
env = os.environ.copy()
env["CARGO_TARGET_DIR"] = str(Path(env.get("RUNNER_TEMP", "/tmp")) / "front-rust-ui-paired")
env["CARGO_TERM_COLOR"] = "never"
bins = {
    "rust-baseline": "rust-baseline",
    "leptos-ssr": "leptos-ssr-compare",
    "dioxus-ssr": "dioxus-ssr-compare",
}
log_file = OUT / "commands.log"

def run_logged(command, label):
    start = time.perf_counter()
    with log_file.open("a", encoding="utf-8") as log:
        log.write(f"\n# {label}\n$ {' '.join(map(str, command))}\n")
        log.flush()
        p = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=False)
        log.write(f"exit={p.returncode}\n")
    if p.returncode:
        raise RuntimeError(f"{label}: exit {p.returncode}; see commands.log")
    return time.perf_counter() - start

class SemanticHTML(HTMLParser):
    def __init__(self):
        super().__init__()
        self.rectangles = []
        self.mains = 0
        self.heading = 0

    def handle_starttag(self, tag, attrs):
        props = dict(attrs)
        if tag == "main" and props.get("data-benchmark") == "ui-compare":
            self.mains += 1
        if tag == "h1":
            self.heading += 1
        if "data-id" in props:
            assert tag == "div", (tag, props)
            assert props.get("role") == "img", props
            assert props.get("aria-label"), props
            self.rectangles.append(
                (int(props["data-id"]), int(props["data-x"]), int(props["data-y"]))
            )

def parse_checked_html(html, n):
    p = SemanticHTML()
    p.feed(html)
    assert (p.mains, p.heading) == (1, 1), (p.mains, p.heading)
    assert len(p.rectangles) == n, (n, len(p.rectangles))
    assert sorted(x[0] for x in p.rectangles) == list(range(1, n + 1))
    assert (2, 164, 28) in p.rectangles
    return sorted(p.rectangles)

# Generate Cargo.lock when absent, then enforce it across every candidate.
run_logged(["cargo", "fetch", "--manifest-path", str(manifest)], "fetch + lock resolution")
lock = ROOT / "Cargo.lock"
assert lock.is_file(), "Cargo.lock not generated"
shutil.copy2(lock, OUT / "Cargo.lock")
build_seconds = run_logged(
    ["cargo", "build", "--manifest-path", str(manifest), "--locked",
     "--release", "--all-features", "--bins"],
    "build all candidates on same runner"
)
run_logged(
    ["cargo", "test", "--manifest-path", str(manifest), "--locked", "--lib"],
    "shared scene command tests"
)

executables = {
    name: Path(env["CARGO_TARGET_DIR"]) / "release" / binary
    for name, binary in bins.items()
}
assert all(p.is_file() for p in executables.values())
baseline_states = {}
for count in (3, 300):
    for name, binary in executables.items():
        html = subprocess.check_output([str(binary), "--html", str(count)], env=env).decode()
        (OUT / f"{name}_{count}.html").write_text(html, encoding="utf8")
        state = parse_checked_html(html, count)
        if count not in baseline_states:
            baseline_states[count] = state
        assert state == baseline_states[count], f"{name} != reference state"
        subprocess.check_call([str(binary), "--check", str(count)], env=env)

# Warm up the executable processes and shared dynamic library/cache state.
for name, binary in executables.items():
    for count in (3, 300):
        subprocess.run(
            [str(binary), "--bench", str(count)], env=env,
            stdout=subprocess.DEVNULL, check=True
        )

# Rotate randomized candidate order to reduce ordering and drift bias.
rng = random.Random(20261025)
samples = {name: {"3": [], "300": []} for name in bins}
rss_samples = {name: {"3": [], "300": []} for name in bins}
rounds = 9
for round_num in range(rounds):
    for count in (3, 300):
        order = list(bins)
        rng.shuffle(order)
        for name in order:
            binary = executables[name]
            rss_file = OUT / "_rss.tmp"
            command = ["/usr/bin/time", "-f", "%M", "-o", str(rss_file),
                       str(binary), "--bench", str(count)]
            proc = subprocess.run(command, env=env, check=True, capture_output=True, text=True)
            data = json.loads(proc.stdout)
            assert data["variant"] == name and data["rectangles"] == count
            assert data["samples"] >= 300 and data["p95_us"] >= data["p50_us"]
            data["round"] = round_num + 1
            samples[name][str(count)].append(data)
            rss_samples[name][str(count)].append(int(rss_file.read_text().strip()))

def median(values):
    return round(float(statistics.median(values)), 4)

rows = []
for name, binary in executables.items():
    row = {
        "variant": name,
        "binary_bytes": binary.stat().st_size,
        "measurements": {}
    }
    for count in ("3", "300"):
        ps = [x["p95_us"] for x in samples[name][count]]
        row["measurements"][count] = {
            "rounds": rounds,
            "p95_us_median": median(ps),
            "p95_us_min": round(min(ps), 4),
            "p95_us_max": round(max(ps), 4),
            "rss_kb_median": median(rss_samples[name][count]),
            "samples": samples[name][count],
        }
    rows.append(row)

metrics = {
    "scope": "native SSR only; one GitHub runner, Rust 1.99+; not FrontRust or browser",
    "toolchain": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "rounds": rounds,
    "build_all_seconds": round(build_seconds, 3),
    "fairness": "same scene and runner, randomized order; no paired browser or wasm comparison",
    "rows": rows,
}
(OUT / "metrics.json").write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf8")

lines = [
    "## Paired native SSR results — one runner",
    "",
    f"Toolchain: {metrics['toolchain']}; repeated rounds: {rounds}.",
    f"All-binaries release build: {build_seconds:.1f} s (one combined build, not per-project times).",
    "",
    "| Native renderer | Binary KiB | 3 shapes p95 median µs (range) | 300 shapes p95 median µs (range) | Native max RSS median MiB (300) |",
    "|---|---:|---:|---:|---:|",
]
for row in rows:
    x = row["measurements"]["3"]
    y = row["measurements"]["300"]
    lines.append(
        f"| {row['variant']} | {row['binary_bytes']/1024:.1f} | "
        f"{x['p95_us_median']:.3f} ({x['p95_us_min']:.3f}–{x['p95_us_max']:.3f}) | "
        f"{y['p95_us_median']:.2f} ({y['p95_us_min']:.2f}–{y['p95_us_max']:.2f}) | "
        f"{y['rss_kb_median']/1024:.2f} |"
    )
lines.extend([
    "",
    "**Limitation:** these times are *server-side HTML rendering only*. RSS refers to",
    "the short-lived native benchmark binary, **not** a browser or interactive app.",
    "All variants share one command/state engine, so only their SSR output path varies.",
    "This experiment is **not** a performance test of the FrontRust source code.",
])
summary = "\n".join(lines) + "\n"
(OUT / "summary.md").write_text(summary, encoding="utf8")
print(summary)
if os.getenv("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf8") as f:
        f.write(summary)

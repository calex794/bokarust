from pathlib import Path
import json
import os
root=Path(".results")
data=[json.loads(p.read_text()) for p in sorted(root.glob("results-*/metrics.json"))]
lines=[
    "# FrontRust comparative feasibility experiment — phase 1",
    "",
    "**Only native SSR / HTML was tested**. The plain Rust control is not FrontRust.",
    "No browser, WASM, interactive actions, cold start, or development-time performance has been measured.",
    "",
    "| Variant | Build (s) | Native binary KiB | SSR 3 objects p95 (µs) | SSR 300 objects p95 (µs) |",
    "|---|---:|---:|---:|---:|",
]
for row in data:
    lines.append(f"| {row['variant']} | {row['build_seconds']:.2f} | {row['binary_bytes']/1024:.0f} | {row['bench_3']['p95_us']:.2f} | {row['bench_300']['p95_us']:.2f} |")
if len(data)!=3: lines.append(f"\n**Incomplete: {len(data)}/3 variants produced metrics.**")
lines.extend([
    "",
    "Tests require exact scene state, correct object count, semantic attributes, undo/redo unit tests.",
    "All variants share the state engine, so this measures native HTML rendering only.",
    "Jobs run on different shared GitHub hosts; cross-job speed ranking is not statistically reliable.",
    "RSS and browser load must be measured in a later matched web experiment.",
    "Real FrontRust source is absent from Lalalalendia/FrontRust; it is not substituted here.",
])
summary="\n".join(lines)+"\n"
print(summary)
if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"],"a",encoding="utf8") as f: f.write(summary)

#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
OUT="$ROOT/results"
mkdir -p "$OUT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://ziglang.org/download/0.17.0/zig-x86_64-linux-0.17.0.tar.xz"
HASH="1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"
curl -fLsS --retry 3 --connect-timeout 20 "$URL" -o "$TMP/zig.tar.xz"
printf '%s  %s\n' "$HASH" "$TMP/zig.tar.xz" | sha256sum -c -
tar -xJf "$TMP/zig.tar.xz" -C "$TMP"
ZIG="$TMP/zig-x86_64-linux-0.17.0/zig"
export ZIG_VERSION="$("$ZIG" version)"
{
  echo "Rust=$(rustc --version)"
  echo "Zig=$ZIG_VERSION SHA256=$HASH"
  echo "CPU=$(grep -m1 'model name' /proc/cpuinfo)"
  echo "CC=$(cc --version | head -1)"
} | tee "$OUT/environment.txt"
python3 - "$ROOT" "$ZIG" <<'PY'
from pathlib import Path
import subprocess,sys,time
p=Path(sys.argv[1])
zig=sys.argv[2]
builds=[
  ("rust",["rustc","--edition=2021","--crate-type=cdylib","-C","opt-level=3","-C","target-cpu=x86-64",str(p/"render.rs"),"-o",str(p/"rust.so")]),
  ("zig",[zig,"build-lib",str(p/"render.zig"),"-dynamic","-O","ReleaseFast","-target","x86_64-linux-gnu","-femit-bin="+str(p/"zig.so")])
]
for name,cmd in builds:
    start=time.perf_counter()
    with (p/"results"/(name+"-build.log")).open("w") as f:
        r=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT)
    (p/"results"/(name+"-build-seconds.txt")).write_text(str(time.perf_counter()-start))
    if r.returncode: raise SystemExit(name+" build failed: see build.log")
PY
cc -O3 -std=c11 -Wall -Wextra "$ROOT/bench.c" -o "$ROOT/bench" -ldl
stat -c '%n %s bytes' "$ROOT"/*.so | tee "$OUT/sizes.txt"
ldd "$ROOT/rust.so" > "$OUT/rust-linked.txt" || true
ldd "$ROOT/zig.so" > "$OUT/zig-linked.txt" || true
python3 "$ROOT/compare.py"

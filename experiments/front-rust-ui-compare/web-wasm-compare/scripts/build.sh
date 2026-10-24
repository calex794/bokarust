#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXP="$(cd "$ROOT/.." && pwd)"
OUT="$ROOT/results"
SITE="$OUT/site"
mkdir -p "$OUT" "$SITE"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ZIG_URL="https://ziglang.org/download/0.17.0/zig-x86_64-linux-0.17.0.tar.xz"
ZIG_SHA="1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"
curl -fLsS --retry 3 --connect-timeout 20 "$ZIG_URL" -o "$TMP/zig.tar.xz"
printf '%s  %s\n' "$ZIG_SHA" "$TMP/zig.tar.xz" | sha256sum -c -
tar -xJf "$TMP/zig.tar.xz" -C "$TMP"
ZIG="$TMP/zig-x86_64-linux-0.17.0/zig"
{
  rustc --version
  "$ZIG" version
  node --version
  echo "ZIG_SHA256=$ZIG_SHA"
  grep -m1 'model name' /proc/cpuinfo || true
} > "$OUT/toolchain.txt"
export RUSTUP_TOOLCHAIN=1.99.1
export CARGO_TARGET_DIR="$RUNNER_TEMP/front-rust-browser-wasm"
rustup target list --installed
rustc --version --verbose
cargo generate-lockfile --manifest-path "$ROOT/rust/Cargo.toml"
cargo build --manifest-path "$ROOT/rust/Cargo.toml" --release --locked --target wasm32-unknown-unknown
cargo build --manifest-path "$ROOT/rust/Cargo.toml" --profile speed --locked --target wasm32-unknown-unknown
mkdir -p "$SITE/rust-small" "$SITE/rust-fast"
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/front_rust_wasm_browser_probe.wasm" "$SITE/rust-small/kernel.wasm"
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/speed/front_rust_wasm_browser_probe.wasm" "$SITE/rust-fast/kernel.wasm"
cp "$EXP/zig-kernel/render.zig" "$OUT/browser-zig.zig"
cat >> "$OUT/browser-zig.zig" <<'ZIG'
const CAP_BROWSER: usize = 2 * 1024 * 1024;
var BROWSER_BUFFER: [CAP_BROWSER]u8 = undefined;
pub export fn output_ptr() usize {
    return @intFromPtr(&BROWSER_BUFFER);
}
pub export fn output_cap() usize { return CAP_BROWSER; }
pub export fn render_html(ptr: [*]u8, cap: usize, n: usize) usize {
    return render_html_zig(ptr, cap, n);
}
ZIG
for MODE in small fast; do
  case "$MODE" in
    small) ZIG_OPT=ReleaseSmall ;;
    fast) ZIG_OPT=ReleaseFast ;;
  esac
  mkdir -p "$SITE/zig-$MODE"
  "$ZIG" build-exe "$OUT/browser-zig.zig" -target wasm32-freestanding \
     -O "$ZIG_OPT" -fno-entry -rdynamic -femit-bin="$SITE/zig-$MODE/kernel.wasm"
done
for VARIANT in rust-small rust-fast zig-small zig-fast; do
  cp "$ROOT/web/index.html" "$SITE/$VARIANT/index.html"
  cp "$ROOT/web/app.mjs" "$SITE/$VARIANT/app.mjs"
done
python3 - "$SITE" <<'PY'
import pathlib,sys
for path in pathlib.Path(sys.argv[1]).glob("*/kernel.wasm"):
    blob=path.read_bytes()
    assert blob[:4]==b'\0asm'
    print(path.parent.name,len(blob),"wasm bytes")
PY

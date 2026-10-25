# Two hard interactive user journeys — controlled research

**This is not the FrontRust framework**, nor a direct performance contest between
Leptos, Dioxus, or React. It extends PR #21's SSR and native/WASM probe with
actual interactive browser state, DOM and Canvas. The comparison is narrowly
JS application core vs Rust/WASM application core, using **exactly the same**
JavaScript renderer, input events and machine.

Scenarios:
- **Board**: 10,000 spatial objects, Canvas viewport culling, 30 actual
  synthetic `pointerdown` / `pointermove` / `pointerup` browser event triples,
  drag one transaction per gesture, 5 undo/redo, wheel zoom.
- **Docs**: 50 synthetic pages / 1,000 Unicode paragraphs; render 24 DOM rows,
  40 `beforeinput` edits (Russian, CJK and ASCII), 13 style toggles, 8 undo/redo,
  composition event trace. This is a document state and browser-input probe,
  **not** a real paginated, shaped rich-text engine or real human typing/IME.

Outcomes are validated through equal FNV checksums of document and board states,
revision counts, pointer and input event counts, and visible DOM/Canvas facts.
Metrics: paired p50/p95 browser-event-to-next-animation-frame and WASM binary
bytes. Script performs six runs on the same Chrome instance, alternating JS/WASM.
It records raw run JSON, Chrome/Rust versions and WASM bytes. Warm-up,
OS scheduling and browser headless limitations prevent interpreting these
figures as universal production rankings. `requestAnimationFrame` represents
frame scheduling, not a GPU presentation timestamp.

Run from repository root on Linux with Rust target and Chrome/Node 22:

```sh
cd experiments/front-rust-ui-compare/interactive
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --lib
mkdir -p results
node scripts/chrome.mjs --wasm target/wasm32-unknown-unknown/release/front_rust_interactive_probe.wasm --out results/interactive.json
```

The GitHub Actions workflow is `.github/workflows/front-rust-interactive.yml`.
The existing PR #21 SSR comparison is separate; **do not combine its native SSR
microseconds with these browser end-to-end milliseconds into a ranking**.

Unverified/missing: interactive Leptos and Dioxus browser counterparts,
actual FrontRust source (its repo has no implementation and read-only connector),
TypeScript+React/Solid full UI, browser WASM code splitting, deployment on VPS,
real human IME, screenreader audits, saved document re-open and federation.
Those require separate scoped slices before any superiority claims.

Traceability: https://app.notion.com/p/40332a84beec8151abc5c61729fd02f8
and https://app.notion.com/p/40332a84beec818699cff01ccf10ac1d
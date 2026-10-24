# FrontRust comparison — experimental SSR-only phase

Three standalone native Rust programs render a common scene with three or
300 rectangles. All use the exact same Rust state engine (src/lib.rs).
The scenario moves rectangle 2 by (24,8), undoes, redoes and renders.
The output validator checks 3/300 objects, ID 2 at (164,28), semantic
attributes, accessible labels and exact object IDs.

Variants:
1. rust-baseline: direct hand-written HTML in Rust (**NOT FrontRust**).
2. leptos-ssr: Leptos 0.8.22 native server HTML.
3. dioxus-ssr: Dioxus 0.7.0 native server HTML.

To reproduce from this directory:
    cargo test --lib
    python3 scripts/measure.py rust-baseline
    python3 scripts/measure.py leptos-ssr
    python3 scripts/measure.py dioxus-ssr

The GitHub Actions matrix compiles, runs unit tests and renders both
scene sizes. Artifacts store build logs, checks, rendered HTML and JSON with
native binary bytes, wall-clock release compile time and render p95.

## Interpretation

This is only the **first test of the measurement harness**. No real
interactive web app, WebAssembly, DOM/Canvas parity, browser performance,
memory usage, cold startup or complexity-of-development is measured.
Numbers are not yet a fair speed ranking: separate GitHub-hosted runners
can have different loads. The initial lockfile is generated per CI job;
it should be checked in after versions are validated.

The real FrontRust repository was effectively empty (only .gitkeep) and
the GitHub connector returned 403 for a branch creation. These files
live only on an isolated Bokarust comparison branch; do not merge them
into the product or label results as FrontRust benchmarks.

## Follow-up

Once FrontRust source and write permissions are available: port this
harness, add a real FrontRust variant, and create interactive demos
of the same three-object create/move/undo/redo/save/restore sequence.
Run controlled same-runner browser tests in repeated randomized order,
and measure JS/WASM bytes, startup, input-to-paint p95, RAM and CPU.

Sources:
- https://docs.rs/leptos/latest/leptos/
- https://docs.rs/dioxus-ssr/0.7.0/dioxus_ssr/
- https://app.notion.com/p/40332a84beec81fa968bef062af6e97a

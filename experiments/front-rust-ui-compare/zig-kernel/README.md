# Zig 0.17 against Rust 1.99: native HTML formatting kernels

Two implementations of the same C ABI, no dependencies on Leptos, Dioxus
or FrontRust. They append identical HTML bytes to a caller-supplied buffer,
with a hand-written decimal writer and no heap allocation in the kernel.

Proof: exact bytes for 3, 300 and 10000 rectangles; the second rectangle
matches the move/undo/redo fixture at (164,28); same C timing harness;
nine randomized alternating rounds and memory, compile and .so byte reports.

GitHub Action runs on this folder only; Zig tarball SHA256 is pinned to
the one used in Akula's previous successful Zig/Rust RGBA experiment.

Not a full app comparison: shared-library sizes include different runtimes,
no browser/SSR/WASM/real FrontRust, and no development effort benchmark.
Read: https://app.notion.com/p/40132a84beec8124a498c703c3e03572

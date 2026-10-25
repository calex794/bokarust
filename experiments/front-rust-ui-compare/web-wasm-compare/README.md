# Zig and Rust real browser WebAssembly bundle comparison

Four real WebAssembly artifacts from the same previously verified HTML
formatter: Rust small/fast and Zig small/fast. Both are built from the
successful native kernel sources in ../zig-kernel/ and expose the same
render_html/output_ptr/output_cap and linear memory ABI.

A single index.html and app.mjs are copied unchanged to all four bundles.
The UI has working buttons for scenes with 3 and 300 rectangles, updates
real DOM, validates the moved second object and checks SHA-256 of generated
HTML for 3, 300 and 10000 objects. Any byte mismatch fails the workflow.

The Node 22 browser driver starts a local HTTP server, visits each bundle
three times in alternating order in the same headless Chromium, disables
cache and measures timings. Package size is the sum of HTML, JS, WASM.
Record raw, gzip and brotli bytes for *all three files*. The browser itself
receives uncompressed files on localhost; compressed sizes are estimates
for production servers configured to support compression.

Important limitations:
- Only compiled HTML formatting differs. No full FrontRust framework or
  a competing Zig UI framework is implemented.
- Chrome button actions are synthetic. rAF measures next animation frame
  callback, not presentation on a physical monitor.
- Localhost load time is not a realistic Internet latency benchmark.
- WASM reserved linear memory is not operating system process RSS.
- WASM rustc and Zig build modes are analogous goals, not equal generated
  LLVM flags or identical runtime availability.
- Compare machines, further optimize Rust and Zig, and audit bundle APIs
  before making product choices.
- Experimental Bokarust branch: do not merge into application main.

Docs:
https://ziglang.org/documentation/0.17.0/
https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html
https://app.notion.com/p/40332a84beec8170a1dacc662c09925d

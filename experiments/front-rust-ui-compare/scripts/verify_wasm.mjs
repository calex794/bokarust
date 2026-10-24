#!/usr/bin/env node
// Actual Rust/WASM module ABI test in Node.js and headless Chrome.
// This is a prototype parity probe, NOT the existing FrontRust project.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { resolve, dirname } from 'node:path';

const wasmPath = process.argv[2];
const native = process.argv[3];
const outPath = process.argv[4];
if (!wasmPath || !native || !outPath) {
  throw new Error('Usage: verify_wasm.mjs <wasm-path> <native-binary> <results-dir>');
}
mkdirSync(outPath, { recursive: true });
const bytes = readFileSync(wasmPath);
if (!WebAssembly.validate(bytes)) throw new Error('Invalid wasm binary');
const start = performance.now();
const { instance } = await WebAssembly.instantiate(bytes, {});
const instantiateMs = performance.now() - start;
const f = instance.exports.front_rust_probe_checksum;
if (typeof f !== 'function') throw new Error('Missing Rust checksum export');

const tests = [3, 300, 10000].map(n => {
  const expected = Number(execFileSync(native, [String(n)], { encoding: 'utf8' }).trim()) >>> 0;
  const actual = f(n) >>> 0;
  if (expected !== actual || actual === 0) {
    throw new Error('Native and wasm32 differ for ' + n + ': ' + actual + ' vs ' + expected);
  }
  return { n, expected };
});
if ((f(2) >>> 0) !== 0 || (f(10001) >>> 0) !== 0) {
  throw new Error('WASM ABI accepted invalid scene sizes');
}
const metrics = {
  scope: 'Rust scene fixture checksum in actual wasm32, Node 22; not FrontRust or browser UI',
  byte_length: bytes.length,
  node_first_instantiate_ms: Number(instantiateMs.toFixed(3)),
  checksums: tests,
  node_version: process.version
};
writeFileSync(resolve(outPath, 'wasm-parity.json'), JSON.stringify(metrics, null, 2) + '\n');
console.log(JSON.stringify(metrics, null, 2));

// Standalone HTML with the compiled binary embedded, no network or JS framework.
// Synchronous DOM completion is reported with data-pass="1" for CI --dump-dom.
const base64 = bytes.toString('base64');
const html = '<!doctype html><html lang="en"><meta charset="utf-8"><title>WASM parity probe</title>' +
  '<body data-pass="pending"><main><h1>Rust scene state in WebAssembly</h1>' +
  '<output id="result" role="status">pending</output></main><script>' +
  'const encoded=' + JSON.stringify(base64) + ';' +
  'const tests=' + JSON.stringify(tests) + ';' +
  '(async()=>{try {' +
  'const raw=Uint8Array.from(atob(encoded), c=>c.charCodeAt(0));' +
  'const {instance}=await WebAssembly.instantiate(raw,{});' +
  'const f=instance.exports.front_rust_probe_checksum;' +
  'if(typeof f!=="function") throw Error("missing export");' +
  'for(const t of tests) if((f(t.n)>>>0)!==t.expected) throw Error("mismatch "+t.n);' +
  'if((f(2)>>>0)!==0 || (f(10001)>>>0)!==0) throw Error("invalid input");' +
  'document.body.setAttribute("data-pass","1");' +
  'document.getElementById("result").textContent="native/wasm/browser parity OK";' +
  '}catch(err){document.body.setAttribute("data-pass","0");' +
  'document.getElementById("result").textContent=String(err);}})();' +
  '</script></body></html>';
writeFileSync(resolve(outPath, 'wasm-probe.html'), html);

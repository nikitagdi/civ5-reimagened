// Loads the wasm_smoke example and prints its report; exit code 1 if a check failed.
//   cargo build -p lua-sys --example wasm_smoke --target wasm32-unknown-unknown
//   node tests/wasm_smoke.mjs <target-dir>/wasm32-unknown-unknown/debug/examples/wasm_smoke.wasm
import { readFile } from "node:fs/promises";

const bytes = await readFile(process.argv[2]);
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
const failed = x.smoke();
const text = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.report_ptr(), x.report_len()));
console.log(text);
console.log(failed === 0 ? "wasm smoke: all passed" : `wasm smoke: ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);

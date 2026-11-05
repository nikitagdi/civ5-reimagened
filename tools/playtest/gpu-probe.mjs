// Probe: which GPU a headless Chrome gets with the given switches (WebGL renderer, WebGPU adapter).
// node tools/playtest/gpu-probe.mjs [--channel chromium] [switch...]
import { chromium } from "playwright";

const argv = process.argv.slice(2);
let channel;
if (argv[0] === "--channel") {
  channel = argv[1];
  argv.splice(0, 2);
}
const browser = await chromium.launch({ headless: true, channel, args: ["--no-sandbox", ...argv] });
const page = await browser.newPage();
await page.goto("about:blank");
const info = await page.evaluate(async () => {
  const c = document.createElement("canvas");
  const gl = c.getContext("webgl2");
  const ext = gl?.getExtension("WEBGL_debug_renderer_info");
  const webgl = gl ? (ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)) : null;
  let webgpu = null;
  try {
    const a = await navigator.gpu?.requestAdapter();
    webgpu = a ? `${a.info?.vendor} ${a.info?.architecture} ${a.info?.description ?? ""}`.trim() : null;
  } catch (e) {
    webgpu = `error ${e}`;
  }
  return { webgl, webgpu };
});
console.log(JSON.stringify({ channel: channel ?? "default", args: argv, ...info }));
await browser.close();

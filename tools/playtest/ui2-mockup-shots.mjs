// Chrome's own rendering of a skin mockup page, as the reference of UI v2's text: the page at 1600 x 900 CSS px and
// a device scale, a screenshot of the whole page and of the top strip (the crop `ui2-remarks` takes of the live UI),
// the strip zoomed 4 x and its sharpness (`lib/sharpness.mjs`).
//   node tools/playtest/ui2-mockup-shots.mjs --page <mockup .html> --out <path prefix> [--dsf 1] [--wait-ms 1500]
// Prints the sharpness as JSON.
import { writeFileSync, readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "playwright";
import { sharpness, zoom } from "./lib/sharpness.mjs";
import { STRIP } from "./scenarios/ui2-remarks.mjs";

const args = process.argv.slice(2);
const opt = (k, d) => {
  const i = args.indexOf(k);
  return i >= 0 ? args[i + 1] : d;
};
const page = opt("--page");
const out = opt("--out");
const dsf = Number(opt("--dsf", "1"));
const waitMs = Number(opt("--wait-ms", "1500"));
if (!page || !out) {
  console.error("usage: ui2-mockup-shots.mjs --page <mockup .html> --out <path prefix> [--dsf 1]");
  process.exit(2);
}
const browser = await chromium.launch({ args: dsf === 1 ? [] : [`--force-device-scale-factor=${dsf}`] });
try {
  const context = await browser.newContext({ viewport: { width: 1600, height: 900 }, ...(dsf === 1 ? { deviceScaleFactor: 1 } : {}) });
  const p = await context.newPage();
  await p.goto(pathToFileURL(path.resolve(page)).href);
  await p.waitForTimeout(waitMs);
  const cdp = await context.newCDPSession(p);
  const capture = async (label, clip) => {
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
    const file = `${out}-${label}.png`;
    writeFileSync(file, Buffer.from(data, "base64"));
    return file;
  };
  await capture("hud");
  const strip = await capture("strip", STRIP);
  const png = readFileSync(strip);
  writeFileSync(`${out}-strip-x4.png`, zoom(png, null, 4));
  console.log(JSON.stringify({ dsf, dpr: await p.evaluate(() => devicePixelRatio), strip: sharpness(png) }));
} finally {
  await browser.close();
}

// Shared Chromium launcher: Playwright is resolved from tools/playtest (no second copy is installed
// here) and Chromium always starts with the pinned flags from chromium-flags.json.
import { createRequire } from "node:module";
import fs from "node:fs";

const HERE = new URL(".", import.meta.url);

/** The Playwright module of tools/playtest (relative to this file, never an absolute path). */
export function loadPlaywright() {
  const req = createRequire(new URL("../../playtest/package.json", HERE));
  return req("playwright");
}

/** Pinned Chromium flags (SPEC determinism table). */
export function chromiumFlags() {
  return JSON.parse(fs.readFileSync(new URL("chromium-flags.json", HERE), "utf8")).args;
}

/** Launches the Playwright-bundled Chromium (never a system Chrome) with the pinned flags. */
export async function launchChromium() {
  const { chromium } = loadPlaywright();
  return chromium.launch({ headless: true, args: chromiumFlags() });
}

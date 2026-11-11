// CLI of the scan (the library part lives in ir.mjs, so session.mjs can import it without a cycle).
//   node scan.mjs <skin_dir> [--entry index.html] [--state name] [--fonts-dir dir] [--pretty]
// Prints the IR as canonical JSON; exit code 1 when the IR carries errors (for example E9-001).
import { pathToFileURL } from "node:url";
import { canonicalJson } from "./canonical.mjs";

export * from "./ir.mjs";
async function cli(argv) {
  const opts = { entry: "index.html", state: "", fontsDir: null, pretty: false };
  const pos = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--entry") opts.entry = argv[++i];
    else if (a === "--state") opts.state = argv[++i];
    else if (a === "--fonts-dir") opts.fontsDir = argv[++i];
    else if (a === "--pretty") opts.pretty = true;
    else pos.push(a);
  }
  if (pos.length !== 1) {
    console.error("usage: node scan.mjs <skin_dir> [--entry index.html] [--state name] [--fonts-dir dir] [--pretty]");
    return 2;
  }
  const { createSession } = await import("./session.mjs");
  const session = createSession();
  try {
    await session.open({ skin_dir: pos[0], entry: opts.entry, fonts_dir: opts.fontsDir });
    const ir = await session.scan({ state: opts.state });
    process.stdout.write((opts.pretty ? JSON.stringify(JSON.parse(canonicalJson(ir)), null, 2) : canonicalJson(ir)) + "\n");
    for (const e of ir.errors) console.error(`error[${e.code}]: ${e.message}${e.url ? `: ${e.url}` : ""}`);
    return ir.errors.length ? 1 : 0;
  } catch (e) {
    console.error(`error[${e.code || "E9-002"}]: ${e.message}`);
    return 1;
  } finally {
    await session.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await cli(process.argv.slice(2));
}

// Markdown of a bench result: node tools/playtest/bench-report.mjs <bench.json> [out.md]
import fs from "node:fs";
import { reportMd } from "./lib/bench.mjs";

const [src, dst] = process.argv.slice(2);
if (!src) {
  console.error("usage: node bench-report.mjs <bench.json> [out.md]");
  process.exit(2);
}
const md = reportMd(JSON.parse(fs.readFileSync(src, "utf8")));
if (dst) fs.writeFileSync(dst, md);
else console.log(md);

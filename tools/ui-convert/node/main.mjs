// Node side of the converter: line-delimited JSON over stdin/stdout.
//
//   request   {"id": <any>, "cmd": "<name>", ...fields}        one JSON object per line
//   response  {"id": <same>, "ok": true,  "result": {...}}
//             {"id": <same>, "ok": false, "error": {"code": "E9-003", "message": "..."}}
//
// Requests are handled strictly one after another, in the order they arrive. Output is canonical
// JSON (sorted keys). stdout carries protocol lines only; diagnostics go to stderr.
//
//   open   {skin_dir, entry?, profile?, port?, fonts_dir?, preview?}
//                                                           -> {chrome, flags, base_url, preview}
//          preview: inject node/preview/preview.js into every page (answered false when the
//          script does not exist)
//   scan   {state?, viewport?}                              -> the IR (see scan.mjs)
//   reload {}                                               -> {}
//   close  {}                                               -> {}   (the process exits afterwards)
// extract, raster, rects and shoot are part of the planned protocol and answer E9-003 for now.
import readline from "node:readline";
import { canonicalJson } from "./canonical.mjs";
import { createSession, ProtocolError } from "./session.mjs";

const PLANNED = new Set(["extract", "raster", "rects", "shoot"]);
const session = createSession();
let closing = false;

function send(obj) {
  return new Promise((resolve) => process.stdout.write(canonicalJson(obj) + "\n", resolve));
}

async function handle(line) {
  if (line.trim() === "") return;
  let req;
  try {
    req = JSON.parse(line);
  } catch (e) {
    return send({ id: null, ok: false, error: { code: "E9-003", message: `invalid JSON: ${e.message}` } });
  }
  if (req === null || typeof req !== "object" || Array.isArray(req) || typeof req.cmd !== "string") {
    return send({ id: req && req.id !== undefined ? req.id : null, ok: false, error: { code: "E9-003", message: "a request is an object with a string field cmd" } });
  }
  const id = req.id === undefined ? null : req.id;
  try {
    let result;
    switch (req.cmd) {
      case "open": result = await session.open(req); break;
      case "scan": result = await session.scan(req); break;
      case "reload": result = await session.reload(); break;
      case "close": result = await session.close(); closing = true; break;
      default:
        throw new ProtocolError("E9-003", PLANNED.has(req.cmd) ? `command not implemented yet: ${req.cmd}` : `unknown command: ${req.cmd}`);
    }
    await send({ id, ok: true, result });
    if (closing) process.exit(0);
    return;
  } catch (e) {
    if (!(e instanceof ProtocolError)) console.error(e && e.stack ? e.stack : String(e));
    return send({ id, ok: false, error: { code: e instanceof ProtocolError ? e.code : "E9-002", message: String(e && e.message) } });
  }
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
let chain = Promise.resolve();
rl.on("line", (line) => {
  chain = chain.then(() => (closing ? undefined : handle(line)));
});
rl.on("close", () => {
  chain.then(() => session.close()).then(() => process.exit(0));
});

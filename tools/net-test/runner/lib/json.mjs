// JSON with 64-bit integers kept exact. View hashes are `u64`; a JavaScript number holds 53 bits, so two
// different hashes could read as the same number. An integer that does not fit comes back as a BigInt
// (Node reads the source text of a number in the reviver). Hashes are compared as BigInt, never as numbers.

/** `JSON.parse` that turns integers beyond 2^53 into BigInt. */
export function parse(text) {
  return JSON.parse(text, (_key, value, ctx) => {
    if (typeof value === "number" && Number.isInteger(value) && !Number.isSafeInteger(value) && /^-?\d+$/.test(ctx?.source ?? "")) {
      return BigInt(ctx.source);
    }
    return value;
  });
}

/** A replacer for `JSON.stringify`: BigInt as a decimal string. */
export function bigintReplacer(_key, value) {
  return typeof value === "bigint" ? value.toString() : value;
}

/** A hash (number, BigInt or decimal string) as a BigInt for comparison. */
export function asBig(x) {
  if (typeof x === "bigint") return x;
  if (typeof x === "string") return BigInt(x);
  if (typeof x === "number" && Number.isSafeInteger(x)) return BigInt(x);
  throw new Error(`not an exact integer: ${x}`);
}

/** A hash as 16 hex digits. */
export const hex = (x) => asBig(x).toString(16).padStart(16, "0");

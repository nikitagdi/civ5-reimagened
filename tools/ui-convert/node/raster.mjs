// Pictures rasterized by Chromium (the runtime has no image decoders): each item at device scale 1 and 2.
//
//   raster {items: [{svg1?, svg2?, src1?, src2?, w1?, h1?, w2?, h2?, natural?}]}
//   -> {items: [{x1: <base64 RGBA, w1*h1*4 bytes>, x2: <base64 RGBA>, w1, h1, w2, h2} | {error}]}
//
// An item is either SVG markup (`svg1`/`svg2`: the same picture with its size for scale 1 and 2) or a file of
// the skin (`src1`/`src2`: paths on the local server; `src2` is the `@2x` file when the skin has one, else the
// same file). The device sizes are given (`w1 h1 w2 h2`), or, with `natural: true` (a file only), taken from
// the file: scale 1 is the natural size of `src1`, scale 2 twice that. The picture is decoded by an <img> and
// drawn into a software 2D canvas of exactly the device size (whole device pixels, the picture's top left on
// the pixel grid) on a transparent background; `getImageData` gives straight (not premultiplied) RGBA, the
// converter premultiplies or keeps the alpha of a mask. Nothing is formatted here: the bytes go over as
// base64. A picture that does not decode (or has no natural size) answers `{error}` instead.
//
// The page lives on the local server's origin, so a skin file drawn into the canvas does not taint it.
import { ProtocolError } from "./session.mjs";

function check(req) {
  const need = (c, m) => { if (!c) throw new ProtocolError("E9-003", `raster: ${m}`); };
  need(Array.isArray(req.items), "items is a list");
  for (const it of req.items) {
    need(it && typeof it === "object", "an item is an object");
    const svg = typeof it.svg1 === "string" && typeof it.svg2 === "string";
    const src = typeof it.src1 === "string" && typeof it.src2 === "string";
    need(svg !== src, "an item has svg1/svg2 or src1/src2");
    if (it.natural === true) {
      need(src, "a natural-size item is a file (src1/src2)");
      for (const k of ["w1", "h1", "w2", "h2"]) need(it[k] === undefined, `a natural-size item has no ${k}`);
    } else {
      need(it.natural === undefined, "natural is true or absent");
      for (const k of ["w1", "h1", "w2", "h2"]) need(Number.isInteger(it[k]) && it[k] > 0 && it[k] <= 4096, `${k} is a size in device pixels`);
    }
  }
}

/** Runs inside the page. */
async function drawAll(items) {
  const b64 = (bytes) => {
    let s = "";
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const load = async (url) => {
    const img = new Image();
    img.src = url;
    await img.decode();
    return img;
  };
  const svgUrl = (markup) => "data:image/svg+xml;charset=utf-8," + encodeURIComponent(markup);
  const out = [];
  for (const it of items) {
    const res = {};
    try {
      let nw = 0;
      let nh = 0;
      for (const k of [1, 2]) {
        const img = await load(it.svg1 !== undefined ? svgUrl(it[`svg${k}`]) : it[`src${k}`]);
        if (it.natural && k === 1) {
          nw = img.naturalWidth;
          nh = img.naturalHeight;
          if (!(nw > 0 && nh > 0)) throw new Error("the picture has no natural size");
          if (nw * 2 > 4096 || nh * 2 > 4096) throw new Error(`the natural size ${nw} x ${nh} is too large`);
        }
        const w = it.natural ? nw * k : it[`w${k}`];
        const h = it.natural ? nh * k : it[`h${k}`];
        const c = document.createElement("canvas");
        c.width = w;
        c.height = h;
        const g = c.getContext("2d", { willReadFrequently: true, alpha: true });
        g.clearRect(0, 0, w, h);
        g.imageSmoothingEnabled = true;
        g.imageSmoothingQuality = "high";
        g.drawImage(img, 0, 0, w, h);
        res[`x${k}`] = b64(new Uint8Array(g.getImageData(0, 0, w, h).data.buffer));
        res[`w${k}`] = w;
        res[`h${k}`] = h;
      }
      out.push(res);
    } catch (e) {
      out.push({ error: String((e && e.message) || "the picture does not decode").split("\n")[0] });
    }
  }
  return out;
}

/** Answers `raster` in a page of its own on the server's origin. */
export async function rasterItems({ context, server, req }) {
  check(req);
  const page = await context.newPage();
  try {
    await page.goto(`${server.url}/__ui2/raster`, { waitUntil: "load" });
    const url = (p) => p && `${server.url}/${p.split("/").map(encodeURIComponent).join("/")}`;
    const items = req.items.map((it) => ({ ...it, src1: url(it.src1), src2: url(it.src2) }));
    for (const it of items) {
      if (it.src1 === undefined) { delete it.src1; delete it.src2; }
    }
    return { items: await page.evaluate(drawAll, items) };
  } catch (e) {
    if (e instanceof ProtocolError) throw e;
    throw new ProtocolError("E9-003", `raster: ${String(e && e.message).split("\n")[0]}`);
  } finally {
    await page.close();
  }
}

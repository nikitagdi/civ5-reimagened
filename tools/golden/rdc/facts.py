"""Facts of the original renderer read out of the parsed captures (`golden parse`), for the other streams.

    python -I facts.py [--cache DIR] [--out DIR]

Reads `<cache>/golden/<scene>/<capture>/` (S1a, S2, ...), the constant maps and listings of
`shader-re dx11 cbmap` / `dx11 asm` (`<cache>/shader-re/*/dx11/`) and, for the LUT table, the colour key files
of the game install (read-only). Writes `<cache>/golden/facts.json` (schema civ5-golden-facts/1: every fact has
`value`, `sources` = capture + event + raw value, and a `status`) and `facts.md` (the human summary).
Stdlib only. Everything written is derived from the user's captures: it stays in `cache/`, never in git.
"""

import collections
import glob
import json
import math
import os
import struct
import sys

SCHEMA = "civ5-golden-facts/1"
# The game folder (with Assets): --game <dir>, else the CIV5_ROOT environment variable.
GAME = os.environ.get("CIV5_ROOT", "")
MAIN = ("S1a", "S2")


def _scene(capture):
    return capture[:2]


# ---------------------------------------------------------------- loading


class Cap(object):
    """One parsed capture with its draws joined to the entries, shader layouts and constants by name."""

    def __init__(self, cache, name, cbmaps):
        self.name = name
        self.dir = os.path.join(cache, "golden", _scene(name), name)
        self.cbmaps = cbmaps
        with open(os.path.join(self.dir, "draws.json"), encoding="utf-8") as f:
            self.draws = json.load(f)
        with open(os.path.join(self.dir, "events.json"), encoding="utf-8") as f:
            ev = json.load(f)
        by = {d["event"]: d for d in self.draws["draws"]}
        self.calls = ev["calls"]
        self.passes = ev["passes"]
        self.by_eid = {}
        for c in self.calls:
            for k, v in by[c["eid"]].items():
                if k.endswith("_entry"):
                    c[k] = v
            self.by_eid[c["eid"]] = c
        p = os.path.join(self.dir, "textures.json")
        self.textures = json.load(open(p, encoding="utf-8")) if os.path.isfile(p) else []

    def named(self, call, stage, name):
        """(floats, cbmap entry) of the constant `name` of the call's `stage`, through the cbmap."""
        ent = call.get(stage + "_entry")
        for k in self.cbmaps.get(ent, []):
            if k["name"] != name:
                continue
            for cb in call["cbuffers"].get(stage, []):
                if cb["slot"] == k["cb"]:
                    o = k["offset"] // 4
                    return cb["floats"][o:o + k["rows"] * 4], k
        return None

    def all_named(self, name, stages=("vs", "ps", "hs", "ds")):
        for c in self.calls:
            for s in stages:
                r = self.named(c, s, name)
                if r:
                    yield c, s, r

    def target_path(self, name):
        return os.path.join(self.dir, "targets", name + ".dds")


def load_cbmaps(cache):
    for d in glob.glob(os.path.join(cache, "shader-re", "*", "dx11", "cbmap")):
        out = {}
        for f in glob.glob(os.path.join(d, "*.json")):
            with open(f, encoding="utf-8") as fh:
                j = json.load(fh)
            out[j["entry"]] = j["consts"]
        return out, os.path.dirname(d)
    return {}, None


def read_asm(dx11_dir, sha):
    if not dx11_dir:
        return None
    p = os.path.join(dx11_dir, "asm", sha + ".txt")
    if not os.path.isfile(p):
        return None
    with open(p, encoding="utf-8", errors="replace") as f:
        return [l.rstrip() for l in f if l.strip() and not l.lstrip().startswith("//")]


# ---------------------------------------------------------------- DDS (our own writer's output)


def read_dds(path):
    """-> dict(format, width, height, data) of a one-subresource DX10 DDS, or None."""
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as f:
        b = f.read()
    h, w = struct.unpack_from("<II", b, 12)
    fmt = struct.unpack_from("<I", b, 128)[0]
    return {"dxgi": fmt, "width": w, "height": h, "data": b[148:]}


def half(h):
    s = -1.0 if h & 0x8000 else 1.0
    e = (h >> 10) & 0x1F
    m = h & 0x3FF
    if e == 0:
        return s * m * 2.0 ** -24
    if e == 31:
        return s * float("inf") if m == 0 else float("nan")
    return s * (1 + m / 1024.0) * 2.0 ** (e - 15)


def channel_stats(img, step=1):
    """Per channel min, max, mean and the number of values > 0 of an RGBA8 / R16_UNORM / R16F-RGBA image."""
    d = img["data"]
    f = img["dxgi"]
    if f in (27, 28, 29):
        n = len(d) // 4
        chans = [d[c::4][::step] for c in range(4)]
        scale = 255.0
        vals = [list(c) for c in chans]
    elif f in (53, 56):
        vals = [list(struct.unpack("<%dH" % (len(d) // 2), d))[::step]]
        scale = 65535.0
    elif f == 10:
        n = len(d) // 8
        raw = struct.unpack("<%dH" % (n * 4), d[: n * 8])
        vals = [[half(x) for x in raw[c::4]][::step] for c in range(4)]
        scale = 1.0
    else:
        return None
    out = []
    for v in vals:
        v = [x for x in v if x == x]
        out.append({"min": min(v) / scale, "max": max(v) / scale, "mean": sum(v) / len(v) / scale,
                    "nonzero": sum(1 for x in v if x > 0), "count": len(v)})
    return out


# ---------------------------------------------------------------- facts


def src(cap, eid, raw, **extra):
    d = {"capture": cap, "event": eid, "raw": raw}
    d.update(extra)
    return d


def distinct_named(caps, name, limit=6):
    """Every distinct value of constant `name` per capture, with the first event and entry of each."""
    out = []
    values = {}
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        seen = collections.OrderedDict()
        for c, s, (v, k) in cap.all_named(name):
            key = tuple(v)
            e = seen.setdefault(key, {"event": c["eid"], "entry": c[s + "_entry"], "cb": k["cb"], "offset": k["offset"],
                                      "rows": k["rows"], "draws": 0})
            e["draws"] += 1
        values[cn] = len(seen)
        for key, e in list(seen.items())[:limit]:
            out.append(src(cn, e["event"], list(key), entry=e["entry"], cb=e["cb"], byte_offset=e["offset"],
                           rows=e["rows"], draws=e["draws"]))
    return {"status": "read" if out else "not found", "name": name, "distinct_values": values, "sources": out}


def fact_view_proj(caps, cache):
    names = ("m_mViewProj", "ViewProjMatrix", "g_mViewProj", "ViewProj", "g_ViewProjMatrix", "g_VP", "mtxViewProj",
             "mtxCompositeVP", "WVP", "g_WVP", "FOWBaseWVP")
    out = []
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        seen = set()
        for c in cap.calls:
            if not c["targets"] or c["targets"][0]["width"] != cap.draws["width"] or c["targets"][0].get("samples") != 4:
                continue
            ent = c.get("vs_entry")
            if not ent or ent in seen:
                continue
            for k in cap.cbmaps.get(ent, []):
                if k["name"] in names and k["rows"] == 4:
                    r = cap.named(c, "vs", k["name"])
                    if r:
                        seen.add(ent)
                        out.append(src(cn, c["eid"], r[0], entry=ent, name=k["name"], cb=k["cb"], byte_offset=k["offset"]))
    return {"status": "read" if out else "not found", "sources": out,
            "note": "main-pass draws only (MSAA frame target); the matrix is four registers = four rows"}


def terrain_calls(cap):
    return [c for c in cap.calls if (c.get("ps_entry") or "").startswith("TerrainShader::TerrainRenderCached")]


def fact_terrain_scale(caps, dx11_dir):
    out = []
    asm_ok = None
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        tc = terrain_calls(cap)
        if not tc:
            continue
        c = tc[0]
        vs = next((cb for cb in c["cbuffers"].get("vs", []) if cb["slot"] == 1), None)
        cb0 = next((cb for cb in c["cbuffers"].get("vs", []) if cb["slot"] == 0), None)
        if vs is None:
            continue
        f = vs["floats"]
        out.append(src(cn, c["eid"], f[64:68], what="cb1[16] of the terrain VS (TerrainShaderScreenVS40)", xyz=f[64:67],
                       cb1_22=f[88:90], cb0_6=cb0["floats"][24:27] if cb0 else None,
                       distinct_over_frame=len({tuple(next(cb for cb in x["cbuffers"]["vs"] if cb["slot"] == 1)["floats"][64:67])
                                                for x in tc})))
        if asm_ok is None:
            asm = read_asm(dx11_dir, c["vs"])
            if asm:
                asm_ok = [l for l in asm if "cb1[16]" in l or "cb1[22]" in l or "sample_l" in l or "l(255" in l]
    return {"status": "read" if out else "not found", "sources": out, "vs_asm": asm_ok,
            "note": "the height is sample(A8_UNORM texture t0 at uv * cb1[22].xy, lod cb0[6].z) * cb1[16].z * 255; "
                    "cb1[16].xyz is g_TerrainScale (x = 1, y = hex row ratio, z = TERRAIN_SCALE_Z)"}


def fact_world_to_tex(caps):
    r = distinct_named(caps, "g_WorldToTerrainTextureSpace")
    for s in r["sources"]:
        f = s["raw"]
        s["world_per_texture_u"] = 1.0 / f[0] if f[0] else None
        s["world_per_texture_v"] = 1.0 / f[5] if f[5] else None
    return r


def fact_fow(caps, dx11_dir):
    """Which FOW pixel shader writes which channel of FowMask, and where the mask is read."""
    res = {"status": "read", "writers": [], "readers": [], "mask_target": None}
    cap = caps.get("S1a") or next(iter(caps.values()))
    for ent in ("FOW::Visible:PS", "FOW::VisibleFluid:PS", "FOW::BasePS:PS", "FOW::TexturePS:PS"):
        eids = cap.draws["by_entry"].get(ent)
        if not eids:
            continue
        c = cap.by_eid[eids[0]]
        asm = read_asm(dx11_dir, c["ps"])
        blend = (c["blend"]["blends"] or [{}])[0]
        t0 = c["targets"][0] if c["targets"] else {}
        res["writers"].append(src(cap.name, c["eid"], [l.strip() for l in (asm or []) if "o0" in l or "sample" in l or "discard" in l],
                                  entry=ent, target=t0.get("resource"), target_format=t0.get("format"),
                                  target_size=[t0.get("width"), t0.get("height")],
                                  blend={"color": blend.get("colorBlend"), "write_mask": blend.get("writeMask")},
                                  draws=len(eids)))
    # the mask target: the target of FOW::Visible
    vis = cap.draws["by_entry"].get("FOW::Visible:PS")
    if vis:
        res["mask_target"] = cap.by_eid[vis[0]]["targets"][0]["resource"]
        for ent in ("TerrainShader::TerrainRenderCachedDiffuseAndSpecPS:PS", "FOW::BasePS:PS"):
            eids = cap.draws["by_entry"].get(ent)
            if not eids:
                continue
            c = cap.by_eid[eids[0]]
            slots = [t["slot"] for t in c["textures"].get("ps", []) if t["resource"] == res["mask_target"]]
            asm = read_asm(dx11_dir, c["ps"]) or []
            uses = []
            for s in slots:
                uses += asm_context(asm, "t%d." % s, 0, 3 if "Terrain" in ent else 1)
            res["readers"].append(src(cap.name, c["eid"], uses, entry=ent, slots=slots))
    return res


def asm_context(asm, pattern, before=0, after=0):
    """Listing lines around the first line that contains `pattern`."""
    for i, l in enumerate(asm or []):
        if pattern in l:
            return [x.strip() for x in asm[max(0, i - before): i + after + 1]]
    return []


def fact_shadow(caps, dx11_dir):
    out = {"status": "read", "sources": []}
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        eids = cap.draws["by_entry"].get("Shadow::BlurAndProcess:PS")
        if not eids:
            continue
        c = cap.by_eid[eids[0]]
        item = src(cn, c["eid"], None, target=c["targets"][0] if c["targets"] else None,
                   inputs=[{k: t.get(k) for k in ("slot", "resource", "format", "view_format", "width", "height", "mips")}
                           for t in c["textures"].get("ps", [])],
                   cbuffer_floats=[cb["floats"][:16] for cb in c["cbuffers"].get("ps", [])])
        stats = {}
        for nm in ("%d-before" % c["eid"], "%d-after" % c["eid"]):
            im = read_dds(cap.target_path(nm))
            if im:
                stats[nm] = channel_stats(im, 7)
        for t in c["textures"].get("ps", []):
            im = read_dds(cap.target_path("%d-in-ps%d" % (c["eid"], t["slot"])))
            if im:
                stats["in-ps%d" % t["slot"]] = channel_stats(im, 7)
        item["pixel_stats"] = stats
        out["sources"].append(item)
        # the casters write what, and the reader of the map
        casters = [x for x in cap.calls if "Shadow" in (x.get("vs_entry") or "") or "_Shadow" in (x.get("ps_entry") or "")]
        if casters:
            x = casters[0]
            out.setdefault("caster", []).append(src(cn, x["eid"], None, entry=[x.get("vs_entry"), x.get("ps_entry")],
                                                    targets=[{k: t.get(k) for k in ("resource", "format", "width", "height", "samples")} for t in x["targets"]],
                                                    depth=x.get("depth_target") and x["depth_target"].get("format"),
                                                    ps_asm=[l.strip() for l in (read_asm(dx11_dir, x["ps"]) or []) if "o0" in l or "ld_" in l]))
        blur = read_asm(dx11_dir, c["ps"])
        out.setdefault("blur_asm", []).append(src(cn, c["eid"], [l.strip() for l in (blur or []) if "o0" in l or "sample" in l][:4] + ["... 8 taps, then:"] +
                                                  [l.strip() for l in (blur or []) if l.lstrip().startswith("mul o0")]))
        tc = terrain_calls(cap)
        if tc:
            t3 = [t for t in tc[0]["textures"].get("ps", []) if t["resource"] == c["targets"][0]["resource"]]
            if t3:
                a = read_asm(dx11_dir, tc[0]["ps"])
                out.setdefault("reader_asm", []).append(src(cn, tc[0]["eid"], asm_context(a, "t%d.xyzw, s%d" % (t3[0]["slot"], t3[0]["slot"]), 2, 3) + asm_context(a, "texture3d)(float,float,float,float) r1.xyz, r0.yzwy", 0, 2),
                                                           entry=tc[0]["ps_entry"], slot=t3[0]["slot"]))
    return out


def fact_page_format(caps):
    out = {"status": "read", "sources": []}
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        tc = terrain_calls(cap)
        if not tc:
            continue
        c = tc[0]
        out["sources"].append(src(cn, c["eid"], [{k: t.get(k) for k in ("slot", "resource", "format", "view_format", "width", "height", "mips", "dimension")}
                                                 for t in c["textures"].get("ps", [])], what="textures bound to TerrainRenderCached*PS"))
        hist = collections.Counter(t["format"] for t in cap.textures)
        b565 = [t for t in cap.textures if "B5G6R5" in t["format"] or "R5G6B5" in t["format"]]
        out["sources"].append(src(cn, None, dict(hist.most_common()), what="format histogram of all %d textures of the capture" % len(cap.textures),
                                  b5g6r5_textures=len(b565)))
        rts = collections.Counter((t["format"], t["width"], t["height"], t["mips"]) for t in cap.textures
                                  if "ColorTarget" in t["creation_flags"] and t["samples"] == 1 and t["format"] != "R8G8B8A8_TYPELESS")
        out["sources"].append(src(cn, None, [[list(k), v] for k, v in sorted(rts.items(), key=lambda kv: -kv[0][1])],
                                  what="single-sample colour targets other than R8G8B8A8 by (format, width, height, mips): count"))
        rts8 = collections.Counter((t["width"], t["height"], t["mips"]) for t in cap.textures
                                   if "ColorTarget" in t["creation_flags"] and t["samples"] == 1 and t["format"] == "R8G8B8A8_TYPELESS")
        out["sources"].append(src(cn, None, [[list(k), v] for k, v in sorted(rts8.items(), key=lambda kv: -kv[0][0])],
                                  what="single-sample R8G8B8A8 colour targets by (width, height, mips): count"))
    return out


def fact_building_cubes(caps):
    out = {"status": "read", "sources": []}
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        seen = {}
        for c in cap.calls:
            if not (c.get("ps_entry") or "").startswith("BuildingShader::BuildingPS"):
                continue
            for t in c["textures"].get("ps", []):
                if t.get("dimension") == "TextureCube" or t.get("array_size") == 6:
                    e = seen.setdefault((t["slot"], t["resource"]), {"event": c["eid"], "entry": c["ps_entry"], "t": t, "draws": 0})
                    e["draws"] += 1
        for (slot, res), e in sorted(seen.items()):
            t = e["t"]
            out["sources"].append(src(cn, e["event"], {k: t.get(k) for k in ("slot", "resource", "format", "view_format", "width", "height", "mips", "array_size", "dimension")},
                                      entry=e["entry"], draws=e["draws"]))
    out["status"] = "read" if out["sources"] else "not found"
    return out


def _filter_name(s):
    f = s.get("filter") or {}
    return "%s/%s/%s%s" % (f.get("minify"), f.get("magnify"), f.get("mip"), "" if f.get("filter") in (None, "Normal") else "/" + str(f.get("filter")))


def fact_samplers(caps):
    """Sampler table of the world draws of the main pass: per (entry, stage, slot) the distinct states."""
    out = {"status": "read", "table": {}}
    for cn in MAIN:
        cap = caps.get(cn)
        if not cap:
            continue
        rows = collections.OrderedDict()
        for c in cap.calls:
            if not c["world"] or not c["targets"] or c["targets"][0]["width"] != cap.draws["width"]:
                continue
            for stage in ("vs", "ds", "ps"):
                ent = c.get(stage + "_entry") or "(%s: %s)" % (stage, (c.get(stage) or "")[:8])
                for s in c["samplers"].get(stage, []):
                    st = "%s | U=%s V=%s W=%s | aniso=%s | cmp=%s | bias=%s | lod=%s..%s" % (
                        _filter_name(s), s.get("addressU"), s.get("addressV"), s.get("addressW"), s.get("maxAnisotropy"),
                        s.get("compareFunction"), s.get("mipBias"), s.get("minLOD"), s.get("maxLOD"))
                    key = "%s:%s:s%d" % (ent, stage, s["slot"])
                    e = rows.setdefault(key, {"states": collections.OrderedDict(), "first_event": c["eid"]})
                    e["states"][st] = e["states"].get(st, 0) + 1
        out["table"][cn] = [{"key": k, "first_event": v["first_event"], "states": dict(v["states"])} for k, v in rows.items()]
    return out


# ---------------------------------------------------------------- LUT (colour keys)


def read_tga(path):
    """Uncompressed 24/32 bit TGA -> dict(width, height, bpp, n, desc, data: raw file-order bytes)."""
    with open(path, "rb") as f:
        b = f.read()
    idlen, cmap, itype = b[0], b[1], b[2]
    w, h = struct.unpack_from("<HH", b, 12)
    bpp, desc = b[16], b[17]
    if itype != 2 or cmap:
        return None
    n = bpp // 8
    return {"width": w, "height": h, "bpp": bpp, "n": n, "desc": desc,
            "data": b[18 + idlen: 18 + idlen + w * h * n]}


def lut_variants(tga):
    """Candidate 32x32x32 RGBA8 volumes a colour key TGA can stand for. The key is a packed table (1600x21,
    1280x26, 3840x9 pixels, more than 32^3 = 32768): the first 32768 pixels in raster order are the volume
    (x fastest, then y, then z); the pixels after it are padding. Rows top-down or in file order; RGB or BGR."""
    n, d = tga["n"], tga["data"]
    w, h = tga["width"], tga["height"]
    out = {}
    for rows_name in ("file_order", "top_down"):
        if rows_name == "file_order" or tga["desc"] & 0x20:
            flat = d
        else:
            flat = b"".join(d[(h - 1 - y) * w * n:(h - y) * w * n] for y in range(h))
        for ch_name, idx in (("bgra", (0, 1, 2)), ("rgba", (2, 1, 0))):
            vol = bytearray()
            for i in range(32 * 32 * 32):
                p = flat[i * n:(i + 1) * n]
                vol += bytes((p[idx[0]], p[idx[1]], p[idx[2]], p[3] if n > 3 else 255))
            out["%s/%s" % (rows_name, ch_name)] = bytes(vol)
    return out

def fact_luts(caps, cache):
    out = {"status": "read", "bound": [], "matches": []}
    cap = caps.get("S1a")
    if not cap:
        return {"status": "not found"}
    c = next((x for x in terrain_calls(cap)), None)
    if c is None:
        return {"status": "not found"}
    kdir = os.path.join(GAME, "Assets", "colorkeys")
    tgas = [f for f in sorted(os.listdir(kdir)) if f.lower().endswith(".tga")] if os.path.isdir(kdir) else []
    # every 3D texture the world draws bind, with the entries and slots
    bound = collections.OrderedDict()
    for x in cap.calls:
        if not x["world"]:
            continue
        for stage, texs in x["textures"].items():
            for t in texs:
                if t.get("dimension") == "Texture3D" or (t.get("depth") or 1) > 1:
                    e = bound.setdefault(t["resource"], {"t": t, "uses": collections.OrderedDict(), "event": x["eid"]})
                    e["uses"]["%s:%s:t%d" % (x.get(stage + "_entry"), stage, t["slot"])] = x["eid"]
    for rid, e in bound.items():
        t = e["t"]
        item = {"resource": rid, "format": t.get("format"), "size": [t.get("width"), t.get("height")],
                "uses": [{"entry_stage_slot": k, "first_event": v} for k, v in e["uses"].items()]}
        out["bound"].append(src(cap.name, e["event"], item))
    out["tga_files"] = tgas
    out["note"] = "matching of the 3D textures to colorkeys/*.tga is by content: see `matches`"
    return out


def match_luts(caps, out, cache):
    """Fills out['matches'] by reading the volumes (the data is in textures/<resource>.dds of S1a)."""
    cap = caps.get("S1a")
    kdir = os.path.join(GAME, "Assets", "colorkeys")
    if not cap or not os.path.isdir(kdir):
        return
    tgas = {}
    for f in sorted(os.listdir(kdir)):
        if f.lower().endswith(".tga"):
            t = read_tga(os.path.join(kdir, f))
            if t:
                tgas[f] = lut_variants(t)
    for b in out.get("bound", []):
        rid = b["raw"]["resource"]
        im = read_dds(os.path.join(cap.dir, "textures", rid + ".dds"))
        if not im:
            continue
        vol = im["data"][: 32 * 32 * 32 * 4]
        exact, best = [], []
        for f, vs in tgas.items():
            for k, v in vs.items():
                if len(v) != len(vol):
                    continue
                if v == vol:
                    exact.append({"file": f, "layout": k})
                else:
                    d = sum(abs(a - b2) for a, b2 in zip(vol[::17], v[::17])) / float(len(vol[::17]))
                    best.append((d, f, k))
        best.sort()
        out["matches"].append({"resource": rid, "exact": exact, "layout": exact[0]["layout"] if exact else None,
                               "nearest": [{"file": f, "layout": k, "mean_abs_diff": d} for d, f, k in best[:2]] if not exact else []})

# ---------------------------------------------------------------- facts.md


def g(x):
    """A float or a list of floats for the summary: 7 significant digits, junk (< 1e-9) as 0."""
    if isinstance(x, list):
        return "[" + ", ".join(g(v) for v in x) + "]"
    if isinstance(x, float):
        return "0" if abs(x) < 1e-9 else ("%.7g" % x)
    return str(x)


def write_md(doc, path):
    F = doc["facts"]
    L = []
    w = L.append
    w("# Facts of the original renderer (from the user's RenderDoc captures)\n")
    w("Written by `tools/golden/rdc/facts.py` from `cache/golden/<scene>/<capture>/`; machine form: `facts.json`. "
      "Captures read: %s. Each value is `capture event: raw`. A value with `~0` is junk of an unwritten part of a "
      "constant buffer (below 1e-9).\n" % ", ".join(doc["captures"]))

    def named(title, key, note=""):
        f = F.get(key, {})
        w("## %s\n" % title)
        if note:
            w(note + "\n")
        for s in f.get("sources", []):
            w("- %s ev %s `%s` cb%s+%s: %s (%s draws)" % (s["capture"], s["event"], s.get("entry"), s.get("cb"),
                                                         s.get("byte_offset"), g(s["raw"]), s.get("draws")))
        if not f.get("sources"):
            w("- not found in the draws")
        w("")

    named("g_FogConsts", "g_FogConsts", "x = -0.05 in both; the w differs between S1a (0.3426898) and S2 (0): per capture, not a constant of the game.")
    named("g_FogColor", "g_FogColor")
    named("f3FogColor (the same colour under its other name)", "f3FogColor")
    named("g_ShadowVP (4 registers = 4 rows)", "g_ShadowVP")
    named("g_ShadowFade", "g_ShadowFade", "Only x is meaningful (a float); the rest of the register is unwritten.")
    f = F["view_proj"]
    w("## View-projection constant (CTAB names)\n")
    w("The name depends on the package; four registers = four rows. The terrain draws (DX11-only tessellation shaders) have no name: "
      "the matrix is cb1[0..3] of their shared 592-byte buffer.\n")
    seen = set()
    for s in f["sources"]:
        k = (s["capture"], s["entry"])
        if k in seen:
            continue
        seen.add(k)
        w("- %s ev %s `%s`: `%s` cb%s+%s" % (s["capture"], s["event"], s["entry"], s["name"], s["cb"], s["byte_offset"]))
    if f["sources"]:
        s0 = f["sources"][0]
        w("- matrix (%s ev %s): %s" % (s0["capture"], s0["event"], g(s0["raw"])))
    w("")
    f = F["g_TerrainScale"]
    w("## TERRAIN_SCALE_Z / g_TerrainScale\n")
    for s in f["sources"]:
        w("- %s ev %s: cb1[16] of the terrain VS = %s (x, y, z, junk); cb1[22].xy = %s; cb0[6].xyz = %s; distinct over the frame: %s" % (
            s["capture"], s["event"], g(s["raw"]), g(s["cb1_22"]), g(s["cb0_6"]), s["distinct_over_frame"]))
    w("- " + f.get("note", ""))
    for l in f.get("vs_asm") or []:
        w("    `%s`" % l)
    w("")
    f = F["g_WorldToTerrainTextureSpace"]
    w("## World to terrain texture (related to g_HexToTex)\n")
    for s in f["sources"]:
        w("- %s ev %s `%s` cb%s+%s: %s; world units per texture: u %s, v %s" % (
            s["capture"], s["event"], s["entry"], s["cb"], s["byte_offset"], g(s["raw"]), g(s["world_per_texture_u"]), g(s["world_per_texture_v"])))
    w("")
    f = F["shadow"]
    w("## Shadow target and its sense\n")
    for s in f["sources"]:
        t = s["target"]
        w("- %s ev %s BlurAndProcess: target %s %sx%s %s; input slot 0 = %s" % (
            s["capture"], s["event"], t["resource"], t["width"], t["height"], t["format"], json.dumps(s["inputs"])))
        for k, v in s["pixel_stats"].items():
            w("    - `%s`: %s" % (k, "; ".join("ch%d min %s max %s mean %s nz %s/%s" % (i, g(c["min"]), g(c["max"]), g(c["mean"]), c["nonzero"], c["count"]) for i, c in enumerate(v))))
    for s in f.get("caster", []):
        w("- %s ev %s caster `%s` -> target %s, depth %s; PS: %s" % (s["capture"], s["event"], s["entry"], json.dumps(s["targets"]), s["depth"], " / ".join(s["ps_asm"])))
    for s in f.get("blur_asm", []):
        w("- %s ev %s BlurAndProcess PS: %s" % (s["capture"], s["event"], " / ".join(s["raw"])))
    for s in f.get("reader_asm", []):
        w("- %s ev %s reader `%s` (t%s):" % (s["capture"], s["event"], s["entry"], s["slot"]))
        for l in s["raw"]:
            w("    `%s`" % l)
    w("")
    f = F["fow_mask"]
    w("## FowMask channel contract\n")
    for s in f["writers"]:
        w("- %s ev %s `%s` -> target %s %s %sx%s, blend %s write mask %s: `%s`" % (
            s["capture"], s["event"], s["entry"], s["target"], s["target_format"], s["target_size"][0], s["target_size"][1],
            s["blend"]["color"], s["blend"]["write_mask"], " / ".join(s["raw"])))
    for s in f["readers"]:
        w("- reader %s ev %s `%s` slots %s:" % (s["capture"], s["event"], s["entry"], s["slots"]))
        for l in s["raw"]:
            w("    `%s`" % l)
    w("")
    f = F["terrain_page"]
    w("## Terrain page format, g_iMaxTileSize\n")
    for s in f["sources"]:
        r = s["raw"]
        if isinstance(r, list) and r and isinstance(r[0], dict):
            w("- %s ev %s textures of the terrain PS: %s" % (s["capture"], s["event"], "; ".join(
                "t%d %s %s %dx%d mips %d %s" % (t["slot"], t["resource"], t["view_format"], t["width"], t["height"], t["mips"], t["dimension"]) for t in r)))
        elif isinstance(r, dict):
            w("- %s: %s; B5G6R5 textures: %s" % (s["capture"], ", ".join("%s %d" % kv for kv in r.items()), s.get("b5g6r5_textures")))
        else:
            w("- %s: %s: %s" % (s["capture"], s.get("what"), "; ".join("%s x%d" % (tuple(k), v) for k, v in r)))
    w("")
    f = F["building_cubes"]
    w("## Cubes of the building shader (RW-4 d)\n")
    for s in f["sources"]:
        w("- %s ev %s `%s`: %s (%s draws)" % (s["capture"], s["event"], s["entry"], json.dumps(s["raw"]), s["draws"]))
    w("")
    f = F["samplers"]
    w("## Sampler table of the main-pass world draws\n")
    for cn, rows in f["table"].items():
        w("### %s\n" % cn)
        common = collections.Counter()
        for r in rows:
            for k in r["states"]:
                common[k.split(" | ", 1)[0] + " | " + " | ".join(k.split(" | ")[2:])] += 1
        w("Shared by every sampler: %s\n" % "; ".join("%s (%d)" % kv for kv in common.most_common(3)))
        groups = collections.OrderedDict()
        for r in rows:
            ent, stage, slot = r["key"].rsplit(":", 2)
            states = []
            for k, v in r["states"].items():
                parts = k.split(" | ")
                states.append("U=%s,V=%s,W=%s" % tuple(x.split("=")[1] for x in parts[1].split()))
            groups.setdefault("%s:%s" % (ent, stage), []).append("%s %s" % (slot, " / ".join(states)))
        for k, v in groups.items():
            w("- `%s` (first ev %s): %s" % (k, rows[[r["key"] for r in rows].index(k + ":" + v[0].split()[0])]["first_event"], "; ".join(v)))
        w("")
    f = F["luts"]
    w("## 3D LUTs (colour keys)\n")
    for s in f.get("bound", []):
        r = s["raw"]
        w("- %s: resource %s %s %s; used by %s" % (s["capture"], r["resource"], r["format"], r["size"],
                                                  ", ".join("%s (ev %s)" % (u["entry_stage_slot"], u["first_event"]) for u in r["uses"])))
    for m in f.get("matches", []):
        w("- content of %s equals (byte for byte, layout %s) %d colourkey files: %s" % (
            m["resource"], m["layout"], len(m["exact"]), ", ".join(sorted(set(x["file"] for x in m["exact"]))) or json.dumps(m["nearest"])))
    w("")
    w("## Not determined\n")
    for k, v in doc.get("undetermined", {}).items():
        w("- **%s**: %s" % (k, v))
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(L) + "\n")


UNDETERMINED = {
    "MakeNormalPS HeightScale": "no capture has a draw of TerrainShader::MakeNormalPS (or any other terrain cache bake draw: "
    "MakeNormal, ResolveDominance, TerrainRenderCache*): the bake ran before the frame; the constant is not in the files.",
    "g_HexToTex / g_TexToHex": "CPU-side constants of cvTerrainSampler, not a shader constant of any captured draw "
    "(HexPS/HexVS are not drawn). Related and read: g_WorldToTerrainTextureSpace and the terrain VS cb0 (origin -1088, -942.208 = "
    "-(2176, 2176 * 0.866) / 2 in S1a).",
    "g_iMaxTileSize": "the HeightMap bake targets are not bound in the frame; see terrain_page for the sizes of the "
    "single-sample colour targets that exist.",
}

# ---------------------------------------------------------------- the file


def main(argv):
    cache = None
    out_dir = None
    it = iter(argv)
    for a in it:
        if a == "--cache":
            cache = next(it)
        elif a == "--out":
            out_dir = next(it)
        elif a == "--game":
            global GAME
            GAME = next(it)
    if not GAME or not os.path.isdir(os.path.join(GAME, "Assets")):
        raise SystemExit("set CIV5_ROOT or pass --game <the game folder with Assets>")
    if cache is None:
        cache = os.environ.get("CIV5_CACHE")
    if cache is None:
        # tools/golden/rdc -> the repository root -> `cache` next to it.
        here = os.path.dirname(os.path.abspath(__file__))
        repo = os.path.dirname(os.path.dirname(os.path.dirname(here)))
        cache = os.path.join(os.path.dirname(repo), "cache")
    out_dir = out_dir or os.path.join(cache, "golden")
    cbmaps, dx11_dir = load_cbmaps(cache)
    caps = {}
    for name in MAIN:  # S1a and S2 carry the facts; the other captures are not needed here
        if os.path.isfile(os.path.join(cache, "golden", _scene(name), name, "events.json")):
            caps[name] = Cap(cache, name, cbmaps)
    facts = collections.OrderedDict()
    facts["g_FogConsts"] = distinct_named(caps, "g_FogConsts")
    facts["g_FogColor"] = distinct_named(caps, "g_FogColor")
    facts["f3FogColor"] = distinct_named(caps, "f3FogColor")
    facts["g_ShadowVP"] = distinct_named(caps, "g_ShadowVP")
    facts["g_ShadowFade"] = distinct_named(caps, "g_ShadowFade")
    facts["view_proj"] = fact_view_proj(caps, cache)
    facts["g_TerrainScale"] = fact_terrain_scale(caps, dx11_dir)
    facts["g_WorldToTerrainTextureSpace"] = fact_world_to_tex(caps)
    facts["shadow"] = fact_shadow(caps, dx11_dir)
    facts["fow_mask"] = fact_fow(caps, dx11_dir)
    facts["terrain_page"] = fact_page_format(caps)
    facts["building_cubes"] = fact_building_cubes(caps)
    facts["samplers"] = fact_samplers(caps)
    luts = fact_luts(caps, cache)
    match_luts(caps, luts, cache)
    facts["luts"] = luts
    doc = {"schema": SCHEMA, "captures": sorted(caps), "facts": facts, "undetermined": UNDETERMINED}
    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, "facts.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    write_md(doc, os.path.join(out_dir, "facts.md"))
    print("facts.json, facts.md:", len(facts), "facts from", ", ".join(sorted(caps)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

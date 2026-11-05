"""Capture -> JSON/DDS writer of `golden parse`. Stdlib only, no RenderDoc import.

The RenderDoc part (extract.py) turns a `.rdc` into a *provider*; this module turns a provider
into the files of `cache/golden/<scene>/<capture>/`. A provider is any object with:

    meta()               -> dict  renderdoc, api, gpu, driver, width, height, swapchain_format, msaa
    events()             -> list  in event order (see "Event dicts" below)
    shader_bytes(id)     -> bytes DXBC container of a shader resource id
    shader_layout(id)    -> dict|None  (optional) constant blocks, resources, samplers with names (-> shaders.json)
    all_textures()       -> list       (optional) every texture: format, size, mips, flags (-> textures.json)
    texture(id)          -> dict  name, format, width, height, depth, mips, layers, samples, cube,
                                  data: list of bytes, layer-major then mip (DDS order)
    buffer(id)           -> dict  name, data: bytes
    image(key)           -> dict  format, width, height, data: bytes (one subresource), or None
                                  keys: "before:<eid>" (frame target just before event eid),
                                  "final" (back buffer at the end), "target:<eid>:before|after:<n>"
                                  (n-th colour target, "depth" for depth/stencil),
                                  "srv:<eid>:<stage>:<slot>" (a texture the call reads, before it)

Event dicts (kind = "push" | "pop" | "draw" | "other"):
    push/pop: eid, name (pop has no name)
    draw:     eid, name, vs/hs/ds/ps (shader resource id or None), topology, index_count,
              vertex_count, instances, input_layout, vertex_buffers, index_buffer, blend,
              depth_stencil, rasterizer, viewports, scissors, targets, depth_target, textures,
              samplers, cbuffers (all copied to events.json as they are)

Everything written here is derived from the user's captures: it belongs in `cache/`, never in git.
"""

import hashlib
import json
import os
import re
import struct

SCHEMA_DRAWS = "civ5-golden-draws/1"
SCHEMA_EVENTS = "civ5-golden-events/1"
SCHEMA_META = "civ5-golden-meta/1"

# Packages whose draws are UI, not world (the first such draw is the UI boundary).
UI_PACKAGES = ("UIShader", "FontShader", "MenuShaders")
# A marker of the UI, in case the capture has `D3DPERF` groups and no catalog to look at.
UI_MARKER_RE = re.compile(r"(?i)(^|[^a-z])(ui|gui|interface|hud|menus?)([^a-z]|$)")

# DXGI_FORMAT numbers (dxgiformat.h) of the formats the world uses, by RenderDoc/D3D name.
DXGI = {
    "UNKNOWN": 0,
    "R32G32B32A32_TYPELESS": 1, "R32G32B32A32_FLOAT": 2, "R32G32B32A32_UINT": 3, "R32G32B32A32_SINT": 4,
    "R32G32B32_TYPELESS": 5, "R32G32B32_FLOAT": 6, "R32G32B32_UINT": 7, "R32G32B32_SINT": 8,
    "R16G16B16A16_TYPELESS": 9, "R16G16B16A16_FLOAT": 10, "R16G16B16A16_UNORM": 11,
    "R16G16B16A16_UINT": 12, "R16G16B16A16_SNORM": 13, "R16G16B16A16_SINT": 14,
    "R32G32_TYPELESS": 15, "R32G32_FLOAT": 16, "R32G32_UINT": 17, "R32G32_SINT": 18,
    "R32G8X24_TYPELESS": 19, "D32_FLOAT_S8X24_UINT": 20, "R32_FLOAT_X8X24_TYPELESS": 21,
    "R10G10B10A2_TYPELESS": 23, "R10G10B10A2_UNORM": 24, "R10G10B10A2_UINT": 25,
    "R11G11B10_FLOAT": 26,
    "R8G8B8A8_TYPELESS": 27, "R8G8B8A8_UNORM": 28, "R8G8B8A8_UNORM_SRGB": 29,
    "R8G8B8A8_UINT": 30, "R8G8B8A8_SNORM": 31, "R8G8B8A8_SINT": 32,
    "R16G16_TYPELESS": 33, "R16G16_FLOAT": 34, "R16G16_UNORM": 35, "R16G16_UINT": 36,
    "R16G16_SNORM": 37, "R16G16_SINT": 38,
    "R32_TYPELESS": 39, "D32_FLOAT": 40, "R32_FLOAT": 41, "R32_UINT": 42, "R32_SINT": 43,
    "R24G8_TYPELESS": 44, "D24_UNORM_S8_UINT": 45, "R24_UNORM_X8_TYPELESS": 46,
    "R8G8_TYPELESS": 48, "R8G8_UNORM": 49, "R8G8_UINT": 50, "R8G8_SNORM": 51, "R8G8_SINT": 52,
    "R16_TYPELESS": 53, "R16_FLOAT": 54, "D16_UNORM": 55, "R16_UNORM": 56, "R16_UINT": 57,
    "R16_SNORM": 58, "R16_SINT": 59,
    "R8_TYPELESS": 60, "R8_UNORM": 61, "R8_UINT": 62, "R8_SNORM": 63, "R8_SINT": 64, "A8_UNORM": 65,
    "BC1_TYPELESS": 70, "BC1_UNORM": 71, "BC1_UNORM_SRGB": 72,
    "BC2_TYPELESS": 73, "BC2_UNORM": 74, "BC2_UNORM_SRGB": 75,
    "BC3_TYPELESS": 76, "BC3_UNORM": 77, "BC3_UNORM_SRGB": 78,
    "BC4_TYPELESS": 79, "BC4_UNORM": 80, "BC4_SNORM": 81,
    "BC5_TYPELESS": 82, "BC5_UNORM": 83, "BC5_SNORM": 84,
    "B5G6R5_UNORM": 85, "B5G5R5A1_UNORM": 86,
    "B8G8R8A8_UNORM": 87, "B8G8R8X8_UNORM": 88,
    "B8G8R8A8_TYPELESS": 90, "B8G8R8A8_UNORM_SRGB": 91,
    "B8G8R8X8_TYPELESS": 92, "B8G8R8X8_UNORM_SRGB": 93,
    "BC6H_TYPELESS": 94, "BC6H_UF16": 95, "BC6H_SF16": 96,
    "BC7_TYPELESS": 97, "BC7_UNORM": 98, "BC7_UNORM_SRGB": 99,
    "R9G9B9E5_SHAREDEXP": 67, "B4G4R4A4_UNORM": 115,
    # RenderDoc's own names of depth formats (the bytes are what the API returns for that texture)
    "D24S8": 45, "D32S8": 20, "D16": 55, "D32": 40, "R5G6B5_UNORM": 85, "R5G5B5A1_UNORM": 86,
}


class CaptureError(Exception):
    """A capture or a provider that cannot be turned into files (the message names why)."""


def sha1_hex(data):
    return hashlib.sha1(data).hexdigest()


def safe_name(s):
    """A resource id as a file name."""
    return re.sub(r"[^A-Za-z0-9_.-]", "_", str(s))


def _finite(o):
    """JSON has no NaN or Infinity (serde_json rejects them): they become null."""
    if isinstance(o, float):
        return o if o - o == 0.0 else None
    if isinstance(o, list):
        return [_finite(x) for x in o]
    if isinstance(o, dict):
        return {k: _finite(v) for k, v in o.items()}
    return o


def write_json(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(_finite(obj), f, indent=1, sort_keys=False, allow_nan=False)
        f.write("\n")


# ---------------------------------------------------------------- DDS


def dxgi_number(fmt):
    """DXGI number of a format name ('R8G8B8A8_UNORM', 'DXGI_FORMAT_R8G8B8A8_UNORM' or a number)."""
    if isinstance(fmt, int):
        return fmt
    name = str(fmt).upper()
    if name.startswith("DXGI_FORMAT_"):
        name = name[len("DXGI_FORMAT_"):]
    if name not in DXGI:
        raise CaptureError("unknown format %r (add it to DXGI in rdlib.py)" % (fmt,))
    return DXGI[name]


def dds_bytes(tex):
    """A DDS file (DX10 header) from a texture dict; all mips, faces and layers."""
    mips, layers = int(tex["mips"]), int(tex["layers"])
    cube = bool(tex.get("cube"))
    depth = int(tex.get("depth", 1))
    data = tex["data"]
    if len(data) != mips * layers:
        raise CaptureError(
            "texture %s: %d subresources, expected %d mips x %d layers"
            % (tex.get("name"), len(data), mips, layers)
        )
    caps = 0x1000
    flags = 0x1 | 0x2 | 0x4 | 0x1000
    if mips > 1:
        flags |= 0x20000
        caps |= 0x8 | 0x400000
    if depth > 1:
        flags |= 0x800000
    caps2 = 0xFE00 if cube else 0
    if cube:
        caps |= 0x8
    if depth > 1:
        caps2 |= 0x200000
    header = struct.pack(
        "<4sIIIIIII11I",
        b"DDS ", 124, flags, int(tex["height"]), int(tex["width"]), 0, depth, mips,
        *([0] * 11)
    )
    pixel_format = struct.pack("<IIIIIIII", 32, 0x4, 0x30315844, 0, 0, 0, 0, 0)  # 'DX10'
    tail = struct.pack("<IIII", caps, caps2, 0, 0) + struct.pack("<I", 0)
    array_size = layers // 6 if cube else layers
    dx10 = struct.pack(
        "<IIIII",
        dxgi_number(tex["format"]),
        4 if depth > 1 else 3,
        4 if cube else 0,
        max(array_size, 1),
        0,
    )
    return header + pixel_format + tail + dx10 + b"".join(data)


def image_as_texture(img, name):
    return {
        "name": name,
        "format": img["format"],
        "width": img["width"],
        "height": img["height"],
        "depth": 1,
        "mips": 1,
        "layers": 1,
        "data": [img["data"]],
    }


# ---------------------------------------------------------------- analysis


def build_marker_tree(events):
    """Nested {name, first_event, last_event, children} from push/pop events."""
    root = {"name": None, "first_event": None, "last_event": None, "children": []}
    stack = [root]
    for e in events:
        if e["kind"] == "push":
            node = {"name": e.get("name", ""), "first_event": e["eid"], "last_event": e["eid"], "children": []}
            stack[-1]["children"].append(node)
            stack.append(node)
        elif e["kind"] == "pop" and len(stack) > 1:
            stack.pop()["last_event"] = e["eid"]
        for n in stack[1:]:
            n["last_event"] = e["eid"]
    return root["children"]


def entry_package(entry):
    return entry.split("::", 1)[0] if entry else ""


def find_ui_boundary(draws, events, index):
    """First draw with a UI-package shader, or first UI marker, whichever is earlier.

    Returns {"event": eid, "reason": "shader"|"marker", "detail": ...} or None.
    """
    best = None
    for d in draws:
        entries = [d.get(k + "_entry") for k in ("vs", "hs", "ds", "ps")]
        hit = next((en for en in entries if entry_package(en) in UI_PACKAGES), None)
        if hit:
            best = {"event": d["event"], "reason": "shader", "detail": hit}
            break
    for e in events:
        if e["kind"] == "push" and UI_MARKER_RE.search(e.get("name", "")):
            if best is None or e["eid"] < best["event"]:
                best = {"event": e["eid"], "reason": "marker", "detail": e.get("name", "")}
            break
    return best


def _target_key(t):
    return t.get("resource") if t else None


def build_passes(draw_events):
    """Passes = runs of draws with the same colour targets and depth target."""
    passes = []
    for d in draw_events:
        key = (
            tuple(_target_key(t) for t in d.get("targets", [])),
            _target_key(d.get("depth_target")),
        )
        if not passes or passes[-1]["_key"] != key:
            passes.append(
                {
                    "_key": key,
                    "index": len(passes),
                    "first_event": d["eid"],
                    "last_event": d["eid"],
                    "draws": 0,
                    "targets": [
                        {k: t.get(k) for k in ("resource", "format", "width", "height", "samples")}
                        for t in d.get("targets", [])
                    ],
                    "depth_target": d.get("depth_target"),
                }
            )
        passes[-1]["last_event"] = d["eid"]
        passes[-1]["draws"] += 1
        d["pass"] = passes[-1]["index"]
    for p in passes:
        del p["_key"]
    return passes


def _coverage(d):
    n = d.get("index_count") or d.get("vertex_count") or 0
    return n * max(int(d.get("instances") or 1), 1)


def select_target_events(spec, world_draws):
    """Events whose targets are saved before/after: 'none', 'world-top3' or '12,40,...'."""
    if spec in (None, "", "none"):
        return []
    if spec == "world-top3":
        by_pair = {}
        for d in world_draws:
            by_pair.setdefault((d.get("vs"), d.get("ps")), []).append(d)
        out = []
        for ds in by_pair.values():
            out += [d["event"] for d in sorted(ds, key=_coverage, reverse=True)[:3]]
        return sorted(out)
    try:
        return sorted(int(x) for x in str(spec).split(",") if x.strip())
    except ValueError:
        raise CaptureError("--targets must be none, world-top3 or event numbers, got %r" % spec)


# ---------------------------------------------------------------- the writer


def process(provider, out_dir, scene, capture, index=None, targets="none"):
    """Writes the files of one capture into `out_dir`; returns the summary dict (= meta.json)."""
    index = index or {}
    meta = provider.meta()
    events = provider.events()
    events.sort(key=lambda e: e["eid"])

    sha_cache = {}

    def sha_of(rid):
        if rid is None:
            return None
        if rid not in sha_cache:
            sha_cache[rid] = sha1_hex(provider.shader_bytes(rid))
        return sha_cache[rid]

    layouts = {}  # sha1 -> reflection layout (names of constants), for the facts
    draws = []  # draws.json entries
    draw_events = []  # events.json calls
    for e in events:
        if e["kind"] != "draw":
            continue
        call = dict(e)
        call.pop("kind", None)
        shas = {}
        for st in ("vs", "hs", "ds", "ps"):
            shas[st] = sha_of(e.get(st))
            if shas[st] and shas[st] not in layouts and hasattr(provider, "shader_layout"):
                layouts[shas[st]] = provider.shader_layout(e.get(st))
            call[st] = shas[st]
            call[st + "_resource"] = e.get(st)
        draw_events.append(call)
    passes = build_passes(draw_events)

    for c in draw_events:
        d = {
            "event": c["eid"],
            "pass": c["pass"],
            "vs": c["vs"],
            "ps": c["ps"],
            "hs": c["hs"],
            "ds": c["ds"],
            "vs_entry": index.get(c["vs"]) if c["vs"] else None,
            "ps_entry": index.get(c["ps"]) if c["ps"] else None,
            "hs_entry": index.get(c["hs"]) if c["hs"] else None,
            "ds_entry": index.get(c["ds"]) if c["ds"] else None,
            "topology": c.get("topology"),
            "index_count": c.get("index_count", 0),
            "vertex_count": c.get("vertex_count", 0),
            "instances": c.get("instances", 1),
            "targets": [
                {k: t.get(k) for k in ("format", "width", "height", "samples")}
                for t in c.get("targets", [])
            ],
            "depth_format": (c.get("depth_target") or {}).get("format"),
        }
        draws.append(d)

    ui = find_ui_boundary(draws, events, index)
    ui_event = ui["event"] if ui else None
    for d in draws:
        d["world"] = ui_event is None or d["event"] < ui_event
    for c, d in zip(draw_events, draws):
        c["world"] = d["world"]
    world_draws = [d for d in draws if d["world"]]
    has_markers = any(e["kind"] == "push" for e in events)

    by_entry = {}
    for d in draws:
        for k in ("vs", "ps", "hs", "ds"):
            en = d.get(k + "_entry")
            if en:
                lst = by_entry.setdefault(en, [])
                if d["event"] not in lst:
                    lst.append(d["event"])

    common = {"scene": scene, "capture": capture}
    write_json(
        os.path.join(out_dir, "draws.json"),
        dict(
            schema=SCHEMA_DRAWS,
            width=meta["width"],
            height=meta["height"],
            ui_boundary_event=ui_event,
            draws=draws,
            by_entry=by_entry,
            **common
        ),
    )
    write_json(
        os.path.join(out_dir, "events.json"),
        dict(
            schema=SCHEMA_EVENTS,
            markers=build_marker_tree(events),
            passes=passes,
            ui_boundary=ui,
            calls=draw_events,
            **common
        ),
    )

    # Textures read by the world draws, in their own formats; buffers of their vertices and indices.
    tex_ids, buf_ids = [], []
    for c in draw_events:
        if not c["world"]:
            continue
        for stage in c.get("textures", {}).values():
            for t in stage:
                r = t.get("resource")
                if r is not None and not t.get("buffer") and r not in tex_ids:
                    tex_ids.append(r)
        for vb in c.get("vertex_buffers", []):
            if vb.get("resource") is not None and vb["resource"] not in buf_ids:
                buf_ids.append(vb["resource"])
        ib = c.get("index_buffer")
        if ib and ib.get("resource") is not None and ib["resource"] not in buf_ids:
            buf_ids.append(ib["resource"])
    for r in tex_ids:
        with open(os.path.join(_mk(out_dir, "textures"), safe_name(r) + ".dds"), "wb") as f:
            f.write(dds_bytes(provider.texture(r)))
    for r in buf_ids:
        with open(os.path.join(_mk(out_dir, "buffers"), safe_name(r) + ".bin"), "wb") as f:
            f.write(provider.buffer(r)["data"])

    # Targets before and after the chosen calls.
    chosen = select_target_events(targets, world_draws)
    known = {c["eid"] for c in draw_events}
    for eid in chosen:
        if eid not in known:
            raise CaptureError("--targets: event %d is not a draw call" % eid)
        n_color = len(next(c for c in draw_events if c["eid"] == eid).get("targets", []))
        for when in ("before", "after"):
            for n in list(range(n_color)) + ["depth"]:
                img = provider.image("target:%d:%s:%s" % (eid, when, n))
                if img is None:
                    continue
                if n == "depth":
                    name = "%d-%s-depth" % (eid, when)
                elif n == 0:
                    name = "%d-%s" % (eid, when)
                else:
                    name = "%d-%s-%d" % (eid, when, n)
                with open(os.path.join(_mk(out_dir, "targets"), name + ".dds"), "wb") as f:
                    f.write(dds_bytes(image_as_texture(img, name)))

    if layouts:
        write_json(os.path.join(out_dir, "shaders.json"), layouts)
    if hasattr(provider, "all_textures"):
        write_json(os.path.join(out_dir, "textures.json"), provider.all_textures())

    # With an explicit event list the textures the call reads are saved too, as they are before it.
    if targets not in (None, "", "none", "world-top3"):
        for eid in chosen:
            call = next(c for c in draw_events if c["eid"] == eid)
            for stage, texs in call.get("textures", {}).items():
                for t in texs:
                    img = provider.image("srv:%d:%s:%d" % (eid, stage, t["slot"]))
                    if img is None:
                        continue
                    name = "%d-in-%s%d" % (eid, stage, t["slot"])
                    with open(os.path.join(_mk(out_dir, "targets"), name + ".dds"), "wb") as f:
                        f.write(dds_bytes(image_as_texture(img, name)))

    world_img = provider.image("before:%d" % ui_event) if ui_event is not None else None
    final_img = provider.image("final")
    if world_img is None:
        world_img = final_img  # no UI boundary: the frame has no UI, or it was not found (meta says which)
    for name, img in (("world.dds", world_img), ("final.dds", final_img)):
        if img is not None:
            with open(os.path.join(out_dir, name), "wb") as f:
                f.write(dds_bytes(image_as_texture(img, name)))

    summary = dict(
        schema=SCHEMA_META,
        renderdoc=meta.get("renderdoc"),
        api=meta.get("api"),
        gpu=meta.get("gpu"),
        driver=meta.get("driver"),
        width=meta["width"],
        height=meta["height"],
        swapchain_format=meta.get("swapchain_format"),
        msaa=meta.get("msaa"),
        event_count=len(events),
        draw_count=len(draws),
        world_draw_count=len(world_draws),
        pass_count=len(passes),
        has_markers=has_markers,
        ui_boundary=ui,
        shader_index_used=bool(index),
        textures_written=len(tex_ids),
        buffers_written=len(buf_ids),
        targets_events=chosen,
        world_image=world_img is not None,
        final_image=final_img is not None,
        **common
    )
    write_json(os.path.join(out_dir, "meta.json"), summary)
    return summary


def _mk(base, sub):
    p = os.path.join(base, sub)
    os.makedirs(p, exist_ok=True)
    return p

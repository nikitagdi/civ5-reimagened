"""Reads one capture and writes `cache/golden/<scene>/<capture>/`.

Run by `golden parse` as `python -I extract.py ...` (stdlib + the RenderDoc module only):

    extract.py --out DIR --scene S1 --capture S1a (--rdc FILE | --model FILE)
               [--index sha1-to-entry.json] [--targets none|world-top3|12,40] [--renderdoc-path DIR]
    extract.py --job job.json          # same keys in a file; for `qrenderdoc.exe FILE --python extract.py`

`--model` takes a capture-equivalent JSON ("civ5-golden-model/1", see JsonProvider): the way to test
everything without RenderDoc. `--rdc` needs the `renderdoc` module (renderdoc.pyd of the RenderDoc
install; `--renderdoc-path` is the folder that has it).

RenderDocProvider is run against RenderDoc 1.46 (D3D11 captures of the original game). That RenderDoc has
no standalone renderdoc.pyd: the module lives inside qrenderdoc.exe (embedded Python 3.8), so the working mode is

    qrenderdoc.exe --python extract.py     (job file in CIV5_GOLDEN_JOB, no capture argument)

The script opens the capture itself through the replay API (OpenCaptureFile) instead of waiting for the UI to
load it: the script runs on the UI thread, so a wait loop would never see the capture load.
"""

import base64
import enum
import json
import os
import struct
import sys
import time

# qrenderdoc runs scripts with exec() and its own __file__: the caller names the folder
_HERE = os.environ.get("CIV5_GOLDEN_RDC_DIR") or os.path.dirname(os.path.abspath(globals().get("__file__", "extract.py")))
if _HERE not in sys.path:  # `python -I` leaves the script folder out of sys.path
    sys.path.insert(0, _HERE)

import rdlib  # noqa: E402

MODEL_SCHEMA = "civ5-golden-model/1"


def _b64(s):
    return base64.b64decode(s)


# ---------------------------------------------------------------- synthetic captures


class JsonProvider(object):
    """A capture-equivalent JSON file. Layout (all binary data is base64):

    {"schema": "civ5-golden-model/1",
     "meta":     {renderdoc, api, gpu, driver, width, height, swapchain_format, msaa},
     "shaders":  {"<id>": "<DXBC container, hex>"},
     "textures": {"<id>": {name, format, width, height, depth?, mips, layers, samples?, cube?, data: [b64, ...]}},
     "buffers":  {"<id>": {name, data: b64}},
     "images":   {"<key>": {format, width, height, data: b64}},      # keys: see rdlib
     "events":   [{eid, kind: push|pop|draw|other, name?, ...draw fields...}]}
    """

    def __init__(self, path):
        with open(path, "r", encoding="utf-8") as f:
            self.m = json.load(f)
        if self.m.get("schema") != MODEL_SCHEMA:
            raise rdlib.CaptureError("%s: schema is not %s" % (path, MODEL_SCHEMA))

    def meta(self):
        return dict(self.m["meta"])

    def events(self):
        return [dict(e) for e in self.m["events"]]

    def shader_bytes(self, rid):
        try:
            return bytes.fromhex(self.m["shaders"][rid])
        except KeyError:
            raise rdlib.CaptureError("shader %r is not in the model" % rid)

    def texture(self, rid):
        try:
            t = dict(self.m["textures"][rid])
        except KeyError:
            raise rdlib.CaptureError("texture %r is not in the model" % rid)
        t["data"] = [_b64(x) for x in t["data"]]
        return t

    def buffer(self, rid):
        try:
            b = dict(self.m["buffers"][rid])
        except KeyError:
            raise rdlib.CaptureError("buffer %r is not in the model" % rid)
        b["data"] = _b64(b["data"])
        return b

    def image(self, key):
        i = self.m.get("images", {}).get(key)
        if i is None:
            return None
        i = dict(i)
        i["data"] = _b64(i["data"])
        return i


# ---------------------------------------------------------------- RenderDoc

_SKIP = ("this", "thisown")


def _plain(obj, depth=0):
    """A RenderDoc python object as JSON values (enums by name, structs as dicts)."""
    if obj is None or isinstance(obj, (bool, int, float, str)):
        if isinstance(obj, enum.Enum):
            return str(obj.name)
        return obj
    if isinstance(obj, enum.Enum):
        return str(obj.name)
    if isinstance(obj, bytes):
        return obj.hex()
    if depth > 4:
        return str(obj)
    if isinstance(obj, (list, tuple)):
        return [_plain(x, depth + 1) for x in obj]
    if hasattr(obj, "Name") and callable(obj.Name) and hasattr(obj, "compType"):  # ResourceFormat
        return _fmt_name(obj)
    if hasattr(obj, "__len__") and hasattr(obj, "__getitem__") and not hasattr(obj, "acquire"):
        return [_plain(x, depth + 1) for x in obj]  # rdcarray
    if hasattr(obj, "__len__") and hasattr(obj, "__getitem__"):
        try:
            return [_plain(obj[i], depth + 1) for i in range(len(obj))]
        except Exception:
            pass
    out = {}
    for k in dir(obj):
        if k.startswith("_") or k in _SKIP:
            continue
        try:
            v = getattr(obj, k)
        except Exception:
            continue
        if callable(v):
            continue
        out[k] = _plain(v, depth + 1)
    return out if out else str(obj)


def _ok(res):
    """True for the result of Open*: a ResultDetails (new RenderDoc) or a ReplayStatus (old)."""
    if hasattr(res, "OK"):
        return res.OK()
    return str(res).endswith("Succeeded")


def _fmt_name(fmt):
    try:
        return fmt.Name()
    except Exception:
        return str(fmt)


class RenderDocProvider(object):
    """Reads a D3D11 capture through the RenderDoc replay API (RenderDoc 1.46: PipeState descriptors)."""

    def __init__(self, rd, controller, path, renderdoc_version, driver):
        self.rd = rd
        self.c = controller
        self.path = path
        self.version = renderdoc_version
        self.driver = driver
        self._events = None
        self._shaders = {}
        self._layouts = {}
        self._textures = {self._id(t.resourceId): t for t in controller.GetTextures()}
        self._buffers = {self._id(b.resourceId): b for b in controller.GetBuffers()}
        self._rids = {}
        for t in controller.GetTextures():
            self._rids[self._id(t.resourceId)] = t.resourceId
        for b in controller.GetBuffers():
            self._rids[self._id(b.resourceId)] = b.resourceId
        self._names = {self._id(r.resourceId): r.name for r in controller.GetResources()}

    # -- ids and names
    def _id(self, rid):
        if rid is None or rid == self.rd.ResourceId.Null():
            return None
        return str(int(rid))

    def _sub(self, mip, sl, sample):
        s = self.rd.Subresource()
        s.mip, s.slice, s.sample = mip, sl, sample
        return s

    # -- walking the actions
    def _walk(self, actions, sf, out):
        rd = self.rd
        flags = rd.ActionFlags
        for a in actions:
            push = bool(a.flags & flags.PushMarker)
            name = a.GetName(sf)
            if push:
                out.append({"eid": a.eventId, "kind": "push", "name": name})
                self._walk(a.children, sf, out)
                last = out[-1]["eid"]
                out.append({"eid": last, "kind": "pop"})
            elif a.flags & flags.Drawcall:
                out.append(self._draw(a, name))
            else:
                out.append({"eid": a.eventId, "kind": "other", "name": name})

    def _view(self, d):
        """A target or SRV descriptor as a dict."""
        rid = self._id(d.resource)
        out = {"resource": rid, "view_format": _fmt_name(d.format),
               "first_mip": d.firstMip, "num_mips": d.numMips,
               "first_slice": d.firstSlice, "num_slices": d.numSlices}
        t = self._textures.get(rid)
        if t is not None:
            out.update({"name": self._names.get(rid, ""), "format": _fmt_name(t.format), "width": t.width,
                        "height": t.height, "samples": t.msSamp, "mips": t.mips, "array_size": t.arraysize,
                        "dimension": str(getattr(t.type, "name", t.type))})
        return out

    def _draw(self, a, name):
        rd, c = self.rd, self.c
        _progress("draw %d" % a.eventId)
        c.SetFrameEvent(a.eventId, True)
        st = c.GetD3D11PipelineState()
        ps = c.GetPipelineState()
        ia, om = st.inputAssembly, st.outputMerger
        indexed = bool(a.flags & rd.ActionFlags.Indexed)
        e = {
            "eid": a.eventId,
            "kind": "draw",
            "name": name,
            "topology": str(getattr(ia.topology, "name", ia.topology)),
            "index_count": int(a.numIndices) if indexed else 0,
            "vertex_count": 0 if indexed else int(a.numIndices),
            "instances": max(int(a.numInstances), 1),
            # Where the draw starts in the shared buffers (the replay, needs them).
            "start_index": int(getattr(a, "indexOffset", 0)) if indexed else 0,
            "base_vertex": int(getattr(a, "baseVertex", 0)) if indexed else 0,
            "start_vertex": 0 if indexed else int(getattr(a, "vertexOffset", 0)),
            "start_instance": int(getattr(a, "instanceOffset", 0)),
            "input_layout": _plain(list(ia.layouts)),
            "vertex_buffers": [
                {"slot": i, "resource": self._id(v.resourceId), "offset": v.byteOffset, "stride": v.byteStride}
                for i, v in enumerate(ia.vertexBuffers)
                if self._id(v.resourceId)
            ],
            "index_buffer": {
                "resource": self._id(ia.indexBuffer.resourceId),
                "offset": ia.indexBuffer.byteOffset,
                "stride": getattr(ia.indexBuffer, "byteStride", None),
            }
            if self._id(ia.indexBuffer.resourceId)
            else None,
            "blend": _plain(om.blendState),
            "depth_stencil": _plain(om.depthStencilState),
            "rasterizer": _plain(st.rasterizer.state),
            "viewports": _plain(list(st.rasterizer.viewports)),
            "scissors": _plain(list(st.rasterizer.scissors)),
            "targets": [],
            "depth_target": None,
            "textures": {},
            "samplers": {},
            "cbuffers": {},
        }
        for d in ps.GetOutputTargets():  # Descriptors, not UsedDescriptors
            if self._id(d.resource):
                e["targets"].append(self._view(d))
        dd = ps.GetDepthTarget()
        if self._id(dd.resource):
            e["depth_target"] = self._view(dd)
        sh_stage = (
            ("vs", st.vertexShader, rd.ShaderStage.Vertex),
            ("hs", st.hullShader, rd.ShaderStage.Hull),
            ("ds", st.domainShader, rd.ShaderStage.Domain),
            ("ps", st.pixelShader, rd.ShaderStage.Pixel),
        )
        for key, sh, stage in sh_stage:
            sid = self._id(sh.resourceId)
            e[key] = sid
            if not sid:
                continue
            refl = sh.reflection
            if sid not in self._shaders:
                self._shaders[sid] = bytes(refl.rawBytes) if refl is not None else b""
                self._layouts[sid] = self._layout(refl)
            texs, samps, cbs = [], [], []
            for u in ps.GetReadOnlyResources(stage, True):
                v = self._view(u.descriptor)
                v["slot"], v["bind"] = self._slot(refl, "readOnlyResources", u.access.index)
                v["srgb"] = "SRGB" in v["view_format"].upper()
                if v["resource"] in self._buffers:  # a buffer SRV (structured or raw), not a texture
                    v["buffer"] = True
                    v["first_element"] = int(u.descriptor.byteOffset)
                    v["byte_size"] = int(u.descriptor.byteSize)
                if v["resource"]:
                    texs.append(v)
            for u in ps.GetSamplers(stage, True):
                s = _plain(u.sampler)
                s = s if isinstance(s, dict) else {"value": s}
                s["slot"], s["bind"] = self._slot(refl, "samplers", u.access.index)
                samps.append(s)
            for u in ps.GetConstantBlocks(stage, True):
                rid = self._id(u.descriptor.resource)
                if not rid:
                    continue
                blen = int(self._buffers[rid].length) if rid in self._buffers else 0
                off = int(u.descriptor.byteOffset)
                size = int(u.descriptor.byteSize)
                if size <= 0 or off + size > blen:
                    size = max(blen - off, 0)
                raw = self._buf(rid, off, size)
                n = len(raw) // 4
                fl = list(struct.unpack("<%df" % n, raw[: n * 4]))
                bad = [i for i, x in enumerate(fl) if x != x or x in (float("inf"), float("-inf"))]
                for i in bad:  # JSON has no NaN/Inf (a buffer holds ints and junk too): 0.0 here, the raw words below
                    fl[i] = 0.0
                slot, bind = self._slot(refl, "constantBlocks", u.access.index)
                cb = {"slot": slot, "bind": bind, "resource": rid, "offset": off, "size": size, "floats": fl}
                if bad:
                    cb["nonfinite"] = {str(i): "0x%08x" % struct.unpack_from("<I", raw, i * 4)[0] for i in bad}
                cbs.append(cb)
            e["textures"][key] = texs
            e["samplers"][key] = samps
            e["cbuffers"][key] = cbs
        return e

    def _buf(self, rid, off, size):
        return bytes(self.c.GetBufferData(self._rids[rid], off, size))

    def _bind(self, refl, field, index):
        """Register and name of reflection entry `index` of `field`."""
        try:
            r = getattr(refl, field)[index]
        except Exception:
            return None
        return {"name": r.name, "register": getattr(r, "fixedBindNumber", None)}

    def _slot(self, refl, field, index):
        """(register, bind): the D3D register of a used descriptor (`slot`; the descriptor's own index is
        the position in the reflection lists, which is not the register)."""
        b = self._bind(refl, field, index)
        return (b["register"] if b and b["register"] is not None else index), b

    def _layout(self, refl):
        """Constant buffers, resources and samplers of a shader (names, offsets) for the facts."""
        if refl is None:
            return None

        def var(v):
            t = v.type
            d = {"name": v.name, "offset": int(v.byteOffset), "type": str(getattr(t.baseType, "name", t.baseType)),
                 "rows": int(t.rows), "cols": int(t.columns), "elements": int(t.elements)}
            if len(t.members):
                d["members"] = [var(m) for m in t.members]
            return d

        out = {"stage": str(getattr(refl.stage, "name", refl.stage)), "entry": refl.entryPoint,
               "constant_blocks": [], "resources": [], "samplers": []}
        for cb in refl.constantBlocks:
            out["constant_blocks"].append({"name": cb.name, "register": int(cb.fixedBindNumber), "size": int(cb.byteSize),
                                           "vars": [var(v) for v in cb.variables]})
        for r in refl.readOnlyResources:
            out["resources"].append({"name": r.name, "register": int(r.fixedBindNumber),
                                     "type": str(getattr(r.textureType, "name", r.textureType))})
        for s in refl.samplers:
            out["samplers"].append({"name": s.name, "register": int(s.fixedBindNumber)})
        return out

    # -- provider interface
    def meta(self):
        c = self.c
        props = c.GetAPIProperties()
        draws = [e for e in self.events() if e["kind"] == "draw" and e["targets"]]
        last = draws[-1]["targets"][0] if draws else {"width": 0, "height": 0, "format": None, "samples": 1}
        return {
            "renderdoc": self.version,
            "api": str(getattr(props.pipelineType, "name", props.pipelineType)),
            "gpu": str(getattr(props.vendor, "name", props.vendor)),
            "driver": self.driver,
            "width": last.get("width"),
            "height": last.get("height"),
            "swapchain_format": last.get("format"),
            "msaa": max([t.get("samples") or 1 for d in draws for t in d["targets"]] or [1]),
        }

    def events(self):
        if self._events is None:
            out = []
            sf = self.c.GetStructuredFile()
            self._walk(self.c.GetRootActions(), sf, out)
            self._events = out
        return self._events

    def all_textures(self):
        """Every texture of the capture (not only the bound ones): for the facts (formats, sizes)."""
        out = []
        for rid, t in self._textures.items():
            out.append({"resource": rid, "name": self._names.get(rid, ""), "format": _fmt_name(t.format),
                        "width": t.width, "height": t.height, "depth": t.depth, "mips": t.mips,
                        "array_size": t.arraysize, "samples": t.msSamp, "cube": bool(t.cubemap),
                        "dimension": str(getattr(t.type, "name", t.type)),
                        "creation_flags": str(t.creationFlags), "byte_size": int(t.byteSize)})
        return out

    def shader_bytes(self, rid):
        return self._shaders[rid]

    def shader_layout(self, rid):
        return self._layouts.get(rid)

    def texture(self, rid_str):
        rid = self._rids[rid_str]
        t = self._textures[rid_str]
        data = []
        layers = t.arraysize  # RenderDoc counts the 6 faces of a cube map in arraysize
        for sl in range(layers):
            for mip in range(t.mips):
                data.append(bytes(self.c.GetTextureData(rid, self._sub(mip, sl, 0))))
        return {
            "name": self._names.get(rid_str, ""),
            "format": _fmt_name(t.format),
            "width": t.width,
            "height": t.height,
            "depth": t.depth,
            "mips": t.mips,
            "layers": layers,
            "cube": bool(t.cubemap),
            "samples": t.msSamp,
            "data": data,
        }

    def buffer(self, rid_str):
        return {"name": self._names.get(rid_str, ""), "data": bytes(self.c.GetBufferData(self._rids[rid_str], 0, 0))}

    def _read(self, rid_str, eid):
        """Sample 0 of mip 0 of a target, as it is after event `eid`."""
        self.c.SetFrameEvent(eid, True)
        t = self._textures[rid_str]
        return {
            "format": _fmt_name(t.format),
            "width": t.width,
            "height": t.height,
            "data": bytes(self.c.GetTextureData(self._rids[rid_str], self._sub(0, 0, 0))),
        }

    def image(self, key):
        ev = self.events()
        draws = {e["eid"]: e for e in ev if e["kind"] == "draw"}
        parts = key.split(":")
        try:
            if parts[0] == "final":
                last = [e for e in ev if e["kind"] == "draw" and e["targets"]][-1]
                return self._read(last["targets"][0]["resource"], last["eid"])
            if parts[0] == "before":
                eid = int(parts[1])
                nxt = next((e for e in ev if e["eid"] >= eid and e["kind"] == "draw" and e["targets"]), None)
                prev = max([e["eid"] for e in ev if e["eid"] < eid] or [0])
                return self._read(nxt["targets"][0]["resource"], prev) if nxt else None
            if parts[0] == "srv":  # srv:<eid>:<stage>:<slot>, the texture as it is before the draw
                eid, stage, slot = int(parts[1]), parts[2], int(parts[3])
                t = next((t for t in draws[eid]["textures"].get(stage, [])
                          if t["slot"] == slot and not t.get("buffer")), None)
                if t is None or t["resource"] not in self._textures or self._textures[t["resource"]].depth > 1:
                    return None
                return self._read(t["resource"], max([e["eid"] for e in ev if e["eid"] < eid] or [0]))
            if parts[0] == "target":
                eid, when, n = int(parts[1]), parts[2], parts[3]
                d = draws[eid]
                tgt = d["depth_target"] if n == "depth" else (d["targets"][int(n)] if int(n) < len(d["targets"]) else None)
                if not tgt:
                    return None
                at = eid if when == "after" else max([e["eid"] for e in ev if e["eid"] < eid] or [0])
                return self._read(tgt["resource"], at)
        except KeyError:
            return None
        return None


def open_rdc(path, renderdoc_path, in_qrenderdoc=False):
    """-> (provider, shutdown callable). Imports `renderdoc` from `renderdoc_path` when it is given."""
    if renderdoc_path and not in_qrenderdoc:
        sys.path.insert(0, renderdoc_path)
        if hasattr(os, "add_dll_directory"):
            os.add_dll_directory(renderdoc_path)
    try:
        import renderdoc as rd
    except ImportError as e:
        raise rdlib.CaptureError(
            "cannot import the renderdoc module (%s); RenderDoc ships it inside qrenderdoc.exe only:"
            " set `mode` to `qrenderdoc` in cache/golden/renderdoc.json" % e
        )
    if not in_qrenderdoc:  # qrenderdoc has initialised the replay already
        rd.InitialiseReplay(rd.GlobalEnvironment(), [])
    cap = rd.OpenCaptureFile()
    if not _ok(cap.OpenFile(path, "", None)):
        raise rdlib.CaptureError("%s: RenderDoc cannot open the file" % path)
    res = cap.OpenCapture(rd.ReplayOptions(), None)
    status, controller = res if isinstance(res, tuple) else (res.first, res.second)
    if not _ok(status):
        raise rdlib.CaptureError("%s: RenderDoc cannot replay the capture (%s)" % (path, status))
    version = rd.GetVersionString() if hasattr(rd, "GetVersionString") else ""
    provider = RenderDocProvider(rd, controller, path, version, cap.DriverName())

    def shutdown():
        controller.Shutdown()
        cap.Shutdown()
        if not in_qrenderdoc:
            rd.ShutdownReplay()

    return provider, shutdown



# ---------------------------------------------------------------- command line


def run(job):
    """job: dict with out, scene, capture, rdc | model, index?, targets?, renderdoc_path?"""
    index = {}
    if job.get("index"):
        with open(job["index"], "r", encoding="utf-8") as f:
            index = json.load(f)
    shutdown = None
    if job.get("model"):
        provider = JsonProvider(job["model"])
    elif job.get("rdc"):
        provider, shutdown = open_rdc(job["rdc"], job.get("renderdoc_path"))
    else:
        raise rdlib.CaptureError("--rdc or --model needed")
    try:
        summary = rdlib.process(
            provider, job["out"], job["scene"], job["capture"], index=index, targets=job.get("targets", "none")
        )
    finally:
        if shutdown:
            shutdown()
    return summary


def _parse_args(argv):
    job = {}
    keys = {"--out": "out", "--scene": "scene", "--capture": "capture", "--rdc": "rdc", "--model": "model",
            "--index": "index", "--targets": "targets", "--renderdoc-path": "renderdoc_path", "--job": "job"}
    it = iter(argv)
    for a in it:
        if a not in keys:
            raise rdlib.CaptureError("unknown argument %r" % a)
        try:
            job[keys[a]] = next(it)
        except StopIteration:
            raise rdlib.CaptureError("%s needs a value" % a)
    if "job" in job:
        with open(job.pop("job"), "r", encoding="utf-8") as f:
            job.update(json.load(f))
    for k in ("out", "scene", "capture"):
        if k not in job:
            raise rdlib.CaptureError("missing --%s" % k)
    return job


def main(argv):
    try:
        job = _parse_args(argv)
        summary = run(job)
    except rdlib.CaptureError as e:
        sys.stderr.write("extract.py: %s\n" % e)
        return 2
    print(json.dumps({"capture": summary["capture"], "events": summary["event_count"], "draws": summary["draw_count"],
                      "ui_boundary": summary["ui_boundary"]}))
    return 0


_PROGRESS = None


def _progress(msg):
    """Progress lines of a qrenderdoc run (a parse takes minutes and the app has no console)."""
    if _PROGRESS:
        _PROGRESS.write("%.1f %s\n" % (time.time(), msg))
        _PROGRESS.flush()


def _qrenderdoc_main():
    """`qrenderdoc.exe --python extract.py`: the job file is named in CIV5_GOLDEN_JOB."""
    import traceback

    job_path = os.environ.get("CIV5_GOLDEN_JOB")
    with open(job_path, "r", encoding="utf-8") as f:
        job = json.load(f)
    done = job.get("done_file")
    code, msg = 1, ""
    global _PROGRESS
    _PROGRESS = open(os.path.join(job["out"], "progress.log"), "w")
    try:
        _progress("opening")
        provider, shutdown = open_rdc(job["rdc"], None, in_qrenderdoc=True)
        _progress("opened")
        try:
            rdlib.process(provider, job["out"], job["scene"], job["capture"],
                          index=_load_index(job), targets=job.get("targets", "none"))
        finally:
            shutdown()
        code = 0
    except Exception:  # report through the done file; qrenderdoc has no exit code
        msg = traceback.format_exc()
        code = 2
    if done:
        with open(done + ".tmp", "w") as f:
            f.write("%d\n%s" % (code, msg))
        os.replace(done + ".tmp", done)
    os._exit(code)  # qrenderdoc has no way to quit from a script

def _load_index(job):
    if job.get("index"):
        with open(job["index"], "r", encoding="utf-8") as f:
            return json.load(f)
    return {}


if "pyrenderdoc" in globals():
    _qrenderdoc_main()
elif __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

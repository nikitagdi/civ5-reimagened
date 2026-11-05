"""D3DDisassemble through the system d3dcompiler_47.dll.

Windows only. Python stdlib only (ctypes). Run isolated:

    python -I d3d_disasm.py <out_dir> <block.bin | dir of .bin> ...

For every block `<name>.bin` the Microsoft disassembly is written to `<out_dir>/<name>.txt`
(UTF-8, as returned by the DLL, no changes). Prints one line per failure and a summary;
exit code 1 when any block failed. Nothing is installed; the DLL is the one in System32.
"""

import ctypes
import os
import sys


def load_compiler():
    if sys.platform != "win32":
        raise SystemExit("d3d_disasm.py: Windows only (needs d3dcompiler_47.dll)")
    path = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "d3dcompiler_47.dll")
    if not os.path.exists(path):
        raise SystemExit("d3d_disasm.py: %s not found" % path)
    dll = ctypes.WinDLL(path)
    fn = dll.D3DDisassemble
    # HRESULT D3DDisassemble(LPCVOID pSrcData, SIZE_T SrcDataSize, UINT Flags,
    #                        LPCSTR szComments, ID3DBlob **ppDisassembly)
    fn.argtypes = [
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_uint,
        ctypes.c_char_p,
        ctypes.POINTER(ctypes.c_void_p),
    ]
    fn.restype = ctypes.c_long
    return fn


# ID3DBlob vtable: QueryInterface, AddRef, Release, GetBufferPointer, GetBufferSize.
_VT_RELEASE = 2
_VT_PTR = 3
_VT_SIZE = 4


def _vcall(obj, index, restype, *argtypes):
    vtable = ctypes.cast(obj, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    proto = ctypes.WINFUNCTYPE(restype, ctypes.c_void_p, *argtypes)
    return proto(vtable[index])(obj)


def disassemble(fn, data):
    """Text of one block, or raises RuntimeError with the HRESULT."""
    buf = ctypes.create_string_buffer(data, len(data))
    blob = ctypes.c_void_p()
    hr = fn(buf, len(data), 0, None, ctypes.byref(blob))
    if hr < 0 or not blob.value:
        raise RuntimeError("D3DDisassemble failed: HRESULT 0x%08X" % (hr & 0xFFFFFFFF))
    try:
        ptr = _vcall(blob, _VT_PTR, ctypes.c_void_p)
        size = _vcall(blob, _VT_SIZE, ctypes.c_size_t)
        return ctypes.string_at(ptr, size)
    finally:
        _vcall(blob, _VT_RELEASE, ctypes.c_ulong)


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    out_dir = argv[1]
    files = []
    for a in argv[2:]:
        if os.path.isdir(a):
            files += sorted(os.path.join(a, n) for n in os.listdir(a) if n.endswith(".bin"))
        else:
            files.append(a)
    os.makedirs(out_dir, exist_ok=True)
    fn = load_compiler()
    failed = 0
    for f in files:
        name = os.path.splitext(os.path.basename(f))[0]
        with open(f, "rb") as fh:
            data = fh.read()
        try:
            text = disassemble(fn, data)
        except RuntimeError as e:
            failed += 1
            print("%s: %s" % (name, e))
            continue
        # Drop the trailing NUL the blob carries.
        text = text.rstrip(b"\0")
        with open(os.path.join(out_dir, name + ".txt"), "wb") as fh:
            fh.write(text)
    print("d3d_disasm: %d block(s), %d failed" % (len(files), failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

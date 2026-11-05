"""Self-test of the helpers of facts.py and rdlib.py (stdlib only).

    python -I selftest_facts.py

Exits 0 and prints "ok" when every assert passes; otherwise the assert raises.
"""

import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import struct

import facts, rdlib


def check_half():
    assert facts.half(0x3C00) == 1.0
    assert facts.half(0x4000) == 2.0
    assert facts.half(0xC000) == -2.0
    assert facts.half(0) == 0.0


def check_g():
    assert facts.g(1e-12) == "0"
    assert facts.g([1.0, 0.25]) == "[1, 0.25]"


def check_finite():
    assert rdlib._finite({"a": [1.0, float("nan"), float("inf")]}) == {"a": [1.0, None, None]}


def check_channel_stats():
    rgba = {"dxgi": 28, "width": 2, "height": 1, "data": bytes([0, 255, 0, 255, 255, 255, 0, 255])}
    s = facts.channel_stats(rgba)[0]
    assert s["min"] == 0.0, s
    assert s["max"] == 1.0, s
    assert s["mean"] == 0.5, s
    assert s["nonzero"] == 1, s
    assert s["count"] == 2, s

    r16 = {"dxgi": 56, "width": 2, "height": 1, "data": struct.pack("<2H", 0, 65535)}
    s = facts.channel_stats(r16)[0]
    assert s["max"] == 1.0, s


def check_lut_variants():
    tga = {"width": 1024, "height": 33, "bpp": 32, "n": 4, "desc": 8,
           "data": bytes(range(256)) * (1024 * 33 * 4 // 256)}
    v = facts.lut_variants(tga)
    assert set(v) == {"file_order/bgra", "file_order/rgba", "top_down/bgra", "top_down/rgba"}, sorted(v)
    for k, vol in v.items():
        assert len(vol) == 32 * 32 * 32 * 4, (k, len(vol))
    d = tga["data"]
    assert v["file_order/bgra"][0:4] == d[0:4]
    assert v["file_order/rgba"][0:4] == bytes((d[2], d[1], d[0], d[3]))


if __name__ == "__main__":
    check_half()
    check_g()
    check_finite()
    check_channel_stats()
    check_lut_variants()
    print("ok")

# tools/granny: third-party code

The `granny` crate is licensed `MIT AND Apache-2.0 AND BSL-1.0` (its `Cargo.toml`): the project's
own code is MIT ([LICENSE-MIT](../LICENSE-MIT)); the two files below contain ported code under the
terms named for each. The license texts are in `tools/granny/`:
[LICENSE-APACHE](../tools/granny/LICENSE-APACHE) (Apache License 2.0) and
[LICENSE-BOOST](../tools/granny/LICENSE-BOOST) (Boost Software License 1.0).

nwn2mdk (<https://github.com/Arbos/nwn2mdk>, HEAD `b6e6705`) is distributed under the Apache
License 2.0 and has no NOTICE file. Its `gr2.cpp` carries no header of its own.

## Oodle1 decompressor

`tools/granny/src/oodle1.rs` is a Rust port of `nwn2mdk-lib/gr2_decompress.cpp` from nwn2mdk.
The file header states:

> Derived from https://github.com/berenm/xoreos-tools/blob/wip/granny-decoder/src/decompress.cpp
> Distributed under the Boost Software License, Version 1.0.

The upstream file the header names (berenm/xoreos-tools, branch `wip/granny-decoder`) is no
longer publicly available, so the Boost-1.0 terms are stated here as the nwn2mdk file header
gives them; they could not be checked against the original source. The port is covered by both
the Boost terms of the file and the Apache-2.0 terms of the repository.

Changes made by the civ5-reimagened authors: rewritten in Rust; every read, index and division is
checked, so corrupt input yields an error.

## Curve decoding

- `tools/granny/src/curve.rs`, formats 0-11 (Da/D3/D4n quantised and constant curves, smallest-three
  quaternion tables and decoding): ported from `nwn2mdk-lib/gr2.cpp` in nwn2mdk (Apache-2.0).
  Changes made by the civ5-reimagened authors: rewritten in Rust, decoded to knots and f32 controls
  with checked reads.
- Formats 12-18 (`D9I1K16uC16u`, `D9I3K16uC16u`, `D9I1K8uC8u`, `D9I3K8uC8u`, `D3I1K32fC32f`,
  `D3I1K16uC16u`, `D3I1K8uC8u`): field layouts and dequantisation follow
  [LSLib](https://github.com/Norbyte/lslib) `LSLib/Granny/Model/CurveData/*.cs` (MIT), read via
  its published source; no code copied.
- The clamped B-spline knot vector (`knot_vector`) and quaternion hemisphere continuity were
  derived from the Civ5 data (stored knots repeat the first value `degree` times) and checked
  by the unit tests; they are not taken from the Granny SDK.

## Reference material (not copied)

- The container, fixup and type-tree logic was written by this project's author, after an earlier
  decoder by the same author that is not published.
- opengr2 (MPL-2.0) `unoodle.dll` was used only as an out-of-repo test oracle.

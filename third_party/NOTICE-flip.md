Third-party code and algorithms used in this repository (see also third_party/NOTICE-*.md).

--------------------------------------------------------------------------------
ꟻLIP (tools/golden/src/metrics/flip.rs)

The LDR-FLIP error metric is a Rust port of the reference implementation
https://github.com/NVlabs/flip (src/pytorch/flip_loss.py and the C++/Python versions).

Copyright (c) 2020-2024, NVIDIA CORPORATION & AFFILIATES. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

--------------------------------------------------------------------------------
flip-evaluator 1.7 (tools/playtest/m5/flip.py, golden scenes of the original UI)

The golden comparison of the original UI calls NVIDIA's own Python package, installed by the user of the tool (pip install flip-evaluator==1.7,
PyPI; BSD 3-Clause, Copyright (c) 2020-2024 NVIDIA CORPORATION & AFFILIATES, the same licence as above; source https://github.com/NVlabs/flip).
Nothing of it is copied into this repository. Our two FLIPs are separate: the Rust port above (golden tool, renders of the map) and this package
(frames of the original UI against the original game's screenshots).

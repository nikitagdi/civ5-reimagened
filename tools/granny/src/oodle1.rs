//! Oodle1 section decompression (Granny compression type 2).
// SPDX-License-Identifier: BSL-1.0 AND Apache-2.0
// Ported from nwn2mdk-lib/gr2_decompress.cpp of nwn2mdk (github.com/Arbos/nwn2mdk, Apache-2.0; the file
// states the Boost Software License 1.0, see tools/granny/LICENSE-BOOST and LICENSE-APACHE). Changed by
// the civ5-reimagened authors: rewritten in Rust, with bounds-checked reads and errors instead of
// undefined behaviour on corrupt input. See third_party/NOTICE-granny.md.
//
// An adaptive range coder feeds three LZ dictionaries, one per section part (32-bit, 16-bit and
// 8-bit aligned data). Integer widths and wrapping arithmetic mirror the original so output stays
// byte-exact; every index and division is checked so corrupt input yields an error.

use crate::GrannyError;

type Result<T> = std::result::Result<T, GrannyError>;

const PARAMS_SIZE: usize = 12;
/// Reading this far past the end means the stream is corrupt (valid streams stay within padding).
const MAX_OVERRUN: usize = 16;

fn err(reason: &'static str) -> GrannyError {
    GrannyError::Oodle1(reason)
}

struct Decoder<'a> {
    numer: u32,
    denom: u32,
    next_denom: u32,
    src: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self {
            numer: u32::from(src.first().copied().unwrap_or(0) >> 1),
            denom: 0x80,
            next_denom: 0,
            src,
            pos: 0,
        }
    }

    fn byte(&self, i: usize) -> u32 {
        u32::from(self.src.get(i).copied().unwrap_or(0))
    }

    fn decode(&mut self, max: u16) -> Result<u16> {
        if max == 0 {
            return Err(err("zero-sized symbol range"));
        }
        while self.denom <= 0x80_0000 {
            self.numer <<= 8;
            self.numer |= (self.byte(self.pos) << 7) & 0x80;
            self.numer |= (self.byte(self.pos + 1) >> 1) & 0x7f;
            self.pos += 1;
            self.denom <<= 8;
        }
        if self.pos > self.src.len() + MAX_OVERRUN {
            return Err(err("compressed stream overrun"));
        }
        self.next_denom = self.denom / u32::from(max);
        if self.next_denom == 0 {
            return Err(err("range underflow"));
        }
        Ok((self.numer / self.next_denom).min(u32::from(max) - 1) as u16)
    }

    fn commit(&mut self, max: u16, val: u16, err_: u16) -> u16 {
        self.numer = self
            .numer
            .wrapping_sub(self.next_denom.wrapping_mul(u32::from(val)));
        if u32::from(val) + u32::from(err_) < u32::from(max) {
            self.denom = self.next_denom.wrapping_mul(u32::from(err_));
        } else {
            self.denom = self
                .denom
                .wrapping_sub(self.next_denom.wrapping_mul(u32::from(val)));
        }
        val
    }

    fn decode_and_commit(&mut self, max: u16) -> Result<u16> {
        let v = self.decode(max)?;
        Ok(self.commit(max, v, 1))
    }
}

/// Adaptive frequency model over a growing set of symbols; index 0 is the escape for new symbols.
struct WeighWindow {
    count_cap: u16,
    ranges: Vec<u16>,
    values: Vec<u16>,
    weights: Vec<u16>,
    weight_total: u16,
    thresh_increase: u16,
    thresh_increase_cap: u16,
    thresh_range_rebuild: u16,
    thresh_weight_rebuild: u16,
}

enum Symbol {
    Known(u16),
    /// A new symbol: the caller decodes its raw value and stores it with `WeighWindow::set_last`.
    New,
}

impl WeighWindow {
    fn new(max_value: u32, count_cap: u16) -> Self {
        let thresh_weight_rebuild = max_value.wrapping_mul(32).clamp(256, 15160);
        let thresh_increase_cap = if max_value > 64 {
            max_value
                .wrapping_mul(2)
                .min(thresh_weight_rebuild / 2 - 32)
        } else {
            128
        };
        Self {
            count_cap: count_cap.wrapping_add(1),
            ranges: vec![0, 0x4000],
            values: vec![0],
            weights: vec![4],
            weight_total: 4,
            thresh_increase: 4,
            thresh_increase_cap: thresh_increase_cap as u16,
            thresh_range_rebuild: 8,
            thresh_weight_rebuild: thresh_weight_rebuild as u16,
        }
    }

    fn rebuild_ranges(&mut self) -> Result<()> {
        if self.weight_total == 0 {
            return Err(err("empty weight table"));
        }
        self.ranges.resize(self.weights.len(), 0);
        let range_weight = 8 * 0x4000 / u32::from(self.weight_total);
        let mut start: u32 = 0;
        for (r, &w) in self.ranges.iter_mut().zip(&self.weights) {
            *r = start as u16;
            start = start.wrapping_add(u32::from(w).wrapping_mul(range_weight) / 8);
        }
        self.ranges.push(0x4000);
        if self.thresh_increase > self.thresh_increase_cap / 2 {
            self.thresh_range_rebuild = self.weight_total.wrapping_add(self.thresh_increase_cap);
        } else {
            self.thresh_increase = self.thresh_increase.wrapping_mul(2);
            self.thresh_range_rebuild = self.weight_total.wrapping_add(self.thresh_increase);
        }
        Ok(())
    }

    fn rebuild_weights(&mut self) {
        for w in &mut self.weights {
            *w /= 2;
        }
        self.weight_total = self.weights.iter().fold(0u32, |a, &w| a + u32::from(w)) as u16;
        let mut i = 1;
        while i < self.weights.len() {
            while i < self.weights.len() && self.weights[i] == 0 {
                self.weights.swap_remove(i);
                self.values.swap_remove(i);
            }
            i += 1;
        }
        // First maximum after the escape slot moves to the back.
        if self.weights.len() > 1 {
            let mut best = 1;
            for j in 2..self.weights.len() {
                if self.weights[j] > self.weights[best] {
                    best = j;
                }
            }
            let last = self.weights.len() - 1;
            self.weights.swap(best, last);
            self.values.swap(best, last);
        }
        if self.weights.len() < usize::from(self.count_cap) && self.weights[0] == 0 {
            self.weights[0] = 1;
            self.weight_total = self.weight_total.wrapping_add(1);
        }
    }

    fn try_decode(&mut self, dec: &mut Decoder) -> Result<Symbol> {
        if self.weight_total >= self.thresh_range_rebuild {
            if self.thresh_range_rebuild >= self.thresh_weight_rebuild {
                self.rebuild_weights();
            }
            self.rebuild_ranges()?;
        }
        let value = dec.decode(0x4000)?;
        let index = self
            .ranges
            .partition_point(|&r| r <= value)
            .checked_sub(1)
            .ok_or(err("range lookup"))?;
        let lo = *self.ranges.get(index).ok_or(err("range lookup"))?;
        let hi = *self.ranges.get(index + 1).ok_or(err("range lookup"))?;
        dec.commit(0x4000, lo, hi.wrapping_sub(lo));

        let w = self.weights.get_mut(index).ok_or(err("weight index"))?;
        *w = w.wrapping_add(1);
        self.weight_total = self.weight_total.wrapping_add(1);
        if index > 0 {
            return self
                .values
                .get(index)
                .map(|&v| Symbol::Known(v))
                .ok_or(err("value index"));
        }
        if self.weights.len() >= self.ranges.len() && dec.decode_and_commit(2)? == 1 {
            let extra = (self.weights.len() - self.ranges.len() + 1) as u16;
            let index = self.ranges.len() + usize::from(dec.decode_and_commit(extra)?) - 1;
            let w = self.weights.get_mut(index).ok_or(err("weight index"))?;
            *w = w.wrapping_add(2);
            self.weight_total = self.weight_total.wrapping_add(2);
            return self
                .values
                .get(index)
                .map(|&v| Symbol::Known(v))
                .ok_or(err("value index"));
        }
        self.values.push(0);
        self.weights.push(2);
        self.weight_total = self.weight_total.wrapping_add(2);
        if self.weights.len() == usize::from(self.count_cap) {
            self.weight_total = self.weight_total.wrapping_sub(self.weights[0]);
            self.weights[0] = 0;
        }
        Ok(Symbol::New)
    }

    /// Decodes a symbol, reading a raw value of range `max` for a new one.
    fn decode(&mut self, dec: &mut Decoder, max: u16) -> Result<u16> {
        match self.try_decode(dec)? {
            Symbol::Known(v) => Ok(v),
            Symbol::New => {
                let v = dec.decode_and_commit(max)?;
                if let Some(last) = self.values.last_mut() {
                    *last = v;
                }
                Ok(v)
            }
        }
    }
}

struct Params {
    decoded_value_max: u32,
    backref_value_max: u32,
    decoded_count: u32,
    highbit_count: u32,
    sizes_count: [u8; 4],
}

impl Params {
    fn read(b: &[u8]) -> Self {
        let w = |k: usize| crate::bytes::le_u32(b, k * 4);
        let (w0, w1) = (w(0), w(1));
        Self {
            decoded_value_max: w0 & 0x1ff,
            backref_value_max: w0 >> 9,
            decoded_count: w1 & 0x1ff,
            highbit_count: w1 >> 19,
            sizes_count: crate::bytes::le(b, 8),
        }
    }
}

struct Dictionary {
    decoded_size: u32,
    backref_size: u16,
    decoded_value_max: u32,
    backref_value_max: u32,
    lowbit_value_max: u32,
    lowbit_window: WeighWindow,
    highbit_window: WeighWindow,
    midbit_windows: Vec<WeighWindow>,
    decoded_windows: Vec<WeighWindow>,
    size_windows: Vec<WeighWindow>,
}

const LONG_BACKREF: [u32; 4] = [128, 192, 256, 512];

impl Dictionary {
    fn new(p: &Params) -> Self {
        let lowbit_value_max = (p.backref_value_max + 1).min(4);
        let midbit_value_max = (p.backref_value_max / 4 + 1).min(256);
        let highbit_value_max = p.backref_value_max / 1024 + 1;
        let midbit_windows = (0..highbit_value_max)
            .map(|_| WeighWindow::new(midbit_value_max - 1, midbit_value_max as u16))
            .collect();
        let decoded_windows = (0..4)
            .map(|_| WeighWindow::new(p.decoded_value_max.wrapping_sub(1), p.decoded_count as u16))
            .collect();
        let mut size_windows = Vec::with_capacity(65);
        for i in 0..4 {
            for _ in 0..16 {
                size_windows.push(WeighWindow::new(64, u16::from(p.sizes_count[3 - i])));
            }
        }
        size_windows.push(WeighWindow::new(64, u16::from(p.sizes_count[0])));
        Self {
            decoded_size: 0,
            backref_size: 0,
            decoded_value_max: p.decoded_value_max,
            backref_value_max: p.backref_value_max,
            lowbit_value_max,
            lowbit_window: WeighWindow::new(lowbit_value_max - 1, lowbit_value_max as u16),
            highbit_window: WeighWindow::new(highbit_value_max - 1, (p.highbit_count + 1) as u16),
            midbit_windows,
            decoded_windows,
            size_windows,
        }
    }

    /// Decodes one literal or back-reference at `out[pos..]`; returns the number of bytes written.
    fn block(&mut self, dec: &mut Decoder, out: &mut [u8], pos: usize) -> Result<usize> {
        let window = self
            .size_windows
            .get_mut(usize::from(self.backref_size))
            .ok_or(err("length state"))?;
        let code = window.decode(dec, 65)?;
        self.backref_size = code;

        if code == 0 {
            let w = &mut self.decoded_windows[pos % 4];
            let v = w.decode(dec, self.decoded_value_max as u16)?;
            *out.get_mut(pos).ok_or(err("literal past end"))? = v as u8;
            self.decoded_size = self.decoded_size.wrapping_add(1);
            return Ok(1);
        }

        let len = if code < 61 {
            u32::from(code) + 1
        } else {
            *LONG_BACKREF
                .get(usize::from(code) - 61)
                .ok_or(err("bad length code"))?
        };
        let range = self.backref_value_max.min(self.decoded_size);
        let low = self
            .lowbit_window
            .decode(dec, self.lowbit_value_max as u16)?;
        let high = self.highbit_window.decode(dec, (range / 1024 + 1) as u16)?;
        let mid = self
            .midbit_windows
            .get_mut(usize::from(high))
            .ok_or(err("offset state"))?
            .decode(dec, (range / 4 + 1).min(256) as u16)?;
        let offset = (usize::from(high) << 10) + (usize::from(mid) << 2) + usize::from(low) + 1;
        self.decoded_size = self.decoded_size.wrapping_add(len);

        let len = len as usize;
        let end = pos.checked_add(len).ok_or(err("backref past end"))?;
        if end > out.len() {
            return Err(err("backref past end"));
        }
        let start = pos.checked_sub(offset).ok_or(err("backref before start"))?;
        // Overlapping copy: repeats the last `offset` bytes, as the original memcpy loop does.
        for i in 0..len {
            out[pos + i] = out[start + i];
        }
        Ok(len)
    }
}

/// Decompresses one Oodle1 section. `stop0`/`stop1` are the section's first-16-bit and
/// first-8-bit offsets, which end the first two of the three dictionaries.
pub fn decompress(src: &[u8], stop0: u32, stop1: u32, out_len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; out_len];
    if src.is_empty() {
        return Ok(out);
    }
    if src.len() < 3 * PARAMS_SIZE {
        return Err(err("stream shorter than its parameter block"));
    }
    let params: [Params; 3] = std::array::from_fn(|i| Params::read(&src[i * PARAMS_SIZE..]));
    let mut dec = Decoder::new(&src[3 * PARAMS_SIZE..]);
    let stops = [stop0 as usize, stop1 as usize, out_len];
    let mut pos = 0usize;
    for (p, &stop) in params.iter().zip(&stops) {
        if stop > out_len {
            return Err(err("stop offset past section end"));
        }
        let mut dict = Dictionary::new(p);
        while pos < stop {
            pos += dict.block(&mut dec, &mut out, pos)?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parameter blocks plus masked xorshift noise. Most such streams contain invalid
    /// back-references; the seeds below decode cleanly, and their CRCs come from the reference
    /// decoder (opengr2 `unoodle.dll`) run on the same input outside the repo.
    fn synthetic(seed: u32, window: u32, mask: u8) -> Vec<u8> {
        let mut p = (256 | (window << 9)).to_le_bytes().to_vec();
        p.extend((255u32 | (3 << 19)).to_le_bytes());
        p.extend([64; 4]);
        let mut src = p.repeat(3);
        let mut s = seed;
        for _ in 0..200 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            src.push(s as u8 & mask);
        }
        src
    }

    #[test]
    fn matches_reference_decoder_on_synthetic_streams() {
        for (seed, window, crc) in [
            (85, 4, 0x3b86_21b8),
            (255, 64, 0x5582_c542),
            (3, 1024, 0x4b45_e00b),
            (185, 1024, 0xbc74_7b25),
            (274, 1024, 0x8b1f_e0fe),
        ] {
            let out = decompress(&synthetic(seed, window, 0x0f), 150, 400, 600).unwrap();
            assert_eq!(crate::crc32(&out), crc, "seed {seed}");
        }
    }

    #[test]
    fn empty_input_gives_zeroed_output() {
        assert_eq!(decompress(&[], 0, 0, 4).unwrap(), vec![0; 4]);
    }

    #[test]
    fn short_input_is_an_error() {
        assert!(decompress(&[1, 2, 3], 0, 0, 4).is_err());
    }

    #[test]
    fn stops_past_end_are_an_error() {
        assert!(decompress(&[0u8; 64], 100, 100, 4).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 0x1234_5678u32;
        for n in 0..200 {
            let src: Vec<u8> = (0..36 + n)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = decompress(&src, (n % 7) as u32, (n % 13) as u32, 64 + n);
        }
    }
}

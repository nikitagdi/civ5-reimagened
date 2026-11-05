//! Random inputs of the harness: constants, vertex and pixel inputs and
//! texture contents. Everything is a pure function of `(seed, name)`, so the oracle and the hand
//! module get the same numbers for the same name without sharing any state.

use crate::oracle::regs::INT_REGS;

/// splitmix64: small, fast, good enough for test data.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// A generator for `(seed, key)`: the same pair always gives the same stream.
    pub fn keyed(seed: u64, key: &str) -> Rng {
        // FNV-1a over the key, mixed with the seed.
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in key.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut r = Rng(h ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n.max(1))) as u32
    }
}

/// The special values mixed into the constants (4.8): 0, ±1, 1e-8, 1e8.
pub const SPECIAL_VALUES: [f32; 5] = [0.0, 1.0, -1.0, 1e-8, 1e8];
/// One component in `SPECIAL_EVERY` is a special value.
const SPECIAL_EVERY: u32 = 8;
/// Largest random loop count (4.8: "repeat counts 0..8").
pub const MAX_REP: i32 = 8;

/// The register file both sides are filled from (SR-D5 shapes).
#[derive(Clone, Debug, PartialEq)]
pub struct Registers {
    pub c: Vec<[f32; 4]>,
    pub i: [[i32; 4]; INT_REGS as usize],
    pub b: u32,
}

impl Registers {
    /// `float_regs` random float registers: uniform in [-2, 2], one component in eight a special
    /// value. Integers are in `0..=8` (they are loop counts), booleans random bits.
    pub fn random(seed: u64, float_regs: usize) -> Registers {
        let mut rng = Rng::keyed(seed, "registers");
        let c = (0..float_regs)
            .map(|_| {
                let mut q = [0f32; 4];
                for x in &mut q {
                    *x = if rng.below(SPECIAL_EVERY) == 0 {
                        SPECIAL_VALUES[rng.below(SPECIAL_VALUES.len() as u32) as usize]
                    } else {
                        rng.range(-2.0, 2.0)
                    };
                }
                q
            })
            .collect();
        let mut i = [[0i32; 4]; INT_REGS as usize];
        for q in &mut i {
            for x in q {
                *x = rng.below(MAX_REP as u32 + 1) as i32;
            }
        }
        Registers {
            c,
            i,
            b: rng.next_u32(),
        }
    }

    /// Sets `count` float registers from `reg` to zero (`zero_in_compare`).
    pub fn zero(&mut self, reg: usize, count: usize) {
        for q in self.c.iter_mut().skip(reg).take(count) {
            *q = [0.0; 4];
        }
    }
}

/// `n` random `vec4`s for the input called `name` (a pixel or a vertex each): mostly in [0, 1)
/// (coordinates and colours), a third of the components in [-2, 2).
pub fn input_slot(seed: u64, name: &str, n: usize) -> Vec<[f32; 4]> {
    let mut rng = Rng::keyed(seed, &format!("input:{name}"));
    (0..n)
        .map(|_| {
            let mut q = [0f32; 4];
            for x in &mut q {
                *x = if rng.below(3) == 0 {
                    rng.range(-2.0, 2.0)
                } else {
                    rng.unit()
                };
            }
            q
        })
        .collect()
}

/// As [`input_slot`], for an input limited to `min..=max` (status.toml `input_ranges`): every
/// component uniform in that range, an integer one time in two (indices are integers).
pub fn input_slot_in(seed: u64, name: &str, n: usize, min: i32, max: i32) -> Vec<[f32; 4]> {
    let mut rng = Rng::keyed(seed, &format!("input:{name}"));
    (0..n)
        .map(|_| {
            let mut q = [0f32; 4];
            for x in &mut q {
                *x = if rng.below(2) == 0 {
                    rng.range(min as f32, max as f32)
                } else {
                    (min + rng.below((max - min + 1).max(1) as u32) as i32) as f32
                };
            }
            q
        })
        .collect()
}

/// `vFace`: +1 or -1 per pixel.
pub fn face_slot(seed: u64, n: usize) -> Vec<[f32; 4]> {
    let mut rng = Rng::keyed(seed, "input:vface");
    (0..n)
        .map(|_| {
            let f = if rng.below(2) == 0 { -1.0 } else { 1.0 };
            [f, 0.0, 0.0, 0.0]
        })
        .collect()
}

/// f32 to IEEE half, round to nearest even (for the 16 bit textures).
pub fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let man = bits & 0x007F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (man | 0x0080_0000) >> (1 - e);
        let round = (m >> 12) & 1;
        let sticky = m & 0x0FFF != 0;
        let h = (m >> 13)
            + if round == 1 && (sticky || (m >> 13) & 1 == 1) {
                1
            } else {
                0
            };
        return sign | h as u16;
    }
    let h = ((e as u32) << 10) | (man >> 13);
    let round = (man >> 12) & 1;
    let sticky = man & 0x0FFF != 0;
    let h = h + if round == 1 && (sticky || h & 1 == 1) {
        1
    } else {
        0
    };
    sign | h as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_streams_are_stable_and_distinct() {
        assert_eq!(Rng::keyed(1, "a").next_u64(), Rng::keyed(1, "a").next_u64());
        assert_ne!(Rng::keyed(1, "a").next_u64(), Rng::keyed(2, "a").next_u64());
        assert_ne!(Rng::keyed(1, "a").next_u64(), Rng::keyed(1, "b").next_u64());
    }

    #[test]
    fn registers_cover_the_documented_ranges() {
        let r = Registers::random(7, 200);
        assert_eq!(r.c.len(), 200);
        assert!(r.i.iter().flatten().all(|v| (0..=MAX_REP).contains(v)));
        let flat: Vec<f32> = r.c.iter().flatten().copied().collect();
        assert!(flat.contains(&1e8), "a special value shows up");
        assert!(flat.iter().all(|&v| v.abs() <= 2.0 || v == 1e8));
        assert!(flat.iter().any(|&v| v < -1.5) && flat.iter().any(|&v| v > 1.5));
    }

    #[test]
    fn a_ranged_input_stays_in_its_range() {
        let v = input_slot_in(3, "v4", 4096, 0, 95);
        assert!(v.iter().flatten().all(|&x| (0.0..=95.0).contains(&x)));
        assert!(v.iter().flatten().any(|&x| x > 90.0) && v.iter().flatten().any(|&x| x < 5.0));
        // The same name and seed give the same stream.
        assert_eq!(v, input_slot_in(3, "v4", 4096, 0, 95));
    }

    #[test]
    fn half_floats_round_to_nearest() {
        assert_eq!(f32_to_f16(0.0), 0);
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f32_to_f16(-2.0), 0xC000);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(65504.0), 0x7BFF);
        assert_eq!(f32_to_f16(1e9), 0x7C00);
        // 1 + 2^-11 is the midpoint between two halves: ties to even (1.0).
        assert_eq!(f32_to_f16(1.0 + 1.0 / 2048.0), 0x3C00);
        assert_eq!(f32_to_f16(1.0 + 3.0 / 2048.0), 0x3C02);
    }
}

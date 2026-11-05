//! The random streams of the terrain build.
//!
//! `TerrainSystem::Init` calls `srand(seed)` first and the stages of the build draw from one stream
//! of the C runtime's `rand` in the order of the stages. The pieces are chosen with
//! `std::random_shuffle`, which the library of Visual Studio 2008 builds from the same `rand`.
//! So the stream and the shuffle are the original's, bit for bit; the order of the calls is the
//! order of the stages (`StageId`).
//!
//! [`FRandom`] is the other generator of `Terrain.cpp`: the jitter of the macro shadow passes.

/// Multiplier of the linear congruential generator of MSVCR90's `rand`.
// re: MSVCR90 rand(): `holdrand = holdrand * 214013 + 2531011`
pub const MSVC_RAND_MULTIPLIER: u32 = 214_013;
/// Increment of the same generator.
// re: MSVCR90 rand()
pub const MSVC_RAND_INCREMENT: u32 = 2_531_011;
/// `rand()` returns `(state >> 16) & RAND_MAX`.
// re: MSVCR90 rand(): RAND_MAX = 0x7fff
pub const RAND_MAX: u32 = 0x7fff;

/// Bits a draw of `rand()` carries: `_RANDOM_BITS` of the STL's `_Random_shuffle`.
// re: VC9 <algorithm> _Random_shuffle: `_RANDOM_BITS = 15` (from memory: to check with RW-5 question 6)
const SHUFFLE_RANDOM_BITS: u32 = 15;

/// `rand` of MSVCR90 with its state: `srand(seed)` sets the state to the seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsvcRand {
    state: u32,
    calls: u64,
}

impl MsvcRand {
    /// A stream after `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        Self {
            state: seed,
            calls: 0,
        }
    }

    /// `srand(seed)`.
    pub fn srand(&mut self, seed: u32) {
        self.state = seed;
        self.calls = 0;
    }

    /// `rand()`: a number in `0..=0x7fff`.
    // re: MSVCR90 rand()
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(MSVC_RAND_MULTIPLIER)
            .wrapping_add(MSVC_RAND_INCREMENT);
        self.calls += 1;
        (self.state >> 16) & RAND_MAX
    }

    /// Spends `n` draws, as a stage does that consumes the stream without using the numbers (the
    /// noise of the coast, RW-10).
    pub fn skip(&mut self, n: u64) {
        for _ in 0..n {
            self.next();
        }
    }

    /// Draws since `srand`.
    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// The state, for the tests of the stream.
    pub fn state(&self) -> u32 {
        self.state
    }
}

/// `std::random_shuffle(first, last)` of Visual Studio 2008 with the default engine (`rand`).
/// For `index` from 2, `next` from `first + 1`: `rn` is a draw of `rand() & 0x7fff` and, while the
/// range of the draws is smaller than `index`, 15 more bits of the next draw; then `next` swaps with
/// `first[rn % index]`. The Mac and Linux binaries the RE reads use libc++ (`__rs_get`, its own
/// engine), so the Windows code is not in civ5-re: the text is the one of the VC9 `<algorithm>`
/// (from memory, the same as the spec 4.2), and the check is S1/S2 (D-3).
// re: VC9 <algorithm> _Random_shuffle; called by FindLinearPacking 0x08a46fd6
pub fn vc9_random_shuffle<T>(items: &mut [T], rng: &mut MsvcRand) {
    for (next, index) in (1..items.len()).zip(2u32..) {
        let mut range: u32 = RAND_MAX;
        let mut rn: u32 = rng.next() & RAND_MAX;
        while range < index && range != u32::MAX {
            rn = (rn << SHUFFLE_RANDOM_BITS) | (rng.next() & RAND_MAX);
            range = (range << SHUFFLE_RANDOM_BITS) | RAND_MAX;
        }
        items.swap(next, (rn % index) as usize);
    }
}

/// Seed of the jitter generator of the macro shadow.
// re: Terrain::Init (Terrain.cpp:242)
pub const JITTER_SEED: u32 = 7;
/// Modulus of the jitter generator.
// re: Terrain::Init (Terrain.cpp:245)
pub const JITTER_MODULUS: u32 = 4_294_954_991;
/// Pairs of the table of the jitter; the pair 0 is not written by the loop of the original.
// re: Terrain::Init (Terrain.cpp:243: `for (int i = 1; i < 32; ++i)`)
pub const JITTER_PAIRS: usize = 32;

/// `FRandom`: `s = (s * 214013 + 2531011) mod 4294954991` in 32 bits, the draw is `s / 4294954991`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FRandom {
    state: u32,
}

impl FRandom {
    /// The generator at its seed (7).
    pub fn new() -> Self {
        Self { state: JITTER_SEED }
    }

    /// The next state of the stream, as the original's `uiSeed`.
    // re: Terrain::Init (Terrain.cpp:245)
    pub fn step(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(MSVC_RAND_MULTIPLIER)
            .wrapping_add(MSVC_RAND_INCREMENT)
            % JITTER_MODULUS;
        self.state
    }

    /// The next offset of a sample: `s / 4294954991 - 0.5`, in `[-0.5, 0.5)`.
    // re: Terrain::Init (Terrain.cpp:246)
    pub fn next_offset(&mut self) -> f32 {
        let s = self.step();
        (f64::from(s) * (1.0 / f64::from(JITTER_MODULUS))) as f32 - 0.5
    }
}

impl Default for FRandom {
    fn default() -> Self {
        Self::new()
    }
}

/// The table of the sub-texel offsets of the shadow passes: pair `i` for `i` in `1..32`, two
/// draws each in the order `j = 0, 1`; pair 0 is zero.
// re: Terrain::Init (Terrain.cpp:239-248, `m_afSampleJitter`)
pub fn sample_jitter() -> [[f32; 2]; JITTER_PAIRS] {
    let mut table = [[0.0; 2]; JITTER_PAIRS];
    let mut rng = FRandom::new();
    for pair in table.iter_mut().skip(1) {
        for v in pair.iter_mut() {
            *v = rng.next_offset();
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first draws of the C runtime after `srand(1)`: 41, 18467, 6334, 26500, 19169, ... The
    /// sequence is the one that every sample of `rand()` with the default seed prints (computed
    /// here from the formula of MSVCR90, no capture of the original).
    #[test]
    fn rand_after_srand_1_gives_the_default_sequence_of_the_crt() {
        let mut r = MsvcRand::new(1);
        let got: Vec<u32> = (0..10).map(|_| r.next()).collect();
        assert_eq!(
            got,
            [
                41, 18467, 6334, 26500, 19169, 15724, 11478, 29358, 26962, 24464
            ]
        );
        assert_eq!(r.calls(), 10);
    }

    #[test]
    fn rand_after_srand_12345_starts_with_the_hand_computed_draw() {
        // 12345 * 214013 + 2531011 = 2644521496; >> 16 = 40352; & 0x7fff = 7584.
        let mut r = MsvcRand::new(12345);
        assert_eq!(r.next(), 7584);
        // A second srand restarts the stream and the count.
        r.srand(1);
        assert_eq!((r.next(), r.calls()), (41, 1));
    }

    #[test]
    fn rand_is_a_function_of_the_seed_only() {
        let run = |seed| {
            let mut r = MsvcRand::new(seed);
            (0..100).map(|_| r.next()).collect::<Vec<_>>()
        };
        assert_eq!(run(77), run(77));
        assert_ne!(run(77), run(78));
        assert!(run(5).iter().all(|&v| v <= RAND_MAX));
    }

    #[test]
    fn skip_spends_draws_like_next() {
        let mut a = MsvcRand::new(9);
        let mut b = MsvcRand::new(9);
        a.skip(17);
        for _ in 0..17 {
            b.next();
        }
        assert_eq!(a, b);
    }

    #[test]
    fn shuffle_of_four_follows_the_hand_computed_draws() {
        // srand(1): 41, 18467, 6334. index 2: 41 % 2 = 1 (swap a[1] with itself); index 3:
        // 18467 % 3 = 2 (a[2] with itself); index 4: 6334 % 4 = 2 (a[3] swaps with a[2]).
        let mut v = [1, 2, 3, 4];
        let mut r = MsvcRand::new(1);
        vc9_random_shuffle(&mut v, &mut r);
        assert_eq!(v, [1, 2, 4, 3]);
        assert_eq!(r.calls(), 3);
    }

    #[test]
    fn shuffle_of_one_to_ten_is_a_permutation_and_repeats() {
        let run = || {
            let mut v: Vec<u32> = (1..=10).collect();
            vc9_random_shuffle(&mut v, &mut MsvcRand::new(12345));
            v
        };
        let a = run();
        assert_eq!(a, run());
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (1..=10).collect::<Vec<u32>>());
        assert_ne!(a, (1..=10).collect::<Vec<u32>>());
    }

    #[test]
    fn shuffle_takes_two_draws_when_the_index_passes_the_range() {
        // Indices 2..=32767 take one draw, 32768.. take two (the range of one draw is 0x7fff).
        let mut v: Vec<u32> = (0..40_000).collect();
        let mut r = MsvcRand::new(3);
        vc9_random_shuffle(&mut v, &mut r);
        assert_eq!(r.calls(), 32_766 + 2 * 7_233);
    }

    #[test]
    fn shuffle_of_nothing_or_one_does_not_draw() {
        let mut r = MsvcRand::new(1);
        vc9_random_shuffle::<u8>(&mut [], &mut r);
        vc9_random_shuffle(&mut [5], &mut r);
        assert_eq!(r.calls(), 0);
    }

    #[test]
    fn jitter_table_is_the_one_of_terrain_cpp() {
        let t = sample_jitter();
        assert_eq!(t[0], [0.0, 0.0]);
        // s1 = 7 * 214013 + 2531011 = 4029102 (below the modulus).
        let s1 = 7u64 * 214_013 + 2_531_011;
        assert_eq!(s1, 4_029_102);
        let want = (s1 as f64 / 4_294_954_991.0) as f32 - 0.5;
        assert_eq!(t[1][0], want);
        // Every value is in [-0.5, 0.5) and the pairs are all different.
        for p in &t[1..] {
            assert!(p.iter().all(|v| (-0.5..0.5).contains(v)), "{p:?}");
        }
        for i in 1..JITTER_PAIRS {
            for j in (i + 1)..JITTER_PAIRS {
                assert_ne!(t[i], t[j]);
            }
        }
    }

    #[test]
    fn frandom_reduces_the_state_by_the_modulus() {
        let mut r = FRandom::new();
        let mut s: u32 = JITTER_SEED;
        for _ in 0..200 {
            s = s.wrapping_mul(214_013).wrapping_add(2_531_011) % JITTER_MODULUS;
            assert_eq!(r.step(), s);
        }
    }
}

//! The C library random numbers of the original (the Windows build): `rand`/`srand` of the MSVC
//! runtime and `std::random_shuffle` of the MSVC STL. The city layout draws its model picks, its
//! burn order and the order of the wonder slots from them.
//!
//! re: `srand_wincompatible` / `rand_wincompatible` (the Linux port's copy of the MSVC LCG) and
//! `cvCityBuildingSim::FindWonderPlacementSlot` (0x087b8988), which seeds `srand(y * 0xff + x)`.

/// The state of the MSVC `rand`: `state * 214013 + 2531011`, the bits 16..30 out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsvcRand {
    state: u32,
}

impl MsvcRand {
    /// `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// `rand()`: 0..=0x7fff.
    pub fn rand(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.state >> 16) & 0x7fff
    }

    /// `std::random_shuffle(first, last)` of the MSVC STL: for each index from 1, a random value
    /// of 15-bit chunks wide enough for `index + 1`, then the swap with `value % (index + 1)`.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        const BITS: u32 = 15;
        const MAX: u64 = (1 << BITS) - 1;
        for i in 1..items.len() {
            let index = i as u64 + 1;
            let mut rm = MAX;
            let mut rn = u64::from(self.rand()) & MAX;
            while rm < index && rm != u64::from(u32::MAX) {
                rn = (rn << BITS) | (u64::from(self.rand()) & MAX);
                rm = (rm << BITS) | MAX;
            }
            items.swap(i, (rn % index) as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_numbers_of_srand_1_are_those_of_the_msvc_runtime() {
        // The well-known sequence of the MSVC CRT after srand(1).
        let mut r = MsvcRand::new(1);
        let v: Vec<u32> = (0..5).map(|_| r.rand()).collect();
        assert_eq!(v, [41, 18467, 6334, 26500, 19169]);
    }

    #[test]
    fn the_shuffle_keeps_every_item_and_follows_the_seed() {
        let mut a: Vec<u32> = (0..12).collect();
        MsvcRand::new(7).shuffle(&mut a);
        let mut b: Vec<u32> = (0..12).collect();
        MsvcRand::new(7).shuffle(&mut b);
        assert_eq!(a, b);
        let mut s = a.clone();
        s.sort_unstable();
        assert_eq!(s, (0..12).collect::<Vec<_>>());
        // Two items, seed 1: rand() = 41, 41 % 2 = 1: the second is swapped with itself.
        let mut two = [0, 1];
        MsvcRand::new(1).shuffle(&mut two);
        assert_eq!(two, [0, 1]);
        // Seed 2: rand() = 45, 45 % 2 = 1 too; seed 3: 48 % 2 = 0, swapped.
        let mut two = [0, 1];
        MsvcRand::new(3).shuffle(&mut two);
        assert_eq!(two, [1, 0]);
    }
}

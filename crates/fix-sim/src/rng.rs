//! A small seeded random number generator: splitmix64. The simulator needs
//! the same numbers for the same seed on every machine.

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`. Returns 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        let wide = u128::from(self.next_u64()) * u128::from(n);
        u64::try_from(wide >> 64).unwrap_or(0)
    }

    /// An index in `0..len`. Returns 0 when `len` is 0.
    pub fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.below(len as u64)).unwrap_or(0)
    }

    /// A number in `low..=high`.
    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high.saturating_sub(low) + 1)
    }

    /// A number in `-limit..=limit`.
    pub fn signed(&mut self, limit: u64) -> i64 {
        let value = i64::try_from(self.range(0, limit * 2)).unwrap_or(0);
        value - i64::try_from(limit).unwrap_or(0)
    }

    /// True with the probability `percent` out of 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn splitmix64_matches_the_reference_sequence() {
        // The first outputs for seed 0 of the reference implementation.
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn range_stays_inside_its_bounds() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!((5..=9).contains(&rng.range(5, 9)));
            assert!((-3..=3).contains(&rng.signed(3)));
        }
    }
}

//! Deterministic seeding and the `Qrng` random generator.
//!
//! Every random decision in the Qinari audio path flows through [`Qrng`]:
//! xoshiro256++ seeded via splitmix64. Given the same seed, the same sequence
//! of values is produced on every machine, so renders are reproducible.

/// A 64-bit seed for a [`Qrng`].
///
/// Seeds are the interchange currency for determinism: recipes derive seeds
/// from text with [`Seed::from_text`] and fork generators with [`Qrng::fork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Seed(
    /// The raw 64-bit seed value.
    pub u64,
);

impl Seed {
    /// Creates a seed from a raw 64-bit value.
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Creates a seed from text using the FNV-1a 64-bit hash.
    ///
    /// FNV-1a is a fixed, machine-independent hash (unlike `DefaultHasher`,
    /// whose output is not guaranteed stable across Rust releases), so the
    /// same string always produces the same seed.
    pub fn from_text(text: &str) -> Self {
        Self(fnv1a_64(text.as_bytes()))
    }
}

/// FNV-1a 64-bit hash over `data`.
fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// One step of splitmix64; used to expand a seed into generator state.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A xoshiro256++ random generator, seeded through splitmix64.
///
/// All Qinari randomness comes from a `Qrng` (or a [`Qrng::fork`] of one);
/// no system randomness reaches the audio path. Not cryptographically
/// secure by design; it only has to be deterministic and well distributed.
#[derive(Debug, Clone)]
pub struct Qrng {
    s0: u64,
    s1: u64,
    s2: u64,
    s3: u64,
    fork_counter: u64,
}

impl Qrng {
    /// Creates a generator from a 64-bit seed.
    ///
    /// The seed is expanded into the 256-bit state with splitmix64; the
    /// resulting state can never be all-zero.
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let s0 = splitmix64(&mut sm);
        let s1 = splitmix64(&mut sm);
        let s2 = splitmix64(&mut sm);
        let s3 = splitmix64(&mut sm);
        // splitmix64 expansion cannot yield an all-zero state in practice;
        // guard explicitly anyway because xoshiro would freeze on it.
        let s0 = if s0 | s1 | s2 | s3 == 0 { 1 } else { s0 };
        Self {
            s0,
            s1,
            s2,
            s3,
            fork_counter: 0,
        }
    }

    /// Creates a generator from a [`Seed`].
    pub fn from_seed(seed: Seed) -> Self {
        Self::new(seed.0)
    }

    /// Returns the next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        let result = self
            .s0
            .wrapping_add(self.s3)
            .rotate_left(23)
            .wrapping_add(self.s0);
        let t = self.s1 << 17;
        self.s2 ^= self.s0;
        self.s3 ^= self.s1;
        self.s1 ^= self.s2;
        self.s0 ^= self.s3;
        self.s2 ^= t;
        self.s3 = self.s3.rotate_left(45);
        result
    }

    /// Returns the next value as `f32` in `[0, 1)` with 24-bit resolution.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Returns the next `f32` in `[lo, hi)`.
    ///
    /// For extremely narrow ranges floating-point rounding may return `hi`,
    /// so treat the result as `[lo, hi]` when the range is tiny.
    ///
    /// # Panics
    ///
    /// Panics if `lo >= hi`.
    pub fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        assert!(lo < hi, "next_range requires lo < hi (got {lo} >= {hi})");
        lo + self.next_f32() * (hi - lo)
    }

    /// Returns the next integer in `[lo, hi]` (both inclusive).
    ///
    /// Uses a single modulo reduction, so the distribution is very slightly
    /// biased for ranges far above 2^32; irrelevant for DSP decisions.
    ///
    /// # Panics
    ///
    /// Panics if `lo > hi`.
    pub fn next_int_range(&mut self, lo: u64, hi: u64) -> u64 {
        assert!(
            lo <= hi,
            "next_int_range requires lo <= hi (got {lo} > {hi})"
        );
        let span = hi.wrapping_sub(lo).wrapping_add(1);
        if span == 0 {
            return self.next_u64();
        }
        lo + self.next_u64() % span
    }

    /// Derives an independent child generator from this one.
    ///
    /// The child seed mixes a fork counter with the parent's current state,
    /// so repeated forks differ, and the child keeps its own state: advancing
    /// the parent afterwards never changes the child's sequence.
    #[must_use]
    pub fn fork(&mut self) -> Self {
        self.fork_counter += 1;
        let mixed = self
            .s0
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(self.fork_counter)
            ^ self.next_u64();
        Self::new(splitmix64(
            &mut (mixed.wrapping_add(self.fork_counter.rotate_left(32))),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_reference_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn seed_from_text_is_stable_and_discriminating() {
        assert_eq!(Seed::from_text("qinari"), Seed::from_text("qinari"));
        assert_ne!(Seed::from_text("qinari"), Seed::from_text("qinark"));
        assert_ne!(Seed::from_text(""), Seed::from_u64(0));
        assert_eq!(Seed::from_u64(7).0, 7);
    }

    #[test]
    fn same_seed_same_sequence() {
        let mut left = Qrng::new(1234);
        let mut right = Qrng::new(1234);
        let mut other = Qrng::new(1235);
        let seq_left: Vec<u64> = (0..64).map(|_| left.next_u64()).collect();
        let seq_right: Vec<u64> = (0..64).map(|_| right.next_u64()).collect();
        let seq_other: Vec<u64> = (0..64).map(|_| other.next_u64()).collect();
        assert_eq!(seq_left, seq_right);
        assert_ne!(seq_left, seq_other);

        let mut floats = Qrng::new(1234);
        let seq_floats: Vec<f32> = (0..64).map(|_| floats.next_f32()).collect();
        let mut replay = Qrng::new(1234);
        for expected in seq_floats {
            assert_eq!(replay.next_f32(), expected);
        }
    }

    #[test]
    fn next_f32_stays_in_unit_half_open_interval() {
        let mut rng = Qrng::new(42);
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut sum = 0.0f64;
        let n = 100_000;
        for _ in 0..n {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v));
            min = min.min(v);
            max = max.max(v);
            sum += f64::from(v);
        }
        // Uniform draws must cover the interval and average to ~0.5.
        assert!(min < 1.0e-4, "min was {min}");
        assert!(max > 0.9999, "max was {max}");
        let mean = sum / f64::from(n);
        assert!((mean - 0.5).abs() < 0.01, "mean was {mean}");
    }

    #[test]
    fn float_ranges_respect_bounds() {
        let mut rng = Qrng::new(7);
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for _ in 0..100_000 {
            let v = rng.next_range(-2.5, 3.5);
            assert!((-2.5..=3.5).contains(&v));
            min = min.min(v);
            max = max.max(v);
        }
        assert!(min < -2.4, "min was {min}");
        assert!(max > 3.4, "max was {max}");
    }

    #[test]
    fn int_range_is_inclusive_and_covers_all_values() {
        let mut rng = Qrng::new(9);
        let mut seen = [false; 5];
        for _ in 0..2_000 {
            let v = rng.next_int_range(3, 7);
            assert!((3..=7).contains(&v));
            seen[(v - 3) as usize] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "not every value in [3, 7] was drawn"
        );
        for _ in 0..10 {
            assert_eq!(rng.next_int_range(5, 5), 5);
        }
    }

    #[test]
    fn matches_reference_xoshiro256_plus_plus_vectors() {
        // Golden values produced independently from the published
        // splitmix64-seeded xoshiro256++ reference (prng.di.unimi.it).
        let mut rng = Qrng::new(42);
        assert_eq!(rng.next_u64(), 0xd076_4d4f_4476_689f);
        assert_eq!(rng.next_u64(), 0x519e_4174_576f_3791);
        assert_eq!(rng.next_u64(), 0xfbe0_7cfb_0c24_ed8c);
        assert_eq!(rng.next_u64(), 0xb37d_9f60_0cd8_35b8);

        let mut rng = Qrng::new(42);
        let floats: Vec<f32> = (0..4).map(|_| rng.next_f32()).collect();
        let expected = [0.814_305_1_f32, 0.318_821, 0.983_894_1, 0.701_135_6];
        for (got, want) in floats.iter().zip(expected) {
            assert!((got - want).abs() < 1.0e-6, "got {got}, want {want}");
        }
    }

    #[test]
    #[should_panic(expected = "next_range requires lo < hi")]
    fn next_range_panics_on_empty_range() {
        Qrng::new(1).next_range(2.0, 2.0);
    }

    #[test]
    #[should_panic(expected = "next_int_range requires lo <= hi")]
    fn next_int_range_panics_on_reversed_bounds() {
        Qrng::new(1).next_int_range(5, 3);
    }

    #[test]
    fn next_int_range_covers_the_full_u64_span() {
        let mut rng = Qrng::new(19);
        let mut previous = rng.next_int_range(0, u64::MAX);
        let mut changed = 0;
        for _ in 0..100 {
            let value = rng.next_int_range(0, u64::MAX);
            if value != previous {
                changed += 1;
            }
            previous = value;
        }
        assert!(changed > 90, "only {changed} of 100 draws differed");
    }

    #[test]
    fn fork_children_track_the_parent_seed() {
        let mut parent_one = Qrng::new(1_000);
        let mut parent_two = Qrng::new(1_000);
        let first: Vec<u64> = (0..8).map(|_| parent_one.fork().next_u64()).collect();
        let second: Vec<u64> = (0..8).map(|_| parent_two.fork().next_u64()).collect();
        assert_eq!(first, second);

        let mut parent_three = Qrng::new(2_000);
        let third: Vec<u64> = (0..8).map(|_| parent_three.fork().next_u64()).collect();
        assert_ne!(first, third);
    }

    #[test]
    fn fork_is_deterministic_and_independent() {
        let mut a = Qrng::new(100);
        let mut child_a = a.fork();
        let seq_a: Vec<u64> = (0..16).map(|_| child_a.next_u64()).collect();
        let mut b = Qrng::new(100);
        let mut child_b = b.fork();
        for expected in &seq_a {
            assert_eq!(child_b.next_u64(), *expected);
        }

        // Advancing the parent after the fork must not change the child.
        let mut c = Qrng::new(100);
        let mut child_c = c.fork();
        for _ in 0..1_000 {
            c.next_u64();
        }
        for expected in &seq_a {
            assert_eq!(child_c.next_u64(), *expected);
        }

        // Children differ from the parent's own continuation and from each other.
        let mut parent_only = Qrng::new(100);
        let parent_seq: Vec<u64> = (0..16).map(|_| parent_only.next_u64()).collect();
        assert_ne!(seq_a, parent_seq);

        let mut d = Qrng::new(100);
        let c1: Vec<u64> = (0..8).map(|_| d.fork().next_u64()).collect();
        let c2: Vec<u64> = (0..8).map(|_| d.fork().next_u64()).collect();
        assert_ne!(c1, c2);
    }
}

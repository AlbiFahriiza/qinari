//! Equal-power stereo panning.

/// Equal-power pan: returns `(left, right)` for `x` panned to `pos`.
///
/// `pos` is in `[-1, 1]` (hard left to hard right, clamped). Gains are
/// `sqrt(0.5 * (1 -/+ pos))`, so the center position gives `-3.01 dB` per
/// side and the total power stays roughly constant while moving.
pub fn pan(x: f32, pos: f32) -> (f32, f32) {
    let pos = pos.clamp(-1.0, 1.0);
    // 0.5 * (1 -/+ pos) written as a midpoint: equal-power gains.
    let left_gain = (0.5 * (1.0 - pos)).sqrt();
    let right_gain = (1.0_f32.midpoint(pos)).sqrt();
    (x * left_gain, x * right_gain)
}

/// A stateful equal-power panner over [`pan`].
#[derive(Debug, Clone, Copy)]
pub struct Panner {
    pos: f32,
    left_gain: f32,
    right_gain: f32,
}

impl Panner {
    /// Creates a panner at `pos` in `[-1, 1]`.
    pub fn new(pos: f32) -> Self {
        let mut panner = Self {
            pos: 0.0,
            left_gain: 0.0,
            right_gain: 0.0,
        };
        panner.set_pos(pos);
        panner
    }

    /// Moves to `pos`, recomputing the gains.
    pub fn set_pos(&mut self, pos: f32) {
        self.pos = pos.clamp(-1.0, 1.0);
        self.left_gain = (0.5 * (1.0 - self.pos)).sqrt();
        self.right_gain = (1.0_f32.midpoint(self.pos)).sqrt();
    }

    /// Returns the current position in `[-1, 1]`.
    pub fn pos(&self) -> f32 {
        self.pos
    }

    /// Pans one sample, returning `(left, right)`.
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        (x * self.left_gain, x * self.right_gain)
    }
}

/// Adds a mono signal into stereo buses with [`pan`].
///
/// Processes `min(mono.len(), left.len(), right.len())` frames, so a short
/// source can be panned into a longer master bus. `pos` is clamped to
/// `[-1, 1]`.
pub fn pan_into(left: &mut [f32], right: &mut [f32], mono: &[f32], pos: f32) {
    let (left_gain, right_gain) = pan(1.0, pos);
    let frames = left.len().min(right.len()).min(mono.len());
    for i in 0..frames {
        left[i] += mono[i] * left_gain;
        right[i] += mono[i] * right_gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_position_is_minus_three_db_per_side() {
        let (l, r) = pan(1.0, 0.0);
        let half_sqrt = std::f32::consts::FRAC_1_SQRT_2;
        assert!((l - half_sqrt).abs() < 1e-6, "left {l}");
        assert!((r - half_sqrt).abs() < 1e-6, "right {r}");
        let db = 20.0 * l.log10();
        assert!((db - (-3.010_3)).abs() < 0.01, "center gain {db} dB");
    }

    #[test]
    fn hard_positions_are_full_one_side() {
        let (l, r) = pan(0.5, -1.0);
        assert!((l - 0.5).abs() < 1e-6, "hard left gain {l}");
        assert_eq!(r, 0.0, "hard left must silence right");
        let (l, r) = pan(0.5, 1.0);
        assert!((r - 0.5).abs() < 1e-6, "hard right gain {r}");
        assert_eq!(l, 0.0);
        // Clamping beyond the range behaves like the extremes.
        let (l1, _) = pan(0.5, -3.0);
        let (l2, _) = pan(0.5, -1.0);
        assert_eq!(l1, l2);
    }

    #[test]
    fn panner_tracks_position_and_processes() {
        let mut panner = Panner::new(0.0);
        let (l, r) = panner.process(2.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l - 2.0 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        panner.set_pos(-1.0);
        assert!((panner.pos() - (-1.0)).abs() < 1e-6);
        let (l, r) = panner.process(2.0);
        assert!((l - 2.0).abs() < 1e-6);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn pan_into_adds_panned_mono_to_buses() {
        let mut left = vec![0.0f32; 8];
        let mut right = vec![0.0f32; 8];
        let mono = vec![1.0f32; 4]; // shorter than the buses
        pan_into(&mut left, &mut right, &mono, -1.0);
        for i in 0..4 {
            assert!((left[i] - 1.0).abs() < 1e-6);
            assert_eq!(right[i], 0.0);
        }
        for i in 4..8 {
            assert_eq!(left[i], 0.0, "beyond source length untouched");
            assert_eq!(right[i], 0.0);
        }

        // Center panning adds -3.01 dB per side to existing content.
        let mut left = vec![0.5f32; 4];
        let mut right = vec![0.5f32; 4];
        let mono = vec![1.0f32; 4];
        pan_into(&mut left, &mut right, &mono, 0.0);
        for i in 0..4 {
            assert!((left[i] - (0.5 + std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
            assert!((right[i] - (0.5 + std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-6);
        }
    }
}

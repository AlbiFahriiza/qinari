//! Event processes: memoryless, renewal, and doubly stochastic streams.
//!
//! A homogeneous Poisson stream is memoryless — every inter-onset gap is
//! an independent exponential — so real event fields that cluster, hold
//! refractory periods, or ride slow environmental drift are structurally
//! wrong under it (`RESEARCH-AUDIO-REALISM.md` §R 1.2-3, §R 2.4). This
//! module keeps [`poisson_events`] for genuinely high-rate streams and
//! adds the two processes natural fields need:
//!
//! - [`renewal_events`]: gamma-distributed intervals with a coefficient
//!   of variation (CV) parameter — CV < 1 gives refractory regularity
//!   (cricket chirp groups), CV > 1 heavy-tailed clustering;
//! - [`cox_events`]: a doubly stochastic (Cox) process whose
//!   instantaneous rate is a caller-supplied function of the frame —
//!   typically a [`crate::latent::LatentField`] — so events cluster
//!   where the latent is high, correlating event statistics with the
//!   bed motion by construction.

use crate::rng::Qrng;

/// Draws one standard normal deviate (Box-Muller on two uniforms).
fn standard_normal(rng: &mut Qrng) -> f64 {
    let u1 = 1.0 - f64::from(rng.next_f32()); // (0, 1]
    let u2 = f64::from(rng.next_f32());
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Draws one gamma-distributed variate of `shape` k and scale 1.
///
/// Marsaglia & Tsang's squeeze method for k >= 1, boosted by
/// `U^(1/k)` for k < 1. Mean k, variance k (scale 1); callers set the
/// scale through their interval mean.
fn gamma_sample(rng: &mut Qrng, shape: f64) -> f64 {
    if shape < 1.0 {
        let boosted = gamma_sample(rng, shape + 1.0);
        let unit = 1.0 - f64::from(rng.next_f32()); // (0, 1]
        return boosted * unit.powf(1.0 / shape);
    }
    let d_gamma = shape - 1.0 / 3.0;
    let c_squeeze = 1.0 / (9.0 * d_gamma).sqrt();
    loop {
        let z_normal = standard_normal(rng);
        let v_cube = (1.0 + c_squeeze * z_normal).powi(3);
        if v_cube <= 0.0 {
            continue;
        }
        let u_accept = 1.0 - f64::from(rng.next_f32()); // (0, 1]
                                                        // Squeeze, then the exact log acceptance branch.
        if u_accept < 1.0 - 0.0331 * z_normal * z_normal * z_normal * z_normal {
            return d_gamma * v_cube;
        }
        if u_accept.ln() < 0.5 * z_normal * z_normal + d_gamma * (1.0 - v_cube + v_cube.ln()) {
            return d_gamma * v_cube;
        }
    }
}

/// Draws event positions (ascending frame indices) for a Poisson process.
///
/// `rate_per_sec` is the expected event count per second; the process is
/// memoryless, so a render's events depend only on `rng`. A non-positive
/// rate yields no events.
pub fn poisson_events(rng: &mut Qrng, frames: usize, sr: u32, rate_per_sec: f32) -> Vec<usize> {
    let mut events = Vec::new();
    if rate_per_sec <= 0.0 || frames == 0 {
        return events;
    }
    let mean_gap = sr as f32 / rate_per_sec;
    let mut cursor = 0.0f32;
    while cursor < frames as f32 {
        // 1 - next_f32 lies in (0, 1]; ln(1) = 0 can stack two events on
        // the same frame, which the accumulator simply sums.
        let draw = 1.0 - rng.next_f32();
        events.push(cursor as usize);
        cursor += -draw.ln() * mean_gap;
    }
    events
}

/// Draws event positions for a renewal process with gamma intervals.
///
/// Intervals are `Gamma(k, theta)` with `k = 1/cv^2` and
/// `theta = 1 / (rate * k)`, so the mean interval is `1 / rate` and the
/// inter-onset CV is exactly the `cv` parameter: `cv = 1` degenerates to
/// the exponential (Poisson), `cv < 1` spaces events more regularly
/// (refractory), `cv > 1` clusters them (heavy-tailed gaps). `cv` is
/// clamped to `[0.05, 5]`; a non-positive rate yields no events.
pub fn renewal_events(
    rng: &mut Qrng,
    frames: usize,
    sr: u32,
    rate_per_sec: f32,
    cv: f32,
) -> Vec<usize> {
    let mut events = Vec::new();
    if rate_per_sec <= 0.0 || frames == 0 {
        return events;
    }
    let cv = f64::from(cv.clamp(0.05, 5.0));
    let shape = 1.0 / (cv * cv);
    let scale = 1.0 / (f64::from(rate_per_sec) * shape);
    let mut cursor = 0.0f64;
    while cursor < frames as f64 {
        events.push(cursor as usize);
        cursor += gamma_sample(rng, shape) * scale * f64::from(sr);
    }
    events
}

/// Draws event positions for a Cox (doubly stochastic) process.
///
/// The instantaneous rate at frame `f` is
/// `mean_rate_per_sec * rate_at(f)`; `rate_at` returns a non-negative
/// multiplier (values below zero are clamped to zero). Simulation
/// integrates the rate — the operational-time warp for a non-homogeneous
/// Poisson process — so the realized count over a window follows the
/// integral of the rate curve: events cluster where `rate_at` is high.
/// Feeding a [`crate::latent::LatentField`] read as `rate_at` correlates
/// the event field with the bed the same latent drives.
///
/// A non-positive mean rate yields no events.
pub fn cox_events<F: FnMut(usize) -> f32>(
    rng: &mut Qrng,
    frames: usize,
    sr: u32,
    mean_rate_per_sec: f32,
    mut rate_at: F,
) -> Vec<usize> {
    let mut events = Vec::new();
    if mean_rate_per_sec <= 0.0 || frames == 0 {
        return events;
    }
    let per_frame = f64::from(mean_rate_per_sec) / f64::from(sr);
    // The next event fires once the integrated rate crosses the next
    // unit-mean exponential draw.
    let mut threshold = -(1.0 - f64::from(rng.next_f32())).ln();
    let mut integrated = 0.0f64;
    for frame in 0..frames {
        let multiplier = f64::from(rate_at(frame).max(0.0));
        integrated += per_frame * multiplier;
        while integrated >= threshold {
            events.push(frame);
            threshold += -(1.0 - f64::from(rng.next_f32())).ln();
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Sample CV of the inter-onset intervals.
    fn interval_cv(events: &[usize]) -> f64 {
        let intervals: Vec<f64> = events
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64 / f64::from(SR))
            .collect();
        let n = intervals.len() as f64;
        assert!(n > 2.0, "need at least three events");
        let mean = intervals.iter().sum::<f64>() / n;
        let var = intervals
            .iter()
            .map(|i| {
                let d = i - mean;
                d * d
            })
            .sum::<f64>()
            / (n - 1.0);
        var.sqrt() / mean
    }

    #[test]
    fn poisson_streams_are_sorted_and_dense_enough() {
        let mut rng = Qrng::new(11);
        let events = poisson_events(&mut rng, 72_000, SR, 20.0);
        assert!(events.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");
        assert!(events.iter().all(|&at| at < 72_000));
        assert!((12..=60).contains(&events.len()), "got {}", events.len());
        assert_eq!(poisson_events(&mut rng, 1000, SR, 0.0).len(), 0);
        assert_eq!(poisson_events(&mut rng, 0, SR, 20.0).len(), 0);
    }

    #[test]
    fn renewal_cv_lands_on_the_parameter() {
        // 200 s at 4 Hz = ~800 events: the sample CV should sit within a
        // tight band of the requested one for each regime.
        for (cv, lo, hi) in [(0.4f32, 0.30, 0.52), (1.0, 0.90, 1.12), (2.0, 1.75, 2.30)] {
            let mut rng = Qrng::new(41);
            let events = renewal_events(&mut rng, 200 * SR as usize, SR, 4.0, cv);
            assert!(
                events.windows(2).all(|pair| pair[0] <= pair[1]),
                "cv {cv}: events must be sorted (sub-frame intervals may stack)"
            );
            let measured = interval_cv(&events);
            assert!(
                (lo..=hi).contains(&measured),
                "cv {cv}: measured {measured}"
            );
            let expected = 4.0 * 200.0;
            assert!(
                (events.len() as f64 / expected > 0.85) && (events.len() as f64 / expected < 1.15),
                "cv {cv}: count {} vs expected ~{expected}",
                events.len()
            );
        }
    }

    #[test]
    fn renewal_events_degenerate_inputs() {
        let mut rng = Qrng::new(43);
        assert_eq!(renewal_events(&mut rng, 48_000, SR, 0.0, 1.0).len(), 0);
        assert_eq!(renewal_events(&mut rng, 0, SR, 5.0, 1.0).len(), 0);
        // cv clamping keeps the extremes usable.
        let events = renewal_events(&mut rng, 48_000, SR, 50.0, 100.0);
        assert!(!events.is_empty(), "cv 100 clamps to 5 and still fires");
    }

    #[test]
    fn cox_constant_rate_behaves_like_poisson() {
        let mut rng = Qrng::new(45);
        let events = cox_events(&mut rng, 60 * SR as usize, SR, 5.0, |_| 1.0);
        let expected = 5.0 * 60.0;
        assert!(
            (events.len() as f64 / expected > 0.8) && (events.len() as f64 / expected < 1.2),
            "count {} vs expected ~{expected}",
            events.len()
        );
        let cv = interval_cv(&events);
        assert!((0.85..=1.15).contains(&cv), "constant rate cv {cv}");
    }

    #[test]
    fn cox_swinging_rate_clusters_events() {
        // Rate multiplier 4.0 for 3 s, 0.05 for 3 s, alternating: events
        // pile into the hot windows, so the inter-onset CV must exceed
        // the memoryless 1.0 clearly.
        let mut rng = Qrng::new(47);
        let events = cox_events(&mut rng, 60 * SR as usize, SR, 4.0, |frame| {
            let phase = (frame as f32 / SR as f32) % 6.0;
            if phase < 3.0 {
                4.0
            } else {
                0.05
            }
        });
        let cv = interval_cv(&events);
        assert!(cv > 1.3, "clustered cv {cv}");
        // And the count still tracks the integrated rate (mean
        // multiplier ~2.0).
        let expected = 4.0 * 60.0 * 2.0;
        assert!(
            (events.len() as f64 / expected > 0.7) && (events.len() as f64 / expected < 1.3),
            "count {} vs expected ~{expected}",
            events.len()
        );
    }

    #[test]
    fn cox_zero_multiplier_is_silent_and_events_are_deterministic() {
        let mut rng = Qrng::new(49);
        assert_eq!(cox_events(&mut rng, 48_000, SR, 4.0, |_| 0.0).len(), 0);

        let draw = |rng: &mut Qrng| {
            cox_events(rng, 10 * SR as usize, SR, 3.0, |frame| {
                0.5 + 0.5 * (frame as f32 / 24_000.0).sin()
            })
        };
        let a = draw(&mut Qrng::new(50));
        let b = draw(&mut Qrng::new(50));
        assert_eq!(a, b, "same seed same events");
        assert_ne!(a.len(), 0, "the swinging-rate stream must fire");
        assert!(a.windows(2).all(|w| w[0] <= w[1]), "sorted");
    }
}

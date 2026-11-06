//! Envelope generators: ADSR, percussive decay, and multi-stage shapes.

/// An attack/decay/sustain/release envelope, processed one sample at a
/// time.
///
/// Every segment is a one-pole exponential approach toward its target
/// where the segment duration equals five time constants (99.3% settled):
/// attack rises toward 1.0, decay falls toward the sustain level, release
/// falls toward zero. This avoids the classic broken "exponential attack
/// from zero" (a pure `level *= k` attack never leaves zero); approaching
/// a non-zero target always makes progress.
#[derive(Debug, Clone, Copy)]
pub struct Adsr {
    attack_samples: usize,
    decay_samples: usize,
    release_samples: usize,
    sustain: f32,
    attack_coef: f32,
    decay_coef: f32,
    release_coef: f32,
    state: AdsrState,
    level: f32,
    segment_pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdsrState {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl Adsr {
    /// Creates an envelope with segment times in seconds and a sustain
    /// level in `[0, 1]`.
    pub fn new(
        attack_secs: f32,
        decay_secs: f32,
        sustain: f32,
        release_secs: f32,
        sample_rate: f32,
    ) -> Self {
        let samples = |secs: f32| (secs * sample_rate).round() as usize;
        let attack_samples = samples(attack_secs);
        let decay_samples = samples(decay_secs);
        let release_samples = samples(release_secs);
        // One-pole rate so each segment is 99.3% (e^-5) complete at its
        // nominal duration; zero-length segments jump straight to target.
        let coef = |samples: usize| {
            if samples == 0 {
                1.0
            } else {
                1.0 - (-5.0 / samples as f32).exp()
            }
        };
        Self {
            attack_samples,
            decay_samples,
            release_samples,
            sustain: sustain.clamp(0.0, 1.0),
            attack_coef: coef(attack_samples),
            decay_coef: coef(decay_samples),
            release_coef: coef(release_samples),
            state: AdsrState::Idle,
            level: 0.0,
            segment_pos: 0,
        }
    }

    /// Starts the attack phase (retriggering from the current level).
    ///
    /// A zero-length attack jumps the level straight to 1.0.
    pub fn trigger(&mut self) {
        if self.attack_samples == 0 {
            self.level = 1.0;
            self.state = AdsrState::Decay;
        } else {
            self.state = AdsrState::Attack;
        }
        self.segment_pos = 0;
    }

    /// Starts the release phase from the current level.
    pub fn release(&mut self) {
        self.state = if self.release_samples == 0 {
            self.level = 0.0;
            AdsrState::Idle
        } else {
            AdsrState::Release
        };
        self.segment_pos = 0;
    }

    /// Returns `true` once the release phase has fully elapsed.
    pub fn is_finished(&self) -> bool {
        self.state == AdsrState::Idle
    }

    /// Advances one sample and returns the envelope level.
    pub fn process(&mut self) -> f32 {
        let target = match self.state {
            AdsrState::Idle => return self.level,
            AdsrState::Attack => 1.0,
            AdsrState::Decay | AdsrState::Sustain => self.sustain,
            AdsrState::Release => 0.0,
        };
        let (coef, length) = match self.state {
            AdsrState::Attack => (self.attack_coef, self.attack_samples),
            AdsrState::Decay => (self.decay_coef, self.decay_samples),
            AdsrState::Release => (self.release_coef, self.release_samples),
            AdsrState::Sustain => {
                self.level += (target - self.level) * self.decay_coef;
                return self.level;
            }
            AdsrState::Idle => unreachable!(),
        };
        self.level += (target - self.level) * coef;
        self.segment_pos += 1;
        if self.segment_pos >= length {
            self.state = match self.state {
                AdsrState::Attack => {
                    if self.decay_samples == 0 {
                        AdsrState::Sustain
                    } else {
                        AdsrState::Decay
                    }
                }
                AdsrState::Decay | AdsrState::Sustain | AdsrState::Idle => AdsrState::Sustain,
                AdsrState::Release => {
                    self.level = 0.0;
                    AdsrState::Idle
                }
            };
            self.segment_pos = 0;
        }
        self.level
    }
}

/// A percussive exponential-decay envelope calibrated by T60.
///
/// The level starts at 0 and is set to 1 by [`Percussive::trigger`]; it
/// then decays by `exp(-t / tau)` with `tau = t60 / 6.91`, so the
/// amplitude crosses -60 dB exactly at `t60` seconds. Callers must call
/// `trigger()` (an untriggered envelope outputs silence).
///
/// [`Percussive::shaped`] extends the same envelope with a short attack
/// ramp and a two-stage decay (fast τ₁ into slow τ₂) — the transient
/// shape real one-shots have and zero-attack exponential clicks do not
/// (`RESEARCH-AUDIO-REALISM.md` §R 2.2).
#[derive(Debug, Clone, Copy)]
pub struct Percussive {
    attack_samples: usize,
    attack_pos: usize,
    attack_inc: f32,
    fast_decay: f32,
    slow_decay: f32,
    crossover: f32,
    level: f32,
}

impl Percussive {
    /// Creates a percussive envelope that decays to -60 dB after
    /// `t60_secs`.
    pub fn from_t60(sample_rate: f32, t60_secs: f32) -> Self {
        let decay = decay_per_sample(sample_rate, t60_secs / 6.91);
        Self {
            attack_samples: 0,
            attack_pos: 0,
            attack_inc: 1.0,
            fast_decay: decay,
            slow_decay: decay,
            // A zero crossover keeps the level in the fast stage forever,
            // which is exactly the single-stage decay of this constructor.
            crossover: 0.0,
            level: 0.0,
        }
    }

    /// Creates a shaped percussive envelope: a linear attack ramp of
    /// `attack_secs` (0.2–5 ms is the natural range), then a two-stage
    /// decay that starts with the fast time constant `tau1_secs` and
    /// switches to the slower `tau2_secs` once the level falls to
    /// `crossover` (a fraction in `(0, 1]`; values outside are clamped).
    ///
    /// Both time constants are e-folding times: the level decays by
    /// `exp(-t / tau)` within its stage. A zero attack jumps straight to
    /// 1 like [`Percussive::from_t60`].
    pub fn shaped(
        sample_rate: f32,
        attack_secs: f32,
        tau1_secs: f32,
        tau2_secs: f32,
        crossover: f32,
    ) -> Self {
        let attack_samples = (attack_secs.max(0.0) * sample_rate).round() as usize;
        Self {
            attack_samples,
            attack_pos: 0,
            attack_inc: if attack_samples == 0 {
                1.0
            } else {
                1.0 / attack_samples as f32
            },
            fast_decay: decay_per_sample(sample_rate, tau1_secs.max(1.0e-6)),
            slow_decay: decay_per_sample(sample_rate, tau2_secs.max(1.0e-6)),
            crossover: crossover.clamp(0.0, 1.0),
            level: 0.0,
        }
    }

    /// Sets the level to the start of the attack (1 when there is no
    /// attack), restarting the decay.
    pub fn trigger(&mut self) {
        self.level = if self.attack_samples == 0 { 1.0 } else { 0.0 };
        self.attack_pos = 0;
    }

    /// Advances one sample and returns the level.
    pub fn process(&mut self) -> f32 {
        if self.attack_pos < self.attack_samples {
            self.attack_pos += 1;
            self.level = (self.attack_pos as f32) * self.attack_inc;
            return self.level;
        }
        let out = self.level;
        let decay = if self.level > self.crossover {
            self.fast_decay
        } else {
            self.slow_decay
        };
        self.level *= decay;
        out
    }

    /// Returns `true` once the level has decayed below -100 dB.
    pub fn is_finished(&self) -> bool {
        self.level < 1.0e-5
    }
}

/// Per-sample decay multiplier for an e-folding time `tau_secs`.
fn decay_per_sample(sample_rate: f32, tau_secs: f32) -> f32 {
    (-1.0 / (tau_secs * sample_rate)).exp()
}

/// The Friedlander blast-wave pressure shape
/// `P(t) = e^(-t/t*) * (1 - t/t*)` for unit peak overpressure.
///
/// The canonical free-field detonation waveform
/// (`RESEARCH-AUDIO-REALISM.md` §R 2.3): peak 1 at `t = 0`, first zero
/// crossing at `t = t*`, then a genuine **negative** (rarefaction)
/// phase — the minimum `-e^-2 ≈ -0.135` lands at `t = 2t*` — decaying
/// back toward zero from below. Returns 0 for `t < 0`.
pub fn friedlander(t_secs: f32, t_star_secs: f32) -> f32 {
    if t_secs < 0.0 || t_star_secs <= 0.0 {
        return 0.0;
    }
    let x = t_secs / t_star_secs;
    (-x).exp() * (1.0 - x)
}

/// The curve one [`SegmentSpec`] follows to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentShape {
    /// Straight line from the segment's start level to its target.
    Linear,
    /// One-pole approach toward the target, 99.3% complete at the
    /// segment's nominal duration (the same convention as [`Adsr`]).
    Smooth,
}

/// One stage of a [`Segment`] envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSpec {
    /// Level the segment moves toward.
    pub target: f32,
    /// Segment duration in seconds (<= 0 jumps straight to the target).
    pub secs: f32,
    /// Ramp curve.
    pub shape: SegmentShape,
}

/// A multi-stage one-shot envelope: a sequence of levels connected by
/// ramps.
///
/// Unlike [`Adsr`] (one fixed A-D-S-R skeleton) a [`Segment`] envelope
/// expresses arbitrary shapes — a thunder clap's build-hold-fall, a
/// two-bump body — as a list of [`SegmentSpec`]s. The envelope starts at
/// 0, each segment departs from the previous segment's target, and after
/// the last segment the level holds the final target (authors close
/// their shape with a zero target). Per-sample processing is O(1) with
/// no allocation; the segment table is built once.
#[derive(Debug, Clone)]
pub struct Segment {
    segments: Vec<PreparedSegment>,
    index: usize,
    pos: usize,
    level: f32,
    total_frames: usize,
}

/// A segment with its per-sample constants precomputed.
#[derive(Debug, Clone, Copy)]
struct PreparedSegment {
    target: f32,
    start: f32,
    length: usize,
    linear_step: f32,
    smooth_coef: f32,
    shape: SegmentShape,
}

impl Segment {
    /// Prepares the segment sequence for `sample_rate`.
    ///
    /// An empty list is a legal, always-finished envelope (it outputs
    /// silence).
    pub fn new(specs: &[SegmentSpec], sample_rate: f32) -> Self {
        let mut segments = Vec::with_capacity(specs.len());
        let mut start = 0.0f32;
        let mut total_frames = 0usize;
        for spec in specs {
            let length = (spec.secs.max(0.0) * sample_rate).round() as usize;
            segments.push(PreparedSegment {
                target: spec.target,
                start,
                length,
                linear_step: if length == 0 {
                    0.0
                } else {
                    (spec.target - start) / length as f32
                },
                smooth_coef: if length == 0 {
                    1.0
                } else {
                    1.0 - (-5.0 / length as f32).exp()
                },
                shape: spec.shape,
            });
            start = spec.target;
            total_frames += length;
        }
        let initial_index = segments.len();
        Self {
            segments,
            // Untriggered envelopes hold silence: only `trigger` arms the
            // shape (a fresh `Percussive` is silent for the same reason).
            index: initial_index,
            pos: 0,
            level: 0.0,
            total_frames,
        }
    }

    /// Restarts the shape from level 0 at segment 0.
    pub fn trigger(&mut self) {
        self.index = 0;
        self.pos = 0;
        self.level = 0.0;
    }

    /// Advances one sample and returns the level.
    ///
    /// After the last segment the final target is held.
    pub fn process(&mut self) -> f32 {
        let Some(segment) = self.segments.get(self.index) else {
            return self.level;
        };
        if segment.length == 0 {
            // Zero-length segments jump to their target.
            self.level = segment.target;
            self.index += 1;
            return self.level;
        }
        self.level = match segment.shape {
            SegmentShape::Linear => segment.start + segment.linear_step * (self.pos + 1) as f32,
            SegmentShape::Smooth => {
                self.level + (segment.target - self.level) * segment.smooth_coef
            }
        };
        self.pos += 1;
        if self.pos >= segment.length {
            self.index += 1;
            self.pos = 0;
            // Pin the level exactly: the smooth curve only approaches.
            self.level = segment.target;
        }
        self.level
    }

    /// Returns `true` once every segment has elapsed.
    pub fn is_finished(&self) -> bool {
        self.index >= self.segments.len()
    }

    /// Returns the total duration of the shape in seconds.
    pub fn total_secs(&self, sample_rate: f32) -> f32 {
        self.total_frames as f32 / sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn adsr_reaches_sustain_within_five_percent() {
        let mut env = Adsr::new(0.1, 0.2, 0.5, 0.3, SR);
        assert!(env.is_finished());
        assert_eq!(env.process(), 0.0, "idle envelope is silent");

        env.trigger();
        let mut level = 0.0;
        for _ in 0..((0.1 + 0.2) * SR) as usize + 100 {
            level = env.process();
        }
        assert!(
            (level - 0.5).abs() < 0.025,
            "sustain level {level}, expected 0.5 +/- 5%"
        );
        // Sustain holds.
        for _ in 0..(0.1 * SR) as usize {
            level = env.process();
        }
        assert!((level - 0.5).abs() < 0.025, "sustain drifted to {level}");
    }

    #[test]
    fn adsr_attack_curve_rises_monotonically() {
        let mut env = Adsr::new(0.2, 0.1, 1.0, 0.1, SR);
        env.trigger();
        let mut previous = -1.0;
        for _ in 0..(0.2 * SR) as usize {
            let level = env.process();
            assert!(level >= previous, "attack not monotonic at {level}");
            previous = level;
        }
        assert!(previous > 0.9, "attack reached only {previous}");
        assert!(previous < 1.0);
    }

    #[test]
    fn adsr_releases_below_one_percent_in_release_time() {
        let mut env = Adsr::new(0.05, 0.05, 0.5, 0.3, SR);
        env.trigger();
        for _ in 0..(0.2 * SR) as usize {
            env.process();
        }
        env.release();
        assert!(!env.is_finished());
        let mut level = 1.0;
        for _ in 0..(0.3 * SR) as usize + 10 {
            level = env.process();
        }
        assert!(level < 0.01, "release level {level}");
        assert!(env.is_finished());
        // Idle output stays at zero.
        assert_eq!(env.process(), 0.0);
    }

    #[test]
    fn adsr_zero_segments_jump_immediately() {
        let mut env = Adsr::new(0.0, 0.0, 0.7, 0.0, SR);
        env.trigger();
        let mut level = 0.0;
        for _ in 0..5 {
            level = env.process();
        }
        assert!((level - 0.7).abs() < 1e-6, "level {level}");
        env.release();
        assert!(env.is_finished());
        assert_eq!(env.process(), 0.0);
    }

    #[test]
    fn percussive_is_silent_until_triggered() {
        let mut env = Percussive::from_t60(SR, 0.5);
        for _ in 0..1000 {
            assert_eq!(env.process(), 0.0, "untriggered envelope must be silent");
        }
        assert!(env.is_finished());
    }

    #[test]
    fn percussive_decays_to_minus_60_db_at_t60() {
        let t60 = 0.5;
        let mut env = Percussive::from_t60(SR, t60);
        env.trigger();
        let first = env.process();
        assert!((first - 1.0).abs() < 1e-4, "first sample {first}");

        let mut level = first;
        for _ in 1..(t60 * SR) as usize {
            level = env.process();
        }
        let db = 20.0 * level.log10();
        assert!(
            (-63.0..=-57.0).contains(&db),
            "level at t60 was {db} dB (expected -60 +/- 3)"
        );
        assert!(env.is_finished() || level < 1.1e-3);
    }

    #[test]
    fn shaped_percussive_ramps_then_decays_two_stage() {
        // 2 ms attack, fast tau 20 ms into slow tau 400 ms at crossover 0.3.
        let attack = 0.002;
        let tau1 = 0.02;
        let tau2 = 0.4;
        let crossover = 0.3;
        let mut env = Percussive::shaped(SR, attack, tau1, tau2, crossover);
        env.trigger();

        // Attack: starts near zero and rises monotonically to 1.
        let mut previous = 0.0f32;
        let attack_n = (attack * SR) as usize;
        for _ in 0..attack_n {
            let level = env.process();
            assert!(level >= previous, "attack not monotonic at {level}");
            previous = level;
        }
        assert!((previous - 1.0).abs() < 0.01, "attack reached {previous}");

        // Fast stage into slow stage: the level crosses the 0.3 crossover
        // at t_x = tau1 * ln(1/crossover); after 2*tau1 total the expected
        // level is crossover * exp(-(2*tau1 - t_x) / tau2).
        let mut level = previous;
        for _ in 0..(2.0 * tau1 * SR) as usize {
            level = env.process();
        }
        let t_cross = tau1 * (1.0 / crossover).ln();
        let expected = crossover * (-(2.0 * tau1 - t_cross) / tau2).exp();
        assert!(
            (level - expected).abs() < 0.02,
            "fast+cross level {level}, expected {expected}"
        );

        // Slow stage: once below the crossover the decay follows tau2.
        let a = level;
        for _ in 0..(tau2 * SR) as usize {
            level = env.process();
        }
        let ratio = level / a;
        assert!(
            (ratio - (-1.0f32).exp()).abs() < 0.05,
            "slow stage ratio {ratio}, expected ~e^-1"
        );
        assert!(level < crossover, "level {level} never crossed {crossover}");
        assert!(!env.is_finished(), "long tail still ringing");
    }

    #[test]
    fn shaped_percussive_zero_attack_matches_single_stage() {
        // With no attack and tau1 == tau2 the shape is the classic
        // single-stage percussive decay.
        let t60 = 0.3;
        let tau = t60 / 6.91;
        let mut env = Percussive::shaped(SR, 0.0, tau, tau, 0.5);
        env.trigger();
        let first = env.process();
        assert!((first - 1.0).abs() < 1e-5, "first sample {first}");
        let mut level = first;
        for _ in 1..(t60 * SR) as usize {
            level = env.process();
        }
        let db = 20.0 * level.log10();
        assert!((-63.0..=-57.0).contains(&db), "level at t60 was {db} dB");
    }

    #[test]
    fn friedlander_shape_pins_the_blast_physics() {
        let t_star = 0.2;
        // Peak overpressure at the origin.
        assert!((friedlander(0.0, t_star) - 1.0).abs() < 1e-6);
        // First zero crossing exactly at t*.
        assert!(friedlander(t_star, t_star).abs() < 1e-6);
        // The negative lobe: minimum -e^-2 at 2t* (design §12 constant).
        let lobe = friedlander(2.0 * t_star, t_star);
        let expected_lobe = -std::f64::consts::E.powi(-2);
        assert!(
            (f64::from(lobe) - expected_lobe).abs() < 1e-3,
            "lobe {lobe}"
        );
        assert!(lobe < 0.0, "the rarefaction phase is negative");
        // Monotone fall to the lobe, then recovery toward zero from below.
        assert!(friedlander(0.5 * t_star, t_star) > 0.0);
        assert!(friedlander(3.0 * t_star, t_star) < 0.0);
        assert!(friedlander(3.0 * t_star, t_star) > lobe);
        // Settled below 1% of Ps by 7t* (at 5t* the value is still
        // -4·e^-5 ≈ -2.7% — the tail decays as t·e^-t, not e^-t).
        assert!(friedlander(7.0 * t_star, t_star).abs() < 0.01);
        // Degenerate inputs.
        assert_eq!(friedlander(-0.1, t_star), 0.0);
        assert_eq!(friedlander(0.1, 0.0), 0.0);
    }

    #[test]
    fn segment_envelope_walks_its_levels() {
        // 0 -> 1 (linear, 50 ms) -> 0.5 (smooth, 50 ms) -> 0 (linear, 50 ms).
        let specs = [
            SegmentSpec {
                target: 1.0,
                secs: 0.05,
                shape: SegmentShape::Linear,
            },
            SegmentSpec {
                target: 0.5,
                secs: 0.05,
                shape: SegmentShape::Smooth,
            },
            SegmentSpec {
                target: 0.0,
                secs: 0.05,
                shape: SegmentShape::Linear,
            },
        ];
        let mut env = Segment::new(&specs, SR);
        assert!(env.is_finished(), "untriggered envelope is done");
        assert_eq!(env.process(), 0.0, "untriggered envelope is silent");
        assert!((env.total_secs(SR) - 0.15).abs() < 1e-6);

        env.trigger();
        let n = (0.05 * SR) as usize;
        let mut level = 0.0;
        for _ in 0..n {
            level = env.process();
        }
        assert!((level - 1.0).abs() < 1e-6, "segment 1 reached {level}");
        for _ in 0..n {
            level = env.process();
        }
        assert!(
            (level - 0.5).abs() < 0.02,
            "smooth segment settled at {level}"
        );
        for _ in 0..n {
            level = env.process();
        }
        assert!(level.abs() < 1e-6, "final level {level}");
        assert!(env.is_finished());
        // Past the end the final target is held.
        for _ in 0..100 {
            assert_eq!(env.process(), 0.0);
        }
    }

    #[test]
    fn segment_envelope_handles_jumps_and_empty_lists() {
        // Zero-length segments jump straight to their target.
        let specs = [
            SegmentSpec {
                target: 0.8,
                secs: 0.0,
                shape: SegmentShape::Linear,
            },
            SegmentSpec {
                target: 0.2,
                secs: 0.01,
                shape: SegmentShape::Linear,
            },
        ];
        let mut env = Segment::new(&specs, SR);
        env.trigger();
        assert!((env.process() - 0.8).abs() < 1e-6, "jump segment");
        let mut level = 0.0;
        for _ in 0..(0.01 * SR) as usize {
            level = env.process();
        }
        assert!((level - 0.2).abs() < 1e-6);

        // An empty spec list is silent and finished.
        let mut empty = Segment::new(&[], SR);
        empty.trigger();
        assert!(empty.is_finished());
        assert_eq!(empty.process(), 0.0);
    }
}

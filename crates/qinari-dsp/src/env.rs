//! Envelope generators: ADSR and percussive decay.

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
#[derive(Debug, Clone, Copy)]
pub struct Percussive {
    decay_per_sample: f32,
    level: f32,
}

impl Percussive {
    /// Creates a percussive envelope that decays to -60 dB after
    /// `t60_secs`.
    pub fn from_t60(sample_rate: f32, t60_secs: f32) -> Self {
        let tau = t60_secs / 6.91;
        Self {
            decay_per_sample: (-1.0 / (tau * sample_rate)).exp(),
            level: 0.0,
        }
    }

    /// Sets the level to 1, restarting the decay.
    pub fn trigger(&mut self) {
        self.level = 1.0;
    }

    /// Advances one sample and returns the level.
    pub fn process(&mut self) -> f32 {
        let out = self.level;
        self.level *= self.decay_per_sample;
        out
    }

    /// Returns `true` once the level has decayed below -100 dB.
    pub fn is_finished(&self) -> bool {
        self.level < 1.0e-5
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
}

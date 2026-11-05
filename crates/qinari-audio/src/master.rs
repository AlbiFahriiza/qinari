//! Loudness mastering: target normalization, character chains, dithering.

use crate::format::BitDepth;
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::dither::Dither;
use qinari_dsp::filter::{Biquad, DcBlocker};
use qinari_dsp::fx::{Compressor, StereoLimiter};
use qinari_dsp::lufs::integrated_lufs;
use qinari_dsp::rng::{Qrng, Seed};

/// Low shelf corner in hertz (the plan leaves shelf frequencies to design;
/// 110 Hz is the usual bass-tilt corner).
const LOW_SHELF_HZ: f32 = 110.0;
/// Presence high-shelf corner in hertz.
const PRESENCE_HZ: f32 = 3_200.0;
/// Air high-shelf corner in hertz.
const AIR_HZ: f32 = 11_000.0;
/// Shelf steepness (RBJ cookbook default).
const SHELF_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Verification loop stops once the output is within this distance of the
/// target, in LU.
const TARGET_TOLERANCE_LU: f32 = 0.3;
/// Largest single gain correction the verification loop applies, in dB.
const MAX_CORRECTION_DB: f32 = 2.0;
/// Maximum number of re-measure/correct passes.
const MAX_VERIFICATION_PASSES: usize = 3;

/// The sonic signature applied to a master.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Character {
    /// No tonal change, near-transparent dynamics.
    Transparent,
    /// Bass lift, relaxed upper mids, darker top end.
    Warm,
    /// Bass and presence lift, faster, firmer dynamics.
    Punchy,
}

/// Tonal and dynamics parameters of one [`Character`].
#[derive(Debug, Clone, Copy)]
struct CharacterParams {
    /// Low-shelf gain in dB.
    low_shelf_db: f32,
    /// Presence-shelf gain in dB.
    presence_db: f32,
    /// Air-shelf gain in dB.
    air_db: f32,
    /// Compressor ratio.
    ratio: f32,
    /// Compressor threshold in dBFS.
    threshold_db: f32,
    /// Compressor attack in seconds.
    attack_secs: f32,
    /// Compressor release in seconds.
    release_secs: f32,
    /// Compressor makeup gain in dB.
    makeup_db: f32,
}

impl Character {
    fn params(self) -> CharacterParams {
        match self {
            Character::Transparent => CharacterParams {
                low_shelf_db: 0.0,
                presence_db: 0.0,
                air_db: 0.0,
                ratio: 1.25,
                threshold_db: -12.0,
                attack_secs: 0.030,
                release_secs: 0.250,
                makeup_db: 0.0,
            },
            Character::Warm => CharacterParams {
                low_shelf_db: 2.0,
                presence_db: -1.0,
                air_db: -1.5,
                ratio: 2.0,
                threshold_db: -16.0,
                attack_secs: 0.030,
                release_secs: 0.300,
                makeup_db: 1.5,
            },
            Character::Punchy => CharacterParams {
                low_shelf_db: 2.5,
                presence_db: 2.0,
                air_db: 0.0,
                ratio: 3.0,
                threshold_db: -18.0,
                attack_secs: 0.008,
                release_secs: 0.120,
                makeup_db: 3.5,
            },
        }
    }
}

/// Settings for one [`master`] run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasteringSettings {
    /// Integrated loudness the output is driven to, in LUFS.
    pub target_lufs: f32,
    /// True-peak-style output ceiling in dBFS enforced by the limiter.
    pub ceiling_db: f32,
    /// Which character chain to run.
    pub character: Character,
}

impl Default for MasteringSettings {
    /// The Qinari house master: -16 LUFS, -1 dBFS ceiling, transparent.
    fn default() -> Self {
        Self {
            target_lufs: -16.0,
            ceiling_db: -1.0,
            character: Character::Transparent,
        }
    }
}

/// What [`master`] measured and did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasteredReport {
    /// Integrated loudness of the input in LUFS (`None` when too short or
    /// too quiet to measure).
    pub input_lufs: Option<f64>,
    /// Integrated loudness of the output in LUFS (`None` when unmeasurable).
    pub output_lufs: Option<f64>,
    /// Total gain applied before the character chain, in dB (including
    /// verification-loop corrections).
    pub applied_gain_db: f32,
    /// Peak absolute sample value of the output.
    pub output_peak: f32,
}

/// Masters `buffer` to the loudness target in `settings`.
///
/// The pipeline: measure the integrated LUFS and apply
/// `target - input` dB, run the character chain (DC blocker, low/presence/
/// air shelves, compressor, ceiling limiter per channel pair), then verify.
/// The verification loop re-measures the output and, while it is more than
/// 0.3 LU from the target, adjusts the gain by the clamped residual
/// (at most ±2 dB) and re-runs the chain from the original input, at most
/// three times.
///
/// Buffers too short or too quiet to measure (< 400 ms of signal above the
/// gating thresholds) skip the gain and verification stages but still run
/// through the chain, so short sound effects keep the character and ceiling
/// behaviour.
pub fn master(buffer: &AudioBuffer, settings: &MasteringSettings) -> (AudioBuffer, MasteredReport) {
    let input_lufs = integrated_lufs(buffer);
    let mut gain_db = input_lufs.map_or(0.0, |lufs| settings.target_lufs - lufs as f32);
    let mut output = process_character_chain(buffer, gain_db, settings);
    let mut output_lufs = integrated_lufs(&output);
    if input_lufs.is_some() {
        for _ in 0..MAX_VERIFICATION_PASSES {
            let Some(measured) = output_lufs else {
                break;
            };
            let residual = settings.target_lufs - measured as f32;
            if residual.abs() <= TARGET_TOLERANCE_LU {
                break;
            }
            gain_db += residual.clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB);
            output = process_character_chain(buffer, gain_db, settings);
            output_lufs = integrated_lufs(&output);
        }
    }
    let report = MasteredReport {
        input_lufs,
        output_lufs,
        applied_gain_db: gain_db,
        output_peak: output.peak(),
    };
    (output, report)
}

/// Applies `gain_db` to a copy of `input` and runs it through the character
/// chain with fresh filter state.
fn process_character_chain(
    input: &AudioBuffer,
    gain_db: f32,
    settings: &MasteringSettings,
) -> AudioBuffer {
    let mut buffer = input.clone();
    buffer.apply_gain_db(gain_db);
    let sample_rate = buffer.sample_rate() as f32;
    let channels = buffer.num_channels();
    let params = settings.character.params();
    let mut dc_blockers = vec![DcBlocker::new(); channels];
    let mut low_shelves =
        vec![rbj_low_shelf(sample_rate, LOW_SHELF_HZ, SHELF_Q, params.low_shelf_db); channels];
    let mut presence_shelves =
        vec![rbj_high_shelf(sample_rate, PRESENCE_HZ, SHELF_Q, params.presence_db); channels];
    let mut air_shelves =
        vec![rbj_high_shelf(sample_rate, AIR_HZ, SHELF_Q, params.air_db); channels];
    let mut compressors = vec![
        Compressor::new(
            sample_rate,
            params.threshold_db,
            params.ratio,
            params.attack_secs,
            params.release_secs,
            params.makeup_db,
        );
        channels
    ];
    // One linked limiter per channel pair; a trailing lone channel is linked
    // with itself so every channel still respects the ceiling.
    let mut limiters = (0..channels.div_ceil(2))
        .map(|_| StereoLimiter::new(sample_rate, settings.ceiling_db))
        .collect::<Vec<_>>();

    let mut frame_samples = vec![0.0f32; channels];
    for frame in 0..buffer.frames() {
        for channel in 0..channels {
            let sample = buffer.get(channel, frame);
            let sample = dc_blockers[channel].process(sample);
            let sample = low_shelves[channel].process(sample);
            let sample = presence_shelves[channel].process(sample);
            let sample = air_shelves[channel].process(sample);
            frame_samples[channel] = compressors[channel].process(sample);
        }
        for (pair, limiter) in limiters.iter_mut().enumerate() {
            let left = frame_samples[pair * 2];
            let right = frame_samples.get(pair * 2 + 1).copied().unwrap_or(left);
            let (limited_left, limited_right) = limiter.process(left, right);
            buffer.set(pair * 2, frame, limited_left);
            if pair * 2 + 1 < channels {
                buffer.set(pair * 2 + 1, frame, limited_right);
            }
        }
    }
    buffer
}

/// RBJ audio-EQ-cookbook low shelf.
///
/// `f0` is clamped below Nyquist with the same margin `qinari-dsp`'s
/// [`Biquad::design`] uses (RBJ poles leave the unit circle at and above
/// Nyquist, which would blow up low-sample-rate masters).
fn rbj_low_shelf(sample_rate: f32, f0: f32, q: f32, gain_db: f32) -> Biquad {
    let f0 = f0.clamp(1.0, sample_rate * 0.5 * 0.4995);
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);
    let sqrt_a = a.sqrt();
    let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
    let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
    let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
    let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;
    Biquad::from_coefficients(b0, b1, b2, a0, a1, a2)
}

/// RBJ audio-EQ-cookbook high shelf (same design as the K-weighting shelf
/// in `qinari-dsp`'s loudness meter). `f0` is Nyquist-clamped like in
/// [`rbj_low_shelf`].
fn rbj_high_shelf(sample_rate: f32, f0: f32, q: f32, gain_db: f32) -> Biquad {
    let f0 = f0.clamp(1.0, sample_rate * 0.5 * 0.4995);
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);
    let sqrt_a = a.sqrt();
    let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;
    Biquad::from_coefficients(b0, b1, b2, a0, a1, a2)
}

/// Requantizes `buffer` for delivery at `depth`, adding dither.
///
/// `Float32` returns the buffer unchanged. Integer depths add TPDF dither;
/// 16-bit additionally uses second-order noise shaping (16 bits is where the
/// shaped floor pays for itself; at 24 bits the flat floor is already below
/// audibility). The dither generator is seeded from the fixed text seed
/// `"qinari-master-dither"` and forked per channel, so the result is
/// deterministic and channel-independent.
///
/// # Panics
///
/// Never for a well-formed buffer: the output preserves the input's channel
/// shapes and sample rate, which is all [`AudioBuffer::from_channels`]
/// requires.
pub fn finalize_for_depth(buffer: &AudioBuffer, depth: BitDepth) -> AudioBuffer {
    match depth {
        BitDepth::Float32 => buffer.clone(),
        BitDepth::Bits16 | BitDepth::Bits24 => {
            let bits = match depth {
                BitDepth::Bits16 => 16,
                BitDepth::Bits24 => 24,
                BitDepth::Float32 => unreachable!("handled above"),
            };
            let noise_shaped = bits == 16;
            let mut channel_rng = Qrng::from_seed(Seed::from_text("qinari-master-dither"));
            let planes = buffer
                .channels()
                .iter()
                .map(|channel| {
                    let mut dither = Dither::new(bits, noise_shaped, channel_rng.fork());
                    channel
                        .iter()
                        .map(|&sample| dither.process(sample))
                        .collect::<Vec<f32>>()
                })
                .collect();
            AudioBuffer::from_channels(planes, buffer.sample_rate())
                .expect("channel shapes and rate are preserved from the input")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tone;
    use qinari_dsp::buffer::db_to_linear;
    use qinari_dsp::lufs::integrated_lufs;

    const SR: u32 = 48_000;
    /// LUFS anchor: a 997 Hz stereo sine at -20 dBFS per channel measures
    /// -20 LUFS to within half a dB.
    const ANCHOR_AMPLITUDE: f32 = 0.1;

    fn anchor() -> AudioBuffer {
        tone(997.0, ANCHOR_AMPLITUDE, 1.5, 2, SR)
    }

    fn mastered_tone(character: Character) -> (AudioBuffer, MasteredReport) {
        let settings = MasteringSettings {
            character,
            ..MasteringSettings::default()
        };
        let (output, report) = master(&anchor(), &settings);
        let measured = integrated_lufs(&output).expect("mastered tone is measurable");
        assert!(
            (measured - f64::from(settings.target_lufs)).abs() <= 1.0,
            "{character:?}: output {measured} LUFS, target {}",
            settings.target_lufs
        );
        let ceiling = db_to_linear(settings.ceiling_db);
        assert!(
            output.peak() <= ceiling * 1.02,
            "{character:?}: peak {} exceeds ceiling {ceiling}",
            output.peak()
        );
        assert!(output.peak() > 0.05, "{character:?}: output is silent");
        assert!(!output.has_non_finite(), "{character:?}: non-finite output");
        (output, report)
    }

    #[test]
    fn transparent_master_hits_the_target_and_converges() {
        let (output, report) = mastered_tone(Character::Transparent);
        let measured = integrated_lufs(&output).expect("measurable");
        assert!(
            (measured - -16.0).abs() <= 0.5,
            "verification loop did not converge: {measured} LUFS"
        );
        let input = report.input_lufs.expect("input is measurable");
        assert!(
            (input - -20.0).abs() <= 0.5,
            "input anchor measured {input} LUFS"
        );
        assert!(
            report.applied_gain_db > 0.0 && report.applied_gain_db < 10.0,
            "applied gain {} dB",
            report.applied_gain_db
        );
        assert_eq!(report.output_peak, output.peak());
        assert!((report.output_lufs.expect("report carries output LUFS") - measured).abs() < 1e-9);
    }

    #[test]
    fn warm_master_hits_the_target() {
        mastered_tone(Character::Warm);
    }

    #[test]
    fn punchy_master_hits_the_target() {
        mastered_tone(Character::Punchy);
    }

    #[test]
    fn mastering_silence_and_short_buffers_is_safe() {
        // Silence cannot be measured: gain stays zero, chain still runs.
        let silence = AudioBuffer::new(2, SR as usize, SR);
        let (output, report) = master(&silence, &MasteringSettings::default());
        assert_eq!(output.frames(), silence.frames());
        assert_eq!(output.peak(), 0.0);
        assert_eq!(report.input_lufs, None);
        assert_eq!(report.output_lufs, None);
        assert_eq!(report.applied_gain_db, 0.0);

        // Shorter than one 400 ms measurement block: no measurement, but the
        // buffer passes through the chain intact.
        let short = tone(997.0, 0.5, 0.2, 1, SR);
        let (output, report) = master(&short, &MasteringSettings::default());
        assert_eq!(output.frames(), short.frames());
        assert_eq!(output.num_channels(), 1);
        assert_eq!(report.input_lufs, None);
        assert!(output.peak() > 0.0);
        assert!(!output.has_non_finite());
    }

    #[test]
    fn mono_master_respects_the_ceiling() {
        // A hot mono tone exercises the lone-channel limiter wiring.
        let loud = tone(220.0, 0.9, 1.0, 1, SR);
        let (output, _report) = master(&loud, &MasteringSettings::default());
        let ceiling = db_to_linear(-1.0);
        assert!(
            output.peak() <= ceiling * 1.02,
            "mono peak {} exceeds ceiling {ceiling}",
            output.peak()
        );
        assert!(!output.has_non_finite());
    }

    #[test]
    fn finalize_16_bit_adds_a_noise_floor_without_dc_bias() {
        let input = tone(997.0, 10.0_f32.powf(-40.0 / 20.0), 0.5, 2, SR);
        let finalized = finalize_for_depth(&input, BitDepth::Bits16);
        let lsb: f64 = 1.0 / 32_768.0;
        for channel in 0..2 {
            let channel_samples = &finalized.channels()[channel];
            let original = &input.channels()[channel];
            let mut noise_sum = 0.0f64;
            let mut mean = 0.0f64;
            for (&quantized, &source) in channel_samples.iter().zip(original) {
                let plain = (source * 32_768.0).round() / 32_768.0;
                noise_sum += f64::from(quantized - plain).powi(2);
                mean += f64::from(quantized - source);
            }
            let count = channel_samples.len() as f64;
            let noise_rms = (noise_sum / count).sqrt();
            assert!(
                noise_rms > 0.1 * lsb,
                "channel {channel}: dither noise floor missing (rms {noise_rms})"
            );
            assert!(
                (mean / count).abs() < 0.1 * lsb,
                "channel {channel}: DC bias {:.3} LSB",
                mean / count / lsb
            );
        }
        // Determinism: the same seed produces the same dither.
        assert_eq!(finalized, finalize_for_depth(&input, BitDepth::Bits16));
    }

    #[test]
    fn finalize_24_bit_dithers_flat_and_deterministically() {
        let input = tone(997.0, 10.0_f32.powf(-40.0 / 20.0), 0.5, 2, SR);
        let finalized = finalize_for_depth(&input, BitDepth::Bits24);
        let mut differences = 0usize;
        for channel in 0..2 {
            for (&quantized, &source) in finalized.channels()[channel]
                .iter()
                .zip(&input.channels()[channel])
            {
                let plain = (source * 8_388_608.0).round() / 8_388_608.0;
                if (quantized - plain).abs() > 0.0 {
                    differences += 1;
                }
            }
        }
        assert!(
            differences > 100,
            "flat dither changed only {differences} samples"
        );
        assert_eq!(finalized, finalize_for_depth(&input, BitDepth::Bits24));
    }

    #[test]
    fn finalize_float_is_a_bit_exact_passthrough() {
        let input = tone(997.0, 0.5, 0.1, 2, SR);
        let finalized = finalize_for_depth(&input, BitDepth::Float32);
        assert_eq!(finalized, input);
    }
}

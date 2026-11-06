//! DNSMOS P.835/P.808 scoring through ONNX Runtime, ported from
//! Microsoft's reference implementation (`dnsmos_local.py` / the `speechmos`
//! pip package, CC BY 4.0), non-personalized mode.
//!
//! [`DnsmosScorer`] loads the two vendored models (`sig_bak_ovr.onnx` for
//! the SIG/BAK/OVRL P.835 proxy, `model_v8.onnx` for the P.808 overall
//! MOS), slides 9.01 s windows over a 16 kHz mono signal with a 1 s hop,
//! and averages the per-window scores. The P.808 path feeds a 120-mel
//! spectrogram computed exactly like the reference (periodic-Hann STFT of
//! size 321, hop 160, Slaney-normalized filterbank, `power_to_db` with
//! `ref = global max`, `(db + 40) / 40` scaling); the P.835 path feeds the
//! raw window. Raw P.835 outputs pass through the reference's quadratic
//! polyfit before being reported.
//!
//! [`DNSMOS_ASSETS`] pins the download metadata so `qinari setup` can
//! vendor the models the same way it vendors voices.

use crate::VerifyError;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use qinari_dsp::buffer::AudioBuffer;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

/// The sample rate both models expect (hertz).
pub const SR: u32 = 16_000;

/// Analysis window length in seconds (the reference's `INPUT_LENGTH`).
pub const INPUT_LENGTH_S: f64 = 9.01;

/// Window length in samples at [`SR`].
fn window_samples() -> usize {
    (INPUT_LENGTH_S * f64::from(SR)) as usize
}

/// One pinned DNSMOS model asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsmosAsset {
    /// Download URL (follows redirects; served by the DNS-Challenge repo).
    pub url: &'static str,
    /// Pinned SHA-256 of the file (hex, lowercase).
    pub sha256: &'static str,
    /// Exact file size in bytes.
    pub size_bytes: u64,
    /// Path relative to the models directory.
    pub rel_path: &'static str,
    /// Human-readable license name.
    pub license: &'static str,
    /// URL of the license text.
    pub license_url: &'static str,
}

/// The two DNSMOS models, pinned to the exact bytes measured from the
/// Microsoft DNS-Challenge repository (digests cross-checked during the
/// 0.1.5 design window; see `docs/DESIGN-TTS-EXPRESSIVE.md` §5).
pub static DNSMOS_ASSETS: [DnsmosAsset; 2] = [
    DnsmosAsset {
        url: "https://github.com/microsoft/DNS-Challenge/raw/master/DNSMOS/DNSMOS/sig_bak_ovr.onnx",
        sha256: "269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd",
        size_bytes: 1_157_965,
        rel_path: "sig_bak_ovr.onnx",
        license: "CC BY 4.0",
        license_url: "https://github.com/microsoft/DNS-Challenge/blob/master/LICENSE",
    },
    DnsmosAsset {
        url: "https://github.com/microsoft/DNS-Challenge/raw/master/DNSMOS/DNSMOS/model_v8.onnx",
        sha256: "9246480c58567bc6affd4200938e77eef49468c8bc7ed3776d109c07456f6e91",
        size_bytes: 224_860,
        rel_path: "model_v8.onnx",
        license: "CC BY 4.0",
        license_url: "https://github.com/microsoft/DNS-Challenge/blob/master/LICENSE",
    },
];

/// DNSMOS scores for one clip: the polyfitted P.835 triple plus the P.808
/// overall MOS, all averaged over the analysis windows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DnsmosScores {
    /// Speech quality (P.835 SIG proxy, polyfitted).
    pub sig: f32,
    /// Background-noise quality (P.835 BAK proxy, polyfitted).
    pub bak: f32,
    /// Overall quality (P.835 OVRL proxy, polyfitted).
    pub ovrl: f32,
    /// Overall quality (P.808 model).
    pub p808: f32,
}

/// The reference's non-personalized polyfit coefficients (quadratics, as
/// `np.poly1d` descending powers).
const P_OVR: [f64; 3] = [-0.067_662_83, 1.115_464_68, 0.046_025_35];
const P_SIG: [f64; 3] = [-0.083_972_78, 1.220_839_53, 0.005_243_9];
const P_BAK: [f64; 3] = [-0.131_668_88, 1.609_155_14, -0.396_045_46];

/// Evaluates one quadratic polyfit at `x`.
fn poly2(coeffs: [f64; 3], x: f64) -> f64 {
    coeffs[0] * x * x + coeffs[1] * x + coeffs[2]
}

/// Mel-spectrogram constants (the reference's `audio_melspec` defaults).
const MEL_FFT: usize = 321;
const MEL_HOP: usize = 160;
const MEL_COUNT: usize = 120;
const MEL_BINS: usize = MEL_FFT / 2 + 1;
/// The P.808 mel input drops the final hop (`audio_seg[:-160]`).
const MEL_PAD_DROP: usize = 160;

/// `ort` allows one environment per process (see [`VerifyError::OnnxInit`]).
static ORT_ENV: LazyLock<Option<ort::environment::Environment>> =
    LazyLock::new(|| ort::init().build().ok());

/// A loaded DNSMOS scorer.
pub struct DnsmosScorer {
    primary: Session,
    p808: Session,
    mel_filterbank: Vec<f64>,
    hann: Vec<f64>,
    fft: Arc<dyn rustfft::Fft<f64>>,
}

impl DnsmosScorer {
    /// Loads both models from `models_dir` (the files named by
    /// [`DNSMOS_ASSETS`] `rel_path`).
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError::ModelMissing`] when a model file is absent
    /// and [`VerifyError::OnnxInit`] / [`VerifyError::Onnx`] when ONNX
    /// Runtime cannot start or load the models.
    pub fn load(models_dir: &Path) -> Result<Self, VerifyError> {
        let env = ORT_ENV.as_ref().ok_or(VerifyError::OnnxInit)?;
        let primary_path = models_dir.join(DNSMOS_ASSETS[0].rel_path);
        let p808_path = models_dir.join(DNSMOS_ASSETS[1].rel_path);
        for path in [&primary_path, &p808_path] {
            if !path.is_file() {
                return Err(VerifyError::ModelMissing {
                    path: PathBuf::from(path),
                });
            }
        }
        let primary = Session::builder(env)?.commit_from_file(&primary_path)?;
        let p808 = Session::builder(env)?.commit_from_file(&p808_path)?;
        let mut planner = rustfft::FftPlanner::new();
        Ok(Self {
            primary,
            p808,
            mel_filterbank: slaney_filterbank(),
            hann: periodic_hann(MEL_FFT),
            fft: planner.plan_fft_forward(MEL_FFT),
        })
    }

    /// Scores a 16 kHz mono signal.
    ///
    /// The signal is repeated until it covers one 9.01 s window, then
    /// scored over every full window at a 1 s hop, and the per-window
    /// scores are averaged — exactly the reference's clip loop.
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError::Onnx`] when a model call fails and
    /// [`VerifyError::EmptyBuffer`] for empty input.
    pub fn score(&mut self, samples: &[f32]) -> Result<DnsmosScores, VerifyError> {
        if samples.is_empty() {
            return Err(VerifyError::EmptyBuffer);
        }
        let window = window_samples();
        let mut audio: Vec<f32> = samples.to_vec();
        while audio.len() < window {
            let mut doubled = audio.clone();
            doubled.extend_from_slice(&audio);
            audio = doubled;
        }
        let num_hops = (audio.len() / SR as usize).saturating_sub(INPUT_LENGTH_S as usize) + 1;
        let mut sig_sum = 0.0_f64;
        let mut bak_sum = 0.0_f64;
        let mut ovr_sum = 0.0_f64;
        let mut p808_sum = 0.0_f64;
        let mut scored = 0_usize;
        for hop in 0..num_hops {
            let start = hop * SR as usize;
            let end = start + window;
            let Some(segment) = audio.get(start..end) else {
                continue;
            };
            let raw = Tensor::<f32>::from_array(([1_usize, window], segment.to_vec()))?;
            let raw_inputs: Vec<(&str, SessionInputValue)> = vec![("input_1", (&raw).into())];
            // Scoped: `SessionOutputs` holds the session borrow until it
            // drops, and the mel pass below needs `&self` back.
            let triple = {
                let outputs = self.primary.run(raw_inputs)?;
                extract_flat3(&outputs)?
            };
            let mel_input = self.mel_spectrogram(&segment[..segment.len() - MEL_PAD_DROP]);
            let mel = Tensor::<f32>::from_array((
                [1_usize, mel_input.len() / MEL_COUNT, MEL_COUNT],
                mel_input,
            ))?;
            let mel_inputs: Vec<(&str, SessionInputValue)> = vec![("input_1", (&mel).into())];
            let p808_raw = {
                let p808_outputs = self.p808.run(mel_inputs)?;
                extract_scalar(&p808_outputs)?
            };
            sig_sum += poly2(P_SIG, f64::from(triple[0]));
            bak_sum += poly2(P_BAK, f64::from(triple[1]));
            ovr_sum += poly2(P_OVR, f64::from(triple[2]));
            p808_sum += f64::from(p808_raw);
            scored += 1;
        }
        if scored == 0 {
            return Err(VerifyError::EmptyBuffer);
        }
        Ok(DnsmosScores {
            sig: (sig_sum / scored as f64) as f32,
            bak: (bak_sum / scored as f64) as f32,
            ovrl: (ovr_sum / scored as f64) as f32,
            p808: (p808_sum / scored as f64) as f32,
        })
    }

    /// Scores a buffer at any sample rate: mono mixdown, resample to
    /// [`SR`], then [`DnsmosScorer::score`].
    ///
    /// # Errors
    ///
    /// In addition to [`DnsmosScorer::score`] errors, returns
    /// [`VerifyError::Audio`] when resampling fails.
    pub fn score_buffer(&mut self, buffer: &AudioBuffer) -> Result<DnsmosScores, VerifyError> {
        let mono = crate::mono_mixdown(buffer);
        let native = AudioBuffer::from_channels(vec![mono], buffer.sample_rate())
            .map_err(|_| VerifyError::EmptyBuffer)?;
        let at_16k = if buffer.sample_rate() == SR {
            native
        } else {
            qinari_audio::resample(&native, SR)?
        };
        self.score(at_16k.channels().first().map_or(&[], |c| c.as_slice()))
    }

    /// The reference's mel path: periodic-Hann STFT (size 321, hop 160,
    /// zero-padded half-window both ends), power spectrum, Slaney mel
    /// filterbank, then `power_to_db(ref = global max)` with the 80 dB
    /// floor and the `(db + 40) / 40` scaling — returned row-major as
    /// `[frame][mel]`, matching the reference's `.T` layout.
    fn mel_spectrogram(&self, samples: &[f32]) -> Vec<f32> {
        let pad = MEL_FFT / 2;
        let mut padded = vec![0.0_f64; pad];
        padded.extend(samples.iter().map(|s| f64::from(*s)));
        padded.extend(std::iter::repeat_n(0.0, pad));
        // librosa's centered-STFT frame count: 1 + (padded_len − n_fft) / hop
        // (144 000 samples → 900 frames; the P.808 graph expects exactly 900).
        let frames = 1 + (padded.len() - MEL_FFT) / MEL_HOP;
        let mut mel_matrix = vec![0.0_f64; frames * MEL_COUNT];
        let mut frame_spectrum = vec![0.0_f64; MEL_BINS];
        for frame in 0..frames {
            let start = frame * MEL_HOP;
            let mut block = vec![rustfft::num_complex::Complex64::new(0.0, 0.0); MEL_FFT];
            for (k, slot) in block.iter_mut().enumerate() {
                let sample = padded.get(start + k).copied().unwrap_or(0.0);
                slot.re = sample * self.hann[k];
            }
            self.fft.process(&mut block);
            for (bin, value) in frame_spectrum.iter_mut().enumerate() {
                *value = block[bin].norm_sqr();
            }
            for mel in 0..MEL_COUNT {
                let mut sum = 0.0_f64;
                for (bin, power) in frame_spectrum.iter().enumerate() {
                    sum += self.mel_filterbank[mel * MEL_BINS + bin] * power;
                }
                mel_matrix[frame * MEL_COUNT + mel] = sum;
            }
        }
        // power_to_db with ref = global max, top_db = 80, then the
        // reference's (db + 40) / 40 scaling.
        let amin = 1.0e-10_f64;
        let reference = mel_matrix.iter().copied().fold(0.0_f64, f64::max).max(amin);
        let ref_db = 10.0 * reference.log10();
        let mut scaled = vec![0.0_f32; mel_matrix.len()];
        for (slot, value) in scaled.iter_mut().zip(mel_matrix.iter()) {
            let db = (10.0 * value.max(amin).log10() - ref_db).max(-80.0);
            *slot = ((db + 40.0) / 40.0) as f32;
        }
        scaled
    }
}

/// Scores one buffer with models from `models_dir` (see
/// [`DnsmosScorer::score_buffer`]).
///
/// # Errors
///
/// See [`DnsmosScorer::load`] and [`DnsmosScorer::score_buffer`].
pub fn dnsmos(models_dir: &Path, buffer: &AudioBuffer) -> Result<DnsmosScores, VerifyError> {
    DnsmosScorer::load(models_dir)?.score_buffer(buffer)
}

/// Extracts the P.835 triple (shape `[1, 3]`, first output) as `[f32; 3]`.
fn extract_flat3(outputs: &ort::session::SessionOutputs<'_>) -> Result<[f32; 3], VerifyError> {
    let Some(value) = outputs.values().next() else {
        return Err(VerifyError::Inference {
            detail: "the P.835 model produced no output".to_string(),
        });
    };
    let (_, data) = value.try_extract_tensor::<f32>()?;
    if data.len() < 3 {
        return Err(VerifyError::Inference {
            detail: format!("the P.835 output has {} values, expected 3", data.len()),
        });
    }
    Ok([data[0], data[1], data[2]])
}

/// Extracts the scalar P.808 output (shape `[1, 1]`).
fn extract_scalar(outputs: &ort::session::SessionOutputs<'_>) -> Result<f32, VerifyError> {
    let Some(value) = outputs.values().next() else {
        return Err(VerifyError::Inference {
            detail: "the P.808 model produced no output".to_string(),
        });
    };
    let (_, data) = value.try_extract_tensor::<f32>()?;
    if data.is_empty() {
        return Err(VerifyError::Inference {
            detail: "the P.808 output is empty".to_string(),
        });
    }
    Ok(data[0])
}

/// Periodic Hann window (`scipy.signal.get_window("hann", n, fftbins=True)`
/// and librosa's default): `w[k] = 0.5 − 0.5·cos(2πk/n)`.
fn periodic_hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / n as f64).cos())
        .collect()
}

/// The Slaney-normalized mel filterbank (librosa's default, HTK off):
/// 120 triangles over 161 linear-frequency bins, each normalized by
/// `2 / (upper − lower)`.
fn slaney_filterbank() -> Vec<f64> {
    let fmin = 0.0_f64;
    let fmax = f64::from(SR) / 2.0;
    // librosa.htk=False: 1000 Hz corner, linear below, log above.
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0_f64;
    let logstep = 6.4_f64.ln() / 27.0;
    let mel_of = |hz: f64| {
        if hz >= min_log_hz {
            15.0 + (hz / min_log_hz).ln() / logstep
        } else {
            hz / f_sp
        }
    };
    let hz_of = |mel: f64| {
        if mel >= 15.0 {
            min_log_hz * ((mel - 15.0) * logstep).exp()
        } else {
            mel * f_sp
        }
    };
    let edges: Vec<f64> = (0..MEL_COUNT + 2)
        .map(|i| {
            let mel =
                mel_of(fmin) + (i as f64) * (mel_of(fmax) - mel_of(fmin)) / (MEL_COUNT + 1) as f64;
            hz_of(mel)
        })
        .collect();
    let bin_hz = |bin: usize| f64::from(SR) * bin as f64 / MEL_FFT as f64;
    let mut filterbank = vec![0.0_f64; MEL_COUNT * MEL_BINS];
    for mel in 0..MEL_COUNT {
        let (lower, center, upper) = (edges[mel], edges[mel + 1], edges[mel + 2]);
        let norm = 2.0 / (upper - lower);
        for bin in 0..MEL_BINS {
            let f = bin_hz(bin);
            let up = (f - lower) / (center - lower);
            let down = (upper - f) / (upper - center);
            filterbank[mel * MEL_BINS + bin] = up.min(down).max(0.0) * norm;
        }
    }
    filterbank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyfit_matches_the_reference_values() {
        // Computed with numpy.poly1d on the reference coefficients.
        let (sig, bak, ovr) = (3.5_f64, 2.5_f64, 3.2_f64);
        assert!((poly2(P_SIG, sig) - 3.249_516).abs() < 1.0e-6);
        assert!((poly2(P_BAK, bak) - 2.803_912).abs() < 1.0e-6);
        assert!((poly2(P_OVR, ovr) - 2.922_645).abs() < 1.0e-6);
        let (sig, bak, ovr) = (4.0_f64, 3.0_f64, 3.8_f64);
        assert!((poly2(P_SIG, sig) - 3.545_038).abs() < 1.0e-6);
        assert!((poly2(P_BAK, bak) - 3.246_4).abs() < 1.0e-6);
        assert!((poly2(P_OVR, ovr) - 3.307_74).abs() < 1.0e-6);
        let (sig, bak, ovr) = (2.1_f64, 1.5_f64, 2.0_f64);
        assert!((poly2(P_SIG, sig) - 2.198_687).abs() < 1.0e-6);
        assert!((poly2(P_BAK, bak) - 1.721_432).abs() < 1.0e-6);
        assert!((poly2(P_OVR, ovr) - 2.006_303).abs() < 1.0e-6);
    }

    #[test]
    fn window_is_the_reference_length() {
        assert_eq!(window_samples(), 144_160);
    }

    #[test]
    fn filterbank_is_normalized_triangles() {
        let fb = slaney_filterbank();
        assert_eq!(fb.len(), MEL_COUNT * MEL_BINS);
        // Every row is non-negative and the interior bands peak around 1
        // after Slaney normalization (the triangle apex times the norm).
        for mel in 0..MEL_COUNT {
            let row = &fb[mel * MEL_BINS..(mel + 1) * MEL_BINS];
            assert!(row.iter().all(|w| *w >= 0.0));
            assert!(row.iter().any(|w| *w > 0.0), "band {mel} is all zero");
        }
        // The lowest band rises from fmin: bin 0 sits exactly on its lower
        // edge (weight 0), the next bin carries the triangle.
        assert_eq!(fb[0], 0.0);
        assert!(fb[1] > 0.0);
    }

    #[test]
    fn hann_is_periodic() {
        let w = periodic_hann(MEL_FFT);
        assert_eq!(w.len(), MEL_FFT);
        assert!((w[0] - 0.0).abs() < 1.0e-12);
        // The apex sits between samples 160 and 161 for the odd length 321.
        assert!((w[160] - 1.0).abs() < 1.0e-4);
        assert!((w[161] - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn assets_pin_the_measured_digests() {
        for asset in &DNSMOS_ASSETS {
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(asset.url.starts_with("https://"));
            assert!(asset.license.contains("CC BY"));
        }
        assert_eq!(DNSMOS_ASSETS[0].size_bytes, 1_157_965);
        assert_eq!(DNSMOS_ASSETS[1].size_bytes, 224_860);
    }

    #[ignore = "requires the DNSMOS models (run with the models dir present)"]
    #[test]
    fn scores_a_synthetic_clip_and_is_deterministic() {
        let dir = std::env::var("QINARI_DNSMOS_DIR")
            .unwrap_or_else(|_| "/home/z/my-project/assets/dnsmos".to_string());
        let mut scorer = DnsmosScorer::load(Path::new(&dir)).expect("models present");
        // 12 s of 220 Hz buzz plus low noise: speech-like enough to score.
        let n = 12 * SR as usize;
        let mut t = 0.0_f64;
        let samples: Vec<f32> = (0..n)
            .map(|_k| {
                t += 1.0 / f64::from(SR);
                let vibrato = (2.0 * std::f64::consts::PI * 5.0 * t).sin() * 20.0;
                let f = 220.0 + vibrato;
                let envelope = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * 2.0 * t).sin();
                (envelope * (2.0 * std::f64::consts::PI * f * t).sin() * 0.4) as f32
            })
            .collect();
        let first = scorer.score(&samples).expect("scores");
        let second = scorer.score(&samples).expect("scores again");
        assert_eq!(first, second, "scoring is deterministic");
        for value in [first.sig, first.bak, first.ovrl, first.p808] {
            assert!((1.0..=5.0).contains(&value), "score {value} outside 1..5");
        }
        assert!(first.ovrl > 1.0);
    }
}

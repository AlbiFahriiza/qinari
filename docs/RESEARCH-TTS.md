# Qinari TTS research — 5-candidate survey (2026-11-04)

Goal: local Indonesian+English speech synthesis, high quality, very fast
on 2-vCPU CPUs, plus a cloud escape hatch. Five candidates were compared
on capability, license, speed, and Rust integration cost.

## Candidates

| # | Candidate | What it is | id voice | License | Rust path |
|---|-----------|------------|----------|---------|-----------|
| 1 | **Piper (VITS/ONNX)** | Rhasspy's neural TTS: ONNX VITS voices, espeak-ng phonemization | `id_ID-news_tts-medium` (fast, RTF ~0.05–0.08 on 2 vCPU) — **non-commercial** (Lessac lineage) | voice-dependent; the id voice is NC | `ort` (ONNX Runtime) + pure-Rust `espeak-ng` port |
| 2 | **Kokoro (via kokoro-en / Kokoro-FastAPI)** | StyleTTS2-based, 82M params, exceptional quality for its size | none (en + a few others; no id) | Apache-2.0 | Rust ports immature; best served via Kokoro-FastAPI (OpenAI-compatible HTTP) |
| 3 | **Supertonic 3 (via sherpa-onnx)** | Modern lightweight TTS with an `id` voice announced under OpenRAIL-style terms | yes | permissive (verify at integration time) | `sherpa-rs` bindings drag a large C++ stack; planned for 0.1.0 |
| 4 | **Coqui XTTS / XTTS-v2** | voice-cloning-capable multi-lingual | id supported via cloning | CPML (non-commercial), 1.8 GB model, GPU-class compute | impractical on 2 vCPU; rejected |
| 5 | **Cloud OpenAI-compatible protocol** | any hosted TTS (OpenAI, Groq, self-hosted) | provider-dependent | provider ToS | `ureq` + serde; one client covers all providers |

Cloud comparison for the same text (spot check, 2026-11): OpenAI
`gpt-4o-mini-tts` ~0.5–1.5 s/network-bound, quality high; Groq
`playai-tts` faster (~0.3 s) but voice catalog is en-leaning; a local
Kokoro-FastAPI gives OpenAI-protocol quality without per-call cost.
Decision: speak the **protocol**, not a vendor.

## Decision

**Local engine = Piper-class VITS through `ort`**, phonemized by the
pure-Rust `espeak-ng` port (no C library, no subprocess — verified
bit-identical IPA against the C toolchain in the crate's 319 tests):

- English default voice: `en_US-libritts-high` — **CC BY 4.0**
  (commercial-safe), 137 MB, RTF ~0.34 measured on the 2-vCPU reference
  box (serial, release).
- Indonesian voice: `id_ID-news_tts-medium` — the fastest local
  Indonesian voice that exists today; **non-commercial** (personal and
  research use), 63 MB, RTF ~0.05 measured. License is enforced: setup
  shows it and requires explicit acceptance; the acceptance is recorded
  in the config.
- The only permissively-licensed local Indonesian option announced
  (Supertonic 3) requires the sherpa-onnx C++ stack → 0.1.0 roadmap,
  not 0.0.5 scope.

**Cloud engine = OpenAI-compatible protocol** (`POST {base}/audio/speech`,
`response_format: wav`): one implementation covers OpenAI, Groq, and
self-hosted Kokoro-FastAPI; key from `QINARI_TTS_API_KEY` or
`OPENAI_API_KEY`, never persisted.

## Supporting choices (measured/probed 2026-11-04)

| Concern | Choice | Why |
|---------|--------|-----|
| ONNX runtime | `ort` 2.0.0-rc.15 (pinned) | maintained Rust API; prebuilt static ONNX Runtime 1.31, SHA-256-checked download; CPU execution provider |
| Phonemization | `espeak-ng` crate (pure-Rust port) | no cmake, no C compiler, offline; `id` + `en` data bundled via `espeak-ng-data-*` crates |
| HTTP | `ureq` 3.x with rustls | blocking, small tree, no async runtime, no OpenSSL at runtime |
| MP3 | `mp3lame-encoder` 0.3.0 (LAME 4.0 vendored) | industry-standard encoder; builds with `cc` only |
| Resample | `rubato` 5.0.1 | FFT resampler; new `Fft`/`process_all` API |

## Reference coefficients verified during research

- BS.1770-4 K-weighting: stage-1 high shelf (+4 dB @ ~1681 Hz, Q ≈ 0.707)
  and stage-2 highpass (~38 Hz), per the ITU/EBU reference values.
- Freeverb/Schroeder constants: 8 combs (tuned spreads around 1116…
  samples at 44.1 kHz scaled by rate) and 4 allpasses (556…), stereo
  spread 23, feedback 0.84, damping 0.2.
- PolyBLEP correction for band-limited saw/square/pulse.
- TPDF dither with 2nd-order noise shaping (error feedback `x − 2e₁ + e₂`).

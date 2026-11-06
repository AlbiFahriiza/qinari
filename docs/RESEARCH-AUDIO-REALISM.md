# Audio Realism Research — Groundwork for 0.1.0

Status: research complete and verified. This document records everything
learned in the realism/TTS/architecture investigation that feeds the
`DESIGN-REALISM-ENGINE.md` blueprint. It is written in full, not summarized:
each finding carries its evidence and its source.

## Method and evidence notation

Three parallel research agents (R1 synthesis realism, R2 TTS quality, R3
engine architecture) each read the Panoptes discipline skill, grounded
themselves in this repository, and returned sourced reports. Their key
claims were then re-verified by the main agent before entering this
document. Evidence tags:

- `[READ n.m]` — claim read directly in this repository, `file:line`.
- `[MEAS]` — measured this session by running code or an experiment
  (commands and numbers recorded here).
- `[MAIN-FETCH]` — web source opened and quoted by the main agent.
- `[AGENT-FETCH]` — web source opened and quoted by a research agent; not
  re-opened by the main agent (spot-checks of the load-bearing subset all
  matched, see §5).
- `[INFER]` — reasoned, not sourced. Flagged, not load-bearing.

## Part 1 — Why procedural audio sounds synthetic

### 1.1 The statistics listeners actually use

The decisive result is McDermott & Simoncelli's texture-synthesis work
(Neuron 2011; abstract re-fetched and quoted by the main agent from the
PMC full text):

> "Statistics of individual frequency channels, capturing spectral power
> and sparsity, generally failed to produce compelling synthetic textures.
> However, combining them with correlations between channels produced
> identifiable and natural-sounding textures." `[MAIN-FETCH]`

Their model: cochlear bandpass filters → per-channel envelope extraction →
compressive nonlinearity → marginal statistics (M) of envelopes,
**cross-channel correlations (C1)**, **within-band autocorrelation (AC)**,
and modulation-filterbank statistics (C2). Naturalness is judged from
these time-averaged joint statistics, not from per-band power alone.
McDermott 2009 (ASPAA) reached the same conclusion one step earlier:
matching marginal statistics is necessary but insufficient; imposing
pairwise envelope correlations is what improves naturalness
`[AGENT-FETCH, DOI 10.1109/aspaa.2009.5346467]`.

Two supporting anchors:

- Long-range correlation structure in natural sound is 1/f-shaped (Voss &
  Clarke 1975, *Nature* 258:317, DOI 10.1038/258317a0) `[AGENT-FETCH]`.
- Envelopes carry much of the information listeners use (Shannon et al.
  1995, *Science* 270:303, DOI 10.1126/science.270.5234.303)
  `[AGENT-FETCH]`; the McDermott 2011 intro cites this line explicitly.
- Listeners directly discriminate departures from perfect interaural
  correlation (Gabriel & Colburn 1981, JASA, DOI 10.1121/1.385821)
  `[AGENT-FETCH]` — relevant because Qinari's beds are currently
  sample-identical left/right.

**Consequence:** an engine whose layers move on independent, exactly
periodic LFOs, whose events arrive memorylessly, and whose beds are
mono-duplicated violates C1, AC, and interaural realism simultaneously.
That is the "generated" signature — the audio equivalent of AI slop — and
it is structural, not a tuning accident.

### 1.2 Qinari's specific synthetic tells (gap inventory)

From a full read of the DSP and recipe layers `[READ]`:

| # | Tell | Where | Violated statistic |
|---|---|---|---|
| 1 | Bed motion is one exactly-periodic sinusoid per layer (e.g. rain gust 0.07 Hz, wind wander 0.06 Hz + gust 0.11 Hz, fire S&H flicker 0.8 Hz), mutually uncorrelated | `recipes.rs:298,354-356,518-527` | C1/AC (cross-band correlation, envelope autocorrelation) |
| 2 | Ambient presets use the same pattern: `gain·(1−d+d·(0.5+0.5·sin(t·rate+phase)))` per layer, period `1/rate` forever | `ambient.rs:394-410` | AC (exactly periodic modulation spectrum — a spike) |
| 3 | All event streams are homogeneous Poisson (memoryless, CV=1): rain droplets, fire crackle, stream bubbles, debris ticks, birdsong phrases | `recipes.rs:182-197` and each recipe | Event-rate autocorrelation (no clustering, no refractory, no bursts) |
| 4 | Per-event parameters are independent uniforms (droplet pitch, crackle cutoff, bubble rate, debris pan) | `recipes.rs:308-325,535-542,605-618,914-922` | Parameter co-variation (physically impossible combinations occur) |
| 5 | Beds are added at pan 0.0 → identical L/R samples → interaural correlation 1.0 across the full spectrum | `recipes.rs:302,360,374,592,600` | Interaural coherence (Gabriel & Colburn) |
| 6 | One-shots have zero attack: `Percussive::trigger` sets level 1 instantly; `noise_burst` has no attack ramp, only an LP step response | `env.rs:166-168`, `noise.rs:115-119` | Transient statistics (every event shares one onset shape) |
| 7 | Only two envelope shapes exist (ADSR one-pole, percussive exponential); no two-stage decay | `env.rs` | Envelope marginal diversity |
| 8 | Tonal layers are pure sines or bare KS plucks; no inharmonic partial sets, no modal banks | `osc.rs`, `karplus.rs:19-44` | Spectral fine structure of struck objects |
| 9 | `Freeverb`, `Delay`, `Compressor` exist in `qinari-dsp` but **no sfx recipe calls them** (grep-verified) — no early reflections, no late tail, no environmental context | `fx.rs:114-400` unused by `recipes.rs` | Reverberation statistics (all events render anechoic) |
| 10 | Explosion = 4 positive-amplitude layers; no shock front, no rarefaction (negative phase), no ground reflection | `recipes.rs:853-927` | Blast physics (see §2.3) |
| 11 | Thunder = one crack + one echo layer at 1.05 s | `recipes.rs:390-459` | Distributed-source multipath (see §2.3) |
| 12 | Birdsong notes are bare sines with sinusoidal vibrato; motifs are built once and reused verbatim | `recipes.rs:692-832` | Source-filter structure + repetition-with-variation (see §2.4) |
| 13 | Analysis side is Goertzel + ZCR only — no FFT, no spectrogram, no modulation spectrum; the engine cannot measure what it renders | `analysis.rs:9-57` | Enables none of the above to be checked |
| 14 | Mastering normalizes everything to −16 LUFS / −1 dBFS, flattening macro-dynamics between distant and near events | `master.rs:95-175` | Crest-factor variation |

Current primitives that a realism upgrade can build on immediately
`[READ]`: PolyBLEP oscillators with random initial phase, RBJ biquads +
TPT SVF (retunable with state preserved), Kellet pink noise, xoshiro256++
tagged streams with fork, Karplus-Strong pluck, one-pole/DC filters,
BS.1770-4 loudness, TPDF dither, equal-power pan, 5 ms-lookahead
brickwall limiter.

### 1.3 Why tuned Python pipelines sound better

The repo's own `benchmarks/numpy_reference.py` mirrors `render_rain` in
NumPy/SciPy `[READ]`. The advantage of that workflow is not vectorization:
it is the **feedback loop**. SciPy gives periodograms and spectrograms in
one line, so a Python author iterates render → inspect spectrum → adjust
until it resembles a reference. McDermott 2009 describes exactly this loop
made systematic: "a synthesis algorithm that measures statistics in a real
sound and imposes them on a sample of noise" `[AGENT-FETCH]`. Qinari today
has no measurement side at all (§1.2 #13), so every recipe is tuned blind.
The fix is not Python; it is closing the loop inside the engine (Part 3).

## Part 2 — Realism techniques (what to build)

### 2.1 Shared latent modulation (co-variation)

Replace per-layer sinusoid LFOs with **one slow stochastic latent per
render** (band-limited noise through chained one-poles at 0.05–0.5 Hz,
optionally 1/f-tilted) that drives bed level, filter cutoffs, gust depth,
**and event rates together**. Cross-band envelope correlation then exists
by construction, and the modulation spectrum is broadband rather than a
spike. All required primitives exist (`NoiseGen`, `OnePole`, existing
`Lfo` plumbing) — this is wiring, not new math. Rationale: the McDermott
result says correlations are the decisive statistics (§1.1).

### 2.2 Transient and envelope design

Give every one-shot an attack ramp (0.2–5 ms) and a two-stage decay
(fast τ₁ into slow τ₂) as a new envelope type. Zero-attack onsets are the
single most uniform "sample-pack" tell; envelopes carry identity (Shannon
1995). Pure-cost change, no new dependencies `[READ env.rs → design]`.

### 2.3 Physical models

**Blast (explosion family).** A real detonation in free field produces the
Friedlander waveform `[MAIN-FETCH, Wikipedia "Blast wave"]`:

    P(t) = Ps · e^(−t/t*) · (1 − t/t*)

with Ps the peak overpressure and t* the first zero crossing, followed by
a genuine **negative-pressure phase** ("a blast wind of negative gauge
pressure, which sucks items back in towards the center"). Ground
reflection produces a **Mach stem**: the reflection catches the original
shock front, creating a high-pressure zone from the ground to the triple
point, "peak pressures… several times higher than the peak pressure of the
original shock front" (same source). Distance lengthens the positive
phase and low-passes the spectrum (air absorption grows superlinearly
with frequency; standardized method ISO 9613-1:1993, DOI
10.3403/bsiso9613 `[AGENT-FETCH]`; specific dB/m values remain unverified
— treat as `[INFER]`). Design shape: sub-ms broadband shock click →
Friedlander-shaped body incl. rarefaction → 2–4 ground-reflection taps at
distance-scaled delays → distance-dependent LP + rumble proportion.

**Thunder.** The rumble is not one echo: Roy (JASA 1980, DOI
10.1121/1.2004715) mapped tortuous 3-D lightning channels to thunder by
Monte-Carlo simulation — "the tortuosity of the initiating lightning
stroke is responsible for the long duration rumble and roll… a rectilinear
channel yields but a single short thunderclap" `[AGENT-FETCH]`. Bodhika
2019 (*Applied Acoustics*, DOI 10.1016/j.apacoust.2018.09.018): clap
energy concentrates below ~300 Hz `[AGENT-FETCH]`. Design shape:
distributed-source model — 4–8 arrivals spread over 1–5 s with
distance-scaled delays and HF damping, driven by one channel-tortuosity
parameter.

**Modal synthesis (impacts).** Struck objects are superpositions of
decaying modes; a `ModalBank { [(f_i, Q_i, t60_i)] }` of existing biquad
resonators excited by shaped noise covers wood/metal/ceramic/thud
(van den Doel & Pai 2007, DOI 10.1201/9781439864869-7 `[AGENT-FETCH]`).
Smith's *Physical Audio Signal Processing* (CCRMA, opened by agent) gives
the waveguide upgrade path for the existing Karplus-Strong: brightness
loop filter, pick-position comb, body resonator, fractional delay.

**Birdsong.** Three verified constraints: (a) oscine song is
source-filter: syringeal source × vocal-tract resonances (Nowicki 1987
helium experiment, *Nature* 325:53, DOI 10.1038/325053a0
`[AGENT-FETCH]`) — bare sines are structurally wrong; notes need a
harmonic/buzzy source plus 1–3 swept resonances. (b) Podos 1997
(*Evolution*, DOI 10.1111/j.1558-5646.1997.tb02441.x): trill rate and
bandwidth trade off — "maximal values of frequency bandwidth were found to
decrease with increasing trill rates" `[AGENT-FETCH]`; fast-and-wide songs
are physiologically impossible, so those two parameters must be drawn
jointly, not independently. (c) Real bouts repeat motifs with
micro-variation and species syntax `[INFER, standard ethology — Catchpole
& Slater's textbook, not opened]`: per-repetition timing/pitch jitter and
a small motif-transition matrix replace verbatim reuse.

### 2.4 Event statistics beyond Poisson

- **Cox (doubly stochastic) processes**: event rate modulated by the same
  latent as the bed (§2.1) — event clusters correlate with gusts for free.
- **Renewal processes with CV≠1**: gamma or log-normal intervals give
  refractory periods (cricket chirp groups) or heavy tails (distant
  thunder flash sequences).
- Keep near-Poisson only for genuinely high-rate streams (rain droplets).
- FMOD validates the same direction industrially: its Scatterer
  instrument randomizes spawn interval (min/max), scatter distance, and
  per-instance volume/pitch `[AGENT-FETCH, FMOD Studio Manual 2.04,
  fmod.com/docs/2.04/studio/working-with-instruments.html]`.

### 2.5 Granular and noise palette

Grains of 1–100 ms layered at varying rate/pitch/position subsume the
sparkle/bubble/crackle/debris loops and give direct control of texture
statistics (Roads 1978, CMJ, DOI 10.2307/3680222 `[AGENT-FETCH]`). Add
brown (1/f²) and tunable 1/f^α noise plus band-limited Gaussian to widen
the achievable marginals (Voss & Clarke anchor §1.1).

### 2.6 Spatial realism

Beds: decorrelate L/R above ~500 Hz (independent noise per channel,
correlated below) — kills the interaural-correlation-1.0 tell (§1.2 #5).
Events: distance model (delay Δt ∝ d, level ∝ 1/d, HF rolloff ∝ d) plus
1–2 early-reflection taps using the currently-unused `Delay`. Wire
`Freeverb` into one-shots with distance-scaled wet gain and HF-damped
feedback so events stop rendering anechoic.

## Part 3 — TTS quality: verified diagnosis

### 3.1 Pipeline as-built `[READ]`

Text → `expand_numbers` (per-locale; id: `.` thousands, `,` decimal)
`textnorm.rs:103-143` → sentence-aware chunking at 3000 chars
(`textnorm.rs:358-418`) → espeak-ng IPA per chunk (`local.rs:271-278`, via
the pure-Rust port with bundled upstream data `f135499`/1.53.0-dev) →
symbol mapping `map_symbols` (`local.rs:282-293`) → one ONNX run per chunk
with `scales = [noise_scale, length_scale/speed, noise_w]`
(`local.rs:188-195`), `sid=0` when present → channel retarget → rubato
FFT resample to 44 100 (`resample.rs:27-60`) → master (−16 LUFS, −1 dBFS
ceiling, transparent character) → dither → 24-bit WAV.

Both bundled voices carry Piper's own defaults: `noise_scale 0.667,
length_scale 1.0, noise_w 0.8`, 22 050 Hz `[MEAS — read from installed
voice.onnx.json]`. Scale order and semantics match upstream Piper
(`[AGENT-FETCH]` piper1-gpl `voice.py:545-548`: scales are
`[noise_scale, length_scale, noise_w]`; docstrings: `length_scale`
controls phoneme duration, `noise_w` phoneme-width jitter, `noise_scale`
prosody variance).

### 3.2 The PAD-interleaving bug (root cause of "aneh") — proven

**Code.** `map_symbols` emits `^ <symbols> $` with **no PAD (id 0)
between symbols** (`local.rs:282-293`); its unit test even locks the
unpadded shape (`local.rs:340`: `vec![1, 5, 3, 6, 2]`) `[READ]`. The
doc comment claims "matching Piper" — it does not.

**Upstream.** Piper's own C++ runtime inserts PAD after BOS and after
every mapped symbol when `interspersePad` is true (the default path):
`phoneme_ids.cpp` — "Pad after bos (_)" and, per phoneme, "// pad (_)"
after the mapped ids `[MAIN-FETCH — raw.githubusercontent.com/rhasspy/
piper-phonemize/master/src/phoneme_ids.cpp, quoted verbatim]`. The same
format appears in piper1-gpl's `phoneme_ids.py:190-204`, the 2023 Python
runtime, and the training pipelines themselves
`[AGENT-FETCH]` — the models were **trained** on padded sequences.

**Experiment (pad-probe) `[MEAS]`.** The same sentence was pushed through
the same ONNX session as the same id voice, once unpadded (as shipped)
and once Piper-padded. Temporary diagnostic test in `local.rs`, run with
`cargo test -p qinari-tts --lib pad_probe -- --nocapture`, then reverted
(repo clean). Two runs each to bracket run-to-run variance:

| Voice | Sentence | ids unpadded/padded | Duration unpadded | Duration padded | Δ |
|---|---|---|---|---|---|
| id_ID-news_tts-medium | "hello nama saya GLM Salam bertemu denganmu!" | 55 / 109 | 1.834 s, 1.892 s | 3.123 s, 3.088 s | **+63–70%** |
| en_US-libritts-high | "Hello, my name is GLM, nice to meet you!" | 55 / 109 | 1.625 s, 1.649 s | 2.252 s, 2.183 s | **+33–38%** |

The unpadded engine rushes speech by roughly two-thirds on the failing
sentence and by a third on English — the duration predictor, trained on
one-symbol-per-position-plus-pad sequences, compresses timing when fed
the never-seen unpadded format. Output peak also rises with pads
(id: 0.29→0.45; en: 0.26→0.41), i.e. healthier synthesis. Render WAVs of
all four probes are kept alongside this research for listening.

**Verdict.** The "weird TTS" is primarily an **engine bug** (one function,
plus its test), affecting every voice — which is exactly why both bundled
voices sounded wrong. The fix is ~10 lines in `map_symbols` plus updated
tests; it is scheduled as the first 0.1.0 change.

### 3.3 What is NOT the engine's fault (measured)

**Voice bandwidth ceiling `[MEAS]`.** Welch spectra (2 048-point windows,
50% overlap, 22 050 Hz mono) of three renderings of the id voice:

| Source | Duration | rolloff95 | 0–500 Hz | 500–1600 Hz | 1600–4000 Hz |
|---|---|---|---|---|---|
| Qinari unpadded (buggy) | 1.83 s | 1 626 Hz | 46.8% | 47.5% | 5.6% |
| Qinari padded (fixed format) | 3.12 s | 1 690 Hz | 42.9% | 48.2% | 8.7% |
| **Piper's own official sample** (`samples/speaker_0.mp3`, HF) | 12.46 s | **1 626 Hz** | 47.0% | 46.9% | 5.9% |

Piper's official sample has the **same** ~1.6 kHz 95%-energy rolloff.
The muffling is the voice, not our chain: `id_ID-news_tts-medium` is the
only Indonesian voice among 177 in the catalog, a medium-tier,
single-speaker model finetuned from the U.S.-English "lessac" voice on a
news corpus, 22 050 Hz `[AGENT-FETCH, HF MODEL_CARD + piper-samples
voices.json]`. There is no higher-quality Indonesian voice to upgrade to
today; en_US-libritts-high (LibriTTS train-clean-360, 904 speakers) is
the quality ceiling among bundled voices.

**Phonemization of the failing sentence `[MEAS]`.** System espeak-ng
1.52 `-v id --ipa` gives `hˈɛlo nˈama sˈaja ɡˌeˌɛlˈɛm sˈalam bərtˈɛmu
dəŋˈanmu!` — "hello" takes Indonesian phonology, and "GLM" (not in the
dictionary) is **spelled letter-by-letter with Indonesian letter names**
("ge-el-em"), because espeak-ng's translator falls back to letter
re-translation for unknown all-caps words (`translate.c:1784-1793`
`[AGENT-FETCH]`). This matches upstream Piper behavior for the same
voice (same espeak voice string) — not a Qinari regression, but it shapes
perception. Mitigation for 0.1.0: pre-phonemization text normalization
(acronym expansion, optional Piper-style `[[ raw phoneme ]]` escape).

**Master chain `[MEAS + READ]`.** On the failing sentence the chain is
measurably transparent: −24.9 LUFS in → −16.2 LUFS out (+8.7 dB), limiter
never engaged (out-peak −2.5 dBFS vs ceiling −1), crest 14.4→13.9 dB,
LRA unchanged. Targets match industry speech practice (Apple podcasts:
−16 LKFS ±1, TP ≤ −1 dBFS, BS.1770-5 `[AGENT-FETCH]`; Spotify −14; EBU
R128 broadcast −23 — a deliberate, quieter alternative). Resampling is
rubato's FFT/windowed-sinc class, not linear; native-22 050 and 44 100
renders have identical band profiles `[MEAS]`.

### 3.4 Remaining engine gaps (real, lower priority)

1. espeak `(lang)` switch markers are not filtered before symbol mapping
   (upstream Piper filters them). If a language switch fires mid-text,
   stray ids would be injected `[READ local.rs → design; AGENT-FETCH
   piper-phonemize phonemize.cpp:87-105]`.
2. Synthesis runs once per 3 000-char chunk; upstream synthesizes per
   sentence. Long texts risk prosody run-ons `[READ textnorm.rs vs
   AGENT-FETCH piper voice.py:373-382]`.
3. piper1-gpl peak-normalizes each chunk; Qinari does not (gain-only
   difference, absorbed by mastering) `[AGENT-FETCH]`.
4. The en voice's training used the rhasspy espeak fork's data vintage;
   our port bundles upstream master data. Minor phoneme-drift risk for
   the en voice only `[AGENT-FETCH]`.

### 3.5 TTS diagnostic checklist (for future regressions)

1. **Pad probe** (decisive, §3.2): same session, padded vs unpadded ids.
2. **Official reference A/B**: Piper's `samples/speaker_0.mp3` for the
   same voice — separates voice ceiling from engine defects (§3.3).
3. **Pure-language control sentence** — separates text effects.
4. **Other-voice control** — localizes to one voice or the engine.
5. **`--no-master` bypass** — separates the DSP chain.
6. **Text-layer swap** (acronym → spelled form) — separates phonemization
   expectations from synthesis defects.
7. **Loudness-match every A/B** before listening (quieter ≠ worse).

## Part 4 — Engine architecture and objective verification

### 4.1 What professional middleware does (and what maps offline)

FMOD Studio 2.04 concepts `[AGENT-FETCH, fmod.com/docs/2.04/studio/…]`:

- **Events** are instanceable units containing **tracks** ("tracks
  function as buses within the event, allowing each event instance to
  function as a small mixer") routing into a **bus graph** with sends.
- **Multi instruments** are weighted playlists whose default **Shuffle**
  mode exists "to avoid obvious repetition of audio, in order to make
  game worlds sound less artificial and more realistic"; selection modes:
  Shuffle / Randomize / Sequential.
- **Scatterer instruments** continuously spawn playlist instances with
  min/max spawn interval, min/max scatter distance, per-instance
  volume/pitch randomization — the industrial version of §2.4.
- **Random modulators** offset a property by a per-trigger uniform draw.
- FMOD itself recommends **offline baking** for deterministic content —
  an offline renderer is exactly the baking tool, so the middleware
  object model maps onto Qinari minus real-time-only features.

FAUST (block-diagram DSP language compiling to Rust among others) and
FunDSP (Rust graph notation with analytic frequency responses) prove that
typed, composable graph IRs are practical and zero-cost
`[AGENT-FETCH]`. Qinari already places events at exact frame indices;
the missing piece is structure (buses, per-layer chains, declarative
variation), not timing.

### 4.2 Analysis-by-synthesis: the closed loop

Lineage `[AGENT-FETCH]`:

- McDermott & Simoncelli 2011 + the Sound Texture Synthesis Toolbox
  (mcdermottlab.mit.edu) — match reference statistics by iteration.
- Caracalla & Roebel 2019 (arXiv:1910.09497) — optimization until
  cross-correlations of feature maps resemble the original texture.
- Gutiérrez et al. 2025 (arXiv:2506.04073, TexStat) — statistics set
  doubling as a **validation metric**; the synthesizer (filtered noise +
  imposed envelopes) is structurally close to Qinari's beds.
- Garcia et al. 2026 (arXiv:2606.00629, QuAP) — perceptually informed
  parameter guidance "derived from empirical optimisation" around a
  procedural engine.
- Negative result, recorded for hygiene: a "Parker et al. 2024 procedural
  sound design" paper hypothesized during planning **does not exist** on
  arXiv (author search returned only unrelated work) — do not cite it.

Optimizers: **Nelder-Mead** (deterministic given the initial simplex;
Rust `argmin` 0.11.0) for ≤ ~10 macro-params (~100–300 renders);
**CMA-ES** (seedable — `cmaes` 0.2.2 `options.seed`, verified in its
source by the agent) for 10–40 params, non-convex responses (~1–5k
renders). Budget arithmetic at measured render costs (43 ms per 10 s
rain) puts 300 renders ≈ 13 s `[MEAS base × INFERRED arithmetic]` —
comfortably an authoring-time loop, never in the render path.

Qinari already contains a miniature closed loop: mastering re-measures
LUFS and corrects within ±0.3 LU over ≤3 passes (`master.rs:148-174`)
`[READ]`. The architectural move is generalizing that pattern from one
scalar to a feature vector.

### 4.3 Objective quality metrics for CI (ranked)

| Rank | Metric | Role | Cost |
|---|---|---|---|
| 1 | Null test (polarity-invert + sum) | exact regression gate; already byte-determinism-tested | trivial |
| 2 | Log-spectral distance + multi-resolution STFT loss vs golden renders | spectral-fidelity regression (Yamamoto et al. 2019, arXiv:1910.11480 defines multi-res STFT) | ~100 lines over realfft |
| 3 | Statistical texture features (envelope moments, modulation spectrum, event-rate stats) vs authored targets | the realism signal without ML (TexStat direction) | STFT + envelopes |
| 4 | BS.1770-4/-5 loudness + true peak | invariant (already implemented, 0.0 LU deviation) | done |
| 5 | FAD (Fréchet Audio Distance) | best reference-free perceptual proxy (Kilgour 2018, arXiv:1812.08466) but needs ML embeddings (FADtk) | out-of-CI, local audit |
| 6 | PESQ / ViSQOL | speech-only; ITU licensing question for PESQ; Rust crates exist but unproven (413/12 downloads) | parked, TTS-only |

### 4.4 Rust ecosystem audit (verified against crates.io / docs.rs)

Adopt: `realfft` 3.5.1 (MSRV 1.61, already in tree via rubato),
`rustfft` 6.4.1, `argmin` 0.11.0, `cmaes` 0.2.2 (all MIT or Apache-2.0 /
MIT). Skip: `dasp` (unmaintained since 2020), `aubio-rs` (2021, C
bindings), `cpal` (I/O only, offline-irrelevant), `biquad` (redundant),
`stft`/`spectrogram` crates (2015/2016). `fundsp` 0.23.0 — maintained,
inspiration/reference for the graph IR, not a hard dependency. All are
dependency crates; the workspace `forbid(unsafe_code)` binds only our
own crates (Cargo book: `[lints]` configures the package's own
compilation) `[AGENT-FETCH + READ Cargo.toml:39-41]`.

### 4.5 Verified infrastructure defect: MSRV is currently fiction

The workspace declares `rust-version = "1.85"` (`Cargo.toml:17`), but the
locked `rubato 5.0.1` declares `rust-version = "1.87"` in its manifest —
verified directly from the published tag's Cargo.toml
`[MAIN-FETCH raw.githubusercontent.com/HEnquist/rubato/v5.0.1/Cargo.toml]`
(the crates.io API's `rust_version` field returns null for it —
inconsistent metadata, but cargo enforces the manifest). rubato 5.0.0
also declares 1.87; 4.2.1 declares none. CI has no MSRV job, so nothing
caught this. **A Rust 1.85 toolchain cannot build Qinari 0.0.5-alpha as
locked.** 0.1.0 must either pin rubato 4.2.1 or raise the MSRV to 1.87 —
and add an MSRV CI job either way.

## Part 5 — Main-agent verification log

Claims re-verified independently, with method and result:

1. **PAD missing in `map_symbols`** — read `local.rs:282-293` and its
   test. Result: confirmed, no PAD, comment falsely says "matching Piper".
2. **Piper inserts PAD** — fetched and quoted
   `piper-phonemize/src/phoneme_ids.cpp` (master). Result: confirmed,
   pad after BOS and after every symbol; EOS unpadded.
3. **Acoustic effect** — pad-probe experiment (§3.2). Result: +63–70%
   duration on id voice, +33–38% on en voice, both far outside the
   ±0.03–0.06 s run-to-run variance.
4. **Voice ceiling** — Welch spectra of our renders vs Piper's official
   `speaker_0.mp3` (§3.3). Result: identical ~1.6 kHz rolloff95 →
   muffling is the voice, not the engine.
5. **Friedlander equation + negative phase + Mach stem** — fetched
   Wikipedia "Blast wave". Result: quoted verbatim, matches R1's report.
6. **McDermott & Simoncelli correlation result** — fetched PMC full
   text. Result: abstract quote matches R1's report exactly.
7. **rubato MSRV conflict** — fetched the v5.0.1 and v5.0.0 tags'
   Cargo.toml and the crates.io API. Result: manifest declares 1.87;
   API field null; CI has no MSRV job. Conflict real.
8. **"Parker et al. 2024"** — R3's arXiv author search (negative
   result) accepted as reported and recorded as a rejected citation.

Spot-check policy: every load-bearing claim entering
`DESIGN-REALISM-ENGINE.md` was either re-verified by the main agent (1–7)
or is tagged `[AGENT-FETCH]`/`[INFER]` above and is not load-bearing
without that tag.

## Part 6 — Sources

Primary papers: McDermott & Simoncelli 2011 (PMC4143345); McDermott 2009
(10.1109/aspaa.2009.5346467); Shannon et al. 1995
(10.1126/science.270.5234.303); Voss & Clarke 1975 (10.1038/258317a0);
Gabriel & Colburn 1981 (10.1121/1.385821); Podos 1997
(10.1111/j.1558-5646.1997.tb02441.x); Nowicki 1987 (10.1038/325053a0);
van den Doel & Pai 2007 (10.1201/9781439864869-7); Roy 1980
(10.1121/1.2004715); Bodhika 2019 (10.1016/j.apacoust.2018.09.018);
Dewey 2017 (10.1007/978-3-319-70831-7_3); ISO 9613-1:1993
(10.3403/bsiso9613); Roads 1978 (10.2307/3680222); Smith 2010
(ccrma.stanford.edu/~jos/pasp/); Caracalla & Roebel 2019
(arXiv:1910.09497); Gutiérrez et al. 2025 (arXiv:2506.04073); Garcia et
al. 2026 (arXiv:2606.00629); Yamamoto et al. 2019 (arXiv:1910.11480);
Kilgour et al. 2018 (arXiv:1812.08466); Hansen CMA-ES tutorial
(arXiv:1604.00772); Kim et al. VITS 2021 (arXiv:2106.06103).

Software documentation: rhasspy/piper + piper-phonemize + piper1-gpl
sources (GitHub); espeak-ng `translate.c`; Hugging Face rhasspy/
piper-voices (tree, MODEL_CARDs, official samples); piper-samples
voices.json; FMOD Studio Manual 2.04 (concepts, instruments, instrument
reference, modulator reference); FAUST; FunDSP; rubato/dasp/aubio-rs/
cpal/argmin/cmaes/pesq/visqol crate metadata (crates.io, docs.rs);
Audacity manual (null testing); Cargo book ([lints] semantics); Apple
podcast audio requirements; Spotify loudness normalization; EBU R128;
ITU-R BS.1770-5; Google ViSQOL; Microsoft FADtk; Shaker/Togelius/Nelson
*Procedural Content Generation in Games* (pcgbook.com).

Repository: every `[READ]` reference above, at tag `v0.0.5-alpha` +
the birdsong commit (`aee7694`).

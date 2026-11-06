# DESIGN — Expressive TTS (Qinari 0.1.0 / 0.2.0)

Status: proposed architecture, evidence-verified. Load-bearing claims cite
`RESEARCH-TTS-EXPRESSIVE.md` (§T), `RESEARCH-AUDIO-REALISM.md` (§R), or
`download/tts_probe/premise_results.json` (P — measured during the mandated
60-minute design window of 2026-11-06 on this repository's own 2-vCPU Xeon
reference box, the same hardware class the performance budget has always
been set on). This document supersedes the TTS scope note in
`DESIGN-REALISM-ENGINE.md` §14 ("no change to the cloud-TTS path"): the
owner's instruction of 2026-11-06 — TTS must become truly expressive at the
quality of AI-animation narrator TTS — reopens that scope. Every other
realism-design stage is untouched.

## 1. Problem: the verdict as five measured defects

"Halo! Nama saya GLM. Senang sekali akhirnya bertemu denganmu." — the user
listened and called the result obviously synthetic. Decomposed and measured:

| # | Defect | Evidence | Number |
|---|--------|----------|--------|
| 1 | Sequencer omits Piper PAD → speech rushed 63–70% | §R 3.2; **new: ASR cross-check (P4)** | current id render transcribed by a modern ASR as *"Aa du buagges ketilikits idu basak akenal"* — near-zero word accuracy; the same sentence from the probe engine transcribes 100% correctly |
| 2 | Bandwidth ceiling | rolloff95, measured (P) | id 1584–1700 Hz, en 1092 Hz |
| 3 | Flat prosody | F0 track + pause stats (P) | F0 σ 2.79–2.94 st; **0 pauses ≥150 ms** on the two-sentence greeting |
| 4 | No expressiveness surface | `engine.rs:26-44` | `SynthRequest` = text, language, speed, voice, sr, channels — nothing else |
| 5 | Baseline quality | DNSMOS P.835/P.808 (P) | OVRL 3.00–3.19, P808 3.53–3.67 |

Defect 1 is already scheduled as TTS-1 in the realism design (§R §4) and
stays first. This design adds the engine and API that fix 2–5.

## 2. Non-negotiable constraints

- **Determinism, two-tier.** SFX renders keep byte-determinism (G11,
  untouched). TTS renders keep the already-documented statistical tier:
  ONNX Runtime multi-threaded float reduction order makes outputs
  bit-unstable across runs; gates are floors and tolerances
  (`tests/live.rs` pattern). The probe engine's only stochasticity is the
  initial-latent draw (`core.py:458`, unseeded `np.random.randn` in the
  SDK) — the Rust port draws it from a seeded `Qrng` stream (§3.2), so
  renders become seed-reproducible up to ORT float-order nondeterminism;
  golden tests keep a duration tolerance (±5%) as belt-and-braces.
- **`#![forbid(unsafe_code)]`**, GPL-3.0, offline-first, `curl`-only
  downloads with SHA-256 pinning (existing voice downloader pattern).
- **Zero new runtime dependencies for 0.1.0.** The probe engine is plain
  ONNX over the already-pinned `ort` 2.0.0-rc.15 CPU execution provider —
  proven this window: the archived SDK drives the identical graphs through
  onnxruntime 1.31, the same runtime family `ort-sys` pins.
- **License gating, three classes.** `permissive` (CC BY / Apache / MIT:
  auto-install, attribute in docs), `restricted` (OpenRAIL-M: commercial
  OK, use-restriction license — show restrictions, one-time explicit
  acceptance, like the NC flow but without the NC wording), and
  `non_commercial` (current `id_ID-news_tts-medium` flow, unchanged).
- **Performance budget.** Local engines must hold RTF < 1.0 on the 2-vCPU
  box. Measured this window: probe engine RTF 0.372–0.56 (0.372 on the
  14.35 s narration demo), model load 1.1–1.2 s (P3) — ≈2× headroom.
  Resident memory ≈ the 400 MB bundle (mmap-friendly ONNX) plus working
  set — acceptable for a render CLI.
- **Size budget.** 398 MB fp32 bundle (4 ONNX + 2 JSON + per-voice 292 KB
  style) vs the existing 137 MB single-voice precedent — accepted; setup
  downloads per-file with pinned hashes (table in §5), resumable, and an
  int8 repack is a noted future option (§T 5.1).
- **CLI compatibility.** Existing subcommands and flags keep working; new
  capability arrives as new flags/voices/engines only.

## 3. Architecture

### 3.1 Engine ladder (final, from §T §5 + probe evidence)

| Lane | Engine | Verdict |
|------|--------|---------|
| Fast + Indonesian | **Supertonic 3 via `ort`** — 99M, 44.1 kHz, 31 langs incl. `id`, 10 voices, `<laugh>/<breath>/<sigh>` tags, OpenRAIL-M | Primary local engine; RTF 0.445–0.56 measured on 2 vCPU |
| Legacy | Piper VITS (both current voices) | Kept for compatibility; `id` voice remains the guaranteed NC fallback |
| Expressive (0.2.0+) | PocketTTS-ONNX (en, cloning) + challengers `qwen3-tts-0.6b-indonesian`, `pocket-tts-indonesian`, `Chatterbox-TTS-Indonesian` | Bench protocol (§3.6); adopt only if ≥2 of 3 axes beat the incumbent |
| Cloud | **ElevenLabs direct** (v4 / v4-Turbo; `id`+`jv` native; $0.04–0.08 per 1K chars) + existing OpenAI-compatible engine | TTS-E3; the only lane with *verified* narrator-class Indonesian today |

Kokoro is deliberately **not** in 0.1.0: Supertonic covers English better
for our bar (44.1 kHz + tags + measured numbers), and Kokoro's misaki G2P
port is real work (§T 4.1). Revisit only if Supertonic-English disappoints
in the listening protocol.

### 3.2 `SupertonicEngine` (new `crates/qinari-tts/src/supertonic.rs`)

Pipeline ported from the archived SDK (`supertonic/core.py`, `pipeline.py`)
and the official Rust example (`supertone-oss-archive/supertonic/rust/` —
MIT sample code, same graph orchestration). The example's own
`Cargo.toml` pins `ort = "2.0.0-rc.7"` — the same 2.0.0-rc series Qinari
already pins (rc.15), and every other dependency it names (`ndarray`,
`hound`, `serde`, `rand`, `rayon`) is already in Qinari's tree: primary
evidence that the ort path is the intended one, not an extrapolation
from the Python SDK.

```
text ── normalize (textnorm.rs, shared) ──> tokenizer (unicode_indexer.json:
         codepoint→unit map; wraps text as <lang>…</lang>, core.py:238)
  → duration_predictor.onnx  (sentence durations; conditioned on style_dp)
  → text_encoder.onnx        (units + style_ttl → latent plan)
  → vector_estimator.onnx    (flow-matching denoiser × total_steps
                              [default 8, range 1–100, config.py:201-211];
                              sig_min 1e-8, latent dim 24 — tts.json verified)
  → vocoder.onnx             (AE decoder → 44 100 Hz mono f32)
```

- **Style assets**: per-voice JSON = `{style_ttl: [1×256] f32, style_dp:
  [1×16] f32, metadata}` (format verified in M1.json this window) —
  `Style` struct with two `ndarray` views; loaded once per engine.
- **Tokenizer**: `unicode_indexer.json` is a flat array of 65 536 ints —
  direct codepoint→unit-index map with `-1` for unsupported characters
  (structure verified this window); the Rust port is a `Vec<i32>` lookup,
  plus the `<lang>…</lang>` wrap and the SDK's normalization passes
  (abbreviation expansion, special-char stripping — `core.py:180-200`).
- **Sample rate**: native 44 100 Hz; requests naming another rate are
  resampled through the existing `rubato` path (`qinari-audio`), unlike
  Piper which rejects non-native rates — a deliberate divergence (the
  cloud engine already behaves this way).
- **Sessions**: four `ort` sessions behind the existing per-process
  `ORT_ENV`; engine guarded by a `Mutex` like `LocalVitsEngine`.
- **Inference contract** (read from `core.py:503-529`, the port's source
  of truth):
  1. `duration_predictor.run({text_ids, style_dp, text_mask})` → latent
     durations; divided by `speed`.
  2. `text_encoder.run({text_ids, style_ttl, text_mask})` → text
     embedding.
  3. Initial latent: `randn(B, 24×6, ceil(wav_len/512/6))` masked to the
     predicted length (`core.py:451-461` — the SDK uses *unseeded*
     `np.random`; the Rust port draws this from a seeded `Qrng` stream, so
     Qinari renders become seed-reproducible up to ORT float-order
     nondeterminism — strictly better than the SDK).
  4. `vector_estimator.run({noisy_latent, text_emb, style_ttl, text_mask,
     latent_mask, current_step, total_step})` × `total_steps` (Euler
     flow-matching loop; `current_step`/`total_step` are f32 `[B]`).
  5. `vocoder.run({latent})` → `[B, samples]` at 44 100 Hz.
- **Chunking**: sentence-boundary chunking capped at 300 chars (the SDK's
  own cap) via `textnorm` — sentence boundaries, not arbitrary cuts, so
  the prosody pass (§3.5) owns inter-chunk pauses; the SDK's fixed 0.3 s
  inter-chunk silence becomes the `--pause` default.
- **Speed**: SDK range 0.7–2.0 maps onto `SynthRequest::speed` 0.5–2.0 with
  clamping + a warning (documented divergence).
- **LOC estimate**: 500–900 (tokenizer, 4-session orchestration, style
  JSON parse, tests). No new crates.

### 3.3 Markup layer (new `crates/qinari-tts/src/markup.rs`)

Qinari-native syntax, engine-mapped:

```
qinari tts "Pada zaman dahulu... [pause:400ms] hiduplah [laugh] seorang anak."
```

- Tags: `[laugh]` `[breath]` `[sigh]` `[pause:<ms>]` — all three expression
  tags probe-verified in Indonesian this window (the narration demo:
  14.35 s render with two `<breath>` + one `<sigh>`, 9 pauses totaling
  4.42 s, F0 σ 4.34, DNSMOS P808 4.23 vs the incumbent's 2 pauses /
  F0 σ 2.56 / P808 3.55 on the same text; files in
  `download/tts_probe/renders/demo_narrasi_*.wav`). The engine's full
  10-tag list extends this set the same way.
- Parser → `Vec<Segment>` (text run | tag event); unknown tag = hard error
  with the tag name; `--no-markup` disables parsing entirely.
- Per-engine tag maps: Supertonic `[laugh]` → `<laugh>` inline; Piper →
  strip tags, `[pause:n]` → inserted silence (the only tag the legacy
  engine can honor); ElevenLabs cloud → `[laughs]`-style passthrough
  (§T 2.2 vocabulary).
- `[pause]` and ellipses feed the same pause-inserter used by the prosody
  pass, so all engines pause identically for the same text.

### 3.4 Voice catalog v2 (`voices.rs`)

```rust
enum EngineKind { Piper, Supertonic, ElevenLabs, OpenAiCompat }
enum LicenseClass { Permissive, Restricted, NonCommercial }

struct VoiceEntry {
    id: &'static str,            // "supertonic-M1"
    engine: EngineKind,
    language: &'static str,      // "id" | "en" | "multi"
    sample_rate: u32,            // 44_100
    style_url: &'static str,     // per-voice 292 KB JSON
    style_sha256: &'static str,
    bundle: AssetBundle,         // shared engine assets (4 ONNX + 2 JSON),
                                 // each pinned {url, sha256, size}
    license: LicenseClass,
    ..                          // existing display fields
}
```

0.1.0 catalog: `en_US-libritts-high`, `id_ID-news_tts-medium` (unchanged,
compat) + `supertonic-M1…M5, F1…F5` (10 voices; one shared 398 MB bundle —
downloaded once — plus per-voice style files). `engine auto` resolves, per
language, to the highest-priority engine whose voices are installed and
whose promotion gate (§3.6) has passed; until the gate passes, `auto`
keeps today's Piper defaults and Supertonic is opt-in via `--voice`.

### 3.5 Prosody post-chain (engine-agnostic)

For flat engines (Piper) and for `[pause]`/ellipsis handling everywhere:

1. **Punctuation-aware pausing** — split on sentence/clause boundaries,
   insert silence by class: comma 160 ms, colon/ellipsis 260 ms, sentence
   end 400 ms (initial constants; tuned against `qinari analyze`
   during TTS-E2 golden authoring).
2. **Bounded rate jitter** — per-sentence length_scale multiplier drawn
   from a seeded `Qrng` stream (±4%), so narration breathes without
   breaking seeded reproducibility (statistical tier).
3. **LUFS-consistent joins** — existing mastering chain.

`--prosody flat|natural` (default: `natural` for Supertonic and cloud,
`flat` for Piper — Piper output stays byte-compatible with 0.0.5 renders
modulo TTS-1, which the user already approved).

### 3.6 Measurement and gates (the anti-"AI banget" machinery)

- **Measurement surface** — one analysis tool, not two: `qinari analyze
  <file>` (realism design VERIFY-1, §6) gains a speech feature set —
  F0 σ/range (semitones), pause count/rate, rolloff95 — plus a `--mos`
  flag that adds DNSMOS SIG/BAK/OVRL/P808 (two ONNX models, 1 157 965 +
  224 860 B, CC BY 4.0, hashes in §5; vendored exactly like voices,
  attribution in docs). Implementation lives in `qinari-verify` as the
  `speech` feature; the SFX feature set (spectrum/envelopes/modulation)
  and the speech set share the STFT machinery.
- **CI golden gates** per engine-voice (golden seed set, tolerance form):
  duration floor + ±5%, non-silence, DNSMOS OVRL ≥ 3.35 for
  `supertonic-*` (measured 3.41–3.51, margin 0.06–0.16), F0 σ ≥ 3.5 st
  (measured 4.27–4.30 untagged), ≥2 pauses per golden paragraph (measured
  4–5). Piper keeps its existing live-test floors.
- **Intelligibility gate (dev-side, pre-release)**: 30-sentence id+en
  corpus → ASR word accuracy ≥ 95% (this window's single-sentence probe
  already showed 100% vs the incumbent's near-0%; the corpus run is the
  scaled version). Tool: the same ASR used in the probe or
  faster-whisper; never shipped in the binary.
- **Promotion gate for `engine auto`**: DNSMOS + F0 + pause gates green on
  all golden seeds AND the owner's listening sign-off on the A/B pairs
  (the exact pairs already exist: `download/tts_probe/renders/
  piper_id_story.wav` vs `supertonic_id_story_M1.wav`, loudness-matched by
  the mastering chain).

## 4. Stages (reconciled with `DESIGN-REALISM-ENGINE.md` §13)

**0.1.0 — "Hearing and Honest"** (realism stages unchanged) **gains:**

- **TTS-1** — PAD fix (realism design §4, verbatim; unchanged).
- **TTS-E1 — SupertonicEngine + catalog v2 + setup.** Acceptance: renders
  id+en through `qinari tts --voice supertonic-M1`; setup pins all hashes
  (§5) and gates the OpenRAIL-M acceptance; RTF < 1.0 measured in CI on
  the 2-vCPU box (budget benchmark, like the SFX ones); golden gate set
  authored and green; legacy Piper path untouched and green.
- **TTS-E2 — markup + prosody post-chain + measurement.** Acceptance:
  `[laugh]`/`[sigh]` measurably change output (duration or F0 σ delta on
  golden — probe already shows +12% duration, +42% F0 σ); `[pause:400ms]`
  inserts exactly 400 ms ±1 frame on every engine; unknown tag = clean
  error; `qinari analyze --speech --mos` prints the §5 table format; Piper
  `--prosody natural` adds pauses to the greeting (0 → ≥2) with seeded
  reproducibility.

**0.2.0 — "Composed and Tuned"** **gains:**

- **TTS-E3 — ElevenLabs cloud engine.** ~100–150 LOC `ureq` client
  (`POST /v1/text-to-speech/{voice_id}`, `xi-api-key`, model `eleven_v4` /
  `eleven_v4_turbo`), tag passthrough per §T 2.2, mock-server tests in the
  `cloud.rs` style, key via `QINARI_ELEVENLABS_API_KEY` (never persisted);
  the existing OpenAI-compatible engine stays as the generic cloud.
- **DNSMOS-in-CI** for all golden TTS seeds (qinari-verify).
- **Challenger bench executed** (pocket-tts-id, qwen3-tts-id,
  chatterbox-id) with the §3.6 protocol; adopt/skip decision recorded in
  `docs/BENCHMARKS.md`. Adoption means a new `VoiceEntry`, not a rewrite.

**0.3.0 candidates (listed, not committed):** PocketTTS English expressive
lane (cloning), int8 repack of the Supertonic bundle, streaming render.

## 5. Evidence: this window's measurements (P)

Same box (2 vCPU Xeon), same sentences, both engines, `total_steps=8`,
`speed=1.0`:

| Metric | piper id | supertonic id M1 | supertonic id F1 | supertonic id M1 +tags | supertonic en M1 |
|---|---|---|---|---|---|
| audio s (greet / story) | 2.66–2.86 / 3.59–3.60† | 6.27 / 7.45 | — / 7.45 | 7.04 | 6.34 |
| steady RTF | 0.34–0.83* | 0.445–0.488 | 0.56 | 0.463–0.492 | 0.475–0.508 |
| F0 σ (st) | 2.79–2.94 | 4.27–4.30 | 2.21 | **6.12** | 4.99 |
| F0 range (st) | 15.0–20.8 | 19.2–29.5 | 15.1 | 30.2 | 29.1 |
| pauses ≥150 ms (greet/story) | **0** / 3 | 4 / 4 | — / 4 | 5 | 5 |
| rolloff95 Hz | 1584–1700 | 1783–2468 | 1249 | 2609 | 1754 |
| DNSMOS SIG | 3.31–3.49 | 3.67–3.70 | 3.66 | 3.74 | 3.63 |
| DNSMOS OVRL | 3.00–3.19 | 3.45–3.50 | 3.41 | 3.51 | 3.42 |
| DNSMOS P808 | 3.53–3.67 | 4.00–4.04 | 3.91 | 3.58 | 3.96 |
| ASR word accuracy (story) | ~0% ("Aa du buagges ketilikits…") | **100%** | — | — | — |

\* Piper RTF includes process start + model load (CLI subprocess); the
probe engine numbers are steady-state after warmup. Model load 1.1–1.2 s.
† Piper durations vary run-to-run (VITS noise sampling — the documented
TTS non-determinism): the greeting rendered 2.66–2.86 s across this
window's two probe runs; ranges shown.

Caveats kept honest: rolloff95 is cumulative-energy dominated (speech
energy concentrates below 3 kHz) — it separates the engines (1584→2468 Hz)
but is not a bandwidth figure; DNSMOS is a distortion MOS, not a
naturalness MOS — a dramatic whisper can score below flat speech — so it
gates *regressions*, while expressiveness is gated by the F0/pause stats
and the owner's ears (§3.6). F1 scores flatter than M1 on F0 σ — voice
choice matters; M1 is the narrator default until the listening pass.

**Ready-to-pin asset hashes** (measured this window, byte-exact against
the HF tree):

```
c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db  3700147   onnx/duration_predictor.onnx
c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff 36416150  onnx/text_encoder.onnx
883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c 256534781 onnx/vector_estimator.onnx
085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba 101424195 onnx/vocoder.onnx
42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09      8253 onnx/tts.json
9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f   277676 onnx/unicode_indexer.json
e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b   291748 voice_styles/M1.json   (M2–F5 same shape, hash at pin time)
269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd  1157965 dnsmos/sig_bak_ovr.onnx
9246480c58567bc6affd4200938e77eef49468c8bc7ed3776d109c07456f6e91   224860 dnsmos/model_v8.onnx
```

Source URL base: `https://huggingface.co/supertone-oss-archive/supertonic-3/resolve/main/…`
(pinned upstream revision `aafc6e32416a594460b32413efc49d7fe4ce6d46`, §T).

## 6. Risks

1. **Archived upstream** — no bugfixes ever. Mitigation: pinned revision +
   hashes; the graphs are static and already run unmodified; the SDK is
   MIT reference code we re-implement, not link.
2. **Supertonic-Indonesian naturalness is unproven against the
   ElevenLabs bar** — it beats the incumbent on every measured axis and
   transcribes perfectly, but "narrator-class" is ultimately the owner's
   call. That is exactly why `engine auto` promotion is gated on the
   listening protocol, not on metrics alone (§3.6).
3. **Flow-matching stochasticity** — neutralized by seeding the
   initial-noise draw ourselves (§3.2 item 3); residual ORT float-order
   nondeterminism covered by the statistical tier + tolerance gates (§2).
4. **398 MB download** — HF availability of an archived repo. Mitigation:
   hash-pinned resumable download; the original `Supertone/*` mirrors also
   carry the files (§T 1.2) as a fallback URL.
5. **Tag semantics beyond the verified three** — the engine documents 10
   tags; only `laugh`/`breath`/`sigh` are probe-verified. The markup layer
   ships only verified tags; the rest are pinned at integration after a
   listening check.
6. **Piper prosody post-chain changes legacy output** — mitigated by
   `--prosody flat` default for Piper (§3.5).

## 7. Design-window verification log

Re-checked while designing (the window's discipline):

1. All four Supertonic ONNX files + both JSON sidecars downloaded and
   byte-matched against the HF API tree; hashes above measured locally.
2. The SDK pipeline ran end-to-end through onnxruntime on this box —
   RTF, durations, and every metric in §5 are measured, not projected.
3. `<laugh>`/`<sigh>`/`<breath>` all passed the SDK's text validation in
   Indonesian and changed the output measurably (greeting: +0.77 s
   duration, +42% F0 σ, +1 pause; narration demo: numbers in §3.3).
4. Style JSON format (style_ttl 256-d, style_dp 16-d, metadata) and
   `tts.json` architecture constants (flow-matching sig_min 1e-8, latent
   24, AE 44 100 Hz) read from the actual files, not the docs.
5. Step/speed ranges read from `config.py:201-211` (steps 1–100 default
   8; speed 0.7–2.0 default 1.05 — Qinari maps and clamps, §3.2).
6. DNSMOS byte sizes match §T 3.1 exactly; scoring code vendored from
   Microsoft's `dnsmos_local.py` (polyfit + 9.01 s windows, non-
   personalized) and cross-checked against the reference implementation.
7. Two script bugs found and fixed during the window (rolloff95 zero-pad;
   missing `ort` import) — results are from the fixed script
   (`scripts/design_premise_checks_tts.py`, outputs in
   `download/tts_probe/premise_results.json` and `renders/`).
8. ASR cross-check run on both engines' story renders — transcripts in
   §5; the incumbent's near-zero intelligibility was a finding, not an
   assumption.
9. The official Rust example's `Cargo.toml` fetched directly: it pins
   `ort = "2.0.0-rc.7"` with `ndarray`/`hound`/`serde`/`rand`/`rayon` —
   same ort release series as Qinari's rc.15 and no dependency Qinari
   lacks; the port adds no new crates by construction.

## 8. Non-goals

No voice training or fine-tuning (unchanged from the realism design §14 —
challengers are adopted only pre-trained, via the bench); no streaming
playback; no SSML (the Qinari markup subset is the whole surface); no GPU
execution provider in 0.1.0 (CPU EP only); no bundled cloud keys; no
Kokoro/misaki port while Supertonic covers the bar.

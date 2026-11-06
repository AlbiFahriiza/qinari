# Expressive TTS research — getting Qinari from "AI banget" to narrator-class (2026-11-06)

Scope: what currently shipping, license-clean, verifiable options exist for
**expressive** speech synthesis in **Indonesian + English**, ideally offline on
a 2-vCPU CPU box, integrable into a pure-Rust GPL-3.0 CLI that already runs
Piper-class VITS through `ort` 2.0.0-rc.15 with a pure-Rust espeak-ng port.
This doc is the successor to `RESEARCH-TTS.md` (2026-11-04) and focuses on the
**expressiveness** question, not the "any TTS at all" question.

Baseline being beaten: `id_ID-news_tts-medium` (VITS, 22 050 Hz, ~1.6 kHz
effective bandwidth, flat prosody, known PAD-sequencer bug making speech too
fast) and `en_US-libritts-high`. User verdict on current output: "TTS-nya
kedengarannya AI banget."

All facts below were re-verified against live sources on 2026-11-06 unless
explicitly marked **UNVERIFIED**. HF rate limiting blocked a handful of raw
README fetches; those claims are marked and re-attemptable later.

---

## 1. The landscape that matters (Priority A)

### 1.1 Comparison table

| Model | Params / disk | Languages (id?) | Expressiveness control | Sample rate | ONNX? | License | Commercial OK? |
|---|---|---|---|---|---|---|---|
| **Supertonic 3** (Supertone) | 99M / ~398 MB fp32 (4 ONNX files) | **31, incl. `id`** | 10 inline tags (`<laugh>`, `<breath>`, `<sigh>` …), 10 preset voice styles, zero-shot style via (paid, now dead) Voice Builder | **44.1 kHz** | **Yes — plain ONNX, official** | **OpenRAIL** | **Yes** (use-restriction based) |
| **Kokoro-82M v1.0** (hexgrad) | 82M / ~300 MB (int8 ~80 MB) | 8 (en, ja, zh, es, fr, hi, it + en-GB) — **no id** | Style vector = 256-d voice embedding (54 presets); no emotion tags | 24 kHz | Yes (community `kokoro-onnx`, official blessings) | Apache-2.0 | Yes |
| **PocketTTS** (Kyutai) | 100M (+ English 1.9M/6.9M/18.2M variants) | 7 (en, fr, de, pt, it, es, nl) — **no id** (base) | voice cloning from ~10 s reference; streaming; no emotion tags | 24 kHz | Community export exists; Rust ports exist | CC-BY-4.0 weights / MIT code | Yes |
| **Chatterbox (V3 multilingual)** (Resemble AI) | 0.5B / several .safetensors (~1–2 GB) | 23 — **no id** (has `ms` Malay) | **`exaggeration` (0.25–2.0) + `cfg_weight` knobs**, zero-shot cloning | 24 kHz (s3gen vocoder) | No official ONNX | **MIT** | Yes (note: watermark embedded) |
| **Dia-1.6B** (Nari Labs) | 1.6B | en only | Inline nonverbals `(laughs)`, `(coughs)`; `[S1]/[S2]` dialogue scripting | 44.1 kHz | No | Apache-2.0 | Yes |
| **Orpheus-3b-0.1-ft** (Canopy Labs) | 3B (Llama-3.2-3B backbone) | en only | Emotion tokens `<laugh>`, `<sigh>`, `<yawn>`, `<cough>`, … + tech prefixes (whisper) | 24 kHz (SNAC codec) | No | Apache-2.0 | Yes |
| **XTTS-v2** (Coqui) | ~1.8 GB | 17 — **no id** | emotion/style only via reference-audio cloning | 24 kHz | No | **CPML (non-commercial)** | No |
| **OpenAudio S1-mini** (Fish Audio) | 0.5B | 13 — **no id** | instruction + reference cloning | 44.1 kHz (VQGAN) | No | **CC-BY-NC-SA-4.0** | No |
| **Fish S2-Pro** (Fish Audio) | not verified (large) | **83, incl. `id`, `jw`, `ms`** | instruction-following, cloning | not verified | No | **"fish-audio-research-license"** | **No** (research/non-commercial, gated) |
| **CosyVoice 2 / Fun-CosyVoice 3** (FunAudioLLM) | 0.5B | 9 (zh, en, fr, es, ja, ko, it, ru, de) — **no id** | natural-language instruct (emotion, speed, volume), cloning, pronunciation inpainting | 24 kHz | No | Apache-2.0 | Yes |
| **Qwen3-TTS-12Hz-0.6B** (Alibaba) | 0.6B | 10 — **no id** in base | text prompting (base is raw token model) | 24 kHz | No (GGUF community only) | Apache-2.0 | Yes |
| **MMS-TTS `ind`** (Meta) | ~63–145 MB VITS | id (single voice) | none | 16 kHz class (config fetch blocked — **UNVERIFIED**) | convertible | **CC-BY-NC-4.0** | No |
| **Maya-1** (Maya Research) | not verified | demos are English — language list **UNVERIFIED** | **voice design from plain-English description** + inline tags like `<laugh_harder>` | 24 kHz | No | Apache-2.0 | Yes |
| **Piper VITS** (today, Qinari) | 63–137 MB | en + id | none (noise/length scales only) | 22.05 kHz | Yes | voice-dependent | en yes / id no |

### 1.2 Candidate-by-candidate detail

#### Supertonic 3 (Supertone) — the sleeper candidate

Primary sources: [HF model card `Supertone/supertonic-3`](https://huggingface.co/Supertone/supertonic-3),
[archived weights `supertone-oss-archive/supertonic-3`](https://huggingface.co/supertone-oss-archive/supertonic-3),
[archived GitHub `supertone-oss-archive/supertonic`](https://github.com/supertone-oss-archive/supertonic),
[supertonic-py voices doc](https://github.com/supertone-oss-archive/supertonic-py/blob/main/docs/voices.md).

- **What it is**: a 99M-parameter on-device TTS by Supertone (the Korean AI-audio
  company), designed from the start for ONNX Runtime, no cloud, no GPU.
  Supertonic 3 (released 2026-04-29) expanded from 5 to **31 languages**, and
  the language list explicitly includes **Indonesian (`id`)**, alongside
  Vietnamese and Hindi, and all major European languages.
- **License**: OpenRAIL-M (verified by the main agent's pass: the archived
  GitHub README states "Sample code: MIT License, Model: OpenRAIL-M License";
  HF cardData tag `license:openrail`). Use-restriction license: commercial
  use allowed, no illegal use + no use-restriction circumvention. This is the
  same family as Stable Diffusion's license.
- **Plain ONNX without sherpa-onnx: YES.** The repo ships four ONNX files plus
  two JSON sidecars, downloadable with no HF login:
  `onnx/text_encoder.onnx` (36.4 MB), `onnx/vector_estimator.onnx` (256.5 MB),
  `onnx/vocoder.onnx` (101.4 MB), `onnx/duration_predictor.onnx` (3.7 MB),
  `onnx/tts.json`, `onnx/unicode_indexer.json`, plus 10 preset voice-style
  JSONs (`M1`–`M5`, `F1`–`F5`). Sizes are the LFS `x-linked-size` values
  measured today. Total ≈ 398 MB fp32. A sherpa-onnx int8 repack also exists
  ([`csukuangfj2/sherpa-onnx-supertonic-tts-int8-2026-03-06`](https://huggingface.co/csukuangfj2/sherpa-onnx-supertonic-tts-int8-2026-03-06))
  but is **not required** — the official Python SDK itself runs the plain ONNX
  files via `onnxruntime`, and the repo has official multi-runtime examples
  "Python, Node.js, Browser (WebGPU), Java, C++, C#, Go, Swift, iOS, **Rust**,
  and Flutter".
- **Expressiveness**: 10 inline expression tags — `<laugh>`, `<breath>`,
  `<sigh>` visible on the card — "without prompt engineering or reference
  audio". Voices are fixed presets (10 of them: lively male M1 … warm M5,
  calm F1 … gentle F5), described in the
  [voices doc](https://github.com/supertone-oss-archive/supertonic-py/blob/main/docs/voices.md).
  Zero-shot custom voice styles existed only via the paid Voice Builder web
  service, which is part of what was shut down in the archive.
- **Speed evidence**: README claims real-time synthesis on a Raspberry Pi
  (video), **average RTF 0.3× on an Onyx Boox Go 6 e-reader** (a weak
  E-Ink CPU — encouraging for a 2-vCPU box), and "entire webpage into audio in
  under a second". Measured WER/CER per language vs. much larger open models
  (VoxCPM2 class) on the
  [MiniMax-MLS-test benchmark](https://huggingface.co/datasets/MiniMaxAI/TTS-MLS-Test)
  are published as charts in the repo (`img/metrics/s3_vs_measured_wer_range_voxcpm2.png`).
  **Caution**: these are the vendor's own charts; no independent naturalness
  (MOS/CMOS) evaluation for Indonesian was found. Supertonic is fast and
  accurate; nothing proves it is *ElevenLabs-expressive* — the tags are a
  coarse but real lever (laughter/breath/sigh are exactly the "non-AI"
  cues).
- **44.1 kHz native output** — immediately kills Qinari's current 1.6 kHz
  bandwidth-ceiling complaint.
- **The catch**: the whole project is **archived**. Development and support
  ended (the GitHub README's first line is an archive banner); weights and
  code survive under `supertone-oss-archive` with licenses retained. For an
  offline-first CLI this is acceptable (no service dependency), but expect no
  bugfixes, no new voices, and a pinned revision (`aafc6e32416a594460b32413efc49d7fe4ce6d46`).

#### Kokoro-82M

Primary sources: [HF `hexgrad/Kokoro-82M`](https://huggingface.co/hexgrad/Kokoro-82M),
[kokoro-onnx GitHub](https://github.com/thewh1teagle/kokoro-onnx).

- Apache-2.0, 82M params, StyleTTS2-derived. v1.0 (2025-01-27): **8 languages,
  54 voices**. As of today **no Kokoro 2.0 exists** — hexgrad's author list on
  HF shows only `Kokoro-82M`, the `v1.1-zh` Chinese finetune, and the `kLegacy`
  museum. Anything calling itself "Kokoro v2/v3" on a `kokoro*.com` domain is a
  scam per the model card's own caution note.
- Languages (via voice embeddings, not a `lang` flag; per the verified pguso
  Rust port README: "English, Mandarin, Spanish, French, Japanese, Italian,
  Hindi, Brazilian Portuguese"; the official `VOICES.md` fetch was rate-limited
  so the exact 8-way split is **UNVERIFIED**). **No Indonesian**, and no
  community Indonesian finetune found.
- `kokoro-onnx` (thewh1teagle, MIT): ships `kokoro-v1.0.onnx` (~300 MB) +
  int8 quantized (~80 MB) + `voices-v1.0.bin`; claims "fast performance near
  real-time on macOS M1". Uses `onnxruntime >= 1.20.1` — same runtime family
  as Qinari's `ort`. From v1.0 it requires the **misaki** G2P package (see
  §4).
- Expressiveness: a 256-d style vector (voice identity), prosody transfer from
  that style, but **no emotion tags, no exaggeration knobs**. Quality per
  parameter is famously high (it hung near the top of TTS Arena when released;
  current ELO **UNVERIFIED** — see §2.4).
- 24 kHz output.

#### Chatterbox (Resemble AI)

Primary source: [HF `ResembleAI/chatterbox`](https://huggingface.co/ResembleAI/chatterbox)
(+ [demo page](https://resemble-ai.github.io/chatterbox_demopage/)).

- **MIT license** (changed from the earlier non-commercial chatterbox terms;
  card `license: mit`, weights `.safetensors` public, no gating).
- Current lineup: **Chatterbox Multilingual V3** (0.5B Llama backbone, 23
  languages — ar, da, de, el, en, es, fi, fr, he, hi, it, ja, ko, **ms**,
  nl, no, pl, pt, ru, sv, sw, tr, zh; **no Indonesian**), plus six dedicated
  single-language finetunes (zh, LatAm/es, pt-BR, es-ES, pt-PT, hi), plus the
  original English-only model with the famous knobs.
- **Expressiveness controls** (English model): `exaggeration` (0.25–0.8 useful
  range, default 0.5) and `cfg_weight` (default 0.5; lower ~0.3 tightens
  pacing on fast speakers). Documented recipe: "expressive or dramatic
  speech: lower `cfg` to ~0.3, raise `exaggeration` to ~0.7". This is the
  most direct "emotion dial" in any open model.
- Zero-shot voice cloning from a reference clip; "alignment-informed
  inference" for stability; trained on 0.5M hours; **outputs are watermarked**
  (Resemble's Perth watermarker) — fine for Qinari's output audio, worth
  stating in docs.
- Resemble's own eval claims it beats ElevenLabs in side-by-side preference
  ([podonos.com/resembleai/chatterbox](https://podonos.com/resembleai/chatterbox)) —
  **vendor-run eval, treat with salt**.
- PyTorch only (`.safetensors` + `conds.pt`); no official ONNX export. CPU
  cost: 0.5B autoregressive transformer + flow-matching vocoder — no
  published 2-vCPU RTF found (**UNVERIFIED**; expect RTF >> 1 on 2 vCPU, GPU
  or Apple Silicon realistically required for interactive use).

#### XTTS-v2 (Coqui)

Primary source: [HF `coqui/XTTS-v2` model card](https://huggingface.co/coqui/XTTS-v2).

- 17 languages: en, es, fr, de, it, pt, pl, tr, ru, nl, cs, ar, zh-cn, ja,
  hu, ko, hi. **Indonesian is NOT in the list** (contrary to forum folklore).
- **CPML** (Coqui Public Model License): non-commercial use only, and Coqui
  the company is gone, so nobody can ever relicense it. For a GPL CLI this is
  opt-in-gated at best. 1.8 GB, 6-second cloning, 24 kHz.
- Verdict for Qinari: **rejected for Indonesian**, marginal for English
  (better open options now exist), dead license path.

#### Fish Speech / OpenAudio (Fish Audio)

Primary sources: [HF `fishaudio/s1-mini`](https://huggingface.co/fishaudio/s1-mini),
[HF `fishaudio/s2-pro`](https://huggingface.co/fishaudio/s2-pro).

- **OpenAudio S1-mini** (`fishaudio/s1-mini`, the renamed `openaudio-s1-mini`):
  **CC-BY-NC-SA-4.0** — non-commercial. 13 languages (zh, en, de, ja, fr, es,
  ko, ar, nl, ru, it, pl, pt). **No Indonesian.**
- **Fish S2-Pro** (2026-03): the interesting one — **83 languages including
  `id`, `jw` (Javanese), `ms`, `su`-adjacent coverage**, instruction-following
  tag on the card. But: license is `fish-audio-research-license` with a
  mandatory gate checkbox "I agree to use this model for **non-commercial use
  ONLY**". So the only giant-multilingual model with first-class Indonesian is
  **not usable commercially**. Model size and quality numbers: fetch was
  rate-limited — **UNVERIFIED**.
- `fishaudio/drama-3` (4.1k downloads, 1.2k likes) exists as an
  emotion/drama-focused 2026 release — not yet inspected (rate limit),
  **UNVERIFIED**.

#### CosyVoice 2 / Fun-CosyVoice 3 (FunAudioLLM / Alibaba Tongyi)

Primary source: [HF `FunAudioLLM/CosyVoice2-0.5B`](https://huggingface.co/FunAudioLLM/CosyVoice2-0.5B)
and [Fun-CosyVoice 3.0 links therein](https://arxiv.org/abs/2505.17589).

- CosyVoice2-0.5B: Apache-2.0, 9 languages (zh, en, fr, es, ja, ko, it, ru,
  de) — **no Indonesian**.
- Fun-CosyVoice 3.0 (`Fun-CosyVoice3-0.5B-2512`, released 2025-12): same 9
  languages + 18 Chinese dialects, natural-language **instruct** support
  ("emotion, speed, volume"), pronunciation inpainting, 150 ms streaming
  latency. Still **no Indonesian**.
- Instruction-following emotion is exactly the right control surface, but the
  language list excludes id. PyTorch-only stack, chunky (LLM + flow + vocoder).

#### Dia (Nari Labs) and Orpheus (Canopy Labs)

Primary sources: [HF `nari-labs/Dia-1.6B`](https://huggingface.co/nari-labs/Dia-1.6B),
[HF `canopylabs/orpheus-3b-0.1-ft`](https://huggingface.co/canopylabs/orpheus-3b-0.1-ft).

- **Dia-1.6B**: Apache-2.0, **English-only**, 1.6B. Its party trick is
  scriptable dialogue (`[S1]`/`[S2]`) with inline nonverbals like `(laughs)`,
  `(coughs)` — the demos are the most "podcast-human" open English audio
  around. No ONNX, no id, heavy.
- **Orpheus-3b-0.1-ft**: Apache-2.0, **English-only**, 3B on a Llama-3.2-3B
  backbone. Emotion via special tokens `<laugh> <sigh> <yawn> <cough> <sniffle>
  <groan> <cry>` and tech-prefixed delivery (e.g. whispering). Canopy Labs'
  long-term status is unclear post-acquisition rumors (**UNVERIFIED**). 3B
  autoregressive on 2 vCPU: RTF far above 1.
- Both are English-only: useful only as reference points for what
  expressiveness means (their tag vocabularies are the design template
  Supertonic copied).

#### MMS-TTS Indonesian (Meta)

Primary source: [HF `facebook/mms-tts-ind`](https://huggingface.co/facebook/mms-tts-ind).

- Exact id: `facebook/mms-tts-ind` (ISO 639-3 `ind`). VITS, part of the MMS
  project ([paper](https://arxiv.org/abs/2305.13516)). **CC-BY-NC-4.0** —
  non-commercial.
- Training data for MMS-TTS is faith-based / readings audio collected via
  religious-content platforms — single-speaker, read-style, monotone. Sample
  rate is 16 kHz class for most MMS-TTS checkpoints (config fetch rate-limited
  — **UNVERIFIED for `ind` specifically**).
- Quality verdict: intelligible and clearly worse than `id_ID-news_tts-medium`
  for naturalness; it solves coverage, not expressiveness. Not a candidate.

### 1.3 HuggingFace sweep: Indonesian TTS finetunes (2026-11-06 live search)

Search queries run against the HF API (sorted by downloads): `indonesian tts`,
`F5-TTS indonesian`, `GPT-SoVITS indonesian`, `tts indonesia`. The `F5-TTS
indonesian` and `GPT-SoVITS indonesian` queries returned **zero** models.
Everything credible that exists:

| Model id | Base | License | Activity | Evidence of quality |
|---|---|---|---|---|
| [`alkhrzmy/qwen3-tts-0.6b-indonesian`](https://huggingface.co/alkhrzmy/qwen3-tts-0.6b-indonesian) | Qwen3-TTS-12Hz-0.6B-Base (Apache-2.0) | Apache-2.0 | 541 dl, 4 likes, last mod 2026-08-15 | none published — **UNVERIFIED** |
| [`grandhigh/Chatterbox-TTS-Indonesian`](https://huggingface.co/grandhigh/Chatterbox-TTS-Indonesian) | ResembleAI/chatterbox | Apache-2.0 | 395 dl, 24 likes, 2025-10-22 | demo samples on card; community word-of-mouth only |
| [`mazrba/Chatterbox-TTS-Indonesian-gguf`](https://huggingface.co/mazrba/Chatterbox-TTS-Indonesian-gguf) | the above, GGUF quant | same | 245 dl | ditto |
| [`anak10thn/pocket-tts-indonesian`](https://huggingface.co/anak10thn/pocket-tts-indonesian) | kyutai/pocket-tts | CC-BY-4.0 | 0 dl, 8 likes, 2026-09-16 | none — brand new, **UNVERIFIED** |
| [`Wikidepia/indonesian-tts`](https://huggingface.co/Wikidepia/indonesian-tts) | old Coqui-style VITS | unset | 68 dl, dormant since 2022 | no |
| [`nayerim/speecht5-indonesian-tts`](https://huggingface.co/nayerim/speecht5-indonesian-tts) | Microsoft SpeechT5 | unset | 44 dl | no |

**Brutal honesty**: no offline, open-weight Indonesian model today has
published, independent evidence of ElevenLabs-class naturalness. The three
live paths to *potentially* expressive offline Indonesian are:

1. **Supertonic 3 built-in `id`** — 99M, ONNX, CPU-fast, expression tags, but
   vendor-only quality evidence (WER charts, no MOS), project archived.
2. **Qwen3-TTS-0.6B Indonesian finetune** — modern 0.6B autoregressive
   architecture (the same class as the models that *do* sound human), Apache,
   but one anonymous author, zero eval, and 0.6B AR decode on 2 vCPU is a
   compute problem, not a research problem.
3. **PocketTTS Indonesian finetune** — the base is *designed* for 2-core CPU
   (6× real-time on a laptop CPU), MIT code, ONNX-exportable, but the id
   finetune is 6 weeks old with 0 downloads and 0 evidence.

Everything else Indonesian on HF is either a toy, an API wrapper, or
non-commercial junk (e.g. `id_ID-news_tts-medium` itself, which Qinari
already ships).

### 1.4 Quick scan: new 2025–2026 open-weight releases with expressiveness

One line each; all verified live unless marked:

- **Maya-1** (`maya-research/maya1`, Apache-2.0): voice *design* from a
  plain-English description ("dark villain, low pitch, gravelly, angry at high
  intensity") + inline tags like `<laugh_harder>`; 24 kHz; English demos;
  multilingual claims **UNVERIFIED**. 8.1k downloads, 941 likes.
  <https://huggingface.co/maya-research/maya1>
- **Qwen3-TTS-12Hz-0.6B-Base** (Apache-2.0): Alibaba's open TTS, 10 languages
  (no id), 826k downloads — the base of the Indonesian finetune above.
  <https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base>
- **PocketTTS** (Kyutai, CC-BY-4.0 weights / MIT code): 100M CPU-first
  streaming TTS, 7 languages, ~6× real-time on a MacBook Air M4 CPU using 2
  cores, 200 ms first-chunk latency; tiny English variants (1.9M–18.2M params)
  added 2026-10; paper <https://arxiv.org/abs/2509.06926>. No Indonesian in
  base. <https://github.com/kyutai-labs/pocket-tts>
- **IndexTTS-2** (`IndexTeam/IndexTTS-2`): Bilibili's follow-up; **en + zh
  only**; emotion steering via prompt text; license not visible in cardData
  (**UNVERIFIED**, GitHub says Apache-2.0). <https://huggingface.co/IndexTeam/IndexTTS-2>
- **Fish S2-Pro / drama-3**: 83 languages incl. id, but research/NC license —
  see above. <https://huggingface.co/fishaudio/s2-pro>
- **VoxCPM2**: referenced by Supertonic's README as the "much larger open TTS
  models" comparison class; not inspected (rate budget). **UNVERIFIED.**
- **Spark-TTS / FireRedTTS-2**: API lookups rate-limited today; historically
  zh/en-focused Spark-TTS (SparkAudio) and FireRedTTS-2 (FireRedTeam, Red
  note/xiaohongshu) — both presumed still zh+en, **UNVERIFIED**.

---

## 2. The bar: what ElevenLabs-class means in November 2026 (Priority A)

Primary sources (all fetched 2026-11-06):
[models overview](https://elevenlabs.io/docs/overview/models.md),
[Eleven v4 capability page](https://elevenlabs.io/docs/overview/capabilities/text-to-speech/eleven-v4.md),
[prompting best practices / audio tags](https://elevenlabs.io/docs/overview/capabilities/text-to-speech/best-practices.md),
[supported languages article](https://elevenlabs.io/docs/help-center/other/what-languages-do-you-support),
[API pricing](https://elevenlabs.io/pricing/api).

### 2.1 Current lineup and Indonesian support

| Model | id | Languages | Indonesian? | Latency (vendor) | Price per 1K chars (API) |
|---|---|---|---|---|---|
| Eleven v4 | `eleven_v4` | 90+ | **Yes** (`id` in v3's 70+ list; v4 superset) | batch | **$0.08** |
| Eleven v4 Turbo | — | 90+ | Yes | ~100 ms median | **$0.04** |
| Eleven v3 | `eleven_v3` | 70+ | **Yes** — `Indonesian (id)` and even `Javanese (jv)` are explicitly in the published list | batch | $0.08 |
| Eleven v3 Conversational | — | 70+ | Yes | ~165–280 ms | $0.04 |
| Multilingual v2 | `eleven_multilingual_v2` | 29 | Yes (in list) | batch | $0.08 |
| Flash v2.5 | `eleven_flash_v2_5` | 32 | Yes | ~75–100 ms | $0.04 |

The v3 language list is published verbatim and includes *Indonesian (id)* and
*Javanese (jv)*; v4 is documented as strictly greater coverage ("90+
languages"). So the entire modern ElevenLabs stack speaks Indonesian natively,
with the caveat from their docs that **the text determines the language while
the voice determines the accent** — for best Indonesian you want a voice
native/cloned in Indonesian, otherwise expect drift.

### 2.2 The expressiveness control surface (v3/v4)

- **Audio tags** inline in the text — documented vocabulary includes:
  - Voice/emotion: `[laughs]`, `[laughs harder]`, `[starts laughing]`,
    `[wheezing]`, `[whispers]`, `[sighs]`, `[exhales]`, `[sarcastic]`,
    `[curious]`, `[excited]`, `[crying]`, `[snorts]`, `[mischievously]`
  - Sound effects: `[gunshot]`, `[applause]`, `[clapping]`, `[explosion]`,
    `[swallows]`, `[gulps]`
  - Experimental: `[strong X accent]`, `[sings]`, `[woo]`, `[fart]`
  - Free-form descriptive tags also work and are the documented best
    practice: `[casual]`, `[annoyed]`, `[under his breath]`, `[ecstatic]`,
    `[dismissive]`, `[quietly curious]`, `[giggle]`, `[soft chuckle]`,
    `[lower, thoughtful]`, `[low, gravelly voice]` — the docs explicitly say
    to write tags that describe voice quality rather than sound cues.
- **Pacing via punctuation**: ellipses add pauses and weight; dashes, CAPS,
  and paragraph structure all affect delivery. v3/v4 **do not support SSML
  break tags**; and v4 removed the Style and Speed sliders that v3 had.
- **Multi-speaker dialogue**: v3/v4 accept scripted dialogue with speaker
  turns in one generation.
- **An LLM "Enhance" button** in their UI auto-inserts audio tags — and
  ElevenLabs publishes the system prompt it uses in the docs. That prompt is a
  ready-made spec for a local "expressivizer" pass in front of any
  tag-capable engine (Supertonic, Chatterbox-English, Dia, Orpheus) — an idea
  worth stealing for Qinari's `--expressive` flag.
- **Voice cloning** (instant from seconds of audio, professional from
  minutes) is the other half of the "non-AI" sound: delivery follows the
  cloned speaker's cadence, not a flat prior.

### 2.3 What actually makes this class sound non-AI — and the numbers problem

Mechanically, the audible gap between Piper-class VITS and ElevenLabs-class
speech is: (1) **prosody variance** — sentence-level F0 contours that vary
per utterance instead of one canonical read; (2) **disfluency and
paralinguistic events** — breaths, laughs, sighs, filled pauses, which
perceptually mask the synthetic signature; (3) **pacing with structure** —
phrase-final lengthening and pause distributions that match information
structure (humans pause at clause boundaries and vary pause duration
bimodally); (4) **spectral bandwidth and dynamics** — Qinari's current 22.05
kHz VITS with ~1.6 kHz effective bandwidth sounds telephone-ish next to
44.1 kHz studio output.

**Measurable numbers**: ElevenLabs publishes **no** F0-std / pause-statistics
numbers, and none of the open-model cards do either. The only quantified,
sourceable claims found today:

- ElevenLabs' own positioning: v4 = "most emotive… audio tags handled with a
  level of nuance beyond previous models" (docs, no numbers).
- Resemble's Podonos-run eval claims Chatterbox **beats ElevenLabs
  side-by-side** ([podonos.com/resembleai/chatterbox](https://podonos.com/resembleai/chatterbox)) —
  vendor-run preference test, no prosody metrics disclosed.
- Supertonic 3 publishes per-language **WER/CER** vs. VoxCPM2-class models
  (repo `img/metrics/`) — intelligibility, not naturalness.
- PocketTTS has a paper (<https://arxiv.org/abs/2509.06926>) that presumably
  carries MOS numbers — **UNVERIFIED** (not fetched today).

The honest conclusion: the "F0 std ≈ human" style evidence does not exist in
public form for any of these systems; community ELO ladders (TTS-Arena,
Artificial Analysis) exist but are JS-rendered and were not retrievable in
this session (**UNVERIFIED**). **The right move for Qinari is to define its
own gate**: compute F0 std / range, pause-count, and pause-duration stats on
generated vs. reference narration, plus predicted MOS (§3). That turns "AI
banget" into a number that any future engine can be judged by.

### 2.4 Pricing model

Per-character API pricing (verified today on the pricing page): **v4/v3/Multilingual-v2
$0.08 per 1K characters; v4 Turbo / v3 Conversational / Flash $0.04 per 1K
characters**. Subscription tiers bundle credits (a 30,000-credits-for-$1
promo is running; historical tiers Free 10k credits/mo → Starter → Creator →
Pro → Scale) — tier prices **UNVERIFIED** today; check
<https://elevenlabs.io/pricing> at integration time. At $0.08/1K chars, one
hour of narration (~9,000 words ≈ 54K chars) costs ≈ **$4.3 with v4** or
≈ $2.2 with Flash/Turbo-class models.

---

## 3. Quality gates: predicted MOS and WER for an offline Rust CLI (Priority B)

The point: turn "AI banget" into numbers Qinari can gate on in CI, before any
human listens. Verified options below.

### 3.1 Comparison table

| Gate | What it measures | Files / size | License | Torch dep? | Embeddable in Qinari via `ort`? |
|---|---|---|---|---|---|
| **DNSMOS (P.835 + P.808)** | SIG / BAK / OVRL — distortion, noise, overall quality of **16 kHz** speech | `sig_bak_ovr.onnx` 1,157,965 B (1.16 MB) + `model_v8.onnx` 224,860 B (0.22 MB) | **CC BY 4.0** (repo `LICENSE` verified) | **No** — ONNX native | **Yes, trivially** (2 tiny files, same runtime) |
| **UTMOS22** (SaruLab, Interspeech 2022) | MOS on 22-point scale, trained for TTS/VC eval | PyTorch `.pt3` (~hundreds of MB with torch) | **UNVERIFIED** (repo README; research-lineage data) | **Yes** | No — dev-side Python; community ONNX exports **UNVERIFIED** |
| **NISQA** (Gabrielmittag) | MOS incl. Naturalness subscore | PyTorch weights | **UNVERIFIED** | **Yes** | No — dev-side Python |
| **Whisper WER (id+en)** | Intelligibility vs. source text | tiny 39 M / base 74 M / small 244 M / medium 769 M / large 1550 M (MIT) | **MIT** | No (transformers, or ONNX) | Possible via `onnx-community/whisper-*` but heavy; dev-side recommended |

### 3.2 DNSMOS — the practical embedded gate

- Files live in the Microsoft DNS-Challenge repo and are served as plain
  downloads (existence + exact sizes verified today via HTTP):
  <https://github.com/microsoft/DNS-Challenge/tree/master/DNSMOS/DNSMOS>
  - `sig_bak_ovr.onnx` → SIG, BAK, OVRL scores (P.835 three-proxy model)
  - `model_v8.onnx` → the P.808 overall-MOS model
- Repo license is **CC BY 4.0** (the `LICENSE` file is the CC BY 4.0 text) —
  commercial use fine with attribution; compatible with a GPL-3.0 CLI that
  documents the model provenance (Qinari already does exactly this for voices).
- Microsoft's own `speechmos` pip package (v0.0.1.1, PyPI, fetched today)
  wraps these same models with `onnxruntime 1.10` + numpy — **no torch** —
  proving the runtime path. Note the naming trap: **`speechmos` on PyPI is
  Microsoft's AECMOS/DNSMOS/PLCMOS, NOT UTMOS.**
- Integration cost in Qinari: ~150 LOC — resample engine output to 16 kHz,
  chunk to ~9 s windows (DNSMOS was trained on 9 s clips), feed mel-ish
  features per the DNSMOS preprocessing, run two sessions, average. Adds
  1.4 MB of model weight.
- Caveat: DNSMOS was built to score noise suppressors on narrowband speech.
  It will clearly flag Qinari's current ~1.6 kHz bandwidth ceiling and any
  buzziness (low SIG/OVRL), but it is **not** a naturalness MOS for expressive
  44.1 kHz TTS — a dramatic `[whisper]` passage can score worse than flat
  speech. Use it as a regression gate, not as an expressiveness score.

### 3.3 UTMOS and NISQA — dev-side only

- **UTMOS22** (`sarulab-speech/utmos22`): the only open MOS predictor trained
  specifically for TTS/VC evaluation (VoiceMOS Challenge 2022 winner). Pure
  PyTorch stack. No official ONNX. Weights license unclear (**UNVERIFIED**) —
  do not ship in the binary; run in CI via Python if wanted.
- **NISQA**: strong MOS predictor with a dedicated *Naturalness* subscore
  (corpus from gabrielmittag/NISQA), PyTorch-only, license **UNVERIFIED**
  (HF/GitHub lookups rate-limited today). Dev-side only.

### 3.4 WER via Whisper (id + en)

- `openai/whisper`: **MIT** (LICENSE verified). Model size ladder from the
  README (verified today): small 244 M, medium 769 M, large 1550 M (VRAM
  figures ~2 / ~5 / ~10 GB). Multilingual from `tiny` up.
- **ONNX path exists**: `onnx-community/whisper-tiny/-base/-small/-large-v3-turbo`
  are live on HF (downloads verified today), so a pure-`ort` WER gate is
  *possible* — but it needs an autoregressive decode loop (same engineering
  problem as §4d) and ≥250 MB of weights for the useful sizes. Recommendation:
  **dev-side Python** (`faster-whisper`) in CI for id+en WER, not shipped in
  the CLI. Suggested gate: `whisper-small` on a fixed 30-sentence id+en
  corpus; alert when WER regresses >2 points vs. the current engine baseline.
- Reference Indonesian WER for Whisper on Common Voice: exact figure not
  verified today (paper table not fetched); roughly mid-teens for large
  models per the original Whisper paper — treat as **UNVERIFIED** and
  measure Qinari's own baseline instead.

### 3.5 The gate Qinari should actually ship (zero extra deps)

Beyond ML MOS: the *expressiveness* gap is measurable with plain DSP in
`qinari-dsp` (no new dependencies, no models):

1. **F0 std / F0 range** (YIN or autocorrelation pitch track, voiced frames
   only) — flat VITS reads pin this near zero; narrator-class speech varies
   widely. Target: report per-utterance F0 σ in semitones.
2. **Pause statistics** — RMS-envelope thresholding: count and duration
   distribution of pauses >150 ms per minute of audio; humans show bimodal
   (breath vs. phrasal) pause lengths.
3. **Spectral rolloff / bandwidth ceiling** — directly quantifies the current
   ~1.6 kHz complaint; a 44.1 kHz engine should roll off near 20 kHz.

These three numbers, printed by `qinari tts measure`, plus dev-side DNSMOS
and WER, give Qinari a reproducible expressiveness benchmark that any future
engine (Supertonic, Kokoro, cloud) can be dropped into and judged by.

---

## 4. Rust runtime realities (Priority B)

### 4.1 Kokoro in Rust + the misaki phonemizer question

**Rust Kokoro exists — several, in fact** (crates.io, verified today):

| Crate | Version / updated | What it is |
|---|---|---|
| [`kokoro-en`](https://github.com/pguso/kokoro) | 0.1.5 / 2026-08 | Rust inference lib for Kokoro-82M ONNX; streaming + one-shot; cross-compiles to mobile; **ports misaki G2P to Rust** |
| `kokoro-micro` / `kokoro-tiny` | 1.3.0 / 0.1.0 | Minimal embedding-first Kokoro wrappers |
| `any-tts` ([TM9657/any-tts](https://github.com/TM9657/any-tts)) | 0.2.0 / 2026-10 | **Candle-based** (pure-Rust ML) trait API over **Kokoro, OmniVoice, Qwen3-TTS, VibeVoice, VibeVoice Realtime, Voxtral**; MIT OR Apache-2.0 |
| `sayd-kokoro`, `kokoro-cli`, `hematite-kokoros` | 0.1.0 / 0.2.0 / 0.1.3 | smaller community efforts (ONNX-RT, CLI, maintained fork) |

Notes for Qinari:

- `kokoro-en` (pguso/kokoro) is the most complete: it runs the same
  `kokoro-v1.0.onnx` + `voices-v1.0.bin` files that kokoro-onnx uses, with
  CoreML/CUDA/DirectML acceleration and CPU fallback. **But its default
  feature `g2p-espeak` links a bundled espeak-ng via CMake** — a C build step.
  A "slim Misaki-only build" exists (`--no-default-features`, optional
  `misaki-lean`). Qinari's existing pure-Rust `espeak-ng` crate could feed the
  OOV-fallback role instead, but see below — the phoneme alphabets differ.
- **What misaki actually is** (from the [misaki README](https://github.com/hexgrad/misaki)):
  English G2P is a **pure-Python rule/dictionary engine** (`misaki[en]`); the
  C espeak-ng is only an optional *fallback for OOV words* (`EspeakFallback`
  via `phonemizer-fork`). Japanese uses pyopenjtalk, Korean g2pk, Chinese
  jieba+paddlespeech, Vietnamese viphoneme. **There is no Indonesian
  misaki backend.**
- **Phoneme alphabet caveat**: misaki output is *not* raw espeak-ng IPA. It is
  stress-marked IPA in Kokoro's own convention (e.g. `misˈɑki ɪz ə ʤˈitəpˈi`,
  with `ˈ`/`ˌ` stress, `ᵊ`, and dictionary-driven tokenization; see
  misaki's `EN_PHONES.md`). Kokoro's phoneme-to-id vocabulary was trained on
  misaki output, so Qinari **cannot** simply feed its existing
  `espeak.text_to_phonemes_phonemeizer()` IPA into a Kokoro model and expect
  correct output — the stress marks and some segment choices differ. Any
  Kokoro integration must either port misaki-en (as `kokoro-en` does), or use
  `kokoro-en` as a dependency and delete its C-espeak feature.

### 4.2 sherpa-rs vs official sherpa-onnx Rust API

- [`sherpa-rs`](https://github.com/thewh1teagle/sherpa-rs) (thewh1teagle) is
  **deprecated** — its README now redirects to the **official Rust API shipped
  by upstream sherpa-onnx** (`sherpa-onnx` crate, v2.0.1, updated 2026-10-27,
  "Safe Rust wrapper"). Official examples live in
  [k2-fsa/sherpa-onnx/rust-api-examples](https://github.com/k2-fsa/sherpa-onnx/blob/master/rust-api-examples/examples/silero_vad_remove_silence.rs).
- Build story: sherpa-onnx is a **C++ library**; the Rust crate wraps it via a
  sys layer. Prebuilt binaries via a `download-binaries`-style feature or
  cmake build; linking is static-friendly, but the crate **contains `unsafe`
  FFI internally** and drags the sherpa-onnx C++ dependency tree. For a
  `forbid(unsafe_code)` workspace like Qinari this is legal (the `unsafe` is
  in the dependency, not in Qinari's code) but breaks the "pure-Rust, cc-only
  builds" property, and adds a large vendor surface to audit.
- Model support in sherpa-onnx is the widest of any runtime: Piper/VITS,
  Kokoro, Matcha, Vits-melo, and — relevant here — **Supertonic** (an int8
  Supertonic package `sherpa-onnx-supertonic-tts-int8-2026-03-06` is
  published by csukuangfj2, a k2-fsa maintainer; whether Supertonic is in the
  official Rust API's TTS enum is **UNVERIFIED**).
- Verdict for Qinari: sherpa-onnx Rust is the *fallback* path if the plain
  ONNX route proves too fiddly for a specific model (e.g. Supertonic int8
  repack), not the default path. The default path remains `ort` + raw ONNX.

### 4.3 piper1-gpl status

[OHF-Voice/piper1-gpl](https://github.com/OHF-Voice/piper1-gpl) — the Open
Home Foundation's GPL rewrite of Piper — is **not a Rust rewrite**: it is a
Python package (`pip install piper-tts`) over a **C/C++ core (`libpiper`)**
with an embedded espeak-ng, plus an HTTP server, Python API, and training
docs. Its README currently carries a **"Looking for Maintainers"** banner.
It adds **no expressiveness** — same Piper VITS voice zoo (including the same
`id_ID-news_tts-medium` Qinari already ships), just cleaner packaging and
licensing. Nothing here changes Qinari's engine calculus; Qinari's own
`ort`-based local engine already equals or beats this path for our purposes.

### 4.4 Can `ort` run an autoregressive LLM-backbone TTS (Llama-style decode + KV-cache)?

**Yes, proven by pykeio's own examples.** The
[pykeio/ort examples directory](https://github.com/pykeio/ort/tree/main/examples)
contains `gpt2` (full token-generation loop), `async-gpt2-api` (an entire
API server around GPT-2 generation), and `phi-3-vision` (a 3.8B-parameter
LLM with KV-cache inference) — all in pure Rust on `ort`, no C glue beyond
`ort-sys`'s prebuilt runtime. The mechanics Qinari would need — feeding
tokens, extracting logits over a vocab, sampling, appending to a `past`
KV-cache tensor, looping until EOS — are exactly what these examples do.

The engineering cost of a Chatterbox/Orpheus/Qwen3-TTS-class engine in Qinari
is therefore real but bounded: an ONNX export of the model first (none of
these ship ONNX officially — Chatterbox and Orpheus are PyTorch; Qwen3-TTS
has community GGUF, not ONNX), then ~500–1500 LOC for the decode loop +
codec (SNAC/VQGAN are also ONNX-exportable, non-autoregressive) + streaming
concatenation. The killer is **CPU speed**: 0.5B–3B autoregressive decode on
2 vCPU will be RTF ≫ 1 (no published number for 2 vCPU exists — **UNVERIFIED**
by definition; pocket-tts is the only model in this class *designed* for that
constraint).

Two escape hatches worth noting:
- **Candle path**: `any-tts` already implements Qwen3-TTS (and Kokoro et al.)
  in pure-Rust Candle — a dependency Qinari could adopt instead of writing an
  ort decode loop, at the cost of a second ML runtime in the tree.
- **GGUF path**: `mazrba/Chatterbox-TTS-Indonesian-gguf` exists, implying
  llama.cpp-class runtimes can execute it — C++ again, so it only matters if
  Qinari ever relaxes its pure-Rust stance.

### 4.5 PocketTTS in Rust (bonus, discovered during research)

Kyutai's PocketTTS — the one TTS *architected for 2-core CPU* — already has
**two independent Rust ports** linked from its official README:
[wasm-pocket-tts / pocket-tts-xn](https://github.com/gradium-ai/xn-ptts)
(LaurentMazare, Rust with the XN framework, runs in-browser via WASM) and
[babybirdprd/pocket-tts](https://github.com/babybirdprd/pocket-tts)
(Candle + WASM + PyO3), plus a community
[ONNX export](https://github.com/KevinAHM/pocket-tts-onnx-export) running on
ONNX Runtime Web. A Rust `ort`-based PocketTTS engine is therefore low-risk
engineering; the open question is only the quality of the 6-week-old
Indonesian finetune (`anak10thn/pocket-tts-indonesian`, CC-BY-4.0).

### 4.6 ElevenLabs from Rust

Unofficial SDKs on crates.io (verified today): `elevenlabs` 0.4.2
([paperfoot/elevenlabs-cli](https://github.com/paperfoot/elevenlabs-cli),
agent-friendly CLI, updated 2026-10), `elevenlabs_tts` 0.2.1
([hamzaelmarjani/elevenlabs_tts](https://github.com/hamzaelmarjani/elevenlabs_tts),
type-safe client, 2025-09), plus `elevenlabs-cli` 0.1.8. All are small and
none is canonical. Recommendation: **do not take a dependency** — ElevenLabs'
TTS endpoint (`POST /v1/text-to-speech/{voice_id}` with `xi-api-key`) is a
~100-LOC `ureq` client, and Qinari already has the HTTP plumbing for its
OpenAI-compatible cloud engine. A cloud lane can support both protocols
behind the existing `TtsEngine` trait.

---

## 5. Verdict: the engine ladder for Qinari

Constraints recap: GPL-3.0, `forbid(unsafe_code)`, pure-Rust + `cc`-only
builds, `ort` 2.0.0-rc.15 already pinned, offline-first, 2-vCPU reference
box, id+en, and the user wants narrator-class expressiveness, not
"acceptable" TTS.

### 5.1 Fast lane — near-realtime CPU, offline

**Primary: Supertonic 3 via plain ONNX on `ort`.**

- Why: it is the only model that simultaneously has Indonesian, expressiveness
  tags, 44.1 kHz output, an ONNX-native design, evidence of real-time CPU
  operation on hardware weaker than our target (RTF 0.3× on an e-ink
  e-reader), and a commercial-friendly license (OpenRAIL). 31 languages means
  one engine covers id+en+more.
- Integration: **zero new dependencies** — the four ONNX files run on the
  already-pinned `ort`. Port the SDK's inference orchestration + tokenizer
  (`unicode_indexer.json`, `tts.json`, voice-style JSONs) from Python to Rust
  (the repo's own Rust example is the reference). Estimated 400–800 LOC plus
  a pinned-asset downloader identical in shape to the existing voice
  downloader (hash-pinned, license-shown). The graph layout (duration
  predictor → text encoder → vector estimator → vocoder) suggests a
  **non-autoregressive** architecture — no KV-cache decode loop needed —
  though that is inference from file names, **UNVERIFIED**.
- Expressiveness: map a small `[tag]` vocabulary (Qinari syntax) to
  Supertonic's `<laugh>`/`<breath>`/`<sigh>`-style tags; optionally add an
  LLM-assisted "expressivizer" mode modeled on ElevenLabs' published Enhance
  prompt.
- Risks: archived project (pin the revision, ship hashes); naturalness for
  Indonesian unproven by anyone independent — gate with §3.5 metrics before
  making it the default; ~398 MB download (fp32). An int8/OnnxSlim pass could
  shrink it (sherpa-onnx int8 repack exists as proof, ~file sizes
  **UNVERIFIED**).
- License posture: OpenRAIL — commercial OK; show license + use restrictions
  at install like the current voices.

**Fallback: Kokoro-82M (English only).** Apache-2.0, CPU-proven, best
quality-per-watt for English narration; no Indonesian, no tags, and the
misaki phoneme-alphabet port is real work (§4.1). Use `kokoro-en` (with its
C-espeak feature off) or run `kokoro-v1.0.onnx` directly.

### 5.2 Expressive lane — best offline quality, CPU cost paid knowingly

**Primary: PocketTTS (Kyutai) via `ort` with the community ONNX export.**

- Why: it is the only expressive-class, cloning-capable model *architected
  for exactly Qinari's hardware target* (100M params, 2 CPU cores, ~6×
  real-time on a laptop CPU, 200 ms first chunk, streaming). Voice cloning
  from a reference clip is the expressiveness mechanism: narration style
  follows the reference speaker's prosody, which is what makes the
  ElevenLabs class sound non-AI. MIT code, CC-BY-4.0 weights, two existing
  Rust ports and an ONNX export to crib from (§4.5).
- CPU cost: ~RTF 0.17 on an M4-class CPU core-pair; expect RTF ≈ 0.5–1 on the
  2-vCPU reference box (**estimate, UNVERIFIED**). English + 6 EU languages;
  no Indonesian.
- Integration: medium — either adopt a Rust port as a dependency or port the
  ONNX pipeline (~800–1500 LOC, autoregressive token decode included but
  small-model).

**Fallback (English, maximum drama): Chatterbox English (MIT)** with the
`exaggeration`/`cfg_weight` knobs exposed as CLI flags. No ONNX export exists
(§4.4) — needs a torch→ONNX export effort plus an ort decode loop, or the
GGUF path. CPU cost on 2 vCPU: RTF ≫ 1 (**UNVERIFIED**), batch-rendering
only. **Also viable: Qwen3-TTS-0.6B via `any-tts` (Candle)** — pure-Rust
dependency, reference-audio prompting, and it is the base of the Indonesian
finetune; same CPU-cost caveat.

### 5.3 Indonesian lane — best available offline, stated plainly

**Primary: Supertonic 3 with `lang="id"`** (same engine as the fast lane —
that is the point). 44.1 kHz alone fixes the bandwidth complaint; tags give
breath/laughter; 10 preset voices. **Quality is unproven for id** — no
independent eval exists. Bench it with the §3 gates on 30 sentences before
promoting it to default; if it fails, it fails honestly.

**Challengers, in order of interest:**

1. `alkhrzmy/qwen3-tts-0.6b-indonesian` (Apache-2.0) — the architecture class
   that genuinely reaches ElevenLabs-level expressiveness elsewhere; zero
   published eval; 0.6B AR decode means minutes-not-seconds renders on
   2 vCPU. Worth a listening test via `any-tts` before deciding.
2. `anak10thn/pocket-tts-indonesian` (CC-BY-4.0) — if the base model's
   quality is good and the finetune holds, this becomes the *best* Indonesian
   lane (CPU-designed, cloning-capable); 6 weeks old, 0 downloads, needs
   verification.
3. `grandhigh/Chatterbox-TTS-Indonesian` (Apache-2.0) — most community
   traction of the id finetunes (24 likes); 0.5B AR, CPU-heavy.

**Guaranteed fallback: the existing `id_ID-news_tts-medium`** (and fix the
PAD-sequencer speed bug regardless — a broken clock is wrong even when a
better clock exists).

**Plainly**: nothing offline-usable today has *evidence* of ElevenLabs-class
Indonesian. The honest framing for the user: offline Indonesian improves in
steps (bandwidth → 44.1 kHz ✓ Supertonic; prosody tags ✓ Supertonic;
cloning-based style — maybe PocketTTS-id), while *verified* narrator-class
Indonesian in 2026-11 still requires the cloud lane.

### 5.4 Optional cloud lane — ElevenLabs direct

**Primary: ElevenLabs** — v4 (`eleven_v4`) for renders, v4 Turbo for
interactive; Indonesian supported across the whole modern lineup (§2.1);
audio tags map 1:1 to Qinari's planned tag syntax; ~$4.3 per narration hour
on v4. Implementation: ~100 LOC `ureq` client (`POST
/v1/text-to-speech/{voice_id}`, `xi-api-key`), keys via the existing env-var
pattern, never persisted. **Fallback: the existing OpenAI-compatible cloud
engine** (OpenAI/gpt-4o-mini-tts, Groq, self-hosted Kokoro-FastAPI) — already
shipped; keep as the generic cloud lane.

### 5.5 Integration cost summary

| Lane | Engine | New deps | LOC estimate | AR decode loop? | License posture |
|---|---|---|---|---|---|
| Fast | Supertonic 3 (`ort`) | none | 400–800 | no (per graph layout) | OpenRAIL, commercial OK, show at install |
| Fast fallback | Kokoro (`kokoro-en` or direct) | 1 (audit its espeak feature) | 300–1000 | no | Apache-2.0 |
| Expressive | PocketTTS ONNX | none (or 1 Rust port) | 800–1500 | small | CC-BY-4.0 / MIT |
| Expressive alt | Qwen3-TTS via `any-tts` | 1 (candle tree) | ~200 | yes (in dep) | Apache-2.0 |
| Expressive (en drama) | Chatterbox (needs ONNX export) | none + export work | 1500+ | yes | MIT (watermarked output) |
| Indonesian | Supertonic `id` (same engine) | none | +tags mapping | no | OpenRAIL |
| Indonesian fallback | current Piper id voice | none | bugfix only | no | NC — keep opt-in gate |
| Cloud | ElevenLabs direct | none (ureq exists) | ~100 | n/a | ToS/key-gated |

Rule for non-commercial or research-licensed models (Fish S2-Pro with its 83
languages including id, XTTS-v2, MMS, OpenAudio S1-mini): **opt-in gated,
never default, never bundled** — the same mechanism the id Piper voice
already uses. Recommended posture: do not ship Fish S2-Pro at all (gated
download flow + NC + no eval = all cost, no certainty).

### 5.6 Recommended sequence

1. Fix the PAD-sequencer speed bug in the current Piper engine (guaranteed
   floor).
2. Ship the §3.5 measurement command + dev-side DNSMOS/WER so every claim
   below is a number, not a vibe.
3. Integrate Supertonic 3 behind the `TtsEngine` trait (fast lane + id lane);
   gate its promotion to default on the measured benchmarks.
4. Listening-test `qwen3-tts-0.6b-indonesian` and
   `pocket-tts-indonesian` via Python first; only integrate if they beat
   Supertonic on the gates.
5. Add the ElevenLabs cloud client (small, high payoff for the user's actual
   complaint).
6. Only then consider the expressive-lane PocketTTS/Chatterbox work.

---

## 6. Main-agent verification pass (2026-11-06, post-agent)

The mission discipline requires agent claims to be re-verified before they
load a design. Independently checked against live sources (no agent-reported
evidence reused):

1. **Supertonic 3** — archived GitHub fetched directly: archive banner
   confirms read-only since 2026-09-09; 99M params, 31 languages incl. `id`,
   44.1 kHz, 10 inline expression tags, RTF 0.3× avg on Onyx Boox Go 6 — all
   confirmed verbatim. **License upgraded from UNVERIFIED to verified
   OpenRAIL-M** (sample code MIT). Official Rust example confirmed present
   (`rust/` with `cargo build`, `example_onnx`), plus a detail the agent
   missed: the Rust example exposes `--total-step` (default 8, denoising
   steps) — the vector estimator is an iterative denoiser, so RTF trades
   against quality via step count. HF API tree confirms the four ONNX files
   byte-for-byte: duration_predictor 3,700,147 B; text_encoder 36,416,150 B;
   vector_estimator 256,534,781 B; vocoder 101,424,195 B (total ≈ 398 MB,
   no login required). HF cardData: `license:openrail`, 8,139 downloads.
2. **PocketTTS** — official Kyutai README fetched directly: 100M params,
   6× real-time on a MacBook Air M4 CPU using 2 cores, ~200 ms first chunk,
   voice cloning from a plain wav, MIT code license — all confirmed. **One
   upgrade over the agent's finding: the community Indonesian model
   (`anak10thn/pocket-tts-indonesian`, "6 layers") is linked from the
   OFFICIAL PocketTTS README's community-models list** — a stronger signal
   than a random HF upload (maintainer-visible, but still zero published
   eval; listening test still required).
3. **ElevenLabs** — supported-languages doc fetched directly: Eleven v3 lists
   both `Indonesian (id)` and `Javanese (jv)` explicitly; Multilingual v2 and
   Flash v2.5 list Indonesian. v4 is documented as the superset (90+).
4. **Indonesian finetunes exist** — HF API confirms all three with live
   stats: `alkhrzmy/qwen3-tts-0.6b-indonesian` (541 dl, 4 likes, last mod
   2026-08-15), `anak10thn/pocket-tts-indonesian` (0 dl, 8 likes,
   2026-09-16), `grandhigh/Chatterbox-TTS-Indonesian` (395 dl, 24 likes,
   2025-10-22). `ResembleAI/chatterbox` itself: 2.33M downloads, 1,896
   likes. Card-level license fields are unset on the HF API for the
   finetunes — confirm each license from the model card at integration time
   before any bundling decision.
5. **DNSMOS** — both ONNX files answer HTTP 200 from the Microsoft repo
   (existence confirmed; byte sizes re-verified by download during the
   design window — see DESIGN-TTS-EXPRESSIVE.md premise checks).

Verdict of the pass: **zero claims rejected** (vs. three rejected in the
audio-realism research pass); two items upgraded (OpenRAIL-M verified;
pocket-tts-id officially linked). The engine ladder in §5 stands as
written.

---

## Sources

Landscape (Section 1):

- Chatterbox: <https://huggingface.co/ResembleAI/chatterbox> ;
  <https://podonos.com/resembleai/chatterbox> ;
  <https://resemble-ai.github.io/chatterbox_demopage/>
- Kokoro: <https://huggingface.co/hexgrad/Kokoro-82M> ;
  <https://github.com/thewh1teagle/kokoro-onnx> ;
  <https://huggingface.co/hexgrad/Kokoro-82M-v1.1-zh>
- XTTS-v2: <https://huggingface.co/coqui/XTTS-v2> ; CPML:
  <https://coqui.ai/cpml>
- Fish Audio: <https://huggingface.co/fishaudio/s1-mini> ;
  <https://huggingface.co/fishaudio/s2-pro> ;
  <https://huggingface.co/fishaudio/drama-3>
- CosyVoice: <https://huggingface.co/FunAudioLLM/CosyVoice2-0.5B> ;
  <https://arxiv.org/abs/2505.17589> (CosyVoice 3)
- Dia: <https://huggingface.co/nari-labs/Dia-1.6B> ; Orpheus:
  <https://huggingface.co/canopylabs/orpheus-3b-0.1-ft>
- Supertonic: <https://huggingface.co/Supertone/supertonic-3> ;
  <https://huggingface.co/supertone-oss-archive/supertonic-3> ;
  <https://github.com/supertone-oss-archive/supertonic> ;
  <https://github.com/supertone-oss-archive/supertonic-py/blob/main/docs/voices.md> ;
  <https://huggingface.co/csukuangfj2/sherpa-onnx-supertonic-tts-int8-2026-03-06> ;
  <https://huggingface.co/onnx-community/Supertonic-TTS-2-ONNX>
- MMS: <https://huggingface.co/facebook/mms-tts-ind> ;
  <https://arxiv.org/abs/2305.13516>
- Indonesian finetunes:
  <https://huggingface.co/alkhrzmy/qwen3-tts-0.6b-indonesian> ;
  <https://huggingface.co/grandhigh/Chatterbox-TTS-Indonesian> ;
  <https://huggingface.co/mazrba/Chatterbox-TTS-Indonesian-gguf> ;
  <https://huggingface.co/anak10thn/pocket-tts-indonesian> ;
  <https://huggingface.co/Wikidepia/indonesian-tts> ;
  <https://huggingface.co/nayerim/speecht5-indonesian-tts>
- New 2025–26: <https://huggingface.co/maya-research/maya1> ;
  <https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base> ;
  <https://github.com/kyutai-labs/pocket-tts> ;
  <https://arxiv.org/abs/2509.06926> ;
  <https://huggingface.co/IndexTeam/IndexTTS-2> ;
  <https://huggingface.co/datasets/MiniMaxAI/TTS-MLS-Test>

The bar (Section 2):

- <https://elevenlabs.io/docs/overview/models.md> ;
  <https://elevenlabs.io/docs/overview/capabilities/text-to-speech/eleven-v4.md> ;
  <https://elevenlabs.io/docs/overview/capabilities/text-to-speech/best-practices.md> ;
  <https://elevenlabs.io/docs/help-center/other/what-languages-do-you-support> ;
  <https://elevenlabs.io/pricing/api> ;
  <https://elevenlabs.io/v3>

Quality gates (Section 3):

- <https://github.com/microsoft/DNS-Challenge/tree/master/DNSMOS/DNSMOS> (and
  the repo `LICENSE`, CC BY 4.0) ; <https://pypi.org/project/speechmos/> ;
  UTMOS: <https://github.com/sarulab-speech/utmos22> ; NISQA:
  <https://github.com/gabrielmittag/NISQA> ; Whisper:
  <https://github.com/openai/whisper> (MIT `LICENSE`) ;
  <https://huggingface.co/onnx-community/whisper-small>

Rust runtime (Section 4):

- <https://github.com/pguso/kokoro> (kokoro-en) ;
  <https://github.com/TM9657/any-tts> ; <https://github.com/hexgrad/misaki> ;
  <https://github.com/thewh1teagle/sherpa-rs> ;
  <https://github.com/k2-fsa/sherpa-onnx> ;
  <https://docs.rs/sherpa-onnx> ;
  <https://github.com/OHF-Voice/piper1-gpl> ;
  <https://github.com/pykeio/ort/tree/main/examples> (gpt2,
  async-gpt2-api, phi-3-vision) ;
  <https://github.com/gradium-ai/xn-ptts> ;
  <https://github.com/babybirdprd/pocket-tts> ;
  <https://github.com/KevinAHM/pocket-tts-onnx-export> ;
  <https://github.com/paperfoot/elevenlabs-cli> ;
  <https://github.com/hamzaelmarjani/elevenlabs_tts> ;
  crates.io queries for `kokoro`, `any-tts`, `elevenlabs*`, `sherpa-onnx`,
  `ort`, `espeak-ng` (all resolved 2026-11-06)

## Unverified (explicit list)

1. Supertonic 3: ~~exact OpenRAIL variant~~ **RESOLVED by the main-agent
   pass: OpenRAIL-M** (GitHub README). Still unverified: the independent
   naturalness quality of the Indonesian voice (no public eval); sizes of the
   int8 sherpa-onnx repack. Partially resolved: the architecture is not
   autoregressive in the LLM sense, but the vector estimator runs an
   iterative denoising loop (`--total-step`, default 8, "higher = better
   quality, slower" per the Rust example README) — RTF claims presumably
   assume the default step count.
2. Fish S2-Pro / drama-3: model sizes, quality, exact research-license text
   (gated + rate-limited).
3. Maya-1: parameter count and language list (rate-limited raw README).
4. Spark-TTS and FireRedTTS-2 current state (API lookups failed today).
5. IndexTTS-2 license (card lacks it; GitHub believed Apache-2.0).
6. MMS-TTS `ind` exact sample rate (config fetch rate-limited).
7. ElevenLabs subscription tier prices (only per-character API prices were
   verified); TTS-Arena / Artificial Analysis current ELO (JS-rendered).
8. UTMOS22 and NISQA weight licenses; existence of UTMOS ONNX exports.
9. Whisper Indonesian WER reference number on Common Voice.
10. Any 2-vCPU RTF number for 0.5B+ autoregressive TTS (Chatterbox, Qwen3,
    Orpheus, Dia) — no published measurements exist; PocketTTS ~RTF 0.17 is
    an M4-CPU figure, its 2-vCPU estimate is extrapolation.
11. Whether the official sherpa-onnx Rust API exposes Supertonic TTS (only
    the community int8 package was verified to exist).
12. PocketTTS paper's MOS numbers (paper not fetched).


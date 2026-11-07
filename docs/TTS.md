# Qinari TTS

Qinari speaks Indonesian and English. The same `qinari tts` command
drives a fully offline neural voice on your machine (Supertonic 3) and
two cloud services: any OpenAI-compatible endpoint, and ElevenLabs.

```
qinari tts "Halo, selamat datang di Qinari." --lang id -o halo.wav
qinari tts "Hello from Qinari." --lang en -o hello.mp3
cat story.txt | qinari tts - --lang en -o story.wav     # "-" reads stdin
qinari tts --file story.txt --lang en -o story.wav     # same, explicit
```

## Voices

Ten offline voices, all bilingual (Indonesian and English), all from the
Supertonic 3 model:

| Id | Style preset | License |
| --- | --- | --- |
| `supertonic-M1` … `supertonic-M5` | `M1` … `M5` (male) | OpenRAIL-M |
| `supertonic-F1` … `supertonic-F5` | `F1` … `F5` (female) | OpenRAIL-M |

Every voice shares one 398 MB engine bundle (downloaded once); each
voice adds only its ~285 KiB style file. `qinari setup` installs them
(or `--voice <ID>` for a subset): every file is downloaded with `curl`,
verified against its pinned SHA-256 digest and size, resumable, and
each installed voice is validated by a real synthesis. `--list-voices`
shows the catalog.

Installing any Supertonic voice means accepting its **OpenRAIL-M**
license — pass `--accept-license`, confirm interactively, or rely on an
earlier acceptance (recorded in the config under
`tts.accepted_licenses`; one acceptance covers all ten voices). `--yes`
without `--accept-license` skips the voices with a note instead of
accepting silently.

## Engine selection

`--engine auto` (the default) uses the **local** engine whenever the
selected voice's files are installed — installation implies the license
was accepted — and falls back to the **cloud** engine otherwise.
`local` and `cloud` force one engine; `local` without an installed
voice is a clear error naming the missing voice.

## Expression markup

Markup tags are parsed unless `--no-markup` speaks them verbatim:

| Tag | Effect |
| --- | --- |
| `[laugh]` | laughter in the voice's own style |
| `[breath]` | an audible breath |
| `[sigh]` | a sigh |
| `[pause:<ms>]` | an exact silence, e.g. `[pause:400]` |

An unknown tag is a hard error naming the offending tag. The cloud
engines map tags to their own vocabularies (`[laugh]` → `[laughs]` on
ElevenLabs, and so on); `[pause:<ms>]` always inserts exact silence.

## The local engine (Supertonic 3)

A full Rust port of the Supertonic 3 pipeline over four ONNX Runtime
graphs (text encoder, duration predictor, vector estimator, vocoder),
parity-verified against the reference Python SDK: identical latent
draws (seeded `Qrng`), identical mel math — waveform correlation 1.0000
and duration delta 0.00% on the parity corpus.

1. **Text normalization** (`textnorm`): numbers become words in the
   active language (Indonesian `tiga puluh empat`, English
   `thirty-four`; Indonesian uses `.` for thousands and `,` for
   decimals, English the reverse), decimals read their separator
   (`koma` / `point`), and anything the normalizer does not recognize
   passes through untouched.
2. **Chunking**: texts are split at sentence boundaries into chunks of
   at most 300 characters (hard cap 20 000 characters per render);
   chunks are joined with edge-silence trimmed so pause durations are
   exact.
3. **Tokens**: the pinned unicode-indexer table maps codepoints to
   model units (with an NFKD expansion table); expression tags map to
   the model's `<laugh>`/`<breath>`/`<sigh>` tokens.
4. **Synthesis**: the four graphs run with seeded initial latents —
   the same `--seed` renders the same audio, byte-identical — at the
   model's native 44.1 kHz, resampled on request. `--steps` (1–100,
   default 8) trades speed for quality; `--speed` 0.5–2.0 maps into the
   model's 0.7–2.0 range with a warning when clamped.
5. **Prosody post-chain** (`--prosody natural`, the default):
   punctuation-aware pausing and bounded seeded rate jitter; `flat`
   returns the raw SDK pacing.

**Styles**: `--style F3` speaks in another installed voice's timbre
(preset or full id); `--style-file path.json` loads a custom
Supertonic voice-style JSON.

Speech renders well under real time on a modest CPU — RTF 0.29–0.44
measured on the 2-vCPU reference box (`docs/BENCHMARKS.md`); `qinari
setup` reports each voice's validation RTF.

## The cloud engines

### OpenAI-compatible lane

Any `/audio/speech` endpoint (defaults: `https://api.openai.com/v1`,
model `gpt-4o-mini-tts`, voice `alloy`; all configurable in `[tts]`).

- The API key comes from `QINARI_TTS_API_KEY`, falling back to
  `OPENAI_API_KEY`. Keys live in the environment only — never in the
  config file, never in the log.
- Requests retry up to the configured count on 5xx responses with
  linear backoff (300 ms × attempt); a persistent failure surfaces as a
  clean `HTTP 503`-style message, exit code 7.
- Chunks are joined with 0.25 s pauses; mono is upmixed when
  `--channels 2` is asked for.

The retry path is pinned by mock-server tests: authentication headers
and body fields, 5xx-then-success retry counting, timeout on a stalled
body, and rejection of non-WAV responses.

### The ElevenLabs lane

`ElevenLabsEngine` speaks the ElevenLabs v1 TTS API (`POST
/v1/text-to-speech/{voice_id}` with the `xi-api-key` header; endpoint
per request, model and retries configured on the engine).

- The key comes from `QINARI_ELEVENLABS_API_KEY` — environment only,
  never in the config file, never in the log.
- Models: `eleven_v4` (the default) or `eleven_v4_turbo`, the ~100 ms
  real-time variant. Both cover 90+ languages, including Indonesian and
  Javanese.
- The voice is an ElevenLabs voice id (their UUID or a library name),
  either raw or with the `elevenlabs-` prefix; an id configured on the
  engine wins.
- Markup passes through to the model's documented audio-tag vocabulary:
  `[laugh]` → `[laughs]`, `[breath]` → `[exhales]`, `[sigh]` →
  `[sighs]`. `[pause:<ms>]` inserts exact silence; joins without a
  pause use the same 0.25 s as the OpenAI lane.
- Output is WAV, at the supported rate nearest the requested one. Note:
  44.1 kHz WAV output requires a Pro-tier ElevenLabs key — name another
  rate to avoid that gate.
- No streaming, no voice cloning, no pronunciation dictionaries:
  blocking, whole-body requests only.

The lane is pinned by the same mock-server test family as the OpenAI
lane, plus tag-map, pause-exactness, and voice-resolution unit tests.

## Measuring speech quality

`qinari analyze <file> --speech --mos` reports the speech feature set
(voiced fraction, F0 mean/σ/range in semitones, pause statistics) and
DNSMOS MOS estimates (SIG/BAK/OVRL plus P.808), scored by the two
hash-pinned DNSMOS models `qinari setup` vendors (CC BY 4.0,
Microsoft DNS-Challenge). The golden gate in the live test suite pins
the quality floor: DNSMOS OVRL ≥ 3.35, F0 σ ≥ 3.5 st, ≥ 2 pauses, and
the golden paragraph's duration within ±5% — on the pinned golden seed.

## Files

The engine bundle lives under `~/.local/share/qinari/models/supertonic/`
(`onnx/` with the four graphs plus tokenizer tables, `voice_styles/`
with the ten presets); the DNSMOS pair under
`~/.local/share/qinari/models/dnsmos/`. `qinari doctor` reports which
pieces are present and exits 8 when TTS is not usable.

## Model attribution

- **Supertonic 3** (voices and engine bundle) —
  [supertone-oss-archive/supertonic-3](https://huggingface.co/supertone-oss-archive/supertonic-3),
  OpenRAIL-M license. Pinned to revision
  `aafc6e32416a594460b32413efc49d7fe4ce6d46`; every asset digest is
  pinned in the voice catalog.
- **DNSMOS** (speech quality estimation) — Microsoft
  [DNS-Challenge](https://github.com/microsoft/DNS-Challenge) DNSMOS
  models, CC BY 4.0. Digests pinned in `qinari-verify`.

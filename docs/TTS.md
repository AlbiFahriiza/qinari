# Qinari TTS

Qinari speaks Indonesian and English. The same `qinari tts` command
drives two engines: a fully offline neural voice on your machine, and
an OpenAI-compatible cloud service.

```
qinari tts "Halo, selamat datang di Qinari." --lang id -o halo.wav
qinari tts "Hello from Qinari." --lang en -o hello.mp3
cat story.txt | qinari tts - --lang en -o story.wav     # "-" reads stdin
qinari tts --file story.txt --lang en -o story.wav     # same, explicit
```

## Voices

| Id | Language | Quality | License |
| --- | --- | --- | --- |
| `en_US-libritts-high` | English | high | CC BY 4.0 |
| `id_ID-news_tts-medium` | Indonesian | medium | non-commercial |

Voices are installed with `qinari setup` (or per voice with
`--voice <ID>`): the model is downloaded with `curl` and verified
against its pinned SHA-256 digest and size; the small `.onnx.json`
config sibling is downloaded and existence-checked (the catalog pins
no digest for it). Each installed voice is validated by a real
synthesis. `--list-voices` shows the catalog.

The Indonesian voice is for non-commercial use. Installing it means
accepting that license — pass `--accept-license`, confirm
interactively, or rely on an earlier acceptance (recorded in the
config under `tts.accepted_licenses`). `--yes` without
`--accept-license` skips the voice with a note instead of accepting
silently.

## Engine selection

`--engine auto` (the default) uses the **local** engine whenever the
language's voice is installed — installation implies the license was
accepted — and falls back to the **cloud** engine otherwise. `local`
and `cloud` force one engine; `local` without an installed voice is a
clear error naming the missing voice.

## The local engine

The local engine is a VITS model (Piper voice lineage) executed with
ONNX Runtime:

1. **Text normalization** (`textnorm`): numbers become words in the
   active language (Indonesian `tiga puluh empat`, English
   `thirty-four`; Indonesian uses `.` for thousands and `,` for
   decimals, English the reverse), decimals read their separator
   (`koma` / `point`), and anything the normalizer does not recognize
   passes through untouched.
2. **Chunking**: texts longer than 3000 characters are split at
   sentence boundaries, then at spaces.
3. **Phonemes**: the pure-Rust eSpeak NG port phonemizes each chunk
   with the language's dictionary (data extracted offline by
   `qinari setup`).
4. **Synthesis**: token ids run through the ONNX graph
   (`input`/`input_lengths`/scales, per the voice's `.onnx.json`),
   producing 22.05 kHz audio that is resampled to the target rate.
   `--speed 0.5..2.0` scales the length.

Both voices synthesize far faster than real time on a modest CPU (see
`docs/BENCHMARKS.md`); the engine reports RTF (render time divided by
audio duration) during `setup` validation.

## The cloud engine

The cloud engine speaks to any OpenAI-compatible `/audio/speech`
endpoint (defaults: `https://api.openai.com/v1`, model
`gpt-4o-mini-tts`, voice `alloy`; all configurable in `[tts]`).

- The API key comes from `QINARI_TTS_API_KEY`, falling back to
  `OPENAI_API_KEY`. Keys live in the environment only — never in the
  config file, never in the log.
- Requests retry up to the configured count on 5xx responses with
  linear backoff (300 ms × attempt); a persistent failure surfaces as
  a clean `HTTP 503`-style message, exit code 7.
- Chunks are joined with 0.25 s pauses; mono is upmixed when
  `--channels 2` is asked for.

The retry path is pinned by mock-server tests: authentication headers
and body fields, 5xx-then-success retry counting, timeout on a stalled
body, and rejection of non-WAV responses.

## Files

Installed voices live under `~/.local/share/qinari/models/<voice-id>/`
(`voice.onnx` + `voice.onnx.json`), eSpeak NG data under
`~/.local/share/qinari/models/espeak-data/`. `qinari doctor` reports
which pieces are present and exits 8 when TTS is not usable.

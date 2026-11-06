# The Qinari audio pipeline

This page follows one render from command line to finished file. The
same pipeline runs for `sfx`, `ambient`, `music`, `tts`, and `master`;
only the generator at the front differs.

```
generator ──► AudioBuffer ──► resample ──► mastering chain ──► dither ──► encode ──► atomic rename
              (f32 planar)     (rubato)     (character + LUFS)  (depth)   (wav/mp3)
```

## 1. Generation

Every generator is a pure function of its inputs — recipe or style,
parameters, duration, seed, sample rate, channels — and renders into a
planar `AudioBuffer` of `f32` samples (`qinari-dsp`). Same inputs,
bit-identical output; seeds are text hashed through a stable string
hash, so `--seed storm` and `--seed 2026` are equally valid.

The DSP core provides the shared vocabulary: oscillators with PolyBLEP
band-limiting, Paul Kellet pink noise, RBJ biquads and a TPT state
variable filter, ADSR and T60 envelopes, delay lines, a Schroeder
reverb, compressors and a lookahead stereo limiter, Karplus–Strong
plucks, and an ITU-R BS.1770 loudness meter. Two safety rules run at
the primitive level: every filter design clamps its cutoff below
Nyquist (poles of an RBJ design leave the unit circle at or above
Nyquist, which once produced NaNs at 8 kHz), and noise bursts are
peak-normalized so recipes build on predictable levels.

Sound effects are layered mixes: a shaped noise bed plus discrete
events (rain droplets are Poisson-distributed pings, footsteps fall on
a step grid, crickets chirp in pulsed events). Ambient presets layer
whole recipes with slow motion of their own. Music is composed, not
looped: per-style chord pools, a motif with development, style-specific
drums, and an intro/verse/bridge/outro layout — a track may run a
little past the requested duration to finish its section.

## 2. Resampling

Renders happen at the generator's natural rate; if `--sr` asks for a
different one, the buffer is resampled with rubato's FFT resampler
(`qinari-audio::resample`). A 1 kHz tone survives the 44.1 kHz to
48 kHz round trip within test tolerance.

## 3. Mastering

With mastering on (the default), the house chain runs
(`qinari-audio::master`):

1. Measure integrated loudness (BS.1770 K-weighting, 400 ms blocks /
   100 ms step, absolute and relative gating).
2. Apply the gain that moves the render to the target LUFS
   (default −16).
3. Run the character chain per channel: DC blocker, character shelves
   (RBJ), compressor, then a stereo-linked lookahead limiter that
   enforces the ceiling as a hard brickwall (default −1.0 dBFS): the
   gain attacks instantly on the 5 ms window maximum, so even a
   full-scale click cannot escape, and a final clamp guarantees it for
   any input.
4. Re-measure and correct with up to three clamped ±2 dB steps while
   the residual exceeds 0.3 LU.

The three characters differ in shelf gains and compressor law:
`transparent` keeps the spectrum flat and barely touches dynamics
(1.25:1 at −12 dB), `warm` lifts lows and tames presence, `punchy`
adds low weight, presence, and a firmer 3:1 ratio. On this machine the
chain lands a 10 s render within 0.1 LU of the target and transient
recipes exactly at the ceiling (see `docs/BENCHMARKS.md`).

`qinari analyze` measures the same integrated LUFS, peak, and RMS with
the same meter, so what the render summary reports and what `analyze`
finds always agree.

## 4. Dither

WAV 16 and 24-bit outputs pass through TPDF dither (±1 LSB); 16-bit
additionally uses second-order noise shaping (`x − 2e₁ + e₂`), which
the test suite verifies pushes the in-band noise floor at least 5×
below flat dither on a −60 dBFS tone. 32f output passes through
untouched. The dither seed is fixed, so identical renders stay
bit-identical.

## 5. Encoding

`.wav` is written by `hound` (16/24 int, 32f) and `.mp3` by LAME
through `mp3lame-encoder` (128–320 kbps). Two LAME pitfalls are pinned
by tests: mono input is duplicated into both PCM channels (a null
right channel segfaults LAME), and the output vector is pre-reserved
because LAME writes into spare capacity. Every output is encoded to a
`.qinari-tmp` sibling first and renamed into place, so a failed encode
never leaves a half-written file behind.

`qinari master <input>` decodes an existing `.wav`/`.mp3` (symphonia),
runs stages 3–4 of this pipeline, and writes the result — the same
chain, entered halfway.

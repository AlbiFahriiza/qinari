# Qinari 0.0.5-alpha build plan (recovery)

STATUS BOARD (updated at each checkpoint):
- M0 docs ✓ 9fb6724 | M1 scaffold ✓ 72b707d | M2 dsp ✓ 3de49b5
  (112 tests) | M3 audio ✓ ae24cf2 (31 tests) | M4 sfx + M5 music +
  M6 tts ✓ 02956a8 — recovered from the rolled-back sandbox and
  gate-verified on the rebuild machine: 224 tests / 0 failed /
  4 ignored (live TTS, need installed voices), clippy -D warnings
  clean, fmt clean. ALL PUSHED to origin/main.
- NEXT: M8 release assets, verification phase, GOALS evidence, tag
  v0.0.5-alpha.
- M7 cli ✓: workspace now 294 tests / 0 failed / 4 ignored (live TTS);
  clippy -D warnings + fmt clean; black-box smoke verified (exit codes
  0/2/6/7/8, byte-identical determinism, negative --target-lufs,
  batch out-NNN, listings, doctor on empty HOME).
- Delegation pattern that works: agents MUST read Panoptes skill fully
  + BUILD-PLAN.md section, then write+gate; subagent Task prompts must
  stay SHORT (point at files, not inline specs). Long prompts or 3
  parallel heavy agents hit tool deadlines — run at most 1-2 heavy
  build agents at a time.
- Verified API surfaces for M4-M7 are in the committed code itself
  (read crates/qinari-dsp/src/*.rs and crates/qinari-audio/src/*.rs).
- LUFS test anchor (corrected): stereo L=R sine at -23 dBFS/channel
  == mono -20 dBFS == -23.0 LUFS.
- Time crate must NOT enter the tree (RUSTSEC-2026-0009 in old
  tracing-appender path) — CLI logging writes via Arc<File> directly. Below are the specs for the remaining
milestones, encoding every design decision and previously-solved bug fix.
Agents: read Panoptes skill fully first (repo-external bundle), then your
section. Build gates before reporting: fmt, clippy -D warnings, tests.

## M2 qinari-dsp (pure computation, no I/O, only dep thiserror)

Modules (lib.rs `pub mod` all, docs on every public item):
- **buffer.rs** `AudioBuffer` planar Vec<Vec<f32>>+sr: new, from_channels
  (reject ragged), frames/num_channels/sample_rate/duration, get/set,
  peak, rms, has_non_finite, apply_gain_db, map_stereo(f), mix, concat,
  fades reach exactly 0.
- **rng.rs** `Seed(pub u64)` from_u64/from_text (string hash, stable);
  `Qrng` xoshiro256++ seeded via splitmix64: new, next_u64, next_f32,
  next_range, next_int_range, fork.
- **analysis.rs** Goertzel magnitude at freq; zcr. Tests: tone selectivity.
- **filter.rs** (ALL f0 clamped to [1.0, nyquist*0.4995] — prevents real
  blow-up: RBJ poles leave unit circle at f0≥Nyquist):
  `Biquad` RBJ TDF2. LP numerators b=[(1−c)/2,(1−c),(1−c)/2] (NOT
  [1,2,1]); HP b=[(1+c)/2,−(1+c),(1+c)/2]; alpha=sin(w0)/(2q). Kinds:
  Low/High/Bandpass/Notch/Allpass/PeakingEq. process(x): y=b0x+z1;
  z1=b1x−a1y+z2; z2=b2x−a2y.
  `Svf` TPT Simper wiring (as fundsp): g=tan(πf0/sr), k=1/q,
  a1=1/(1+g(g+k)), a2=ga1, a3=ga2; v3=x−ic2; v1=a1ic1+a2v3;
  v2=ic2+a2ic1+a3v3; ic1=2v1−ic1; ic2=2v2−ic2; OUT low=v2, band=v1,
  high=x−kv1−v2 (transcribing v1/v2 swapped was a real bug). set() keeps
  state. `OnePole` a=exp(−2πf0/sr), hp via invert. `DcBlocker`
  y=x−x1+0.995y1.
  Tests: Goertzel response (LP@1k passes 500Hz, kills 4kHz >10dB);
  Nyquist regression: LP at sr=8k f0=6000 fed 8k noise samples stays
  finite/bounded.
- **osc.rs** `OscKind{Sine,Saw,Square,Triangle,Pulse}`; `Osc::new(kind,f,
  sr,&mut Qrng)` random phase; PolyBLEP (dt=f/sr) for Saw/Square/Pulse;
  Triangle=square→leaky integrator; set_freq, glide_to, set_pulse_width;
  `FmOsc::new(carrier,ratio,index,sr)`. midi_to_freq/freq_to_midi.
  fast_sine helper. Tests: harmonic structure via Goertzel.
- **noise.rs** `NoiseGen{White,Pink}` (Voss or Kellet, documented);
  `noise_burst(sr,rng,cutoff,decay)` lowpassed noise, exponential decay,
  PEAK-NORMALIZED to 0.7; `swept_noise(sr,rng,f0,f1,secs,q)` bandpass
  sweep, also normalized to 0.7 (recipes depend on this).
- **env.rs** `Adsr` exponential curves (one-pole toward target — not
  naive exp from 0, which never rises); trigger/release/is_finished.
  `Percussive::from_t60(sr,t60)` level starts 0, `trigger()` sets 1,
  decay exp(−t/τ), τ=t60/6.91. Tests: sustain ±5%, −60dB±3dB at t60,
  untriggered = silence.
- **lfo.rs** Lfo{Sine,Triangle,Saw,Square,SampleHold(rng)} [−1,1].
- **pan.rs** equal-power: lg=√(0.5(1−pos)), rg=√(0.5(1+pos)).
- **fx.rs** `Delay` (feedback); `Freeverb` Schroeder: per ch 8 combs
  (1116,1188,1277,1356,1422,1491,1557,1617 ×sr/44100, fb 0.84, damp
  0.2) + 4 allpass (556,441,341,225, g 0.5), right ch +23 samples;
  `Compressor(thresh,ratio,attack,release,makeup)` envelope+gain law;
  `StereoLimiter` lookahead 5ms stereo-LINKED (gain from max of the two
  channels, monotonic-deque sliding max keyed by absolute position — a
  circular-index version was an index-safety bug; fast attack 5ms,
  release 50ms, ceiling linear); `Drive` tanh; `Crush` bit quantize;
  `Tremolo` LFO amp.
- **karplus.rs** `pluck(sr,f0,t60,rng)`: D=round(sr/f0) noise burst;
  y[n]=decay·½(y[n−D]+y[n−D−1]); decay=10^(−3D/(sr·t60)).
  Tests (calibrated physics): 220Hz peak 0.015–0.05, neighbors ≥10×
  lower; retune moves spectral peak.
- **lufs.rs** `KWeighting` (RBJ shelf f=1681.97 Q=.7071 gain+3.999dB +
  HP f=38.135 Q=0.5, arbitrary sr); `LoudnessMeter::new(ch,sr)`
  process(&[f32]) per-sample frames, 400ms blocks/100ms step, absolute
  −70 LU + relative −10 LU gating; integrated() Option<f64>;
  `integrated_lufs(&AudioBuffer)`. Test anchor: 997Hz @−20dBFS stereo
  (L=R) = −23.0 LUFS ±0.5; silence → None.
- **dither.rs** `Dither::new(bits,noise_shaped,rng)` TPDF ±1LSB twice;
  shaped: quantizer input x'=x−2e1+e2 (SUBTRACT history — sign was a
  real bug), error=q−x'. Test: 16-bit −60dBFS 1kHz 0.5s: shaped in-band
  (300–1500Hz) noise ≥5× lower than flat; no DC bias; finite.

## M3 qinari-audio (deps: dsp + hound + mp3lame-encoder + rubato + symphonia)

- **format.rs** `validate_output_path` (only .wav/.mp3 else FormatError
  with ".wav and .mp3 only" message), `BitDepth{Bits16,Bits24,Float32}`,
  `Mp3Bitrate{128,192,256,320}`.
- **wav.rs** read/write via hound (16/24 int, 32f).
- **mp3.rs** `write_mp3(path,&buffer,bitrate)`: mp3lame-encoder
  LAME 4.0. CRITICAL (two real segfaults fixed): (1) MONO: duplicate the
  mono buffer into both pcm_l and pcm_r AND set Mode::Mono — passing
  NULL right channel segfaults; (2) MUST `reserve(bytes)` on the output
  Vec before `encode_to_vec` — LAME writes into spare capacity, 0-cap
  buffer segfaults. Close/flush per chunk.
- **resample.rs** rubato 5.0.1: `Fft::new(input_rate, output_rate,
  channels, subsec length)` + `process_all` (verify exact API from the
  installed source in ~/.cargo/registry — don't guess).
- **decode.rs** symphonia 0.6.1 API (read installed source; differs
  totally from 0.5): decode wav+mp3 to AudioBuffer.
- **master.rs** `Character{Transparent,Warm,Punchy}`;
  `MasteringSettings{target_lufs,ceiling_db,character}`;
  `master(&buffer,&settings)->(AudioBuffer,MasteredReport)`:
  1) measure integrated_lufs, apply target−current gain;
  2) CharacterChain per character params:
     transparent (low_shelf 0, presence 0, air 0, ratio 1.25,
     thresh −12, attack 30ms, release 250ms, makeup 0)
     warm (low +2dB, presence −1, air −1.5, ratio 2, thresh −16,
     attack 30ms, release 300ms, makeup 1.5)
     punchy (low 2.5, presence 2, air 0, ratio 3, thresh −18,
     attack 8ms, release 120ms, makeup 3.5)
     chain = DcBlocker per ch → shelf EQs (RBJ) → Compressor per ch →
     StereoLimiter(ceiling);
  3) VERIFICATION LOOP: re-measure, residual=target−measured; while
     |residual|>0.3 and iters<3: apply clamped ±2dB step, rerun chain;
  4) report input/output LUFS, applied gain, peak.
  `finalize_for_depth(&buffer,depth)`: 32f passthrough; 16/24 TPDF
  dither (Dither::new(bits, noise_shaped = (bits==16),
  Qrng::from_seed(Seed::from_text("qinari-master-dither")))).
  Tests: 997Hz −20dBFS → target −16 ±1.0 LU (also warm/punchy), peak ≤
  ceiling+ε; mp3 roundtrip; wav depths; resample 48k↔44.1k preserves
  1kHz tone; mono mp3 (was segfault).

## M4 qinari-sfx (deps: dsp + strsim)

- **params.rs** `ParamSpec` (name, kind Float/Int, default, min, max,
  doc), `spec!` macro; ParamSet::get(name)->f64 with default fallback;
  validation errors: unknown param → "did you mean …", out of range →
  "out of range (min..max)".
- **recipes.rs** 29 recipes EXACTLY these ids: rain, wind, thunder,
  ocean, fire, stream, explosion, impact, whoosh, sword, laser, blaster,
  teleport, forcefield, robot_talk, ui_success, ui_error, ui_click,
  sonar, alarm, engine, glitch, riser, hit, sub_drop, heartbeat,
  footsteps, spell_cast, crickets. `Recipe{id,description,params: &[],
  render: fn(&RenderContext)->Result<AudioBuffer,String>}`;
  `recipes() -> &'static [Recipe]`. `RenderContext{params,seed,sr,
  channels,frames}`; `Bus` (add(frame,ch,val), add_panned(frame,pos,&[f32]),
  buffer()). Helpers: ping (sine+Percussive t60, trigger!), noise_burst,
  swept_noise, poisson_events(rng,frames,sr,rate)->Vec<usize> (sorted),
  seconds_to_frames. Design each recipe from its name using the DSP
  vocabulary (e.g. rain = pink bed LP 1500+6500·int−1200·dist + HP
  400−250·dist + 0.07Hz gust + poisson droplet pings 2200–6700Hz panned;
  laser = FM osc pitch fall 2000→200Hz 300ms; ui_success = two sine
  blips 880→1318; heartbeat = 60bpm paired thumps LP'd; footsteps =
  noise bursts on step grid; crickets = pulsed 4.2kHz chirps events).
  All recipes: use ctx rng forks; Percussive ALWAYS trigger().
- **ambient.rs** 5 presets: storm, campfire, ocean, spaceship, meadow —
  layered recipes with own motion; `ambient_presets()`, `find_preset`,
  `render_ambient(preset,secs,seed,sr,ch)`; `AmbientError{UnknownPreset,
  BadDuration(1..3600), Sfx, Render}`.
- **lib.rs** `RenderRequest{recipe,params,duration_secs,seed,sr,ch}`,
  `render(&req)`, `find_recipe`, `suggest_recipe(typo,n)` (Levenshtein),
  `SfxError{UnknownRecipe,BadDuration(0.05..600),BadChannels,
  BadParameter,Render}`.
- tests/: all recipes stereo 48k non-silent bounded deterministic;
  ALL recipes at sr=8000 stay finite (Nyquist clamp regression);
  mono renders; unknown recipe suggests; param validation.

## M5 qinari-music (deps: dsp)

`STYLES` (id, description, bpm (min,typ,max)): lofi (72,82,90) warm
jazzy swung; ambient (58,64,72) pads no drums; synthwave (100,110,118)
driving; cinematic (76,88,96) tense; chiptune (128,140,150) squares.
`TrackRequest{style,duration_secs,bpm,seed,sr,ch}`,
`render` (duration 5..600 min, may run past to finish section),
`MusicError{UnknownStyle,BadDuration,BadChannels}`. Composer: seeded
chord progressions (per style pools), motif with development
(transposition/variation/density), drums per style (noise/synth),
section layout intro/verse/bridge/outro. Tests: 5 styles render
non-silent bounded, sections audibly distinct (energy variance between
sections > threshold), determinism.

## M6 qinari-tts (deps: dsp + ort =2.0.0-rc.15 + espeak-ng + espeak-ng-data-{en,id} + ureq + serde/serde_json + hound + thiserror)

- **voices.rs** `VoiceEntry{id,language,quality,sample_rate:22050,
  size_bytes,url,config_url,sha256,license,license_url,non_commercial}`;
  catalog: en_US-libritts-high 136673811B, CC BY 4.0, url
  https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/
  libritts/high/en_US-libritts-high.onnx (+ .onnx.json config; verify
  real URLs + sha256 by downloading once and hashing — NEVER invent
  hashes); id_ID-news_tts-medium 62950044B NON-COMMERCIAL (Lessac
  lineage) (+ config). voices(), find_voice(), default_voice(lang).
  Store pinned sha256/size constants (measured, quoted in report).
- **textnorm.rs** expand_numbers(text,lang): id uses '.' thousands ','
  decimals; en reversed; numbers→words under_thousand (en "thirty-four"
  hyphenated; id "tiga puluh empat"); decimals "koma"/"point"; huge →
  passthrough. chunk_text(text,3000): sentence boundaries then spaces.
- **engine.rs** SynthRequest{text,language,speed,voice,sr,channels},
  validate (non-empty, ≤20000 chars, speed 0.5..=2.0, ch 1|2),
  Synthesis{audio,stats}, SynthesisStats{elapsed,audio_secs,rtf}.
- **local.rs** LocalVitsEngine::load(model_path,espeak_dir,&voice) +
  synthesize: espeak-ng crate (pure Rust port; set data dir via API —
  read installed source for init) phonemize per language → token ids →
  VITS ONNX via ort (Environment behind LazyLock — only ONE Environment
  per process allowed, that was a real bug; inputs typically
  input/input_lengths/scales `[0.667,1.0,0.8]`-ish — read the voice's
  .onnx.json for the exact input names/values and feed accordingly);
  22.05kHz output → AudioBuffer. ensure_espeak_data(dir,&[langs]):
  writes bundled espeak-ng-data crates' data to dir, marker file
  .qinari-espeak-{lang} for idempotency. Tests: textnorm units;
  phoneme mapping sanity; #[ignore] live tests: en voice RTF<1.0
  serial, id voice RTF<0.5, speed changes duration.
- **cloud.rs** CloudConfig{base_url,model,voice,api_key,timeout,
  retries}; OpenAiCompatibleEngine::new (key from config else env
  QINARI_TTS_API_KEY then OPENAI_API_KEY; NoApiKey error) via ureq
  agent timeout_global; synthesize: normalize → chunk(3000) →
  POST {base}/audio/speech json{model,voice,input,
  response_format:"wav",speed} → decode wav bytes (hound, int/float,
  any channels) → join 0.25s pauses → mono→stereo if asked.
  RETRY (real fixed bug): ureq 3.x returns non-2xx as
  Err(StatusCode(code)) — retry 5xx THERE (attempt<retries, backoff
  300ms·n), NOT only in the Ok arm (dead code before). 5 std-only
  mock-server tests (thread-per-conn on 127.0.0.1:0, scripted Vec<u8>
  responses, record requests): bearer auth+model/format in body+wav
  decodes; retry 5xx×2→200 (3 requests); persistent 503 fails with
  "HTTP 503"; stalled body → timeout error; non-wav body → clean
  "response" error. Mock header parsing must be case-insensitive (ureq
  lowercases header names — real flaky-test cause).

## M7 qinari-cli (deps: all + clap derive+wrap_help + clap_complete + dialoguer + directories + indicatif + serde + toml + sha2 + strsim + tracing + tracing-subscriber(env-filter,registry) + assert_cmd/predicates dev)

- **xcode.rs** ExitCode{Ok=0,Internal=1,Usage=2,Setup=3,Network=4,
  Render=5,Encode=6,Tts=7,Unhealthy=8}; QinariError{message,code} ctors
  usage/setup/download/render/encode/tts; From<FormatError>→encode(6).
- **config.rs** Paths::resolve via directories ProjectDirs (config
  ~/.config/qinari/config.toml, models ~/.local/share/qinari/models,
  espeak_dir models/espeak-data, state ~/.local/state/qinari);
  voice_model(id)=models/id/voice.onnx; voice_config .onnx.json.
  Config{version,output{sr 44100,bit "24",mp3 320,master true},
  mastering{lufs −16,ceiling −1,character transparent}, tts{engine auto,
  base https://api.openai.com/v1, model gpt-4o-mini-tts, voice alloy,
  accepted_licenses[]}} serde default; load (missing→default; corrupt
  →hard error with path); save creates parent of config_file.
- **download.rs** curl subprocess (—fail —location —silent —show-error
  —retry 3 —continue-at -), DownloadSpec{url,dest,expected_size,
  expected_sha256,label}; verify_existing sha256 (mismatch→Checksum
  err); HEAD content-length for progress; indicatif bar; atomic rename
  from .part; file_sha256 streamed. tracing::debug/info/warn events.
- **render.rs** OutputOverrides; RenderOptions::resolve (validate
  sr 8000..=192000 → Config error exit 3; depth 16/24/32f; bitrate
  128/192/256/320; character); finish_and_write: resample if needed →
  master (or skip) → finalize_for_depth → encode to .qinari-tmp →
  rename → RenderSummary{out,duration,sr,peak,in/out LUFS,size,
  mastered} with describe(); tracing::info "render complete".
- **commands.rs** parse_duration (ms/s/m/h + bare secs);
  parse_sets (NAME=VALUE, unknown → "did you mean"); seed_from (empty →
  nanos); suggest(typo,cands) Levenshtein ≤3; sfx_error/ambient_error/
  music_error mappers: UnknownX→usage(2)+suggestion, BadParam/Duration/
  Channels→usage(2), Render→render(5). setup (list-voices; banner;
  check_environment curl+dirs; ensure_config; ensure_espeak_data
  offline; install_voice per voice: NC license gate —
  accept_license||already→ok, --yes without accept→skip note, else
  dialoguer Confirm; PERSIST acceptance to config
  accepted_licenses (real bug fixed: was never recorded); download
  model+config; validate_synthesis with a real sentence + RTF);
  doctor(paths,config)->bool (curl, espeak markers en+id, installed
  voices ≥1 = health; missing config informational NOT unhealthy;
  cloud key presence informational; engine pref) exit 8 unhealthy;
  sfx (list; batch N variations out-NNN.wav, seed+run offset);
  ambient; music; tts (lang ∈ {id,en} else usage error — real fix;
  engine auto=local when voice for lang INSTALLED (installed implies
  license accepted) else cloud); master_command (decode→master→write,
  tracing); analyze (missing file → usage error 2 — real fix; LUFS/
  peak/rms/duration print); bench (sfx rain 10s + music lofi 10s,
  error-propagating, --json); info (--json); read_tts_text (file/stdin
  "-" default).
- **main.rs** clap: global --config/--quiet/--verbose Count;
  #[command(allow_negative_numbers=true)] ON Cli AND each command
  struct (real bug: --target-lufs -14 failed otherwise); struct-variant
  subcommands Setup/Sfx/Ambient/Music/Tts/Master/Analyze/Bench/Info/
  Completions/Doctor; run per-command handlers; --config →
  paths.config_file.clone_from(path) BEFORE Config::load (real bug: was
  parsed but never used); install_broken_pipe_hook (panic hook →
  Broken pipe → exit(141) silent, no unsafe); init_logging:
  OpenOptions append Arc<File> writer to state/qinari.log (NO
  tracing-appender — it dragged the `time` crate + RUSTSEC advisory),
  level WARN/INFO/DEBUG by -v; error arm tracing::error always.
- **tests/cli.rs** assert_cmd: version/help; listings (29 recipes, 5
  ambient, 5 styles, 2 voices w/ NON-COMMERCIAL marker); sfx render +
  decode non-silent + duration; unknown recipe stderr contains
  "did you mean"; flac exit 6 ".wav and .mp3 only"; determinism
  byte-cmp; batch 3; mp3 roundtrip; master+analyze; tts unknown voice
  exit 7; bad duration exit 2; bad lang exit 2; doctor exit 8 on empty
  HOME + exit 0 with stubs (isolated HOME env); info --json;
  completions.

## M8 release assets
- benches: dsp.rs (filters/osc/lufs), sfx.rs (rain/thunder/explosion/
  laser/footsteps 10s + ambient storm 60s, sample_size 10), music.rs
  (lofi/synthwave/cinematic 30s), audio.rs (resample/master/mp3).
  clippy pedantic wants `;` after b.iter closures. [[bench]]
  harness=false + criterion workspace dev-dep.
- benchmarks/numpy_reference.py: faithful vectorized rain (Voss pink,
  RBJ lfilter, 0.07Hz gust, Poisson droplets, equal-power pan, 0.7
  norm); QINARI_BIN env → prints both + speedup.
- Installer.sh: set -euo pipefail; curl+sha256 verify; --prefix
  (default ~/.local), --tag, --uninstall, --build, --dev, --version,
  --help; exit codes 0/1/2/3/4/5; bash -n + shellcheck clean.
- .github/workflows/release.yml: on tags v*; build, smoke, tar
  dist/qinari-x86_64-unknown-linux-gnu.tar.gz + .sha256 sidecar,
  verify checksum logic, softprops gh-release with body.
- docs/BENCHMARKS.md (measured numbers + env + reproduce cmds + numpy
  comparison), docs/CLI.md (all commands/flags/config/env/exit codes/
  examples; 29 recipes), docs/AUDIO-PIPELINE.md, docs/TTS.md, README
  final. GOALS.md evidence column updated to PASS with numbers.

## Verification phase (after M8)
Re-run: fmt, clippy, test ×3 (flaky check), bench --no-run, deny,
bash -n + shellcheck Installer.sh, black-box CLI sweep vs docs/CLI.md,
docs claims audit vs source, git history secret scan. Fix findings,
re-verify. GOALS.md evidence. Final push + tag v0.0.5-alpha.

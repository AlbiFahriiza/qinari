# Verification 0.2.0 — derived parameters and evidence

Per-task freeze records. Every number that an implementation may treat as a
target lives here first, with its primary-source citation. Nothing in this
file may be copied into code or tests without the citation traveling along.

Source file convention: `[S1] txt L<n>` refers to line `n` of the
layout-preserved extraction of the primary source at
`work/sources/s1-mcdermott-2011.txt` (2820 lines, main article + supplement).

## P0-2 — McDermott-Simoncelli 2011 texture-statistic model

Primary source (unblocked and independently re-verified in the P0-2
session of 2026-11-08):

- [S1] McDermott, J.H. & Simoncelli, E.P. (2011). Sound texture perception
  via statistics of the auditory periphery: evidence from sound synthesis.
  *Neuron* 71(5):926–940, incl. Supplemental Information.
- Source URL:
  `https://mcdermottlab.mit.edu/papers/McDermott_Simoncelli_2011_sound_texture_synthesis.pdf`,
  accessed 2026-11-08.
- PDF: `work/sources/s1-mcdermott-2011.pdf`,
  SHA-256 `14e0f5f6bebb51a4101b0efd37d420109f2499b43a48dcd0393e0d067bcda9f3`
  (recomputed twice — initial derivation and the P0-2 freeze session —
  matches).
- Text extraction: `work/sources/s1-mcdermott-2011.txt` (layout-preserved,
  2820 lines by `wc -l`). Every formula below was additionally re-extracted
  from the PDF with a second, independent extractor (pypdf) during the
  freeze session; the two extractions agree everywhere except glyph
  ordering inside displayed equations, where pypdf preserves the radical
  that the layout-preserving extraction renders as `pffiffiffi`.

Secondary source (the authors' own published instantiation, consulted where
the paper is silent or ambiguous, as sanctioned by PLAN-0.2.0 §7.5 P0-2
step 1):

- [S1-impl] McDermott Lab *Sound Texture Synthesis Toolbox v1.7* (MATLAB,
  "Dec 2012 -- Josh McDermott"), downloaded during the initial derivation
  from
  `https://mcdermottlab.mit.edu/Sound_Texture_Synthesis_Toolbox_v1.7.zip`
  (accessed 2026-11-08),
  SHA-256 `ee4a968ae0bdf43df33e9108bfe37c4f59ab7442452d2a50bf5d67411b690bdd`
  (recomputed in the freeze session, matches).
  Key files read: `synthesis_parameters_2011_Neuron_paper.m`,
  `make_erb_cos_filters.m`, `make_constQ_cos_filters.m`,
  `make_octave_cos_filters2.m`, `generate_subbands.m`,
  `measure_texture_stats.m`, `stat_mod_power_win.m`,
  `stat_corr_filt_win_full.m`, `stat_mod_C2_win.m`,
  `stat_central_moment_win.m`, `stat_var_win.m`, `set_measurement_window.m`,
  `apply_filter.m`.
- Policy: **where [S1]'s printed formulas are explicit, they win**; where
  [S1] is silent (cutoff frequencies, anti-alias shape), [S1-impl] supplies
  the authors' choice; where the two diverge, the divergence is recorded in
  §G and the paper's formula is the frozen spec.

Page map of the extraction (from the page footers in the text): p.927 =
txt L100–142 (Results, "Texture Model"); p.928 = L143–257 (Figure 2);
p.929 = L258–352 (Figure 3); p.936 = L~1500–1517 (Experimental Procedures,
"Auditory Model" opening); p.937 = L1518–1608; p.938 = L1609–1706;
Supplemental pp.9–11 = L2385–2499 (C2 mechanics, Fig. S6); Suppl. p.13 =
L2631–2652; Suppl. p.15 = L2728–2759.

### A. Cochlear filterbank — 32 channels = 30 bandpass + 2 endpoint filters

The "30 vs 32" conflict is resolved by the paper itself: the Results
narrative says "a bank of 30 bandpass cochlear filters" (p.927, txt
L127–128), while every statistic formula indexes `k ∈ [1,32]` (p.937–938,
txt L1533, L1556, L1603, L1626, L1633). The Experimental Procedures state
the bank "perfectly tiled the frequency spectrum … (to achieve this, the
filter bank also included lowpass and highpass filters at the endpoints of
the spectrum)" (p.936–937, txt L1524–1530). 30 bandpass + 1 lowpass + 1
highpass = 32 channels; the statistic totals in §D confirm 32 exactly.
[S1-impl] `make_erb_cos_filters.m` returns "N+2 filters … because FILTS
also contains lowpass and highpass filters", and
`synthesis_parameters_2011_Neuron_paper.m` sets `N_audio_channels = 30`
"excluding lowpass and highpass filters on ends of spectrum". The
implementation uses **32 channels**: index 0 = lowpass, 1..=30 = bandpass,
31 = highpass.

The center-frequency construction is fixed by [S1-impl]
`make_erb_cos_filters.m` + `synthesis_parameters_2011_Neuron_paper.m`
(`low_audio_f = 20` Hz, `hi_audio_f = 10000` Hz), and it is the only
construction that reproduces every Figure 2 axis label:

- **32 cutoffs** equally spaced on the ERB scale from 20 Hz to
  10000 Hz (31 intervals): `u_i = ERB(20) + i·S`, `S = (ERB(10000)−ERB(20))/31`,
  `i = 0..31`, cutoff_i = ERB⁻¹(u_i).
- Bandpass k (k = 1..30) spans cutoffs k..k+2: center at the ERB midpoint
  `uc_k = ERB(20) + k·S`, full support `[cutoff_k, cutoff_{k+2}]`
  (ERB width 2S), half-cosine response on the ERB scale.
- ERB scale (G&M 1990 as cited p.936 txt L1508; [S1-impl] `freq2erb.m`):
  `ERB(f) = 9.265·ln(1 + f/228.8455)` Hz (228.8455 = 24.7·9.265).

The paper's "spanning 52–8844 Hz" (p.936, txt L1507–1510) is the resulting
**bandpass-center span**, quoted to the precision of its own Figure 2
labels: computed centers 51.69 / 596.32 / 2198.28 / 2507.39 / 8844.46 Hz
round to the labels 52 / 596 / 2200 / 2507 / 8844 (Figure 2, p.928, txt
L150/L158/L160/L163; panel B "Envelopes (2200 Hz Channel)", txt L168).
Discrimination against the alternative "centers anchored exactly at
52 and 8844 over 29 intervals" (the pre-derivation's reading): that
construction gives center 10 = 596.93 Hz and center 20 = 2508.34 Hz, which
round to 597 and 2508 and contradict the printed labels 596 and 2507; the
cutoff construction matches all five anchors. Rejected.

Frozen parameters:

| Parameter | Value | Citation |
|---|---|---|
| Bandpass filter count | 30 | p.936, txt L1507; p.927, txt L127; [S1-impl] params N=30 |
| Cutoff range | 20–10000 Hz, 32 ERB-evenly-spaced cutoffs | [S1-impl] `make_erb_cos_filters.m` (cutoffs from `freq2erb(low_lim)` to `freq2erb(hi_lim)`, N+2 of them) + params `low_audio_f=20`, `hi_audio_f=10000`; paper quotes the resulting center span "52–8844 Hz" (p.936, txt L1507–1510) |
| ERB scale | `ERB(f) = 9.265·ln(1 + f/228.8455)` | G&M 1990 as cited p.936 txt L1508; [S1-impl] `freq2erb.m` |
| Response shape | half-cosine ("positive portion of a cosine function"), zero-phase, on the ERB scale: `H_k(f) = cos(π·(ERB(f)−uc_k)/(2S))` on the support, 0 outside | p.936, txt L1505–1506; [S1-impl] same construction |
| Half-support | one cutoff spacing S (supports of width 2S; adjacent centers S apart → adjacent filters overlap by 50%; summed squared response constant = perfect tiling, cos²+sin²) | 50% overlap: [S1-impl] `make_erb_cos_filters.m` header "Adjacent filters overlap by 50%"; Suppl. p.13 txt L2644–2646 ("again equally spaced on an ERB scale, with adjacent filters overlapping by 50%"); perfect tiling: p.936–937 txt L1524–1530 |
| 3 dB bandwidth | S (one center spacing) ≈ 1.11 × ERB_GM(f_c); the paper describes the outcome as "≈5% larger than ERBs measured at 55 dB SPL" | follows from the half-cosine (3 dB at half the half-support); p.936, txt L1510–1513 |
| Endpoint filters | lowpass = sqrt(1−H_1²) below the first bandpass center (transition 20→51.69 Hz, flat 1 below), highpass = sqrt(1−H_30²) above the last bandpass center (transition 8844.46→10000 Hz, flat 1 above), so the summed squared response is 1 over [0, Nyquist] | p.936–937, txt L1524–1530; [S1-impl] `make_erb_cos_filters.m` end filters |
| High cutoff at low sample rates | hi_lim clipped to Nyquist (the 2011 analysis resampled audio to 20 kHz; this implementation analyzes at the native rate, so above 10 kHz only the highpass channel is active) | [S1-impl] `make_erb_cos_filters.m` `if hi_lim>sr/2` guard + params `audio_sr=20000`; native-rate choice documented in `cochlea.rs` |

### B. Envelope path

| Parameter | Value | Citation |
|---|---|---|
| Envelope | magnitude of the analytic signal (Hilbert) of each subband | p.937, txt L1536–1538 |
| Compression | envelope raised to the power 0.3 (basilar-membrane compression), applied at the native rate before downsampling | p.937, txt L1539–1540; p.927, txt L110–112; order (compress → downsample): [S1-impl] `measure_texture_stats.m` |
| Envelope rate | 400 Hz (Nyquist 200 Hz), downsampled following low-pass filtering; negatives from resampling ringing clipped to 0 | p.937, txt L1540–1542; [S1-impl] `measure_texture_stats.m` (`resample` then `subband_envs(subband_envs<0)=0`) |
| Anti-alias low-pass shape | **not specified in [S1].** Frozen implementation choice: zero-phase, gain 1 to 190 Hz, half-cosine rolloff 190→200 Hz, 0 above, applied by FFT-domain band-limited resampling to 400 Hz ([S1-impl] uses MATLAB `resample`'s default polyphase FIR in the same role) | gap filled per [S1-impl] role; documented in `cochlea.rs` |
| Statistic window | weighted time-average, `Σ_t w(t) = 1`; w rises 0→1 over the first 1 s and falls 1→0 over the last 1 s (half cycle of a raised cosine), flat between; all filtering FFT-domain with circular boundary conditions; segments typically 7 s | p.937, txt L1572–1587 ("Boundary Handling"); [S1-impl] `set_measurement_window.m` + `make_windows_rcos_flat_no_ends.m` (windowing option 2, `win_steepness=.5`) |
| Short-signal rule (extension) | taper length `min(1 s, duration/2)`; window = rising taper × falling taper; [S1] only specifies the ≥2 s case | gap; documented in `cochlea.rs` |
| Level normalization | **none** (the 2011 pipeline normalized RMS to 0.01 before measurement — a synthesis-stage choice, [S1-impl] `format_orig_sound.m`; this verifier measures the buffer as presented, so only M1 is level-dependent) | deviation, documented in `cochlea.rs` |

### C. Modulation banks

Marginal bank (modulation power), 20 filters:

| Parameter | Value | Citation |
|---|---|---|
| Filter count | 20 | p.937, txt L1551; p.927, txt L114; [S1-impl] params `N_mod_channels=20` |
| Center frequencies | equally spaced on a log scale, 0.5–200 Hz inclusive: `f_n = 0.5·400^((n−1)/19)` | p.937, txt L1550–1551; [S1-impl] `make_constQ_cos_filters.m` `Cfs = 2.^(log2(low):step:log2(hi))`; anchor check: Figure 3A axis labels 1.8/8.5/41.3/200 Hz (p.929, txt L279) = computed n=5/10/15/20 → 1.765/8.541/41.331/200 Hz |
| Response shape | half-cosine on the **linear** frequency axis (the "log scale" attaches to the center spacing, not the shape) | p.937, txt L1549–1551; [S1-impl] `make_constQ_cos_filters.m` ("easy-to-implement version: filters are symmetric on linear scale"); discriminated by the 38.4% overlap cross-check below |
| Q | 2 for 3 dB bandwidths: full support [f_c/2, 3f_c/2], 3 dB passband [0.75·f_c, 1.25·f_c] | p.937, txt L1551; [S1-impl] `make_constQ_cos_filters.m` (`bw=Cf/Q; l=Cf−bw; h=Cf+bw`) |
| Nyquist filter | the 200 Hz filter is only the lower half of its half-cosine (support [100, 200] Hz; the 400 Hz envelope grid cannot represent the upper half) | p.937, txt L1561–1562; falls out of the grid in [S1-impl] |
| Overlap cross-check | the linearly-spaced variant bank (fixed BW 17.04 Hz, spacing 10.5 Hz) is stated to reproduce "the same degree of overlap between the passbands (defined by the 3dB-down points) of adjacent filters (38.4%) as was present in the constant Q filterbank (averaging the overlap on the low and high end of a filter)". With the frozen spec: linear-bank overlap (17.04−10.5)/17.04 = 38.38%; constant-Q low-side 32.4% / high-side 44.4% (linear Hz), average 38.38% ✓. A log-symmetric half-cosine with the same 3 dB Q would give 36.4% ✗ | Suppl. p.15, txt L2735–2745 |
| Bank gain normalization | none ([S1-impl] divides the whole bank by one scalar so the summed squared response averages 1 — a synthesis-stage nicety not in the paper; not adopted) | [S1-impl] `make_constQ_cos_filters.m` last lines; deviation recorded §G |

C1/C2 bank (modulation correlations), 7 filters:

| Parameter | Value | Citation |
|---|---|---|
| Filter count | 7 | p.937, txt L1569–1570 ("yielding seven filters") |
| Center frequencies | octave steps 1.5625, 3.125, 6.25, 12.5, 25, 50, 100 Hz | p.937, txt L1569; [S1-impl] `make_octave_cos_filters2.m` (cutoffs hi_lim/2^j > low_mod_f_C12=1 Hz → centers 1.5625…100) |
| Response shape | half-cosine on the log-frequency axis | p.937, txt L1566–1568 ("half-cosines on a log-scale"); [S1-impl] `make_octave_cos_filters2.m` (cosine of log2 f) |
| Bandwidth | full support [f_c/2, 2·f_c] (half-support one octave); 3 dB passband [f_c/√2, f_c·√2]. The printed parenthetical is **Q = √2** (p.937, txt L1566–1568: the layout-preserved extraction shows the radical `pffiffiffi` on its own line between "tuned (Q =" and "2),", and the pypdf re-extraction reads the sentence as "more broadly tuned ( Q = √2 )"), i.e. the 3-dB quality factor itself: Q_3dB = f_c/(f_c√2 − f_c/√2) = √2. "More broadly tuned" is then exact and needs no further reading: the C1/C2 bank's 3-dB Q (√2) is lower than the marginal bank's (2), so its passbands are wider. The [S1-impl] construction (`l = c/2; h = 2c`, half-cosine of log2 f) has 3 dB points at c·2^(±1/2) = [c/√2, c√2], i.e. Q_3dB = √2 — paper and reference implementation agree directly. Two independent cross-checks confirm the [c/2, 2c] support: (1) the linearly-spaced C1 replacement bank (centers 3.1–167.2 Hz, step 32.8 Hz, BW 32.8 Hz — 3 dB passbands exactly touching, 0% overlap) "had comparable overlap to the octave-spaced modulation filter bank", and [f_c/√2, f_c·√2] bands tile with exactly 0% overlap ✓; (2) C2's frequency doubling maps the lower band's support [f_c/2, 2f_c] exactly onto the upper band's support [f_c, 4f_c] ✓ (p.938, txt L1626–1628) | p.937, txt L1566–1569; Suppl. p.15, txt L2754–2759; [S1-impl] `make_octave_cos_filters2.m` (`l=c/2; h=2c`) |
| DC response | 0 for every filter (lowest support starts at 0.78125 Hz); bands are exactly zero-mean under circular filtering ("the filters fn are zero-mean", p.938 txt L1615–1616) | p.938, txt L1615–1616 |

### D. Statistics (all computed on compressed 400 Hz envelopes, weighted by w(t), Σ_t w(t) = 1)

| Statistic | Definition | Index set | Count | Citation |
|---|---|---|---|---|
| Cochlear marginals M1–M4 | M1 = μ_k = Σw·s_k; M2 = Σw(s_k−μ_k)²/μ_k² ("the variance was normalized by the squared mean"); M3 = Σw(s_k−μ_k)³/σ_k³; M4 = Σw(s_k−μ_k)⁴/σ_k⁴, σ_k² = Σw(s_k−μ_k)²; raw (non-excess) kurtosis | k ∈ [1,32] | 128 | p.937, txt L1594–1605 |
| C (cochlear cross-band correlation) | weighted Pearson correlation of compressed envelopes: `C_jk = Σw(s_j−μ_j)(s_k−μ_k)/(σ_jσ_k)`; equivalently expressible via squared sums/differences | j,k ∈ [1,32], (k−j) ∈ {1,2,3,5,8,11,16,21} ("a subset of eight of its neighbors") | 31+30+29+27+24+21+16+11 = 189 | p.937, txt L1554–1559, L1573–1587 |
| Modulation power | `M_kn = Σw·b_kn²/σ_k²` — variance of marginal-bank band n normalized by the cochlear envelope variance σ_k² | k ∈ [1,32], n ∈ [1,20] | 640 | p.937, txt L1589–1603 |
| C1 (cross-frequency correlation within a modulation band) | `C1_jk,n = Σw·b̃_j,n·b̃_k,n/(σ_j,n·σ_k,n)` with `σ_j,n = sqrt(Σw·b̃_j,n²)`; bands b̃ from the 7-filter octave bank | j,k ∈ [1,32], (k−j) ∈ {1,2} ("two nearest neighbors along the cochlear axis"), n ∈ [2,7] | 6×(31+30) = 366 | p.938, txt L1629–1647 |
| C2 (across-modulation-band complex correlation) | analytic bands `a_k,n = b̃_k,n + i·H[b̃_k,n]`; `d_k,n = a_k,n²/|a_k,n|` — squaring doubles the phase and squares the magnitude, dividing by the magnitude once **restores the original magnitude** ("dividing by the magnitude to preserve the frequency doubling but retain the original magnitude"; "the magnitude is preserved but that the new signal oscillates at twice the rate"); factor 2 because complex exponentiation is only defined for integer powers; `C2_k,mn = Σw·conj(d_k,m)·a_k,n/(σ_k,m·σ_k,n)` with σ as defined for C1 (weighted std of the real band); complex-valued, real and imaginary parts measured and imposed independently. Bound: by Cauchy–Schwarz with \|d\|=\|a_m\| against real-band σ, \|C2\| ≤ 2, with equality iff an octave pair is perfectly phase-locked everywhere (a property of the printed formula, not a bug) | k ∈ [1,32], m ∈ [1,6], n = m+1 | 192 | p.938, txt L1615–1635, L1676–1703; Suppl. pp.9–11, txt L2416–2451 (Fig. S6), esp. L2418–2419 and L2424–2426 for d = a²/\|a\|; conjugation order follows both texts (`d*·a`); [S1-impl] `stat_mod_C2_win.m` computes `c = a.^2./abs(a)` ✓ |
| AC | **not among the 1515 statistics of the 2011 model** (the total enumeration lists exactly five families and sums to 1515 with no autocorrelation term; [S1-impl] measures an `env_ac` diagnostic but `synthesis_parameters_2011_Neuron_paper.m` sets `P.constraint_set.env_ac = 0` — measured, never imposed). Implemented and labeled strictly as `AC (Qinari extension)`: circular weighted autocorrelation of the compressed envelope within a cochlear channel at lags 25/50/100/250/500/1000 ms, normalized by the geometric mean of the weighted energies of the two lag-aligned copies — the symmetric form of the C/C1 normalizers, so weighted Cauchy–Schwarz bounds \|AC\| ≤ 1 exactly. (A one-sided variance normalizer was rejected: the measurement window is not shift-invariant, so it is unbounded — a truncated odd-length test signal drove it to 1.99 on boundary-leakage channels.) | — | 32×6 (extension) | absence: p.938, txt L1642–1645 |

Total: 128 + 189 + 640 + 366 + 192 = **1515** ✓ (p.938, txt L1642–1645:
"In total, there are 128 cochlear marginal statistics, 189 cochlear
cross-correlations, 640 modulation band variances, 366 C1 correlations,
and 192 C2 correlations, for a total of 1515 statistics.")

### E. The 7-vs-6 band reconciliation (documented conflict, now resolved)

The v1 spec said "C1 from 7 octave filters 1.5625–100 Hz"; the external
review said "C1 on six modulation bands 3.125–100 Hz". Both were right,
describing different layers (PLAN-0.2.0 §2.3 item 3):

- The C1/C2 **bank** has **seven** filters, 1.5625–100 Hz (p.937, txt
  L1563–1570). The 1.5625 Hz filter exists because C2 needs the full octave
  chain (m ∈ [1,6] pairs (1.5625, 3.125) … (50, 100)).
- The C1 **statistic** is imposed on **six** of those bands, n ∈ [2,7] =
  3.125–100 Hz, excluding 1.5625 Hz (p.938, txt L1633 `n ∈ [2,7]`, txt
  L1646–1647 "for six modulation bands spanning 3–100 Hz"; Figure 3C band
  labels 3.13/6.25/12.5/25/50/100 Hz, p.929, txt L284/L296/L310).
- [S1-impl] confirmation: `measure_texture_stats.m` sets
  `mod_C1_filts = mod_C2_filts(:,2:end)` — the C1 filters are the octave
  bank minus its first (1.5625 Hz) filter.
- Count check: 6 bands × (31 offset-1 pairs + 30 offset-2 pairs) = 366 ✓.

### F. Naming rules (binding, PLAN-0.2.0 §7.5)

- **C** = cochlear cross-band envelope correlation (the statistic the 0.1.5
  code mislabeled "C1": Pearson correlation of dB envelopes between STFT
  bands).
- **C1** = cross-frequency correlation within the same modulation band.
- **C2** = across-modulation-band complex correlation within one cochlear
  channel.
- **AC** = Qinari extension; never described as part of the 2011 model.

### G. Recorded divergences: [S1] printed formulas vs [S1-impl] v1.7 code

The toolbox is a Dec-2012 revision of the 2011 code and deviates from the
paper's printed formulas in five places. The paper wins in all five; the
divergences are recorded here so a future cross-check against MATLAB runs
of the toolbox knows what to expect.

| # | [S1] printed formula (frozen) | [S1-impl] v1.7 | Note |
|---|---|---|---|
| 1 | `d_k,n = a²_k,n/|a_k,n|` | same (`c = a.^2./abs(a)`) | agreement — this corrects the pre-derivation's `a²/\|a\|²`, which would destroy the magnitude ("retain the original magnitude", Suppl. txt L2419) |
| 2 | M2 = variance/mean² | `stat_central_moment_win` returns std/mean ("**Note - the 2nd moment is not the variance…**") | paper's formula + "variance was normalized by the squared mean" (txt L1601–1605) |
| 3 | C and C1 weighted throughout (weighted μ, σ) | `stat_corr_filt_win_full` removes the **unweighted** mean and normalizes by **unweighted** stds (numerator weighted) | paper's σ definition for C1 is explicitly weighted (txt L1637–1641); C's formula likewise (txt L1554–1559) |
| 4 | C2 = Σw·conj(d)·a/(σ_mσ_n), σ = real-band std | `stat_mod_C2_win` correlates Re(d) with Re(a) and Im(a) separately, normalizing by stds of the **real parts** (weighted) | paper + supplement both print the full complex product ("the product of the two complex signals: d_k,m(t)* a_k,n(t), which multiplies the amplitudes and subtracts the phases", Suppl. txt L2439); toolbox drops the Im(d) cross-terms; for a perfectly phase-locked octave pair the paper formula gives 2·e^{iΔφ}, the toolbox e^{iΔφ} |
| 5 | window ramps exactly 1 s at each end | `make_windows_rcos_flat_no_ends` sets ramp length to `floor(L/(N+1))` samples — 0.875 s for a 7 s, 400 Hz envelope (2800/8) | paper states the 1 s interval explicitly ("over the 1 s intervals at the beginning and end of the signal"), so the paper wins; the toolbox value is an off-by-a-taper implementation rounding |

### H. Freeze-session re-verification record (2026-11-08)

Before freezing, every number in §§A–D was re-derived from the primary
sources by the implementing session, independently of the initial
derivation:

- Both SHA-256 hashes recomputed and matched.
- Cochlear center frequencies recomputed from the frozen cutoff
  construction: 51.69 / 596.32 / 2198.28 / 2507.39 / 8844.46 Hz round to
  all five Figure 2 axis labels (52/596/2200/2507/8844); the alternative
  center-anchored construction (52–8844 over 29 intervals) reproduces only
  3 of 5 labels (597, 2508) and stays rejected. (Both constructions give
  the same ERB spacing S = 1.1106; they differ only in the anchor values.)
- Marginal-bank centers recomputed: 1.765 / 8.541 / 41.331 / 200 Hz at
  n = 5/10/15/20 match the Figure 3A labels (1.8/8.5/41.3/200).
- The 38.4% overlap statement re-derived: constant-Q bank low-side 32.4%,
  high-side 44.4%, average 38.4% (Suppl. p.15, txt L2743–2745); the
  linearly-spaced bank gives (17.04−10.5)/17.04 = 38.4% ✓; a log-symmetric
  half-cosine with the same 3-dB Q = 2 has 3 dB points at [0.7808·f_c,
  1.2808·f_c] and averages 36.4% ✗ — the linear-symmetric shape is
  confirmed.
- The C1/C2 bank quality factor re-read with a second PDF extractor and
  corrected from the initial derivation's "(Q = 2)" to the printed
  **(Q = √2)** (§C) — the only substantive correction of the freeze pass;
  it *strengthens* the frozen construction (paper and [S1-impl] now agree
  directly instead of needing a reinterpretation of the parenthetical).
- Statistic total re-added: 128 + 189 + 640 + 366 + 192 = 1515 ✓
  (p.938, txt L1642–1645).

Spec frozen at this point; implementation follows §§A–D and F only.

### I. P0-2 exit-criteria status (PLAN-0.2.0 §7.5)

- Synthetic analytic-value tests, C/C1/C2 within tolerance 0.02: DONE —
  `modulation.rs` tests `c_statistic_matches_analytic_values_on_synthetic_signals`,
  `c1_statistic_matches_analytic_values_on_synthetic_signals`,
  `c2_statistic_matches_analytic_values_on_synthetic_signals`.
- Unit tests of the filterbank responses (centers, spacing, bandwidths)
  against the frozen spec: DONE — `cochlea.rs` tests
  `cutoffs_and_centers_match_the_frozen_spec`,
  `bandpass_response_is_half_cosine_with_erb_3db_bandwidth`,
  `filterbank_tiles_the_spectrum_perfectly`,
  `low_and_high_pass_are_flat_at_the_extremes`.
- Corpus-stability criterion (C, C1, C2 with CV < 0.3 across ≥3 corpus
  recordings per category): **Not verified: blocked on corpus (P0-4)** —
  the texture corpus does not exist yet (it is P0-4's deliverable); this
  criterion will be executed by the corpus/evaluation tasks, not faked
  here.

//! The procedural composer: section layout, harmony pools, motif
//! development and per-style arrangement.
//!
//! Everything is deterministic given the seed: the layout, the key, the
//! chord progression, the motif and every humanization draw come from one
//! [`Qrng`] forked per layer, so a request renders sample-identical audio
//! on every machine.

use crate::{Style, TrackRequest};
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::env::{Adsr, Percussive};
use qinari_dsp::filter::{Biquad, BiquadKind, Svf};
use qinari_dsp::fx::{Delay, Freeverb};
use qinari_dsp::karplus;
use qinari_dsp::lfo::{Lfo, LfoShape};
use qinari_dsp::noise::{NoiseGen, NoiseKind};
use qinari_dsp::osc::{fast_sine, midi_to_freq, Osc, OscKind};
use qinari_dsp::pan;
use qinari_dsp::rng::Qrng;

/// Time signature numerator; the composer works in 4/4 throughout.
const BEATS_PER_BAR: f32 = 4.0;

/// Seconds in one bar at `bpm`.
fn bar_secs(bpm: f32) -> f32 {
    BEATS_PER_BAR * 60.0 / bpm
}

// ---------------------------------------------------------------------------
// Section layout
// ---------------------------------------------------------------------------

/// Musical role of one section of the arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionKind {
    /// Sparse opening: pads (and the lofi vinyl bed) only.
    Intro,
    /// Full arrangement: drums, bass, motif and pads.
    Verse,
    /// Breakdown: pads and a mirrored motif, no drums or bass.
    Bridge,
    /// Closing section: pads and a thinned motif, faded out.
    Outro,
}

/// One planned section: its role and its bar range.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Section {
    /// Role of the section.
    pub(crate) kind: SectionKind,
    /// First bar of the section, on the track-wide bar grid.
    pub(crate) start_bar: usize,
    /// Length of the section in bars; always at least one.
    pub(crate) bars: usize,
}

impl Section {
    /// Bar just past the end of the section.
    pub(crate) fn end_bar(&self) -> usize {
        self.start_bar + self.bars
    }
}

/// Plans the section layout covering at least `requested_secs`.
///
/// The form always opens with an intro and closes with an outro. Small
/// requests get intro / verse / outro, medium ones add a bridge, long
/// ones extend with alternating verse / bridge pairs. Section lengths
/// scale with the request; the total always reaches the target bar count
/// (the last verse absorbs rounding remainder), so the rendered length is
/// at least the requested duration and at most a few bars past it.
pub(crate) fn plan_sections(requested_secs: f32, bpm: f32) -> Vec<Section> {
    let bar = bar_secs(bpm);
    let target_bars = (requested_secs / bar).ceil().max(3.0) as usize;

    let middle: &[(SectionKind, usize)] = if target_bars <= 6 {
        &[(SectionKind::Verse, 2)]
    } else if target_bars <= 24 {
        &[
            (SectionKind::Verse, 2),
            (SectionKind::Bridge, 1),
            (SectionKind::Verse, 2),
        ]
    } else {
        &[
            (SectionKind::Verse, 2),
            (SectionKind::Verse, 2),
            (SectionKind::Bridge, 1),
            (SectionKind::Verse, 2),
        ]
    };
    let intro_weight = 1usize;
    let outro_weight = 1usize;
    let total_weight: usize =
        intro_weight + outro_weight + middle.iter().map(|&(_, w)| w).sum::<usize>();
    let per_weight = target_bars as f32 / total_weight as f32;

    let scaled = |weight: usize| ((weight as f32) * per_weight).round().max(1.0) as usize;
    let mut sections: Vec<(SectionKind, usize)> = vec![(SectionKind::Intro, scaled(intro_weight))];
    for &(kind, weight) in middle {
        sections.push((kind, scaled(weight)));
    }

    let verse_bars = sections
        .iter()
        .filter(|&&(kind, _)| kind == SectionKind::Verse)
        .map(|&(_, bars)| bars)
        .max()
        .unwrap_or(1);
    let bridge_bars = sections
        .iter()
        .find(|&&(kind, _)| kind == SectionKind::Bridge)
        .map_or(1, |&(_, bars)| bars);

    let bar_total =
        |sections: &[(SectionKind, usize)]| sections.iter().map(|&(_, bars)| bars).sum::<usize>();
    let mut total = bar_total(&sections);
    while total + verse_bars + bridge_bars <= target_bars {
        sections.push((SectionKind::Verse, verse_bars));
        sections.push((SectionKind::Bridge, bridge_bars));
        total += verse_bars + bridge_bars;
    }
    sections.push((SectionKind::Outro, scaled(outro_weight)));
    total = bar_total(&sections);

    if total < target_bars {
        let missing = target_bars - total;
        if let Some(slot) = sections
            .iter_mut()
            .rev()
            .find(|(kind, _)| *kind == SectionKind::Verse)
        {
            slot.1 += missing;
        }
    }

    let mut start_bar = 0;
    sections
        .into_iter()
        .map(|(kind, bars)| {
            let section = Section {
                kind,
                start_bar,
                bars,
            };
            start_bar += bars;
            section
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Harmony
// ---------------------------------------------------------------------------

/// A chord: root offset from the tonic in semitones plus stacked
/// intervals.
struct Chord {
    root: i32,
    intervals: &'static [i32],
}

const fn chord(root: i32, intervals: &'static [i32]) -> Chord {
    Chord { root, intervals }
}

const MAJ: &[i32] = &[0, 4, 7];
const MIN: &[i32] = &[0, 3, 7];
const MAJ7: &[i32] = &[0, 4, 7, 11];
const MIN7: &[i32] = &[0, 3, 7, 10];
const DOM7: &[i32] = &[0, 4, 7, 10];
const SUS2: &[i32] = &[0, 2, 7];
const ADD9: &[i32] = &[0, 4, 7, 14];

const MAJOR_SCALE: &[i32] = &[0, 2, 4, 5, 7, 9, 11];
const MINOR_SCALE: &[i32] = &[0, 2, 3, 5, 7, 8, 10];

/// Per-style harmony: the motif scale and the pool of progressions.
struct Harmony {
    scale: &'static [i32],
    pool: &'static [&'static [Chord]],
}

const LOFI_A: &[Chord] = &[
    chord(2, MIN7),
    chord(7, DOM7),
    chord(0, MAJ7),
    chord(9, MIN7),
];
const LOFI_B: &[Chord] = &[
    chord(0, MAJ7),
    chord(5, MAJ7),
    chord(9, MIN7),
    chord(7, DOM7),
];
const LOFI_C: &[Chord] = &[
    chord(9, MIN7),
    chord(2, MIN7),
    chord(7, DOM7),
    chord(0, MAJ7),
];
static LOFI_HARMONY: Harmony = Harmony {
    scale: MAJOR_SCALE,
    pool: &[LOFI_A, LOFI_B, LOFI_C],
};

const AMBIENT_A: &[Chord] = &[chord(0, ADD9), chord(5, MAJ7)];
const AMBIENT_B: &[Chord] = &[
    chord(0, MAJ7),
    chord(7, SUS2),
    chord(9, MIN7),
    chord(5, MAJ7),
];
const AMBIENT_C: &[Chord] = &[
    chord(9, MIN7),
    chord(5, MAJ7),
    chord(0, ADD9),
    chord(7, SUS2),
];
static AMBIENT_HARMONY: Harmony = Harmony {
    scale: MAJOR_SCALE,
    pool: &[AMBIENT_A, AMBIENT_B, AMBIENT_C],
};

const SYNTH_A: &[Chord] = &[chord(0, MIN), chord(8, MAJ), chord(3, MAJ), chord(10, MAJ)];
const SYNTH_B: &[Chord] = &[chord(0, MIN), chord(5, MIN), chord(8, MAJ), chord(7, MIN)];
const SYNTH_C: &[Chord] = &[chord(0, MIN7), chord(10, MAJ), chord(8, MAJ), chord(3, MAJ)];
static SYNTH_HARMONY: Harmony = Harmony {
    scale: MINOR_SCALE,
    pool: &[SYNTH_A, SYNTH_B, SYNTH_C],
};

const CINE_A: &[Chord] = &[chord(0, MIN), chord(5, MIN), chord(0, MIN), chord(8, MAJ)];
const CINE_B: &[Chord] = &[chord(0, MIN), chord(8, MAJ), chord(5, MIN), chord(0, MIN)];
const CINE_C: &[Chord] = &[chord(0, MIN), chord(3, MAJ), chord(10, MAJ), chord(0, MIN)];
static CINE_HARMONY: Harmony = Harmony {
    scale: MINOR_SCALE,
    pool: &[CINE_A, CINE_B, CINE_C],
};

const CHIP_A: &[Chord] = &[chord(0, MAJ), chord(7, MAJ), chord(9, MIN), chord(5, MAJ)];
const CHIP_B: &[Chord] = &[chord(0, MAJ), chord(9, MIN), chord(5, MAJ), chord(7, MAJ)];
const CHIP_C: &[Chord] = &[chord(9, MIN), chord(5, MAJ), chord(0, MAJ), chord(7, MAJ)];
static CHIP_HARMONY: Harmony = Harmony {
    scale: MAJOR_SCALE,
    pool: &[CHIP_A, CHIP_B, CHIP_C],
};

fn harmony_for(id: &str) -> &'static Harmony {
    match id {
        "lofi" => &LOFI_HARMONY,
        "ambient" => &AMBIENT_HARMONY,
        "synthwave" => &SYNTH_HARMONY,
        "cinematic" => &CINE_HARMONY,
        "chiptune" => &CHIP_HARMONY,
        _ => unreachable!("style ids are validated against STYLES before compose"),
    }
}

// ---------------------------------------------------------------------------
// Motif
// ---------------------------------------------------------------------------

/// One note of the melodic motif: position and length in beats plus a
/// scale degree (0 = tonic, 7 = tonic one octave up).
#[derive(Debug, Clone, Copy)]
struct MotifNote {
    beat: f32,
    len: f32,
    degree: i32,
    gain: f32,
}

/// Rhythm cells worth exactly two beats; a motif phrase is two of them.
const MOTIF_CELLS: [&[f32]; 8] = [
    &[2.0],
    &[1.0, 1.0],
    &[1.0, 0.5, 0.5],
    &[0.5, 0.5, 1.0],
    &[0.5, 0.5, 0.5, 0.5],
    &[1.5, 0.5],
    &[0.75, 0.75, 0.5],
    &[1.25, 0.25, 0.5],
];

/// Generates a four-beat motif phrase: a rhythm cell pair under a random
/// walk over the scale degrees.
fn generate_motif(rng: &mut Qrng) -> Vec<MotifNote> {
    let mut notes = Vec::new();
    let mut beat = 0.0f32;
    let mut degree = [0, 2, 4, 7][rng.next_int_range(0, 3) as usize];
    for _ in 0..2 {
        let cell = MOTIF_CELLS[rng.next_int_range(0, MOTIF_CELLS.len() as u64 - 1) as usize];
        for (index, &len) in cell.iter().enumerate() {
            if index > 0 {
                let step = [-2, -1, -1, 0, 0, 0, 1, 1, 2][rng.next_int_range(0, 8) as usize];
                degree = (degree + step).clamp(0, 9);
            }
            notes.push(MotifNote {
                beat,
                len,
                degree,
                gain: 1.0,
            });
            beat += len;
        }
    }
    notes
}

/// Splits every note of at least one beat into two halves (density
/// development for the third and later verses).
fn densify(notes: &[MotifNote]) -> Vec<MotifNote> {
    let mut out = Vec::new();
    for note in notes {
        if note.len >= 1.0 {
            out.push(MotifNote {
                beat: note.beat,
                len: note.len / 2.0,
                degree: note.degree,
                gain: note.gain,
            });
            out.push(MotifNote {
                beat: note.beat + note.len / 2.0,
                len: note.len / 2.0,
                degree: note.degree,
                gain: note.gain,
            });
        } else {
            out.push(*note);
        }
    }
    out
}

/// Mirrors the contour around the first degree (bridge variation).
fn mirror(notes: &[MotifNote]) -> Vec<MotifNote> {
    let axis = notes.first().map_or(0, |note| note.degree);
    notes
        .iter()
        .map(|note| MotifNote {
            beat: note.beat,
            len: note.len,
            degree: (2 * axis - note.degree).clamp(0, 9),
            gain: note.gain * 0.95,
        })
        .collect()
}

/// Keeps every second note, stretched (outro thins the melody out).
fn thin(notes: &[MotifNote]) -> Vec<MotifNote> {
    notes
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, note)| MotifNote {
            beat: note.beat,
            len: note.len * 1.75,
            degree: note.degree,
            gain: note.gain * 0.8,
        })
        .collect()
}

/// The phrase as played in one section kind.
fn developed_phrase(motif: &[MotifNote], kind: SectionKind, verse_index: usize) -> Vec<MotifNote> {
    match kind {
        SectionKind::Intro => Vec::new(),
        SectionKind::Verse => {
            if verse_index >= 2 {
                densify(motif)
            } else {
                motif.to_vec()
            }
        }
        SectionKind::Bridge => mirror(motif),
        SectionKind::Outro => thin(motif),
    }
}

/// Maps a scale degree to a MIDI note in the motif register.
fn degree_midi(key_root: i32, scale: &[i32], degree: i32) -> i32 {
    let octave = degree.div_euclid(7);
    let index = degree.rem_euclid(7) as usize;
    key_root + 12 + 12 * octave + scale[index]
}

// ---------------------------------------------------------------------------
// Voice specs and per-style profiles
// ---------------------------------------------------------------------------

/// Parameters of a one-shot oscillator note with an ADSR envelope and an
/// optional lowpass.
struct NoteSpec {
    kind: OscKind,
    /// Detune fraction; a second oscillator sounds at `freq * (1+detune)`
    /// when positive.
    detune: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    /// Lowpass corner in hertz; 0 leaves the note unfiltered.
    cutoff: f32,
    /// Duty cycle for [`OscKind::Pulse`].
    pulse_width: f32,
}

/// Parameters of the legato lead renderer.
struct LeadSpec {
    kind: OscKind,
    attack: f32,
    release: f32,
    /// Portamento time between notes; 0 jumps.
    glide_secs: f32,
    /// Vibrato rate; 0 disables.
    vibrato_hz: f32,
    /// Vibrato depth as a frequency fraction.
    vibrato_depth: f32,
    /// Gain of an extra oscillator one octave down; 0 disables.
    sub_gain: f32,
    /// Lowpass corner in hertz; 0 leaves the lead unfiltered.
    cutoff: f32,
}

const LOFI_PAD: NoteSpec = NoteSpec {
    kind: OscKind::Saw,
    detune: 0.001,
    attack: 0.06,
    decay: 0.2,
    sustain: 0.8,
    release: 0.4,
    cutoff: 1100.0,
    pulse_width: 0.5,
};
const SYNTH_PAD: NoteSpec = NoteSpec {
    kind: OscKind::Saw,
    detune: 0.004,
    attack: 0.03,
    decay: 0.1,
    sustain: 0.9,
    release: 0.2,
    cutoff: 3800.0,
    pulse_width: 0.5,
};
const CINE_PAD: NoteSpec = NoteSpec {
    kind: OscKind::Saw,
    detune: 0.002,
    attack: 0.5,
    decay: 0.3,
    sustain: 0.85,
    release: 1.0,
    cutoff: 2400.0,
    pulse_width: 0.5,
};
const CHIP_PAD: NoteSpec = NoteSpec {
    kind: OscKind::Pulse,
    detune: 0.0,
    attack: 0.004,
    decay: 0.05,
    sustain: 0.9,
    release: 0.06,
    cutoff: 0.0,
    pulse_width: 0.25,
};
const LOFI_BASS: NoteSpec = NoteSpec {
    kind: OscKind::Triangle,
    detune: 0.0,
    attack: 0.008,
    decay: 0.25,
    sustain: 0.55,
    release: 0.2,
    cutoff: 700.0,
    pulse_width: 0.5,
};
const SYNTH_BASS: NoteSpec = NoteSpec {
    kind: OscKind::Saw,
    detune: 0.0,
    attack: 0.003,
    decay: 0.05,
    sustain: 0.75,
    release: 0.04,
    cutoff: 750.0,
    pulse_width: 0.5,
};
const CHIP_BASS: NoteSpec = NoteSpec {
    kind: OscKind::Square,
    detune: 0.0,
    attack: 0.002,
    decay: 0.04,
    sustain: 0.8,
    release: 0.03,
    cutoff: 0.0,
    pulse_width: 0.5,
};
const SUB_BASS: NoteSpec = NoteSpec {
    kind: OscKind::Sine,
    detune: 0.0,
    attack: 0.05,
    decay: 0.4,
    sustain: 0.9,
    release: 0.4,
    cutoff: 0.0,
    pulse_width: 0.5,
};
const SWELL_BASS: NoteSpec = NoteSpec {
    kind: OscKind::Sine,
    detune: 0.0,
    attack: 1.2,
    decay: 0.3,
    sustain: 0.8,
    release: 1.8,
    cutoff: 0.0,
    pulse_width: 0.5,
};
const CHIP_LEAD: NoteSpec = NoteSpec {
    kind: OscKind::Square,
    detune: 0.0,
    attack: 0.001,
    decay: 0.06,
    sustain: 0.85,
    release: 0.03,
    cutoff: 0.0,
    pulse_width: 0.5,
};
const CHIP_ARP: NoteSpec = NoteSpec {
    kind: OscKind::Pulse,
    detune: 0.0,
    attack: 0.001,
    decay: 0.03,
    sustain: 0.8,
    release: 0.02,
    cutoff: 0.0,
    pulse_width: 0.25,
};
const SAW_LEAD: LeadSpec = LeadSpec {
    kind: OscKind::Saw,
    attack: 0.012,
    release: 0.09,
    glide_secs: 0.0,
    vibrato_hz: 5.5,
    vibrato_depth: 0.004,
    sub_gain: 0.0,
    cutoff: 5200.0,
};
const GLIDE_LEAD: LeadSpec = LeadSpec {
    kind: OscKind::Triangle,
    attack: 0.22,
    release: 0.4,
    glide_secs: 0.08,
    vibrato_hz: 0.0,
    vibrato_depth: 0.0,
    sub_gain: 0.4,
    cutoff: 0.0,
};

/// Which drum kit a style uses; `None` (ambient) plays no drums.
enum Kit {
    None,
    Lofi,
    Synthwave,
    Cinematic,
    Chiptune,
}

/// Which lead voice carries the motif.
enum LeadVoice {
    /// Karplus-Strong plucked keys (lofi).
    KarplusKeys,
    /// Saw lead with vibrato (synthwave).
    SawVibrato,
    /// Legato triangle lead with portamento and a sub octave (cinematic).
    LegatoGlide,
    /// Retriggered square lead (chiptune).
    SquareChip,
    /// Long-decaying sine bells (ambient).
    SineBell,
}

/// Which bass line a style plays.
enum BassVoice {
    /// Two root notes per bar (lofi).
    HalfNote,
    /// Driving eighth notes (synthwave).
    EighthsSaw,
    /// Sustained sub sine per bar (cinematic).
    SubSustained,
    /// Eighth-note square pulse (chiptune).
    EighthsSquare,
    /// Slow sine swell per chord (ambient).
    SineSwell,
}

/// Which pad renderer a style uses.
enum PadVoice {
    /// Filtered oscillator stack from the pad spec.
    Standard,
    /// Detuned triangles behind a slowly sweeping filter (ambient).
    Ambient,
}

/// Whole-mix post processing.
enum PostFx {
    None,
    /// Freeverb wash.
    Reverb {
        mix: f32,
    },
    /// Feedback echo, a dotted-eighth apart.
    Echo {
        beats: f32,
        feedback: f32,
        mix: f32,
    },
}

/// How the layers of one style fit together.
struct StyleProfile {
    /// Swing amount applied to off-beat eighths (0 = straight).
    swing: f32,
    kit: Kit,
    lead: LeadVoice,
    pad_voice: PadVoice,
    pad: NoteSpec,
    pad_gain: f32,
    bass: BassVoice,
    /// Bars each chord lasts.
    chord_bars: usize,
    post_fx: PostFx,
}

static LOFI_PROFILE: StyleProfile = StyleProfile {
    swing: 0.3,
    kit: Kit::Lofi,
    lead: LeadVoice::KarplusKeys,
    pad_voice: PadVoice::Standard,
    pad: LOFI_PAD,
    pad_gain: 0.11,
    bass: BassVoice::HalfNote,
    chord_bars: 1,
    post_fx: PostFx::None,
};
static AMBIENT_PROFILE: StyleProfile = StyleProfile {
    swing: 0.0,
    kit: Kit::None,
    lead: LeadVoice::SineBell,
    pad_voice: PadVoice::Ambient,
    pad: NoteSpec {
        kind: OscKind::Triangle,
        detune: 0.003,
        attack: 1.8,
        decay: 0.5,
        sustain: 0.9,
        release: 3.2,
        cutoff: 0.0,
        pulse_width: 0.5,
    },
    pad_gain: 0.3,
    bass: BassVoice::SineSwell,
    chord_bars: 2,
    post_fx: PostFx::Reverb { mix: 0.35 },
};
static SYNTH_PROFILE: StyleProfile = StyleProfile {
    swing: 0.0,
    kit: Kit::Synthwave,
    lead: LeadVoice::SawVibrato,
    pad_voice: PadVoice::Standard,
    pad: SYNTH_PAD,
    pad_gain: 0.13,
    bass: BassVoice::EighthsSaw,
    chord_bars: 1,
    post_fx: PostFx::Echo {
        beats: 0.75,
        feedback: 0.3,
        mix: 0.45,
    },
};
static CINE_PROFILE: StyleProfile = StyleProfile {
    swing: 0.0,
    kit: Kit::Cinematic,
    lead: LeadVoice::LegatoGlide,
    pad_voice: PadVoice::Standard,
    pad: CINE_PAD,
    pad_gain: 0.12,
    bass: BassVoice::SubSustained,
    chord_bars: 1,
    post_fx: PostFx::Reverb { mix: 0.28 },
};
static CHIP_PROFILE: StyleProfile = StyleProfile {
    swing: 0.0,
    kit: Kit::Chiptune,
    lead: LeadVoice::SquareChip,
    pad_voice: PadVoice::Standard,
    pad: CHIP_PAD,
    pad_gain: 0.09,
    bass: BassVoice::EighthsSquare,
    chord_bars: 1,
    post_fx: PostFx::None,
};

/// Octave-jump patterns for the eighth-note basses, one entry per eighth.
const BASS_PATTERNS: [&[i32]; 4] = [
    &[0, 0, 0, 0, 0, 0, 0, 0],
    &[0, 0, 0, 12, 0, 0, 12, 0],
    &[0, 0, 12, 0, 0, 0, 0, 12],
    &[0, 12, 0, 0, 0, 12, 0, 0],
];

/// Stereo spread of the pad voices.
const PAD_PANS: [f32; 4] = [-0.45, 0.0, 0.45, 0.2];

fn profile_for(id: &str) -> &'static StyleProfile {
    match id {
        "lofi" => &LOFI_PROFILE,
        "ambient" => &AMBIENT_PROFILE,
        "synthwave" => &SYNTH_PROFILE,
        "cinematic" => &CINE_PROFILE,
        "chiptune" => &CHIP_PROFILE,
        _ => unreachable!("style ids are validated against STYLES before compose"),
    }
}

// ---------------------------------------------------------------------------
// Track plan
// ---------------------------------------------------------------------------

/// Everything decided before a single sample is rendered.
struct TrackPlan {
    /// Tonic of the key as a MIDI note (C3 .. B3).
    key_root: i32,
    scale: &'static [i32],
    main: &'static [Chord],
    bridge: &'static [Chord],
    motif: Vec<MotifNote>,
    sections: Vec<Section>,
    total_bars: usize,
}

fn build_plan(rng: &mut Qrng, style: &Style, bpm: f32, duration_secs: f32) -> TrackPlan {
    let harmony = harmony_for(style.id);
    let pool = harmony.pool;
    let main_index = rng.next_int_range(0, pool.len() as u64 - 1) as usize;
    let bridge_index = if pool.len() > 1 {
        let index = rng.next_int_range(0, pool.len() as u64 - 1) as usize;
        if index == main_index {
            (index + 1) % pool.len()
        } else {
            index
        }
    } else {
        0
    };
    let key_root = 48 + rng.next_int_range(0, 11) as i32;
    let mut motif_rng = rng.fork();
    let motif = generate_motif(&mut motif_rng);
    let sections = plan_sections(duration_secs, bpm);
    let total_bars = sections.last().map_or(0, Section::end_bar);
    TrackPlan {
        key_root,
        scale: harmony.scale,
        main: pool[main_index],
        bridge: pool[bridge_index],
        motif,
        sections,
        total_bars,
    }
}

// ---------------------------------------------------------------------------
// Mix bus
// ---------------------------------------------------------------------------

/// The channel bus every layer adds into.
struct MixBus {
    data: Vec<Vec<f32>>,
}

impl MixBus {
    fn new(channels: usize, frames: usize) -> Self {
        Self {
            data: vec![vec![0.0; frames]; channels],
        }
    }

    fn frames(&self) -> usize {
        self.data.first().map_or(0, Vec::len)
    }

    /// Adds `samples` at `start`, panned when the bus is stereo. Mono
    /// buses ignore the pan; writes past the end are dropped.
    fn add(&mut self, start: usize, samples: &[f32], gain: f32, pan_pos: f32) {
        let stereo = self.data.len() > 1;
        let (left_gain, right_gain) = pan::pan(1.0, pan_pos);
        for (index, channel) in self.data.iter_mut().enumerate() {
            let channel_gain = if stereo {
                gain * if index == 0 { left_gain } else { right_gain }
            } else {
                gain
            };
            for (offset, &sample) in samples.iter().enumerate() {
                if let Some(slot) = channel.get_mut(start + offset) {
                    *slot += channel_gain * sample;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Voice renderers
// ---------------------------------------------------------------------------

/// Renders one oscillator note with envelope, optional detune partner and
/// optional lowpass; `len_frames` is the gated length and the release
/// rings out past it.
fn osc_note(spec: &NoteSpec, freq: f32, len_frames: usize, sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let release_frames = (spec.release * sr) as usize;
    let total = len_frames + release_frames + 8;
    let mut primary = Osc::new(spec.kind, freq, sr, rng);
    if spec.kind == OscKind::Pulse {
        primary.set_pulse_width(spec.pulse_width);
    }
    let mut secondary = (spec.detune > 0.0).then(|| {
        let mut osc = Osc::new(spec.kind, freq * (1.0 + spec.detune), sr, rng);
        if spec.kind == OscKind::Pulse {
            osc.set_pulse_width(spec.pulse_width);
        }
        osc
    });
    let mut env = Adsr::new(spec.attack, spec.decay, spec.sustain, spec.release, sr);
    env.trigger();
    let mut filter = (spec.cutoff > 0.0)
        .then(|| Biquad::design(BiquadKind::Lowpass, sr, spec.cutoff, 0.707, 0.0));
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        if n == len_frames {
            env.release();
        }
        let mut sample = primary.process();
        if let Some(osc) = secondary.as_mut() {
            sample = sample.midpoint(osc.process());
        }
        let mut value = sample * env.process();
        if let Some(biquad) = filter.as_mut() {
            value = biquad.process(value);
        }
        out.push(value);
        if n >= len_frames && env.is_finished() {
            break;
        }
    }
    out
}

/// One scheduled note for the lead renderers.
struct SchedNote {
    start: usize,
    len: usize,
    freq: f32,
    gain: f32,
}

/// Renders a monophonic legato lead over the whole event list: a single
/// oscillator glides between notes while two alternating envelopes let
/// each note's tail ring under the next one. The returned vector begins
/// at the first event's frame; the caller adds it at that offset.
fn render_legato(spec: &LeadSpec, events: &[SchedNote], sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let Some(first) = events.first() else {
        return Vec::new();
    };
    let origin = first.start;
    let last_end = events.last().map_or(0, |event| event.start + event.len) - origin;
    let total = last_end + (spec.release * sr * 3.0) as usize + 8;

    let glide_coef = if spec.glide_secs > 0.0 {
        1.0 - (-1.0 / (spec.glide_secs * sr)).exp()
    } else {
        1.0
    };
    let mut base_freq = first.freq;
    let mut glide_target = first.freq;
    let mut osc = Osc::new(spec.kind, first.freq, sr, rng);
    let mut sub = (spec.sub_gain > 0.0).then(|| Osc::new(spec.kind, first.freq * 0.5, sr, rng));
    let mut envs = [
        Adsr::new(spec.attack, 0.05, 1.0, spec.release, sr),
        Adsr::new(spec.attack, 0.05, 1.0, spec.release, sr),
    ];
    let mut gains = [1.0f32; 2];
    let mut current = 0usize;
    let mut next_event = 0usize;
    let mut vibrato =
        (spec.vibrato_hz > 0.0).then(|| Lfo::new(LfoShape::Sine, spec.vibrato_hz, sr, rng));
    let mut filter = (spec.cutoff > 0.0)
        .then(|| Biquad::design(BiquadKind::Lowpass, sr, spec.cutoff, 0.707, 0.0));

    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        if next_event < events.len() && events[next_event].start - origin == n {
            envs[current].release();
            current = 1 - current;
            envs[current].trigger();
            gains[current] = events[next_event].gain;
            glide_target = events[next_event].freq;
            next_event += 1;
        }
        base_freq += (glide_target - base_freq) * glide_coef;
        let vibrato_factor = match vibrato.as_mut() {
            Some(lfo) => 1.0 + spec.vibrato_depth * lfo.process(),
            None => 1.0,
        };
        osc.set_freq(base_freq * vibrato_factor);
        let mut sample = osc.process();
        if let Some(sub_osc) = sub.as_mut() {
            sub_osc.set_freq(base_freq * 0.5);
            sample += spec.sub_gain * sub_osc.process();
        }
        let mut value = sample * (gains[0] * envs[0].process() + gains[1] * envs[1].process());
        if let Some(biquad) = filter.as_mut() {
            value = biquad.process(value);
        }
        out.push(value);
        if next_event >= events.len()
            && n >= last_end
            && envs[0].is_finished()
            && envs[1].is_finished()
        {
            break;
        }
    }
    out
}

/// A sine kick with an exponential pitch drop and a short noise click.
fn kick(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (0.4 * sr) as usize;
    let drop = (0.05 * sr) as usize;
    let click = (0.002 * sr) as usize;
    let mut osc = Osc::new(OscKind::Sine, 120.0, sr, rng);
    let mut env = Percussive::from_t60(sr, 0.26);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let progress = (n as f32 / drop as f32).min(1.0);
        osc.set_freq(120.0 - 74.0 * (1.0 - (1.0 - progress) * (1.0 - progress)));
        let mut sample = osc.process() * env.process();
        if n < click {
            sample += (rng.next_f32() * 2.0 - 1.0) * 0.25 * (1.0 - n as f32 / click as f32);
        }
        out.push(sample);
    }
    out
}

/// A snare: highpassed noise body plus a short 185 Hz tone.
fn snare(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (0.3 * sr) as usize;
    let mut noise = NoiseGen::new(NoiseKind::White, rng.fork());
    let mut filter = Biquad::design(BiquadKind::Highpass, sr, 1300.0, 0.8, 0.0);
    let mut env = Percussive::from_t60(sr, 0.15);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let body = 0.35 * fast_sine(185.0 * n as f32 / sr);
        out.push((filter.process(noise.process()) + body) * env.process());
    }
    out
}

/// A closed (`t60` 50 ms) or open (300 ms) hi-hat.
fn hat(sr: f32, rng: &mut Qrng, open: bool) -> Vec<f32> {
    let t60 = if open { 0.3 } else { 0.05 };
    let len = ((t60 * 1.6) * sr) as usize;
    let mut noise = NoiseGen::new(NoiseKind::White, rng.fork());
    let mut filter = Biquad::design(BiquadKind::Highpass, sr, 6800.0, 0.7, 0.0);
    let mut env = Percussive::from_t60(sr, t60);
    env.trigger();
    (0..len)
        .map(|_| filter.process(noise.process()) * env.process())
        .collect()
}

/// A chiptune kick: square wave dropping from 150 Hz to 50 Hz.
fn chip_kick(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (0.25 * sr) as usize;
    let drop = (0.03 * sr) as usize;
    let mut osc = Osc::new(OscKind::Square, 150.0, sr, rng);
    let mut env = Percussive::from_t60(sr, 0.16);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let progress = (n as f32 / drop as f32).min(1.0);
        osc.set_freq(150.0 - 100.0 * (1.0 - (1.0 - progress) * (1.0 - progress)));
        out.push(osc.process() * env.process());
    }
    out
}

/// A chiptune snare: noise burst plus a 130 Hz square blip.
fn chip_snare(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (0.22 * sr) as usize;
    let mut noise = NoiseGen::new(NoiseKind::White, rng.fork());
    let mut env = Percussive::from_t60(sr, 0.1);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let blip = 0.4 * fast_sine(130.0 * n as f32 / sr);
        out.push((noise.process() * 0.8 + blip) * env.process());
    }
    out
}

/// A chiptune hat: very short highpassed noise.
fn chip_hat(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (0.06 * sr) as usize;
    let mut noise = NoiseGen::new(NoiseKind::White, rng.fork());
    let mut filter = Biquad::design(BiquadKind::Highpass, sr, 5500.0, 0.7, 0.0);
    let mut env = Percussive::from_t60(sr, 0.035);
    env.trigger();
    (0..len)
        .map(|_| filter.process(noise.process()) * env.process())
        .collect()
}

/// A concert tom: sine with a small pitch drop plus a noise skirt.
fn tom(sr: f32, rng: &mut Qrng, midi: i32) -> Vec<f32> {
    let len = (0.55 * sr) as usize;
    let drop = (0.09 * sr) as usize;
    let base = midi_to_freq(midi as f32);
    let mut osc = Osc::new(OscKind::Sine, base * 1.15, sr, rng);
    let mut noise = NoiseGen::new(NoiseKind::White, rng.fork());
    let mut env = Percussive::from_t60(sr, 0.38);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let progress = (n as f32 / drop as f32).min(1.0);
        osc.set_freq(base * (1.15 - 0.15 * progress));
        let membrane = osc.process();
        let skirt = 0.12 * noise.process();
        out.push((membrane + skirt) * env.process());
    }
    out
}

/// A cinematic boom: long 44 Hz sine dropping slightly.
fn boom(sr: f32, rng: &mut Qrng) -> Vec<f32> {
    let len = (2.2 * sr) as usize;
    let drop = (0.25 * sr) as usize;
    let mut osc = Osc::new(OscKind::Sine, 44.0, sr, rng);
    let mut env = Percussive::from_t60(sr, 1.5);
    env.trigger();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let progress = (n as f32 / drop as f32).min(1.0);
        osc.set_freq(44.0 - 5.0 * progress);
        out.push(osc.process() * env.process());
    }
    out
}

/// An ambient bell: sine plus a slightly detuned partial, long decay.
fn bell(sr: f32, freq: f32, t60: f32) -> Vec<f32> {
    let len = (1.5 * t60 * sr) as usize;
    let mut env = Percussive::from_t60(sr, t60);
    env.trigger();
    (0..len)
        .map(|n| {
            let fundamental = fast_sine(freq * n as f32 / sr);
            let partial = 0.3 * fast_sine(freq * 2.01 * n as f32 / sr);
            (fundamental + partial) * 0.8 * env.process()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------------

/// Renders all sections of one track, layer by layer.
struct Composer<'a> {
    sr: f32,
    bar_frames: usize,
    beat_frames: f32,
    plan: &'a TrackPlan,
    profile: &'static StyleProfile,
    pad_rng: Qrng,
    drums_rng: Qrng,
    bass_rng: Qrng,
    lead_rng: Qrng,
    arp_rng: Qrng,
    stab_rng: Qrng,
    bed_rng: Qrng,
}

impl Composer<'_> {
    fn run(&mut self, bus: &mut MixBus) {
        if matches!(self.profile.kit, Kit::Lofi) {
            self.render_vinyl(bus);
        }
        let sections = self.plan.sections.clone();
        let mut verse_count = 0usize;
        for section in sections {
            let verse_index = if section.kind == SectionKind::Verse {
                let index = verse_count;
                verse_count += 1;
                index
            } else {
                0
            };
            self.render_section(bus, section, verse_index);
        }
    }

    /// Arrangement: pads everywhere, the full kit in verses, the motif
    /// everywhere but the intro, plus per-style extras.
    fn render_section(&mut self, bus: &mut MixBus, section: Section, verse_index: usize) {
        let gain = match section.kind {
            SectionKind::Intro => 0.7,
            SectionKind::Verse => 1.0,
            SectionKind::Bridge => 0.9,
            SectionKind::Outro => 0.62,
        };
        if matches!(self.profile.pad_voice, PadVoice::Ambient) {
            self.render_pad_ambient(bus, section, gain);
        } else {
            self.render_pad(bus, section, gain);
        }
        if section.kind == SectionKind::Verse {
            self.render_drums(bus, section, verse_index);
            self.render_bass(bus, section);
        }
        if section.kind != SectionKind::Intro {
            self.render_motif(bus, section, verse_index);
        }
        self.render_extras(bus, section);
    }

    /// Style extras dispatched on the kit: lofi keys and vinyl, cinematic
    /// booms, chiptune arpeggios.
    fn render_extras(&mut self, bus: &mut MixBus, section: Section) {
        match self.profile.kit {
            Kit::Lofi => {
                if matches!(section.kind, SectionKind::Verse | SectionKind::Bridge) {
                    self.render_stabs(bus, section);
                }
            }
            Kit::Cinematic => self.render_booms(bus, section),
            Kit::Chiptune => {
                if section.kind == SectionKind::Verse {
                    self.render_arp(bus, section);
                }
            }
            Kit::None | Kit::Synthwave => {}
        }
    }

    fn chord_at(&self, prog: &'static [Chord], bar: usize) -> &'static Chord {
        let index = (bar / self.profile.chord_bars.max(1)) % prog.len();
        &prog[index]
    }

    fn progression_for(&self, kind: SectionKind) -> &'static [Chord] {
        if kind == SectionKind::Bridge {
            self.plan.bridge
        } else {
            self.plan.main
        }
    }

    /// Frame index of an absolute beat.
    fn at(&self, beat: f32) -> usize {
        (beat * self.beat_frames).round() as usize
    }

    /// Applies the style's swing to a beat that sits on an off-beat
    /// eighth.
    fn swing(&self, beat: f32) -> f32 {
        let eighths = beat * 2.0;
        let nearest = eighths.round() as i64;
        if (eighths - nearest as f32).abs() < 1.0e-4 && nearest.rem_euclid(2) == 1 {
            beat + self.profile.swing * 0.5
        } else {
            beat
        }
    }

    fn humanize(&mut self) -> f32 {
        self.drums_rng.next_range(0.85, 1.0)
    }

    fn render_pad(&mut self, bus: &mut MixBus, section: Section, gain_scale: f32) {
        let chord_bars = self.profile.chord_bars.max(1);
        let prog = self.progression_for(section.kind);
        let len_frames = chord_bars * self.bar_frames;
        for bar in (section.start_bar..section.end_bar()).step_by(chord_bars) {
            let chord = self.chord_at(prog, bar);
            for (voice, &interval) in chord.intervals.iter().enumerate() {
                let midi = self.plan.key_root + chord.root + interval;
                let freq = midi_to_freq(midi as f32);
                let samples = osc_note(
                    &self.profile.pad,
                    freq,
                    len_frames,
                    self.sr,
                    &mut self.pad_rng,
                );
                let pan_pos = PAD_PANS[voice % PAD_PANS.len()];
                bus.add(
                    bar * self.bar_frames,
                    &samples,
                    self.profile.pad_gain * gain_scale,
                    pan_pos,
                );
            }
        }
    }

    /// The ambient pad: detuned triangles behind a state-variable filter
    /// whose corner sweeps on a slow LFO (a different rate per voice, so
    /// the bed keeps evolving).
    fn render_pad_ambient(&mut self, bus: &mut MixBus, section: Section, gain_scale: f32) {
        let chord_bars = self.profile.chord_bars.max(1);
        let prog = self.progression_for(section.kind);
        let len_frames = chord_bars * self.bar_frames;
        let spec = &self.profile.pad;
        for bar in (section.start_bar..section.end_bar()).step_by(chord_bars) {
            let chord = self.chord_at(prog, bar);
            for (voice, &interval) in chord.intervals.iter().enumerate() {
                let midi = self.plan.key_root + chord.root + interval;
                let freq = midi_to_freq(midi as f32);
                let mut osc_a = Osc::new(spec.kind, freq, self.sr, &mut self.pad_rng);
                let mut osc_b = Osc::new(
                    spec.kind,
                    freq * (1.0 + spec.detune),
                    self.sr,
                    &mut self.pad_rng,
                );
                let mut lfo = Lfo::new(
                    LfoShape::Sine,
                    0.05 + 0.011 * voice as f32,
                    self.sr,
                    &mut self.pad_rng,
                );
                let mut svf = Svf::new(self.sr, 800.0, 0.8);
                let mut env =
                    Adsr::new(spec.attack, spec.decay, spec.sustain, spec.release, self.sr);
                env.trigger();
                let release_frames = (spec.release * self.sr) as usize;
                let total = len_frames + release_frames;
                let mut samples = Vec::with_capacity(total);
                for n in 0..total {
                    if n == len_frames {
                        env.release();
                    }
                    let sweep = lfo.process() * 0.5 + 0.5;
                    svf.set(self.sr, 350.0 + 1150.0 * sweep, 0.8);
                    let raw = osc_a.process().midpoint(osc_b.process());
                    samples.push(svf.process(raw * env.process()).low);
                }
                let pan_pos = PAD_PANS[voice % PAD_PANS.len()];
                bus.add(
                    bar * self.bar_frames,
                    &samples,
                    self.profile.pad_gain * gain_scale,
                    pan_pos,
                );
            }
        }
    }

    fn render_bass(&mut self, bus: &mut MixBus, section: Section) {
        match self.profile.bass {
            BassVoice::HalfNote => self.bass_half_notes(bus, section),
            BassVoice::EighthsSaw | BassVoice::EighthsSquare => self.bass_eighths(bus, section),
            BassVoice::SubSustained => self.bass_sustained(bus, section),
            BassVoice::SineSwell => self.bass_swell(bus, section),
        }
    }

    /// Lofi bass: root on the beat and a syncopated push before beat
    /// three.
    fn bass_half_notes(&mut self, bus: &mut MixBus, section: Section) {
        let prog = self.progression_for(section.kind);
        for bar in section.start_bar..section.end_bar() {
            let chord = self.chord_at(prog, bar);
            let midi = self.plan.key_root - 12 + chord.root;
            let freq = midi_to_freq(midi as f32);
            for &(beat_offset, len) in &[(0.0f32, 1.4f32), (2.5, 1.2)] {
                let beat = bar as f32 * BEATS_PER_BAR + self.swing(beat_offset);
                let len_frames = (len * self.beat_frames) as usize;
                let note = osc_note(&LOFI_BASS, freq, len_frames, self.sr, &mut self.bass_rng);
                bus.add(self.at(beat), &note, 0.4, 0.0);
            }
        }
    }

    /// Driving eighth-note bass with a seeded octave-jump pattern.
    fn bass_eighths(&mut self, bus: &mut MixBus, section: Section) {
        let saw = matches!(self.profile.bass, BassVoice::EighthsSaw);
        let (spec, gain) = if saw {
            (&SYNTH_BASS, 0.38f32)
        } else {
            (&CHIP_BASS, 0.3f32)
        };
        let pattern = BASS_PATTERNS[self
            .bass_rng
            .next_int_range(0, BASS_PATTERNS.len() as u64 - 1)
            as usize];
        let prog = self.progression_for(section.kind);
        let len = (0.46 * self.beat_frames) as usize;
        for bar in section.start_bar..section.end_bar() {
            let chord = self.chord_at(prog, bar);
            let root = self.plan.key_root - 12 + chord.root;
            for (step, &jump) in pattern.iter().enumerate() {
                let midi = root + jump;
                let beat = bar as f32 * BEATS_PER_BAR + step as f32 * 0.5;
                let note = osc_note(
                    spec,
                    midi_to_freq(midi as f32),
                    len,
                    self.sr,
                    &mut self.bass_rng,
                );
                bus.add(self.at(beat), &note, gain, 0.0);
            }
        }
    }

    /// Cinematic sub bass: one sustained sine per bar.
    fn bass_sustained(&mut self, bus: &mut MixBus, section: Section) {
        let prog = self.progression_for(section.kind);
        for bar in section.start_bar..section.end_bar() {
            let chord = self.chord_at(prog, bar);
            let midi = self.plan.key_root - 24 + chord.root;
            let note = osc_note(
                &SUB_BASS,
                midi_to_freq(midi as f32),
                self.bar_frames,
                self.sr,
                &mut self.bass_rng,
            );
            bus.add(bar * self.bar_frames, &note, 0.45, 0.0);
        }
    }

    /// Ambient bass: a slow sine swell on each chord root.
    fn bass_swell(&mut self, bus: &mut MixBus, section: Section) {
        let chord_bars = self.profile.chord_bars.max(1);
        let prog = self.progression_for(section.kind);
        let len_frames = chord_bars * self.bar_frames;
        for bar in (section.start_bar..section.end_bar()).step_by(chord_bars) {
            let chord = self.chord_at(prog, bar);
            let midi = self.plan.key_root - 12 + chord.root;
            let note = osc_note(
                &SWELL_BASS,
                midi_to_freq(midi as f32),
                len_frames,
                self.sr,
                &mut self.bass_rng,
            );
            bus.add(bar * self.bar_frames, &note, 0.32, 0.0);
        }
    }

    fn render_drums(&mut self, bus: &mut MixBus, section: Section, verse_index: usize) {
        match self.profile.kit {
            Kit::Lofi => self.drums_lofi(bus, section),
            Kit::Synthwave => self.drums_synthwave(bus, section, verse_index),
            Kit::Cinematic => self.drums_cinematic(bus, section),
            Kit::Chiptune => self.drums_chiptune(bus, section, verse_index),
            Kit::None => {}
        }
    }

    /// Boom-bap: kick and snare backbone, swung hats, occasional ghost
    /// kick.
    fn drums_lofi(&mut self, bus: &mut MixBus, section: Section) {
        for bar in section.start_bar..section.end_bar() {
            let base = bar as f32 * BEATS_PER_BAR;
            let kick_samples = kick(self.sr, &mut self.drums_rng);
            bus.add(self.at(base), &kick_samples, 0.5, 0.0);
            bus.add(
                self.at(base + self.swing(2.5)),
                &kick(self.sr, &mut self.drums_rng),
                0.42,
                0.0,
            );
            if self.drums_rng.next_f32() < 0.25 {
                bus.add(
                    self.at(base + self.swing(3.75)),
                    &kick(self.sr, &mut self.drums_rng),
                    0.2,
                    0.0,
                );
            }
            bus.add(
                self.at(base + 1.0),
                &snare(self.sr, &mut self.drums_rng),
                0.33 * self.humanize(),
                0.05,
            );
            bus.add(
                self.at(base + 3.0),
                &snare(self.sr, &mut self.drums_rng),
                0.3 * self.humanize(),
                0.05,
            );
            for step in 0..8usize {
                let beat = base + self.swing(step as f32 * 0.5);
                let accent = if step % 2 == 0 { 0.1 } else { 0.065 };
                bus.add(
                    self.at(beat),
                    &hat(self.sr, &mut self.drums_rng, false),
                    accent * self.humanize(),
                    0.25,
                );
            }
        }
    }

    /// Four-on-the-floor kick, backbeat snare, offbeat hats that double to
    /// sixteenths from the second verse on.
    fn drums_synthwave(&mut self, bus: &mut MixBus, section: Section, verse_index: usize) {
        for bar in section.start_bar..section.end_bar() {
            let base = bar as f32 * BEATS_PER_BAR;
            for beat in 0..4usize {
                bus.add(
                    self.at(base + beat as f32),
                    &kick(self.sr, &mut self.drums_rng),
                    0.5,
                    0.0,
                );
            }
            bus.add(
                self.at(base + 1.0),
                &snare(self.sr, &mut self.drums_rng),
                0.3,
                0.1,
            );
            bus.add(
                self.at(base + 3.0),
                &snare(self.sr, &mut self.drums_rng),
                0.3,
                0.1,
            );
            if verse_index >= 1 {
                for step in 0..16usize {
                    let beat = base + step as f32 * 0.25;
                    bus.add(
                        self.at(beat),
                        &hat(self.sr, &mut self.drums_rng, false),
                        0.05 * self.humanize(),
                        0.3,
                    );
                }
            } else {
                for beat in 0..4usize {
                    bus.add(
                        self.at(base + beat as f32 + 0.5),
                        &hat(self.sr, &mut self.drums_rng, false),
                        0.09,
                        0.3,
                    );
                }
            }
            if bar % 2 == 1 {
                bus.add(
                    self.at(base + 3.5),
                    &hat(self.sr, &mut self.drums_rng, true),
                    0.1,
                    0.3,
                );
            }
        }
    }

    /// Toms on a mid-bar pattern, a rising fill in the last bar.
    fn drums_cinematic(&mut self, bus: &mut MixBus, section: Section) {
        for bar in section.start_bar..section.end_bar() {
            let base = bar as f32 * BEATS_PER_BAR;
            bus.add(
                self.at(base),
                &tom(self.sr, &mut self.drums_rng, 36),
                0.5 * self.humanize(),
                -0.15,
            );
            bus.add(
                self.at(base + 1.5),
                &tom(self.sr, &mut self.drums_rng, 43),
                0.42 * self.humanize(),
                0.1,
            );
            let last = bar + 1 == section.end_bar();
            if last {
                for &(beat, midi) in &[(3.0, 36), (3.25, 40), (3.5, 43), (3.75, 48)] {
                    bus.add(
                        self.at(base + beat),
                        &tom(self.sr, &mut self.drums_rng, midi),
                        0.4,
                        0.2,
                    );
                }
            } else {
                bus.add(
                    self.at(base + 3.0),
                    &tom(self.sr, &mut self.drums_rng, 48),
                    0.4 * self.humanize(),
                    0.25,
                );
            }
        }
    }

    /// Chip drums: square kick, noise snare, straight hats that double to
    /// sixteenths from the second verse on.
    fn drums_chiptune(&mut self, bus: &mut MixBus, section: Section, verse_index: usize) {
        for bar in section.start_bar..section.end_bar() {
            let base = bar as f32 * BEATS_PER_BAR;
            bus.add(
                self.at(base),
                &chip_kick(self.sr, &mut self.drums_rng),
                0.45,
                0.0,
            );
            bus.add(
                self.at(base + 2.0),
                &chip_kick(self.sr, &mut self.drums_rng),
                0.4,
                0.0,
            );
            bus.add(
                self.at(base + 1.0),
                &chip_snare(self.sr, &mut self.drums_rng),
                0.3,
                0.08,
            );
            bus.add(
                self.at(base + 3.0),
                &chip_snare(self.sr, &mut self.drums_rng),
                0.3,
                0.08,
            );
            let divisions = if verse_index >= 1 { 16 } else { 8 };
            for step in 0..divisions {
                let beat = base + step as f32 * (BEATS_PER_BAR / divisions as f32);
                let pan_pos = if step % 2 == 0 { -0.3 } else { 0.3 };
                bus.add(
                    self.at(beat),
                    &chip_hat(self.sr, &mut self.drums_rng),
                    0.08,
                    pan_pos,
                );
            }
        }
    }

    /// The motif as played through one section: phrase repetition with an
    /// answer bar, development and swing.
    fn render_motif(&mut self, bus: &mut MixBus, section: Section, verse_index: usize) {
        let phrase = developed_phrase(&self.plan.motif, section.kind, verse_index);
        if phrase.is_empty() {
            return;
        }
        let transpose = if section.kind == SectionKind::Verse && verse_index >= 1 {
            12
        } else {
            0
        };
        let kind_gain = match section.kind {
            SectionKind::Verse => 1.0,
            SectionKind::Bridge => 0.85,
            SectionKind::Outro => 0.45,
            SectionKind::Intro => return,
        };
        let mut events = Vec::new();
        let last_index = phrase.len() - 1;
        for bar in 0..section.bars {
            let odd = bar % 2 == 1;
            for (index, note) in phrase.iter().enumerate() {
                if odd && index == last_index {
                    continue;
                }
                let beat = (section.start_bar + bar) as f32 * BEATS_PER_BAR + self.swing(note.beat);
                let midi =
                    degree_midi(self.plan.key_root, self.plan.scale, note.degree) + transpose;
                events.push(SchedNote {
                    start: self.at(beat),
                    len: ((note.len * self.beat_frames) as usize).max(2),
                    freq: midi_to_freq(midi as f32),
                    gain: note.gain * kind_gain,
                });
            }
        }
        self.emit_lead(bus, &events);
        if section.kind == SectionKind::Verse && verse_index >= 1 {
            let doubled: Vec<SchedNote> = events
                .iter()
                .map(|event| SchedNote {
                    start: event.start,
                    len: event.len,
                    freq: event.freq * 2.0,
                    gain: event.gain * 0.45,
                })
                .collect();
            self.emit_lead(bus, &doubled);
        }
    }

    /// Plays scheduled motif events through the style's lead voice.
    fn emit_lead(&mut self, bus: &mut MixBus, events: &[SchedNote]) {
        let Some(first) = events.first() else {
            return;
        };
        match self.profile.lead {
            LeadVoice::KarplusKeys => {
                for event in events {
                    let note = karplus::pluck(self.sr, event.freq, 1.7, &mut self.lead_rng);
                    bus.add(event.start, &note, event.gain * 0.5, -0.1);
                }
            }
            LeadVoice::SquareChip => {
                for event in events {
                    let note = osc_note(
                        &CHIP_LEAD,
                        event.freq,
                        event.len,
                        self.sr,
                        &mut self.lead_rng,
                    );
                    bus.add(event.start, &note, event.gain * 0.24, 0.0);
                }
            }
            LeadVoice::SineBell => {
                for event in events {
                    let note = bell(self.sr, event.freq, 3.2);
                    bus.add(event.start, &note, event.gain * 0.3, 0.05);
                }
            }
            LeadVoice::SawVibrato => {
                let rendered = render_legato(&SAW_LEAD, events, self.sr, &mut self.lead_rng);
                bus.add(first.start, &rendered, 0.3, 0.0);
            }
            LeadVoice::LegatoGlide => {
                let rendered = render_legato(&GLIDE_LEAD, events, self.sr, &mut self.lead_rng);
                bus.add(first.start, &rendered, 0.38, 0.0);
            }
        }
    }

    /// Chiptune arpeggio: sixteenth notes cycling the chord tones an
    /// octave up, alternating sides of the stereo field.
    fn render_arp(&mut self, bus: &mut MixBus, section: Section) {
        const ORDER: [usize; 4] = [0, 1, 2, 1];
        let prog = self.progression_for(SectionKind::Verse);
        let len = (0.23 * self.beat_frames) as usize;
        for bar in section.start_bar..section.end_bar() {
            let chord = self.chord_at(prog, bar);
            let base = bar as f32 * BEATS_PER_BAR;
            for step in 0..16usize {
                let interval_index = ORDER[step % ORDER.len()] % chord.intervals.len();
                let midi = self.plan.key_root + 12 + chord.root + chord.intervals[interval_index];
                let beat = base + step as f32 * 0.25;
                let note = osc_note(
                    &CHIP_ARP,
                    midi_to_freq(midi as f32),
                    len,
                    self.sr,
                    &mut self.arp_rng,
                );
                let pan_pos = if step % 2 == 0 { -0.45 } else { 0.45 };
                bus.add(self.at(beat), &note, 0.14, pan_pos);
            }
        }
    }

    /// Lofi keys: karplus chord stabs, slightly strummed.
    fn render_stabs(&mut self, bus: &mut MixBus, section: Section) {
        let prog = self.progression_for(section.kind);
        let gain = if section.kind == SectionKind::Bridge {
            0.3
        } else {
            0.4
        };
        for bar in section.start_bar..section.end_bar() {
            let chord = self.chord_at(prog, bar);
            let second = if self.stab_rng.next_f32() < 0.5 {
                2.5
            } else {
                2.75
            };
            for beat_offset in [0.0f32, second] {
                let beat = bar as f32 * BEATS_PER_BAR + self.swing(beat_offset);
                for (voice, &interval) in chord.intervals.iter().enumerate().take(3) {
                    let midi = self.plan.key_root + chord.root + interval;
                    let strum = beat + voice as f32 * 0.03;
                    let note =
                        karplus::pluck(self.sr, midi_to_freq(midi as f32), 1.9, &mut self.stab_rng);
                    bus.add(self.at(strum), &note, gain, -0.15);
                }
            }
        }
    }

    /// Cinematic booms at every section start.
    fn render_booms(&mut self, bus: &mut MixBus, section: Section) {
        let gain = match section.kind {
            SectionKind::Intro => 0.28,
            SectionKind::Verse => 0.4,
            SectionKind::Bridge => 0.3,
            SectionKind::Outro => 0.25,
        };
        let samples = boom(self.sr, &mut self.drums_rng);
        bus.add(section.start_bar * self.bar_frames, &samples, gain, 0.0);
    }

    /// The lofi vinyl bed: filtered pink noise plus sparse crackle.
    fn render_vinyl(&mut self, bus: &mut MixBus) {
        let frames = bus.frames();
        let mut bed = NoiseGen::new(NoiseKind::Pink, self.bed_rng.fork());
        let mut filter = Biquad::design(BiquadKind::Highpass, self.sr, 300.0, 0.7, 0.0);
        let mut surface = Vec::with_capacity(frames);
        for _ in 0..frames {
            surface.push(filter.process(bed.process()));
        }
        let mut crackle = vec![0.0f32; frames];
        let click_probability = 14.0 / self.sr;
        for n in 0..frames {
            if self.bed_rng.next_f32() < click_probability {
                let magnitude = self.bed_rng.next_range(0.03, 0.09);
                let sign = if self.bed_rng.next_f32() < 0.5 {
                    -1.0
                } else {
                    1.0
                };
                for k in 0..4usize {
                    if let Some(slot) = crackle.get_mut(n + k) {
                        *slot += sign * magnitude * (1.0 - k as f32 / 4.0);
                    }
                }
            }
        }
        for (n, slot) in surface.iter_mut().enumerate() {
            *slot += crackle[n];
        }
        bus.add(0, &surface, 0.02, 0.0);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Whole-mix post processing.
fn apply_post_fx(fx: &PostFx, bus: &mut MixBus, sr: f32, bpm: f32) {
    let frames = bus.frames();
    match fx {
        PostFx::None => {}
        PostFx::Reverb { mix } => {
            let mut reverb = Freeverb::new(sr);
            if bus.data.len() == 2 {
                for n in 0..frames {
                    let (left, right) = (bus.data[0][n], bus.data[1][n]);
                    let (wet_l, wet_r) = reverb.process(left, right);
                    bus.data[0][n] = left + mix * (wet_l - left);
                    bus.data[1][n] = right + mix * (wet_r - right);
                }
            } else {
                for n in 0..frames {
                    let dry = bus.data[0][n];
                    let (wet, _) = reverb.process(dry, dry);
                    bus.data[0][n] = dry + mix * (wet - dry);
                }
            }
        }
        PostFx::Echo {
            beats,
            feedback,
            mix,
        } => {
            let time = beats * 60.0 / bpm;
            for channel in &mut bus.data {
                let mut delay = Delay::new(sr, time + 0.05, *feedback);
                delay.set_delay_secs(time);
                for slot in channel.iter_mut() {
                    let dry = *slot;
                    let echo = delay.process(dry);
                    *slot = dry + mix * (echo - dry);
                }
            }
        }
    }
}

/// De-clicks the start, fades the end to exactly zero and scales the mix
/// down if it exceeds full scale.
///
/// `fade_out_frames` is the closing fade length (the composer fades most
/// of the outro away).
fn finalize(mut bus: MixBus, sample_rate: u32, fade_out_frames: usize) -> AudioBuffer {
    let sr = sample_rate as f32;
    let frames = bus.frames();
    let fade_in_frames = ((0.003 * sr) as usize).min(frames);
    let fade_out_frames = fade_out_frames.min(frames / 3);
    for channel in &mut bus.data {
        if fade_in_frames > 0 {
            for (n, sample) in channel.iter_mut().enumerate().take(fade_in_frames) {
                *sample *= n as f32 / fade_in_frames as f32;
            }
        }
        if fade_out_frames > 0 {
            let start = frames - fade_out_frames;
            for (n, sample) in channel.iter_mut().enumerate().skip(start) {
                *sample *= (frames - 1 - n) as f32 / fade_out_frames as f32;
            }
        }
    }
    let mut peak = 0.0f32;
    for channel in &bus.data {
        for &sample in channel {
            peak = peak.max(sample.abs());
        }
    }
    if peak > 1.0 {
        let gain = 0.98 / peak;
        for channel in &mut bus.data {
            for sample in channel.iter_mut() {
                *sample *= gain;
            }
        }
    }
    AudioBuffer::from_channels(bus.data, sample_rate)
        .expect("bus channels are uniform and the sample rate is positive")
}

/// Composes and renders one validated request.
pub(crate) fn compose(request: &TrackRequest, style: &Style, bpm: f32) -> AudioBuffer {
    let sr = request.sample_rate as f32;
    let bar_frames = (bar_secs(bpm) * sr).round() as usize;
    let mut rng = Qrng::from_seed(request.seed);
    let plan = build_plan(&mut rng, style, bpm, request.duration_secs);
    let profile = profile_for(style.id);
    let total_frames = plan.total_bars * bar_frames;
    let mut composer = Composer {
        sr,
        bar_frames,
        beat_frames: bar_frames as f32 / BEATS_PER_BAR,
        plan: &plan,
        profile,
        pad_rng: rng.fork(),
        drums_rng: rng.fork(),
        bass_rng: rng.fork(),
        lead_rng: rng.fork(),
        arp_rng: rng.fork(),
        stab_rng: rng.fork(),
        bed_rng: rng.fork(),
    };
    let mut bus = MixBus::new(request.channels as usize, total_frames);
    composer.run(&mut bus);
    apply_post_fx(&profile.post_fx, &mut bus, sr, bpm);
    let outro_frames = plan
        .sections
        .last()
        .map_or(0, |section| section.bars * bar_frames);
    let fade_out = (outro_frames * 3 / 4).min((2.5 * sr) as usize).max(1);
    finalize(bus, request.sample_rate, fade_out)
}

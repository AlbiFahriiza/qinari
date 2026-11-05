//! Behavior tests for the composer, driven through the public `render`
//! API, plus unit tests of the internal section planner.

use crate::compose::{plan_sections, SectionKind};
use crate::{find_style, render, MusicError, TrackRequest, STYLES};
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::rng::Seed;

const SR: u32 = 48_000;

fn request(style: &str, secs: f32) -> TrackRequest {
    TrackRequest {
        style: style.to_string(),
        duration_secs: secs,
        bpm: 0.0, // falls back to the style's typical tempo
        seed: Seed::from_text("qinari-music-tests"),
        sample_rate: SR,
        channels: 2,
    }
}

/// RMS over all channels between `from` (inclusive) and `to` (exclusive).
fn rms_range(buffer: &AudioBuffer, from: usize, to: usize) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for channel in buffer.channels() {
        for &sample in channel.iter().take(to).skip(from) {
            sum += f64::from(sample) * f64::from(sample);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

#[test]
fn styles_catalog_matches_the_spec() {
    let expected = [
        ("lofi", 72.0, 82.0, 90.0),
        ("ambient", 58.0, 64.0, 72.0),
        ("synthwave", 100.0, 110.0, 118.0),
        ("cinematic", 76.0, 88.0, 96.0),
        ("chiptune", 128.0, 140.0, 150.0),
    ];
    assert_eq!(STYLES.len(), expected.len());
    for (style, (id, min, typical, max)) in STYLES.iter().zip(expected) {
        assert_eq!(style.id, id);
        assert!(
            (style.bpm_min - min).abs() < 1.0e-6,
            "{id} min {}",
            style.bpm_min
        );
        assert!(
            (style.bpm_typical - typical).abs() < 1.0e-6,
            "{id} typical {}",
            style.bpm_typical
        );
        assert!(
            (style.bpm_max - max).abs() < 1.0e-6,
            "{id} max {}",
            style.bpm_max
        );
        assert!(!style.description.is_empty(), "{id} needs a description");
        assert_eq!(find_style(id).map(|found| found.id), Some(id));
    }
    assert!(find_style("polka").is_none());
}

#[test]
fn all_styles_render_twelve_seconds_stereo_non_silent_and_finite() {
    for style in STYLES {
        let buffer = render(&request(style.id, 12.0)).expect(style.id);
        assert_eq!(buffer.num_channels(), 2, "{}", style.id);
        assert!(
            buffer.frames() >= 12 * SR as usize,
            "{}: {} frames",
            style.id,
            buffer.frames()
        );
        assert!(!buffer.has_non_finite(), "{}", style.id);
        assert!(buffer.peak() > 0.02, "{} peak {}", style.id, buffer.peak());
        assert!(buffer.rms() > 0.008, "{} rms {}", style.id, buffer.rms());
    }
}

#[test]
fn renders_are_deterministic_and_seed_sensitive() {
    let first = render(&request("lofi", 8.0)).expect("render");
    let second = render(&request("lofi", 8.0)).expect("render");
    assert_eq!(first, second);

    let mut other = request("lofi", 8.0);
    other.seed = Seed::from_text("a different seed");
    let third = render(&other).expect("render");
    assert_ne!(first, third);
}

#[test]
fn outro_fades_below_the_body_of_the_arrangement() {
    for style in STYLES {
        let buffer = render(&request(style.id, 12.0)).expect(style.id);
        let frames = buffer.frames();
        let middle = rms_range(&buffer, frames / 3, 2 * frames / 3);
        let tail = rms_range(&buffer, frames - frames / 10, frames);
        assert!(
            middle > 2.5 * tail,
            "{}: middle rms {} vs tail rms {}",
            style.id,
            middle,
            tail
        );
    }
}

#[test]
fn sections_are_audibly_distinct() {
    for style in STYLES {
        let bpm = style.bpm_typical;
        let sections = plan_sections(12.0, bpm);
        let buffer = render(&request(style.id, 12.0)).expect(style.id);
        let bar_frames = (240.0 / bpm * SR as f32).round() as usize;

        let section_rms: Vec<(SectionKind, f32)> = sections
            .iter()
            .map(|section| {
                let from = section.start_bar * bar_frames;
                let to = (section.end_bar() * bar_frames).min(buffer.frames());
                (section.kind, rms_range(&buffer, from, to))
            })
            .collect();
        let intro = section_rms[0].1;
        let verse = section_rms
            .iter()
            .filter(|(kind, _)| *kind == SectionKind::Verse)
            .map(|(_, rms)| *rms)
            .fold(f32::NEG_INFINITY, f32::max);
        let outro = section_rms.last().expect("outro exists").1;

        assert!(intro > 0.0, "{}: silent intro", style.id);
        assert!(
            verse > 1.25 * intro,
            "{}: verse rms {} vs intro rms {}",
            style.id,
            verse,
            intro
        );
        assert!(
            verse > 1.25 * outro,
            "{}: verse rms {} vs outro rms {}",
            style.id,
            verse,
            outro
        );
    }
}

#[test]
fn section_plan_covers_the_request_without_wild_overshoot() {
    let cases = [
        (5.0f32, 58.0f32),
        (5.0, 150.0),
        (12.0, 82.0),
        (30.0, 110.0),
        (90.0, 64.0),
        (300.0, 140.0),
        (600.0, 96.0),
    ];
    for &(secs, bpm) in &cases {
        let sections = plan_sections(secs, bpm);
        assert!(sections.len() >= 3, "{secs}s at {bpm} bpm: {sections:?}");
        assert_eq!(sections[0].kind, SectionKind::Intro);
        assert_eq!(
            sections.last().map(|section| section.kind),
            Some(SectionKind::Outro)
        );
        assert!(sections.iter().all(|section| section.bars >= 1));
        for pair in sections.windows(2) {
            assert_eq!(pair[0].start_bar + pair[0].bars, pair[1].start_bar);
        }
        let bar = 240.0 / bpm;
        let total_bars: usize = sections.iter().map(|section| section.bars).sum();
        assert!(
            total_bars as f32 * bar >= secs - 1.0e-4,
            "{secs}s at {bpm} bpm: {total_bars} bars"
        );
        assert!(
            total_bars as f32 * bar <= secs + 4.0 * bar,
            "{secs}s at {bpm} bpm: {total_bars} bars is too long"
        );
    }
}

#[test]
fn unknown_style_is_rejected() {
    let error = render(&request("polka", 10.0)).unwrap_err();
    assert!(matches!(error, MusicError::UnknownStyle { .. }));
    assert!(error.to_string().contains("polka"));
}

#[test]
fn duration_bounds_are_enforced() {
    let rejected = render(&request("lofi", 4.0)).unwrap_err();
    assert!(matches!(rejected, MusicError::BadDuration { .. }));

    let five = render(&request("lofi", 5.0)).expect("5 s is the minimum");
    assert!(five.duration() >= 5.0);

    let mut boundary = request("ambient", 600.0);
    boundary.sample_rate = 8_000;
    boundary.channels = 1;
    let upper = render(&boundary).expect("600 s is the maximum");
    assert!(upper.duration() >= 600.0);

    let mut beyond = request("ambient", 601.0);
    beyond.sample_rate = 8_000;
    let rejected = render(&beyond).unwrap_err();
    assert!(matches!(rejected, MusicError::BadDuration { .. }));
}

#[test]
fn bpm_is_clamped_to_the_style_range() {
    let mut req = request("lofi", 8.0);

    req.bpm = 5.0;
    let clamped_low = render(&req).expect("render");
    req.bpm = 72.0;
    let at_min = render(&req).expect("render");
    assert_eq!(clamped_low, at_min);

    req.bpm = 999.0;
    let clamped_high = render(&req).expect("render");
    req.bpm = 90.0;
    let at_max = render(&req).expect("render");
    assert_eq!(clamped_high, at_max);

    req.bpm = 0.0;
    let fallback = render(&req).expect("render");
    req.bpm = 82.0;
    let typical = render(&req).expect("render");
    assert_eq!(fallback, typical);

    // The clamp has to matter: different tempi render different lengths.
    assert_ne!(at_min, at_max);
}

#[test]
fn mono_render_and_channel_validation() {
    for style in ["lofi", "chiptune"] {
        let mut req = request(style, 8.0);
        req.channels = 1;
        let buffer = render(&req).expect(style);
        assert_eq!(buffer.num_channels(), 1);
        assert!(!buffer.has_non_finite());
        assert!(buffer.peak() > 0.02, "{} peak {}", style, buffer.peak());
    }

    let mut req = request("lofi", 8.0);
    req.channels = 0;
    assert!(matches!(
        render(&req).unwrap_err(),
        MusicError::BadChannels { .. }
    ));
    req.channels = 3;
    assert!(matches!(
        render(&req).unwrap_err(),
        MusicError::BadChannels { .. }
    ));
}

#[test]
fn peak_never_exceeds_full_scale() {
    for style in STYLES {
        let buffer = render(&request(style.id, 10.0)).expect(style.id);
        assert!(buffer.peak() <= 1.0, "{} peak {}", style.id, buffer.peak());
    }
}

#[test]
fn rendered_length_covers_the_request_and_stays_close() {
    let cases = [
        ("lofi", 5.0f32),
        ("chiptune", 12.0),
        ("ambient", 20.0),
        ("cinematic", 30.0),
    ];
    for &(style_id, secs) in &cases {
        let style = find_style(style_id).expect("style");
        let buffer = render(&request(style_id, secs)).expect(style_id);
        assert!(
            buffer.duration() >= f64::from(secs) - 1.0e-9,
            "{}: {} s rendered",
            style_id,
            buffer.duration()
        );
        let bar = 240.0 / f64::from(style.bpm_typical);
        assert!(
            buffer.duration() <= f64::from(secs) + 4.0 * bar + 0.05,
            "{}: {} s rendered for a {} s request",
            style_id,
            buffer.duration(),
            secs
        );
    }
}

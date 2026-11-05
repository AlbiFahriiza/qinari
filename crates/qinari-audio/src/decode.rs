//! Decoding of `.wav` and `.mp3` files via `symphonia` into [`AudioBuffer`]s.

use crate::AudioError;
use qinari_dsp::buffer::AudioBuffer;
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

/// Decodes the first audio track of a `.wav` or `.mp3` file.
///
/// The extension is passed to the prober as a hint, but the actual format is
/// detected from the stream contents. Samples are converted to `f32` as
/// needed; damaged packets are skipped, and a stream that ends early keeps
/// the frames decoded so far.
///
/// # Errors
///
/// Returns [`AudioError::Io`] when the file cannot be opened and
/// [`AudioError::Decode`] when the format cannot be probed, has no audio
/// track, uses an unsupported codec, decodes no frames at all, or yields
/// channels of differing lengths.
pub fn decode_audio<P: AsRef<Path>>(path: P) -> Result<AudioBuffer, AudioError> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| AudioError::Decode(error.to_string()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioError::Decode("no audio track found".into()))?;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .ok_or_else(|| AudioError::Decode("no audio codec parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
        .map_err(|error| AudioError::Decode(error.to_string()))?;
    let track_id = track.id;
    let (planes, sample_rate) = drain_track(&mut format, &mut decoder, track_id)?;
    AudioBuffer::from_channels(planes, sample_rate)
        .map_err(|error| AudioError::Decode(error.to_string()))
}

/// Decodes every packet of the selected track into planar `f32` channels.
///
/// Damaged packets are skipped, and a stream that dies mid-file ends the
/// loop and keeps the frames decoded so far.
fn drain_track(
    format: &mut Box<dyn symphonia::core::formats::FormatReader>,
    decoder: &mut Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
) -> Result<(Vec<Vec<f32>>, u32), AudioError> {
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut sample_rate = 0u32;
    while let Ok(Some(packet)) = format.next_packet() {
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = audio.spec();
        let channels = spec.channels().count();
        if channels == 0 {
            return Err(AudioError::Decode("zero-channel audio track".into()));
        }
        if planes.is_empty() {
            planes = vec![Vec::new(); channels];
            sample_rate = spec.rate();
        } else if planes.len() < channels {
            planes.resize(channels, Vec::new());
        }
        let mut interleaved = vec![0.0f32; audio.samples_interleaved()];
        audio.copy_to_slice_interleaved(&mut interleaved);
        for chunk in interleaved.chunks_exact(channels) {
            for (channel, &sample) in chunk.iter().enumerate() {
                planes[channel].push(sample);
            }
        }
    }
    if planes.is_empty() {
        return Err(AudioError::Decode("no audio frames decoded".into()));
    }
    Ok((planes, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Mp3Bitrate;
    use crate::mp3::write_mp3;
    use crate::test_support::{temp_file, tone};
    use qinari_dsp::analysis::goertzel_magnitude;

    #[test]
    fn decodes_a_hand_written_16bit_stereo_wav() {
        let path = temp_file("decode-handwritten-16.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
        for frame in 0..44_100 {
            let sample = 0.4 * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / 44_100.0).sin();
            writer
                .write_sample((sample * 32_767.0) as i16)
                .expect("write");
            writer
                .write_sample((sample * 32_767.0) as i16)
                .expect("write");
        }
        writer.finalize().expect("finalize");

        let decoded = decode_audio(&path).expect("wav must decode");
        assert_eq!(decoded.num_channels(), 2);
        assert_eq!(decoded.sample_rate(), 44_100);
        assert_eq!(decoded.frames(), 44_100);
        for channel in 0..2 {
            let measured = goertzel_magnitude(&decoded.channels()[channel], 440.0, 44_100.0);
            assert!(
                (measured - 0.4).abs() < 0.01,
                "channel {channel} tone {measured}, expected ~0.4"
            );
        }
    }

    #[test]
    fn decodes_a_hand_written_24bit_mono_wav() {
        let path = temp_file("decode-handwritten-24.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
        for frame in 0..48_000 {
            let sample = 0.3 * (2.0 * std::f32::consts::PI * 300.0 * frame as f32 / 48_000.0).sin();
            writer
                .write_sample((sample * 8_388_607.0) as i32)
                .expect("write");
        }
        writer.finalize().expect("finalize");

        let decoded = decode_audio(&path).expect("wav must decode");
        assert_eq!(decoded.num_channels(), 1);
        assert_eq!(decoded.sample_rate(), 48_000);
        assert_eq!(decoded.frames(), 48_000);
        let measured = goertzel_magnitude(&decoded.channels()[0], 300.0, 48_000.0);
        assert!(
            (measured - 0.3).abs() < 0.01,
            "tone {measured}, expected ~0.3"
        );
    }

    #[test]
    fn decodes_a_qinari_written_mp3() {
        let original = tone(997.0, 0.5, 0.6, 2, 44_100);
        let path = temp_file("decode-qinari.mp3");
        write_mp3(&path, &original, Mp3Bitrate::Kbps128).expect("encode");
        let decoded = decode_audio(&path).expect("mp3 must decode");
        assert_eq!(decoded.num_channels(), 2);
        assert_eq!(decoded.sample_rate(), 44_100);
        let expected = 0.6 * 44_100.0;
        assert!(
            (decoded.frames() as f64 - expected).abs() < expected * 0.1,
            "frames {}, expected ~{expected}",
            decoded.frames()
        );
        let measured = goertzel_magnitude(&decoded.channels()[0], 997.0, 44_100.0);
        assert!(
            (measured - 0.5).abs() < 0.1,
            "tone {measured}, expected ~0.5"
        );
        assert!(!decoded.has_non_finite());
    }

    #[test]
    fn missing_files_report_io_errors() {
        let error =
            decode_audio("/nonexistent/qinari-audio-test.wav").expect_err("missing file must fail");
        assert!(matches!(error, AudioError::Io(_)), "got {error}");
    }

    #[test]
    fn garbage_files_fail_to_probe() {
        let path = temp_file("garbage.wav");
        std::fs::write(&path, b"this is definitely not an audio file").expect("write");
        let error = decode_audio(&path).expect_err("garbage must fail");
        assert!(matches!(error, AudioError::Decode(_)), "got {error}");
    }
}

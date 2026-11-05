//! Voice catalog: the two bundled Piper-class VITS voices with pinned
//! download metadata.
//!
//! Every `sha256` and `size_bytes` below was measured on the exact file the
//! pinned URL serves (downloaded once and hashed); the CLI's downloader
//! verifies against these constants before installing a voice. The English
//! voice is commercial-safe (CC BY 4.0); the Indonesian voice is
//! non-commercial (Lessac lineage) and the CLI gates its installation on
//! explicit license acceptance.

use crate::textnorm::Language;

/// A downloadable text-to-speech voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceEntry {
    /// Catalog id, e.g. `en_US-libritts-high`.
    pub id: &'static str,
    /// BCP-47 language family: `en` or `id`.
    pub language: &'static str,
    /// Piper quality tier: `high` or `medium`.
    pub quality: &'static str,
    /// Native synthesis sample rate in hertz (both bundled voices are
    /// 22 050 Hz).
    pub sample_rate: u32,
    /// Exact size of the `.onnx` model file in bytes.
    pub size_bytes: u64,
    /// Download URL of the `.onnx` model file.
    pub url: &'static str,
    /// Download URL of the `.onnx.json` voice configuration file.
    pub config_url: &'static str,
    /// SHA-256 of the `.onnx` model file (hex, lowercase).
    pub sha256: &'static str,
    /// Human-readable license name of the training dataset.
    pub license: &'static str,
    /// URL of the license text or model card describing it.
    pub license_url: &'static str,
    /// Whether the voice may only be used non-commercially.
    pub non_commercial: bool,
}

/// Measured (not invented) digest of `en_US-libritts-high.onnx` as served by
/// <https://huggingface.co/rhasspy/piper-voices> at tag `v1.0.0`.
const EN_LIBRITTS_SHA256: &str = "9127a559e11603f10b366d1a20ac7426826081dbc521de4c2420c57728d73f0f";

/// Measured digest of `id_ID-news_tts-medium.onnx`. The Indonesian voice is
/// only published on the repository's `main` branch (the `v1.0.0` tag does
/// not contain an `id` tree), so its URLs pin `main`.
const ID_NEWS_TTS_SHA256: &str = "ed8f02aa593f7af6b19acbdb8142e0da0dd72f46194eb33d38e0eb10a52597e8";

static VOICES: [VoiceEntry; 2] = [
    VoiceEntry {
        id: "en_US-libritts-high",
        language: "en",
        quality: "high",
        sample_rate: 22_050,
        size_bytes: 136_673_811,
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/libritts/high/en_US-libritts-high.onnx",
        config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/libritts/high/en_US-libritts-high.onnx.json",
        sha256: EN_LIBRITTS_SHA256,
        license: "CC BY 4.0",
        license_url: "https://creativecommons.org/licenses/by/4.0/",
        non_commercial: false,
    },
    VoiceEntry {
        id: "id_ID-news_tts-medium",
        language: "id",
        quality: "medium",
        sample_rate: 22_050,
        size_bytes: 62_950_044,
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/id/id_ID/news_tts/medium/id_ID-news_tts-medium.onnx",
        config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/id/id_ID/news_tts/medium/id_ID-news_tts-medium.onnx.json",
        sha256: ID_NEWS_TTS_SHA256,
        license: "Non-commercial (Lessac lineage)",
        license_url: "https://huggingface.co/rhasspy/piper-voices/tree/main/id/id_ID/news_tts/medium",
        non_commercial: true,
    },
];

/// Returns the full voice catalog.
pub fn voices() -> &'static [VoiceEntry] {
    &VOICES
}

/// Looks a voice up by its catalog id (e.g. `id_ID-news_tts-medium`).
pub fn find_voice(id: &str) -> Option<&'static VoiceEntry> {
    VOICES.iter().find(|voice| voice.id == id)
}

/// Returns the default voice for a language code (`en` or `id`).
///
/// This is the voice the CLI installs when the user does not pick one, and
/// the voice `engine auto` prefers for its language.
pub fn default_voice(language: &str) -> Option<&'static VoiceEntry> {
    VOICES.iter().find(|voice| voice.language == language)
}

/// Returns the default voice for a [`Language`].
pub fn default_voice_for(language: Language) -> Option<&'static VoiceEntry> {
    default_voice(language.as_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_both_languages_with_pinned_metadata() {
        let all = voices();
        assert_eq!(all.len(), 2);
        for voice in all {
            assert_eq!(voice.sample_rate, 22_050);
            assert!(std::path::Path::new(voice.url)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("onnx")));
            assert!(voice.config_url.ends_with(".onnx.json"));
            assert_eq!(voice.sha256.len(), 64);
            assert!(voice.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert_ne!(voice.license, "");
        }
        let en = find_voice("en_US-libritts-high").expect("en voice");
        assert_eq!(en.language, "en");
        assert_eq!(en.size_bytes, 136_673_811);
        assert!(!en.non_commercial);
        let id = find_voice("id_ID-news_tts-medium").expect("id voice");
        assert_eq!(id.language, "id");
        assert_eq!(id.size_bytes, 62_950_044);
        assert!(id.non_commercial);
    }

    #[test]
    fn lookup_helpers() {
        assert!(find_voice("does-not-exist").is_none());
        assert_eq!(
            default_voice("en").map(|v| v.id),
            Some("en_US-libritts-high")
        );
        assert_eq!(
            default_voice("id").map(|v| v.id),
            Some("id_ID-news_tts-medium")
        );
        assert!(default_voice("fr").is_none());
        assert_eq!(
            default_voice_for(Language::Id).map(|v| v.id),
            Some("id_ID-news_tts-medium")
        );
    }
}

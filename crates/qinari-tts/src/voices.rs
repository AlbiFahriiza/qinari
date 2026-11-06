//! Voice catalog v2: the ten bundled Supertonic 3 voices, the shared engine
//! bundle they are downloaded against, and their pinned digests.
//!
//! Every `sha256` and `size_bytes` below was measured on the exact bytes the
//! pinned URL serves (the `HuggingFace` repository
//! `supertone-oss-archive/supertonic-3` at revision `aafc6e…`); `qinari
//! setup` verifies against these constants before installing anything, and
//! the engine re-verifies the bundle when it loads. All ten voices share one
//! OpenRAIL-M license: commercial use is allowed, but the license carries
//! use restrictions, so installation is gated on explicit acceptance
//! (recorded once, shared by every Supertonic voice).

/// Which engine a catalog voice runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// The bundled local Supertonic 3 engine (ONNX Runtime, 44 100 Hz).
    Supertonic,
    /// A future `ElevenLabs` cloud engine (engine lane only; not
    /// installable).
    ElevenLabs,
    /// The OpenAI-compatible cloud engine (configured, not downloaded).
    OpenAiCompat,
}

impl EngineKind {
    /// The lowercase CLI spelling of this engine.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Supertonic => "supertonic",
            EngineKind::ElevenLabs => "elevenlabs",
            EngineKind::OpenAiCompat => "openai-compat",
        }
    }
}

/// How a voice's license constrains use, and how installation is gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseClass {
    /// Permissive (CC BY / Apache / MIT): auto-install, attribute in docs.
    Permissive,
    /// Use-restriction license (OpenRAIL-M): commercial use is fine, but
    /// the restrictions must be shown and explicitly accepted once.
    Restricted,
    /// Non-commercial only: shown and accepted like `Restricted`, with the
    /// non-commercial wording.
    NonCommercial,
}

/// One pinned file of the shared engine bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleFile {
    /// Path of the file inside the model directory (e.g. `onnx/tts.json`).
    pub rel_path: &'static str,
    /// Download URL of the file.
    pub url: &'static str,
    /// Exact size in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the file (hex, lowercase).
    pub sha256: &'static str,
}

/// The shared Supertonic 3 engine bundle: four ONNX graphs plus the two JSON
/// sidecars, downloaded once for all ten voices (~380 MiB total).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetBundle {
    /// The pinned files, in download order.
    pub files: &'static [BundleFile],
    /// Human-readable license name of the model.
    pub license: &'static str,
    /// URL of the license text.
    pub license_url: &'static str,
    /// License class controlling the installation gate.
    pub license_class: LicenseClass,
}

impl AssetBundle {
    /// Total download size of the bundle in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }
}

/// A downloadable text-to-speech voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceEntry {
    /// Catalog id, e.g. `supertonic-M1`.
    pub id: &'static str,
    /// The engine this voice runs on.
    pub engine: EngineKind,
    /// BCP-47 language family: `id`, `en`, or `multi` for the
    /// multilingual Supertonic voices (31 languages each).
    pub language: &'static str,
    /// Native synthesis sample rate in hertz.
    pub sample_rate: u32,
    /// Name of the voice-style preset (`M1`…`F5`); the style file is
    /// `<model-dir>/voice_styles/<style_name>.json`.
    pub style_name: &'static str,
    /// Download URL of the per-voice style JSON file.
    pub style_url: &'static str,
    /// Exact size of the style file in bytes.
    pub style_size_bytes: u64,
    /// SHA-256 of the style file (hex, lowercase).
    pub style_sha256: &'static str,
    /// The shared engine bundle this voice needs.
    pub bundle: &'static AssetBundle,
    /// Human-readable license name (the model's OpenRAIL-M).
    pub license: &'static str,
    /// URL of the license text.
    pub license_url: &'static str,
    /// License class controlling the installation gate.
    pub license_class: LicenseClass,
    /// Key under which license acceptance is recorded (shared by all
    /// Supertonic voices, since they carry the same license).
    pub license_id: &'static str,
}

/// Pinned revision of the `HuggingFace` repository every Supertonic asset
/// is downloaded from.
pub const HF_BASE: &str =
    "https://huggingface.co/supertone-oss-archive/supertonic-3/resolve/aafc6e32416a594460b32413efc49d7fe4ce6d46";

/// Builds the pinned download URL for one repository-relative path.
macro_rules! hf_url {
    ($path:literal) => {
        concat!(
            "https://huggingface.co/supertone-oss-archive/supertonic-3",
            "/resolve/aafc6e32416a594460b32413efc49d7fe4ce6d46",
            $path
        )
    };
}

/// The shared engine bundle: four ONNX graphs plus `tts.json` (model
/// configuration) and `unicode_indexer.json` (the tokenizer's
/// codepoint-to-unit table). Digests measured on the pinned revision.
static SUPERTONIC_BUNDLE: AssetBundle = AssetBundle {
    files: &[
        BundleFile {
            rel_path: "onnx/duration_predictor.onnx",
            url: hf_url!("/onnx/duration_predictor.onnx"),
            size_bytes: 3_700_147,
            sha256: "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db",
        },
        BundleFile {
            rel_path: "onnx/text_encoder.onnx",
            url: hf_url!("/onnx/text_encoder.onnx"),
            size_bytes: 36_416_150,
            sha256: "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff",
        },
        BundleFile {
            rel_path: "onnx/vector_estimator.onnx",
            url: hf_url!("/onnx/vector_estimator.onnx"),
            size_bytes: 256_534_781,
            sha256: "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c",
        },
        BundleFile {
            rel_path: "onnx/vocoder.onnx",
            url: hf_url!("/onnx/vocoder.onnx"),
            size_bytes: 101_424_195,
            sha256: "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba",
        },
        BundleFile {
            rel_path: "onnx/tts.json",
            url: hf_url!("/onnx/tts.json"),
            size_bytes: 8_253,
            sha256: "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09",
        },
        BundleFile {
            rel_path: "onnx/unicode_indexer.json",
            url: hf_url!("/onnx/unicode_indexer.json"),
            size_bytes: 277_676,
            sha256: "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f",
        },
    ],
    license: "OpenRAIL-M",
    license_url: "https://huggingface.co/supertone-oss-archive/supertonic-3",
    license_class: LicenseClass::Restricted,
};

/// One Supertonic voice: the preset name, the pinned style file digest, and
/// the shared bundle.
const fn supertonic_voice(
    id: &'static str,
    style_name: &'static str,
    style_url: &'static str,
    style_size_bytes: u64,
    style_sha256: &'static str,
) -> VoiceEntry {
    VoiceEntry {
        id,
        engine: EngineKind::Supertonic,
        language: "multi",
        sample_rate: 44_100,
        style_name,
        style_url,
        style_size_bytes,
        style_sha256,
        bundle: &SUPERTONIC_BUNDLE,
        license: "OpenRAIL-M",
        license_url: "https://huggingface.co/supertone-oss-archive/supertonic-3",
        license_class: LicenseClass::Restricted,
        license_id: "supertonic-openrail-m",
    }
}

static VOICES: [VoiceEntry; 10] = [
    supertonic_voice(
        "supertonic-M1",
        "M1",
        hf_url!("/voice_styles/M1.json"),
        291_748,
        "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b",
    ),
    supertonic_voice(
        "supertonic-M2",
        "M2",
        hf_url!("/voice_styles/M2.json"),
        292_055,
        "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50",
    ),
    supertonic_voice(
        "supertonic-M3",
        "M3",
        hf_url!("/voice_styles/M3.json"),
        290_198,
        "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b",
    ),
    supertonic_voice(
        "supertonic-M4",
        "M4",
        hf_url!("/voice_styles/M4.json"),
        291_522,
        "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303",
    ),
    supertonic_voice(
        "supertonic-M5",
        "M5",
        hf_url!("/voice_styles/M5.json"),
        291_469,
        "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2",
    ),
    supertonic_voice(
        "supertonic-F1",
        "F1",
        hf_url!("/voice_styles/F1.json"),
        292_046,
        "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2",
    ),
    supertonic_voice(
        "supertonic-F2",
        "F2",
        hf_url!("/voice_styles/F2.json"),
        292_423,
        "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6",
    ),
    supertonic_voice(
        "supertonic-F3",
        "F3",
        hf_url!("/voice_styles/F3.json"),
        290_794,
        "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab",
    ),
    supertonic_voice(
        "supertonic-F4",
        "F4",
        hf_url!("/voice_styles/F4.json"),
        291_808,
        "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b",
    ),
    supertonic_voice(
        "supertonic-F5",
        "F5",
        hf_url!("/voice_styles/F5.json"),
        291_479,
        "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d",
    ),
];

/// Returns the full voice catalog (the ten bundled Supertonic voices).
pub fn voices() -> &'static [VoiceEntry] {
    &VOICES
}

/// Looks a voice up by its catalog id (e.g. `supertonic-M1`).
pub fn find_voice(id: &str) -> Option<&'static VoiceEntry> {
    VOICES.iter().find(|voice| voice.id == id)
}

/// Returns the default voice for a language code.
///
/// Every bundled voice is multilingual, so the codes Qinari speaks (`en`,
/// `id`) and the catalog's own `multi` all resolve to the narrator default
/// `supertonic-M1` (the design window's measured pick); anything else gets
/// no voice.
pub fn default_voice(language: &str) -> Option<&'static VoiceEntry> {
    let exact = VOICES.iter().find(|voice| voice.language == language);
    exact.or_else(|| {
        crate::textnorm::Language::from_code(language)?;
        VOICES.first()
    })
}

/// Returns the default voice for a [`Language`].
pub fn default_voice_for(language: crate::textnorm::Language) -> Option<&'static VoiceEntry> {
    default_voice(language.as_code())
}

/// Looks up a Supertonic voice by style preset name (`M1`…`F5`) or by full
/// catalog id (`supertonic-M1`), for style-preset requests.
pub fn find_by_style_name(name: &str) -> Option<&'static VoiceEntry> {
    let bare = name.trim_start_matches("supertonic-");
    VOICES.iter().find(|voice| voice.style_name == bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_ten_supertonic_voices_with_pinned_metadata() {
        let all = voices();
        assert_eq!(all.len(), 10);
        let expected: [&str; 10] = [
            "supertonic-M1",
            "supertonic-M2",
            "supertonic-M3",
            "supertonic-M4",
            "supertonic-M5",
            "supertonic-F1",
            "supertonic-F2",
            "supertonic-F3",
            "supertonic-F4",
            "supertonic-F5",
        ];
        for (voice, id) in all.iter().zip(expected) {
            assert_eq!(voice.id, id);
            assert_eq!(voice.engine, EngineKind::Supertonic);
            assert_eq!(voice.language, "multi");
            assert_eq!(voice.sample_rate, 44_100);
            assert_eq!(voice.license_class, LicenseClass::Restricted);
            assert_eq!(voice.style_name, &id["supertonic-".len()..]);
            // Style pins: 64 lowercase hex chars, plausible JSON size.
            assert_eq!(voice.style_sha256.len(), 64);
            assert!(voice.style_sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(
                (280_000..300_000).contains(&voice.style_size_bytes),
                "{} style size {}",
                voice.id,
                voice.style_size_bytes
            );
            assert!(voice
                .style_url
                .ends_with(&format!("/voice_styles/{}.json", voice.style_name)));
            assert!(voice
                .style_url
                .contains("aafc6e32416a594460b32413efc49d7fe4ce6d46"));
        }
    }

    #[test]
    fn bundle_pins_six_files_with_pinned_revision_urls() {
        let bundle = find_voice("supertonic-M1").expect("M1").bundle;
        assert_eq!(bundle.files.len(), 6);
        let mut total = 0;
        for file in bundle.files {
            assert!(file.rel_path.starts_with("onnx/"));
            assert!(file.url.starts_with(
                "https://huggingface.co/supertone-oss-archive/supertonic-3/resolve/aafc6e32416a594460b32413efc49d7fe4ce6d46/"
            ));
            assert_eq!(file.sha256.len(), 64);
            assert!(file.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            total += file.size_bytes;
        }
        assert_eq!(bundle.size_bytes(), total);
        // The ~398 MB bundle from the design doc (379.9 MiB).
        assert_eq!(total, 398_361_202);
        assert_eq!(bundle.license_class, LicenseClass::Restricted);
    }

    #[test]
    fn lookup_helpers() {
        assert!(find_voice("does-not-exist").is_none());
        assert!(find_voice("supertonic-F3").is_some());
        // Every supported language code resolves to the narrator default.
        assert_eq!(default_voice("en").map(|v| v.id), Some("supertonic-M1"));
        assert_eq!(default_voice("id").map(|v| v.id), Some("supertonic-M1"));
        assert_eq!(default_voice("multi").map(|v| v.id), Some("supertonic-M1"));
        assert!(default_voice("zz").is_none());
        assert_eq!(
            default_voice_for(crate::textnorm::Language::Id).map(|v| v.id),
            Some("supertonic-M1")
        );
        // Style lookup accepts both spellings.
        assert_eq!(
            find_by_style_name("F2").map(|v| v.id),
            Some("supertonic-F2")
        );
        assert_eq!(
            find_by_style_name("supertonic-F2").map(|v| v.id),
            Some("supertonic-F2")
        );
        assert!(find_by_style_name("Z9").is_none());
    }
}

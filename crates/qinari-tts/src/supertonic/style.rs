//! Voice styles: the per-voice JSON files that steer Supertonic's timbre
//! and pacing (`voice_styles/M1.json` … `F5.json`).
//!
//! A style file holds `style_ttl` (the text-to-latent style, shape
//! `1×50×256`) and `style_dp` (the duration-predictor style, `1×8×16`),
//! each as `{dims, data}` plus a metadata block. Both tensors are kept as
//! flat row-major `Vec<f32>` plus their shapes, exactly what the ONNX
//! graphs take.

use crate::TtsError;
use serde::Deserialize;
use std::path::Path;

/// One `{dims, data}` tensor section of a style file.
#[derive(Debug, Deserialize)]
struct StyleComponent {
    dims: Vec<usize>,
    data: serde_json::Value,
}

/// The parsed body of a voice-style JSON file.
#[derive(Debug, Deserialize)]
struct StyleFile {
    style_ttl: StyleComponent,
    style_dp: StyleComponent,
}

/// A loaded voice style: the two conditioning tensors.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    ttl_dims: Vec<usize>,
    ttl: Vec<f32>,
    dp_dims: Vec<usize>,
    dp: Vec<f32>,
}

impl Style {
    /// Parses a style from its JSON text.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::Style`] when the JSON is malformed, the
    /// `style_ttl`/`style_dp` sections are missing, the dims are not
    /// three-dimensional, or the data does not match the dims.
    pub fn from_json(text: &str) -> Result<Self, TtsError> {
        let bad = |detail: String| TtsError::Style { detail };
        let file: StyleFile = serde_json::from_str(text)
            .map_err(|error| bad(format!("invalid style JSON: {error}")))?;
        let ttl = component_to_flat(&file.style_ttl, "style_ttl")?;
        let dp = component_to_flat(&file.style_dp, "style_dp")?;
        Ok(Self {
            ttl_dims: file.style_ttl.dims,
            ttl,
            dp_dims: file.style_dp.dims,
            dp,
        })
    }

    /// Loads a style from `path`.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::Style`] when the file cannot be read or does
    /// not parse (see [`Style::from_json`]).
    pub fn load(path: &Path) -> Result<Self, TtsError> {
        let text = std::fs::read_to_string(path).map_err(|error| TtsError::Style {
            detail: format!("could not read {}: {error}", path.display()),
        })?;
        Self::from_json(&text)
    }

    /// The `style_ttl` tensor shape (row-major).
    #[must_use]
    pub fn ttl_dims(&self) -> &[usize] {
        &self.ttl_dims
    }

    /// The flat `style_ttl` data (row-major, `ttl_dims` shaped).
    #[must_use]
    pub fn ttl_data(&self) -> &[f32] {
        &self.ttl
    }

    /// The `style_dp` tensor shape (row-major).
    #[must_use]
    pub fn dp_dims(&self) -> &[usize] {
        &self.dp_dims
    }

    /// The flat `style_dp` data (row-major, `dp_dims` shaped).
    #[must_use]
    pub fn dp_data(&self) -> &[f32] {
        &self.dp
    }
}

/// Flattens one `{dims, data}` section, validating shape and extent.
fn component_to_flat(component: &StyleComponent, name: &str) -> Result<Vec<f32>, TtsError> {
    let bad = |detail: String| TtsError::Style { detail };
    if component.dims.len() != 3 || component.dims.contains(&0) {
        return Err(bad(format!(
            "{name} dims {:?} are not a non-empty 3-D tensor",
            component.dims
        )));
    }
    let expected: usize = component.dims.iter().product();
    let data = component
        .data
        .as_array()
        .ok_or_else(|| bad(format!("{name} data is not a nested array")))?;
    let mut flat = Vec::with_capacity(expected);
    if !flatten_nested(data, &mut flat) {
        return Err(bad(format!("{name} data is not a nested numeric array")));
    }
    if flat.len() != expected {
        return Err(bad(format!(
            "{name} has {} values but dims {:?} need {expected}",
            flat.len(),
            component.dims
        )));
    }
    Ok(flat)
}

/// Recursively flattens nested JSON arrays of numbers into `out`.
/// Returns `false` on the first non-numeric leaf.
fn flatten_nested(data: &[serde_json::Value], out: &mut Vec<f32>) -> bool {
    for value in data {
        match value {
            serde_json::Value::Number(n) => {
                let Ok(parsed) = n.as_f64().map(|v| v as f32).ok_or(()) else {
                    return false;
                };
                out.push(parsed);
            }
            serde_json::Value::Array(inner) => {
                if !flatten_nested(inner, out) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal style file with tiny tensors (the real ones are
    /// `1×50×256` and `1×8×16`).
    fn style_json() -> String {
        r#"{
            "style_ttl": {"dims": [1, 2, 3], "type": "float32",
                          "data": [[[0.5, 0.25, 0.0], [1.5, 2.5, 3.5]]]},
            "style_dp":  {"dims": [1, 2, 2], "type": "float32",
                          "data": [[[0.1, 0.2], [0.3, 0.4]]]},
            "metadata": {"speaker": "test"}
        }"#
        .to_string()
    }

    #[test]
    fn parses_dims_and_flat_data() {
        let style = Style::from_json(&style_json()).expect("valid style");
        assert_eq!(style.ttl_dims(), &[1, 2, 3]);
        assert_eq!(style.ttl_data(), &[0.5, 0.25, 0.0, 1.5, 2.5, 3.5]);
        assert_eq!(style.dp_dims(), &[1, 2, 2]);
        assert_eq!(style.dp_data(), &[0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn rejects_malformed_styles() {
        // Not JSON at all.
        assert!(Style::from_json("nope").is_err());
        // Missing section.
        assert!(Style::from_json(r#"{"style_ttl": {"dims":[1,1,1],"data":[[[1]]]}}"#).is_err());
        // Data/dims mismatch.
        let short = style_json().replace("0.5, 0.25, 0.0", "0.5, 0.25");
        assert!(Style::from_json(&short).is_err());
        // Wrong rank.
        let flat = r#"{"style_ttl": {"dims":[6], "data":[1,2,3,4,5,6]},
                        "style_dp": {"dims":[1,2,2], "data":[[[1,2],[3,4]]]}}"#;
        assert!(Style::from_json(flat).is_err());
        // Non-numeric leaf.
        let text = style_json().replace("3.5", "\"x\"");
        assert!(Style::from_json(&text).is_err());
        // Missing file.
        assert!(Style::load(Path::new("/nonexistent/M1.json")).is_err());
    }
}

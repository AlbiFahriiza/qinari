//! Golden-target machinery: tolerance files and the verification gate.
//!
//! A target file is JSON with one `{ "min": .., "max": .. }` tolerance per
//! dotted feature path (the paths are exactly
//! [`FeatureReport::flatten`](crate::report::FeatureReport::flatten)'s
//! keys; `null` bounds are unbounded on that side):
//!
//! ```json
//! {
//!   "name": "rain",
//!   "description": "Golden targets for the seeded rain seed set",
//!   "tolerances": {
//!     "spectrum.rolloff95_hz": { "min": 5000, "max": 9500 },
//!     "events.interval_cv": { "min": 0.4, "max": 1.6 }
//!   }
//! }
//! ```
//!
//! [`check_against`] returns one [`Violation`] per out-of-range or missing
//! value; an empty vector means the report passes the target. Targets are
//! authored from `qinari analyze` output, reviewed, and committed under
//! `tests/golden/`.

use crate::report::FeatureReport;
use crate::VerifyError;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One-sided-or-two-sided tolerance for a single feature.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, serde::Serialize)]
pub struct Tolerance {
    /// Lower bound (`null` = unbounded below).
    pub min: Option<f64>,
    /// Upper bound (`null` = unbounded above).
    pub max: Option<f64>,
}

impl Tolerance {
    /// Where `value` sits with respect to the tolerance.
    fn check(&self, value: f64) -> bool {
        if let Some(min) = self.min {
            if value < min {
                return false;
            }
        }
        if let Some(max) = self.max {
            if value > max {
                return false;
            }
        }
        true
    }
}

/// A parsed golden-target file.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct TargetFile {
    /// Target name (the recipe or fixture it belongs to).
    pub name: String,
    /// Free-form documentation of what the target gates.
    pub description: Option<String>,
    /// Tolerances by dotted feature path.
    pub tolerances: BTreeMap<String, Tolerance>,
}

/// One failed gate.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum Violation {
    /// A measured value outside its tolerance.
    OutOfRange {
        /// The feature path.
        path: String,
        /// The measured value.
        value: f64,
        /// The lower bound (`null` if unbounded).
        min: Option<f64>,
        /// The upper bound (`null` if unbounded).
        max: Option<f64>,
    },
    /// A feature the report does not carry (e.g. no events were detected)
    /// while the target expects a bounded value.
    Missing {
        /// The feature path.
        path: String,
        /// The lower bound that could not be applied.
        min: Option<f64>,
        /// The upper bound that could not be applied.
        max: Option<f64>,
    },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::OutOfRange {
                path,
                value,
                min,
                max,
            } => {
                write!(f, "{path} = {value} outside [{min:?}, {max:?}]")
            }
            Violation::Missing { path, min, max } => {
                write!(
                    f,
                    "{path} is missing (no value to compare to [{min:?}, {max:?}])"
                )
            }
        }
    }
}

/// Loads a golden-target file.
///
/// # Errors
///
/// Returns [`VerifyError::TargetParse`] when the file cannot be read or
/// does not parse as a [`TargetFile`].
pub fn load_target<P: AsRef<Path>>(path: P) -> Result<TargetFile, VerifyError> {
    let text = std::fs::read_to_string(path)?;
    let target: TargetFile = serde_json::from_str(&text)?;
    Ok(target)
}

/// Checks a report against a target.
///
/// Every tolerance entry must name a feature the report flattens;
/// unknown paths are an error, not a silent pass. A bounded tolerance on
/// a feature the report carries as `null` (missing statistic) is a
/// [`Violation::Missing`].
///
/// # Errors
///
/// Returns [`VerifyError::UnknownFeature`] when a tolerance names a path
/// that does not exist in the report's flattened form.
pub fn check_against(
    report: &FeatureReport,
    target: &TargetFile,
) -> Result<Vec<Violation>, VerifyError> {
    // The flattened paths must outlive the map: bind the vec first.
    let flat = report.flatten();
    let values: std::collections::HashMap<&str, Option<f64>> = flat
        .iter()
        .map(|(path, value)| (path.as_str(), *value))
        .collect();
    let mut violations = Vec::new();
    for (path, tolerance) in &target.tolerances {
        let Some(&value) = values.get(path.as_str()) else {
            return Err(VerifyError::UnknownFeature { path: path.clone() });
        };
        match value {
            Some(v) => {
                if !tolerance.check(v) {
                    violations.push(Violation::OutOfRange {
                        path: path.clone(),
                        value: v,
                        min: tolerance.min,
                        max: tolerance.max,
                    });
                }
            }
            None => {
                if tolerance.min.is_some() || tolerance.max.is_some() {
                    violations.push(Violation::Missing {
                        path: path.clone(),
                        min: tolerance.min,
                        max: tolerance.max,
                    });
                }
            }
        }
    }
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_target() -> TargetFile {
        serde_json::from_str(
            r#"{
                "name": "sample",
                "description": null,
                "tolerances": {
                    "spectrum.rolloff95_hz": { "min": 100.0, "max": 1000.0 },
                    "events.count": { "min": null, "max": 5.0 },
                    "speech.f0.f0_mean_hz": { "min": null, "max": null }
                }
            }"#,
        )
        .expect("valid target")
    }

    #[test]
    fn tolerance_bounds_are_applied_inclusively() {
        let t = Tolerance {
            min: Some(1.0),
            max: Some(3.0),
        };
        assert!(t.check(1.0) && t.check(2.0) && t.check(3.0));
        assert!(!t.check(0.999) && !t.check(3.001));
        let open = Tolerance {
            min: None,
            max: Some(0.0),
        };
        assert!(open.check(-1e9));
        assert!(!open.check(0.1));
    }

    #[test]
    fn target_files_round_trip() {
        let target = sample_target();
        assert_eq!(target.name, "sample");
        assert_eq!(target.tolerances.len(), 3);
        let text = serde_json::to_string(&target).expect("serializes");
        let back: TargetFile = serde_json::from_str(&text).expect("parses");
        assert_eq!(back, target);
    }

    #[test]
    fn violations_display_their_paths() {
        let v = Violation::OutOfRange {
            path: "events.rate_hz".into(),
            value: 9.5,
            min: Some(0.0),
            max: Some(2.0),
        };
        let text = v.to_string();
        assert!(text.contains("events.rate_hz"));
        assert!(text.contains("9.5"));
        let m = Violation::Missing {
            path: "speech.f0.f0_mean_hz".into(),
            min: Some(0.0),
            max: None,
        };
        assert!(m.to_string().contains("missing"));
    }
}

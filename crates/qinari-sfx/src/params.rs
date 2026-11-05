//! Parameter specifications and validated parameter sets.
//!
//! Recipes declare their tunables as [`ParamSpec`] tables (built with the
//! [`spec`] macro). At render time the caller's `NAME=VALUE` pairs are
//! checked against the table by [`ParamSet::new`]: unknown names get a
//! "did you mean" suggestion, out-of-range values get the allowed range.
//! Recipes then read values with [`ParamSet::get`], which falls back to
//! the declared default when the caller omitted a parameter.

use strsim::levenshtein;
use thiserror::Error;

/// Errors raised while validating caller-supplied parameters against a
/// recipe's declared [`ParamSpec`] table.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ParamError {
    /// A parameter name is not declared for the recipe.
    ///
    /// `suggestion` is a pre-formatted clause: `; did you mean "x"?` when a
    /// close declared name exists, or ` (this recipe takes no parameters)`
    /// when the recipe declares none at all.
    #[error("unknown parameter {name:?} for recipe {recipe:?}{suggestion}")]
    Unknown {
        /// The recipe the caller tried to set a parameter on.
        recipe: String,
        /// The name the caller used.
        name: String,
        /// Formatted hint clause appended to the message.
        suggestion: String,
    },
    /// A value lies outside the declared range of the parameter.
    #[error("parameter {name:?} for recipe {recipe:?} is out of range ({min}..{max})")]
    OutOfRange {
        /// The recipe the caller tried to set a parameter on.
        recipe: String,
        /// The name the caller used.
        name: String,
        /// The declared minimum (inclusive).
        min: f64,
        /// The declared maximum (inclusive).
        max: f64,
    },
}

/// Formats the hint clause for an undeclared parameter name.
///
/// With no declared names the clause explains the recipe takes no
/// parameters; otherwise the Levenshtein-closest declared name wins
/// (ties go to the first table entry).
fn suggestion_clause(candidates: &[&str], name: &str) -> String {
    if candidates.is_empty() {
        return " (this recipe takes no parameters)".to_string();
    }
    let mut best = candidates[0];
    let mut best_distance = levenshtein(name, best);
    for &candidate in &candidates[1..] {
        let distance = levenshtein(name, candidate);
        if distance < best_distance || (distance == best_distance && candidate < best) {
            best = candidate;
            best_distance = distance;
        }
    }
    format!("; did you mean {best:?}?")
}

/// The numeric flavour of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Continuous value.
    Float,
    /// Whole-number value; [`ParamSet::get`] rounds to the nearest integer.
    Int,
}

/// One declared recipe parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Parameter name as the caller writes it (e.g. `intensity`).
    pub name: &'static str,
    /// Numeric flavour.
    pub kind: ParamKind,
    /// Value used when the caller omits the parameter.
    pub default: f64,
    /// Smallest accepted value (inclusive).
    pub min: f64,
    /// Largest accepted value (inclusive).
    pub max: f64,
    /// One-line description shown by listings.
    pub doc: &'static str,
}

/// Builds a [`ParamSpec`] from compact fields.
///
/// The first argument becomes the parameter name (an identifier, written
/// by callers verbatim), the second the numeric flavour. Example:
/// `spec!(intensity, Float, 0.5, 0.0, 1.0, "Rain density.")`.
macro_rules! spec {
    ($name:ident, $kind:ident, $default:expr, $min:expr, $max:expr, $doc:expr) => {
        $crate::params::ParamSpec {
            name: stringify!($name),
            kind: $crate::params::ParamKind::$kind,
            default: $default,
            min: $min,
            max: $max,
            doc: $doc,
        }
    };
}

pub(crate) use spec;

/// A validated set of parameter values for one render.
#[derive(Debug, Clone)]
pub struct ParamSet {
    specs: &'static [ParamSpec],
    values: Vec<(String, f64)>,
}

impl ParamSet {
    /// Validates `values` against `specs`.
    ///
    /// Every name must be declared for the recipe and every value must lie
    /// within the declared range (inclusive); NaN fails every range. When
    /// several pairs share a name the first one wins in [`ParamSet::get`].
    ///
    /// # Errors
    ///
    /// Returns [`ParamError::Unknown`] for an undeclared name and
    /// [`ParamError::OutOfRange`] for a value outside its range.
    pub fn new(
        specs: &'static [ParamSpec],
        recipe: &'static str,
        values: &[(String, f64)],
    ) -> Result<Self, ParamError> {
        for (name, value) in values {
            let Some(spec) = specs.iter().find(|s| s.name == name.as_str()) else {
                let names: Vec<&str> = specs.iter().map(|s| s.name).collect();
                return Err(ParamError::Unknown {
                    recipe: recipe.to_string(),
                    name: name.clone(),
                    suggestion: suggestion_clause(&names, name),
                });
            };
            if !(*value >= spec.min && *value <= spec.max) {
                return Err(ParamError::OutOfRange {
                    recipe: recipe.to_string(),
                    name: name.clone(),
                    min: spec.min,
                    max: spec.max,
                });
            }
        }
        Ok(Self {
            specs,
            values: values.to_vec(),
        })
    }

    /// Returns the value for `name`.
    ///
    /// When the caller omitted the parameter the declared default is used.
    /// `Int` parameters come back rounded to the nearest whole number.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not declared by the spec table. Recipes ask
    /// only for names they declare, so this is a recipe bug, never caller
    /// input.
    pub fn get(&self, name: &str) -> f64 {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("parameter {name:?} is not declared by this recipe"));
        for (given, value) in &self.values {
            if given == name {
                return Self::flavour(spec, *value);
            }
        }
        Self::flavour(spec, spec.default)
    }

    /// Applies the spec's numeric flavour to a raw value.
    fn flavour(spec: &ParamSpec, value: f64) -> f64 {
        if spec.kind == ParamKind::Int {
            value.round()
        } else {
            value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[ParamSpec] = &[
        spec!(intensity, Float, 0.5, 0.0, 1.0, "Density."),
        spec!(count, Int, 4.0, 1.0, 9.0, "How many."),
    ];

    #[test]
    fn get_returns_caller_value_default_and_int_rounding() {
        let values = vec![("intensity".to_string(), 0.9), ("count".to_string(), 2.6)];
        let set = ParamSet::new(SPECS, "demo", &values).unwrap();
        assert!((set.get("intensity") - 0.9).abs() < 1e-12, "caller value");
        assert_eq!(set.get("count"), 3.0, "Int value rounds to nearest");
        assert!((set.get("intensity") - set.get("intensity")).abs() < 1e-12);

        let empty = ParamSet::new(SPECS, "demo", &[]).unwrap();
        assert!(
            (empty.get("intensity") - 0.5).abs() < 1e-12,
            "default fallback"
        );
        assert_eq!(empty.get("count"), 4.0, "Int default rounds too");
    }

    #[test]
    fn first_duplicate_name_wins() {
        let values = vec![
            ("intensity".to_string(), 0.2),
            ("intensity".to_string(), 0.8),
        ];
        let set = ParamSet::new(SPECS, "demo", &values).unwrap();
        assert!((set.get("intensity") - 0.2).abs() < 1e-12);
    }

    #[test]
    fn unknown_name_suggests_closest_declared() {
        let values = vec![("intensit".to_string(), 0.5)];
        let err = ParamSet::new(SPECS, "demo", &values).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("did you mean"), "{message}");
        assert!(message.contains("intensity"), "{message}");
        assert!(message.contains("demo"), "{message}");
        assert!(matches!(err, ParamError::Unknown { .. }));
    }

    #[test]
    fn unknown_name_on_zero_param_recipe_says_so() {
        let err = ParamSet::new(&[], "demo", &[("anything".to_string(), 1.0)]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("takes no parameters"), "{message}");
    }

    #[test]
    fn out_of_range_shows_the_range() {
        for bad in [1.5, -0.25, f64::NAN] {
            let values = vec![("intensity".to_string(), bad)];
            let err = ParamSet::new(SPECS, "demo", &values).unwrap_err();
            let message = err.to_string();
            assert!(message.contains("out of range (0..1)"), "{bad}: {message}");
        }
        let err = ParamSet::new(SPECS, "demo", &[("count".to_string(), 10.0)]).unwrap_err();
        assert!(err.to_string().contains("out of range (1..9)"));
    }

    #[test]
    fn range_boundaries_are_inclusive() {
        let edges = vec![("intensity".to_string(), 0.0), ("count".to_string(), 9.0)];
        assert!(ParamSet::new(SPECS, "demo", &edges).is_ok());
    }

    #[test]
    fn suggestion_picks_nearest_then_alphabetical() {
        // "coun" is distance 1 from "count" and far from "intensity".
        let clause = suggestion_clause(&["intensity", "count"], "coun");
        assert!(clause.contains("count"), "{clause}");
        // Equidistant names resolve to the alphabetically smaller one.
        let clause = suggestion_clause(&["bbb", "aaa"], "ccc");
        assert!(clause.contains("aaa"), "{clause}");
        assert!(suggestion_clause(&[], "zzz").contains("takes no parameters"));
    }
}

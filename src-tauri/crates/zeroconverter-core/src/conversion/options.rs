//! What the user can set for an output format, declared by the engine.
//!
//! Each family says, per target format, which options its encoder actually
//! honours ([`crate::conversion::traits::ConverterHandler::output_options`]).
//! The app's options panel, the `+` window and the command line's
//! `capabilities` are all built from these lists, so a setting is shown
//! exactly where it does something and with the engine's own default.
//!
//! A spec describes an option; it does not check one. Each family still reads
//! and checks its options when it validates a request (`ImageOptions`,
//! `AudioOptions::parse`, video quality), and the tests next to every family
//! keep the two in step: each default and preset validates, each bound is
//! enforced.
use serde::ser::{Serialize, SerializeMap, Serializer};

/// A value an option can take. `Unset` leaves the choice to the engine
/// (keep the source's sample rate, pick a bitrate for the channel count, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionValue {
    Int(i64),
    Text(&'static str),
    Bool(bool),
    Unset,
}

impl Serialize for OptionValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Int(v) => s.serialize_i64(*v),
            Self::Text(v) => s.serialize_str(v),
            Self::Bool(v) => s.serialize_bool(*v),
            Self::Unset => s.serialize_none(),
        }
    }
}

impl OptionValue {
    pub fn to_json(self) -> serde_json::Value {
        match self {
            Self::Int(v) => v.into(),
            Self::Text(v) => v.into(),
            Self::Bool(v) => v.into(),
            Self::Unset => serde_json::Value::Null,
        }
    }
}

/// How the value is entered. The UI picks a control from this; the command
/// line reports it as a type and bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// A whole number in `min..=max`, best shown as a slider (quality).
    Slider { min: i64, max: i64 },
    /// A whole number in `min..=max`. With `presets`, the UI offers those as
    /// a list (bitrates, sample rates); the engine accepts any value in range.
    Number {
        min: i64,
        max: i64,
        presets: &'static [i64],
    },
    /// Exactly one of `values`.
    Choice { values: &'static [OptionValue] },
    /// On or off.
    Toggle,
}

/// Which outputs share one remembered value in the UI. `Family`: every image
/// target uses the same resize; `Format`: MP3 and Opus each keep a bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Family,
    Format,
}

/// Show an option only while another one has one of these values
/// (resize width only for "max width" and "fit inside box").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShowWhen {
    pub key: &'static str,
    pub values: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    /// The request key, as the family reads it (`quality`, `bitrate_kbps`).
    pub key: &'static str,
    pub control: Control,
    /// What the engine does when the key is absent.
    pub default: OptionValue,
    /// `kbps`, `hz`, `px`; `None` for plain numbers and choices.
    pub unit: Option<&'static str>,
    pub scope: Scope,
    /// Also offered in the small `+` window, not only in the main panel.
    pub quick: bool,
    pub show_when: Option<ShowWhen>,
    /// One line for the command line's `capabilities` (English).
    pub note: Option<&'static str>,
}

impl OptionSpec {
    /// The JSON type a request uses for this key.
    pub fn json_type(&self) -> &'static str {
        match self.control {
            Control::Slider { .. } | Control::Number { .. } => "integer",
            Control::Toggle => "boolean",
            Control::Choice { values } => match values.iter().find(|v| **v != OptionValue::Unset) {
                Some(OptionValue::Text(_)) => "string",
                Some(OptionValue::Bool(_)) => "boolean",
                _ => "integer",
            },
        }
    }

    /// Every value the UI may offer: the default, the presets or choices, and
    /// the bounds. Used by the tests that keep specs and validation in step.
    pub fn offered_values(&self) -> Vec<OptionValue> {
        let mut out = vec![self.default];
        match self.control {
            Control::Slider { min, max } => {
                out.extend([OptionValue::Int(min), OptionValue::Int(max)])
            }
            Control::Number { min, max, presets } => {
                out.extend([OptionValue::Int(min), OptionValue::Int(max)]);
                out.extend(presets.iter().map(|p| OptionValue::Int(*p)));
            }
            Control::Choice { values } => out.extend_from_slice(values),
            Control::Toggle => out.extend([OptionValue::Bool(false), OptionValue::Bool(true)]),
        }
        out
    }

    /// Values just outside the declared bounds, which validation must refuse.
    pub fn out_of_range_values(&self) -> Vec<OptionValue> {
        match self.control {
            Control::Slider { min, max } | Control::Number { min, max, .. } => {
                vec![OptionValue::Int(min - 1), OptionValue::Int(max + 1)]
            }
            Control::Choice { values } => {
                if values.iter().any(|v| matches!(v, OptionValue::Text(_))) {
                    vec![OptionValue::Text("not-a-choice")]
                } else {
                    let ints = values.iter().filter_map(|v| match v {
                        OptionValue::Int(i) => Some(*i),
                        _ => None,
                    });
                    ints.max()
                        .map(|m| OptionValue::Int(m + 1))
                        .into_iter()
                        .collect()
                }
            }
            Control::Toggle => Vec::new(),
        }
    }
}

/// The shape the app and the command line read:
/// `{"key":"quality","control":"slider","min":1,"max":100,"default":90,...}`.
impl Serialize for OptionSpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("key", self.key)?;
        m.serialize_entry("type", self.json_type())?;
        match self.control {
            Control::Slider { min, max } => {
                m.serialize_entry("control", "slider")?;
                m.serialize_entry("min", &min)?;
                m.serialize_entry("max", &max)?;
            }
            Control::Number { min, max, presets } => {
                m.serialize_entry("control", "number")?;
                m.serialize_entry("min", &min)?;
                m.serialize_entry("max", &max)?;
                m.serialize_entry("presets", presets)?;
            }
            Control::Choice { values } => {
                m.serialize_entry("control", "choice")?;
                m.serialize_entry("values", values)?;
            }
            Control::Toggle => m.serialize_entry("control", "toggle")?,
        }
        m.serialize_entry("default", &self.default)?;
        if let Some(unit) = self.unit {
            m.serialize_entry("unit", unit)?;
        }
        m.serialize_entry(
            "scope",
            match self.scope {
                Scope::Family => "family",
                Scope::Format => "format",
            },
        )?;
        m.serialize_entry("quick", &self.quick)?;
        if let Some(when) = self.show_when {
            m.serialize_entry(
                "show_when",
                &serde_json::json!({ "key": when.key, "values": when.values }),
            )?;
        }
        if let Some(note) = self.note {
            m.serialize_entry("note", note)?;
        }
        m.end()
    }
}

#[cfg(test)]
pub(crate) mod spec_tests {
    //! Shared checks that a family's specs and its validation agree.
    use super::*;
    use crate::app_error::AppError;

    /// For every spec of `target`: each offered value validates, each value
    /// past a bound is refused. `validate` gets the options map for one key.
    pub fn specs_agree_with_validation(
        target: &str,
        specs: &[&OptionSpec],
        validate: &dyn Fn(serde_json::Value) -> Result<(), AppError>,
    ) {
        // Optional numbers another option switches on (resize width and
        // height) get their minimum, so "max width" is complete on its own.
        let mut filled = serde_json::Map::new();
        for spec in specs {
            if let (OptionValue::Unset, Control::Number { min, .. }) = (spec.default, spec.control)
            {
                filled.insert(spec.key.into(), min.into());
            }
        }
        for spec in specs {
            let with = |value: OptionValue| {
                let mut map = filled.clone();
                if let Some(when) = spec.show_when {
                    map.insert(when.key.into(), when.values[0].into());
                }
                map.insert(spec.key.into(), value.to_json());
                serde_json::Value::Object(map)
            };
            for value in spec.offered_values() {
                if value == OptionValue::Unset && spec.show_when.is_some() {
                    continue; // required once its switch is on
                }
                assert!(
                    validate(with(value)).is_ok(),
                    "{target}: {} = {value:?} is offered but refused",
                    spec.key
                );
            }
            for value in spec.out_of_range_values() {
                assert!(
                    validate(with(value)).is_err(),
                    "{target}: {} = {value:?} is outside the spec but accepted",
                    spec.key
                );
            }
        }
    }

    #[test]
    fn spec_serializes_to_the_documented_shape() {
        const SPEC: OptionSpec = OptionSpec {
            key: "bitrate_kbps",
            control: Control::Number {
                min: 32,
                max: 320,
                presets: &[128, 320],
            },
            default: OptionValue::Int(320),
            unit: Some("kbps"),
            scope: Scope::Format,
            quick: true,
            show_when: None,
            note: None,
        };
        assert_eq!(
            serde_json::to_value(SPEC).unwrap(),
            serde_json::json!({
                "key": "bitrate_kbps", "type": "integer", "control": "number",
                "min": 32, "max": 320, "presets": [128, 320], "default": 320,
                "unit": "kbps", "scope": "format", "quick": true
            })
        );
    }
}

//! The option schema exported from OrcaSlicer's `print_config_def`.
//!
//! Profiles store every value as a string; the type, default, limits and GUI metadata of an
//! option only exist in OrcaSlicer's C++ sources. `Snapmaker_Orca_schema_export` (in the
//! OrcaSlicer repository, `src/dev-utils/`) dumps them to `schema.json`, and a copy generated
//! from the OrcaSlicer commit the regression tests target is bundled with this crate.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::{fmt, fs, io};

use indexmap::IndexMap;
use serde::Deserialize;

use crate::model::{PresetKind, RawValue};

/// The `format_version` this crate understands.
pub const SCHEMA_FORMAT_VERSION: u32 = 1;

const BUNDLED_SCHEMA: &str = include_str!("../schema/schema.json");

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("cannot read schema: {0}")]
    Io(#[from] io::Error),
    #[error("invalid schema: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("schema format version {0} is not supported (expected {SCHEMA_FORMAT_VERSION})")]
    UnsupportedVersion(u32),
}

/// `ConfigOptionType` without the `co` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionType {
    Float,
    Floats,
    Int,
    Ints,
    String,
    Strings,
    Percent,
    Percents,
    FloatOrPercent,
    FloatsOrPercents,
    Point,
    Points,
    Point3,
    Bool,
    Bools,
    Enum,
    Enums,
    None,
}

impl OptionType {
    pub fn is_vector(self) -> bool {
        use OptionType::*;
        matches!(self, Floats | Ints | Strings | Percents | FloatsOrPercents | Points | Bools | Enums)
    }

    /// The type of one element of a vector; scalars return themselves.
    pub fn element(self) -> OptionType {
        use OptionType::*;
        match self {
            Floats => Float,
            Ints => Int,
            Strings => String,
            Percents => Percent,
            FloatsOrPercents => FloatOrPercent,
            Points => Point,
            Bools => Bool,
            Enums => Enum,
            scalar => scalar,
        }
    }

    pub fn as_str(self) -> &'static str {
        use OptionType::*;
        match self {
            Float => "float",
            Floats => "floats",
            Int => "int",
            Ints => "ints",
            String => "string",
            Strings => "strings",
            Percent => "percent",
            Percents => "percents",
            FloatOrPercent => "float_or_percent",
            FloatsOrPercents => "floats_or_percents",
            Point => "point",
            Points => "points",
            Point3 => "point3",
            Bool => "bool",
            Bools => "bools",
            Enum => "enum",
            Enums => "enums",
            None => "none",
        }
    }
}

impl fmt::Display for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `ConfigOptionMode`: the GUI level at which an option becomes visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionMode {
    Simple,
    Advanced,
    Develop,
}

/// One `ConfigOptionDef`. Empty strings, empty lists and false flags are omitted by the exporter.
#[derive(Debug, Clone, Deserialize)]
pub struct OptionDef {
    #[serde(rename = "type")]
    pub type_: OptionType,
    pub nullable: bool,
    pub mode: OptionMode,
    /// `FFF`, `SLA`, `any` or `unknown`.
    pub printer_technology: String,
    /// Encoded the way profiles store values.
    pub default: Option<RawValue>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub full_label: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tooltip: String,
    /// Usually the unit, e.g. `mm/s`.
    #[serde(default)]
    pub sidetext: String,
    #[serde(default)]
    pub ratio_over: String,
    #[serde(default)]
    pub gui_type: Option<String>,
    #[serde(default)]
    pub gui_flags: String,
    /// GUI input limits; OrcaSlicer does not enforce them when loading.
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub max_literal: Option<f64>,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub full_width: bool,
    #[serde(default)]
    pub is_code: bool,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub shortcut: Vec<String>,
    /// Values offered in the GUI, with `enum_labels` as their display names.
    #[serde(default)]
    pub enum_values: Vec<String>,
    #[serde(default)]
    pub enum_labels: Vec<String>,
    /// Snapmaker-specific GUI value sets.
    #[serde(default)]
    pub enum_values_u1: Vec<String>,
    #[serde(default)]
    pub enum_labels_u1: Vec<String>,
    #[serde(default)]
    pub enum_values_ex: Vec<String>,
    #[serde(default)]
    pub enum_labels_ex: Vec<String>,
    /// Values an enum option accepts when a profile is loaded.
    #[serde(default)]
    pub enum_keys: Vec<String>,
}

impl OptionDef {
    /// The label to show for this option: `full_label` when set, else `label`.
    pub fn display_label(&self) -> &str {
        if self.full_label.is_empty() { &self.label } else { &self.full_label }
    }
}

/// The preset a preset without `inherits` starts from (`PresetCollection::default_preset()`).
#[derive(Debug, Clone, Deserialize)]
pub struct DefaultPreset {
    pub name: String,
    pub values: IndexMap<String, RawValue>,
}

#[derive(Debug, Clone, Deserialize)]
struct DefaultPresets {
    process: DefaultPreset,
    filament: DefaultPreset,
    machine: DefaultPreset,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Schema {
    pub format_version: u32,
    /// OrcaSlicer commit the schema was generated from.
    pub source_commit: String,
    pub options: BTreeMap<String, OptionDef>,
    default_presets: DefaultPresets,
    /// `Preset::print_options()`, `filament_options()`, `printer_options()`,
    /// `machine_limits_options()` and `PrintConfigDef::extruder_option_keys()`.
    pub option_lists: BTreeMap<String, Vec<String>>,
}

impl Schema {
    /// The schema bundled with this crate.
    pub fn bundled() -> Arc<Schema> {
        static BUNDLED: OnceLock<Arc<Schema>> = OnceLock::new();
        BUNDLED.get_or_init(|| Arc::new(Schema::from_json(BUNDLED_SCHEMA).expect("bundled schema is valid"))).clone()
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Schema, SchemaError> {
        Schema::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> Result<Schema, SchemaError> {
        // Check the version before the structure, so an incompatible file gets a clear error.
        #[derive(Deserialize)]
        struct Version {
            format_version: u32,
        }
        let Version { format_version } = serde_json::from_str(text)?;
        if format_version != SCHEMA_FORMAT_VERSION {
            return Err(SchemaError::UnsupportedVersion(format_version));
        }
        Ok(serde_json::from_str(text)?)
    }

    pub fn option(&self, key: &str) -> Option<&OptionDef> {
        self.options.get(key)
    }

    pub fn default_preset(&self, kind: PresetKind) -> &DefaultPreset {
        match kind {
            PresetKind::Process => &self.default_presets.process,
            PresetKind::Filament => &self.default_presets.filament,
            PresetKind::Machine => &self.default_presets.machine,
        }
    }

    /// Whether presets of `kind` may set `key`. This is the check of `Preset::remove_invalid_keys`,
    /// which keeps only keys present in the default preset of the kind.
    pub fn belongs_to(&self, kind: PresetKind, key: &str) -> bool {
        self.default_preset(kind).values.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::{OptionType, Schema, SchemaError};
    use crate::model::{PresetKind, RawValue};

    #[test]
    fn bundled_schema_loads() {
        let schema = Schema::bundled();
        let wall_loops = schema.option("wall_loops").expect("wall_loops is defined");
        assert_eq!(wall_loops.type_, OptionType::Int);
        assert_eq!(wall_loops.default, Some(RawValue::Str("2".into())));
        assert!(schema.belongs_to(PresetKind::Process, "wall_loops"));
        assert!(!schema.belongs_to(PresetKind::Filament, "wall_loops"));
        let support = schema.option("support_type").unwrap();
        assert!(support.enum_keys.iter().any(|key| key == "tree(auto)"));
    }

    #[test]
    fn rejects_other_format_versions() {
        let err = Schema::from_json(r#"{ "format_version": 2 }"#).unwrap_err();
        assert!(matches!(err, SchemaError::UnsupportedVersion(2)));
    }
}

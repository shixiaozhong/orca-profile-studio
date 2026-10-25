use std::fmt;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PresetKind {
    Process,
    Filament,
    Machine,
}

impl PresetKind {
    /// Order in which OrcaSlicer loads the sections of a vendor.
    pub const ALL: [PresetKind; 3] = [PresetKind::Process, PresetKind::Filament, PresetKind::Machine];

    pub fn as_str(self) -> &'static str {
        match self {
            PresetKind::Process => "process",
            PresetKind::Filament => "filament",
            PresetKind::Machine => "machine",
        }
    }

    /// Key of this section in a vendor index file.
    pub fn list_key(self) -> &'static str {
        match self {
            PresetKind::Process => "process_list",
            PresetKind::Filament => "filament_list",
            PresetKind::Machine => "machine_list",
        }
    }

    pub(crate) fn from_list_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.list_key() == key)
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for PresetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VendorId(pub(crate) u32);

impl VendorId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresetId(pub(crate) u32);

impl PresetId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A config option value exactly as written in the JSON file.
///
/// Profiles only ever store strings or arrays of strings; the option's real type lives in
/// OrcaSlicer's `PrintConfig.cpp`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum RawValue {
    Str(String),
    List(Vec<String>),
}

impl RawValue {
    /// The value as a scalar. OrcaSlicer collapses a one-element array for scalar options.
    pub fn as_scalar(&self) -> Option<&str> {
        match self {
            RawValue::Str(text) => Some(text),
            RawValue::List(items) if items.len() == 1 => Some(&items[0]),
            RawValue::List(_) => None,
        }
    }
}

/// A vendor index file `<Vendor>.json` at the top of the profiles directory.
#[derive(Debug, Clone, Default)]
pub struct Vendor {
    pub id: VendorId,
    /// File stem of the index; also the name of the vendor's directory.
    pub name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub force_update: Option<String>,
    /// Path of the index file, relative to the profiles directory.
    pub index_path: PathBuf,
}

/// A `machine_model` file: describes a printer model and its nozzle variants. Not a preset.
#[derive(Debug, Clone, Default)]
pub struct MachineModel {
    pub vendor: VendorId,
    /// The name used in `machine_model_list`; `printer_model` of machine presets refers to it.
    pub id: String,
    pub name: Option<String>,
    pub model_id: Option<String>,
    /// Nozzle variants from `nozzle_diameter`; `printer_variant` must be one of them.
    pub variants: Vec<String>,
    pub technology: Option<String>,
    pub family: Option<String>,
    pub bed_model: Option<String>,
    pub bed_texture: Option<String>,
    pub hotend_model: Option<String>,
    pub default_materials: Vec<String>,
    pub path: PathBuf,
    /// Keys OrcaSlicer does not read into `VendorProfile::PrinterModel`.
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Keys that OrcaSlicer keeps apart from config options when reading a preset file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetMeta {
    pub name: String,
    pub type_: Option<String>,
    pub from: Option<String>,
    pub instantiation: Option<String>,
    pub inherits: Option<String>,
    pub setting_id: Option<String>,
    pub filament_id: Option<String>,
    pub description: Option<String>,
    pub renamed_from: Option<String>,
    pub version: Option<String>,
    pub url: Option<String>,
    pub is_custom_defined: Option<String>,
}

impl PresetMeta {
    pub(crate) const KEYS: &'static [&'static str] = &[
        "name",
        "type",
        "from",
        "instantiation",
        "inherits",
        "setting_id",
        "filament_id",
        "description",
        "renamed_from",
        "version",
        "url",
        "is_custom_defined",
    ];

    /// Stores a metadata value; `key` must be one of [`Self::KEYS`] in lower case.
    pub(crate) fn set(&mut self, key: &str, value: String) {
        let slot = match key {
            "name" => {
                self.name = value;
                return;
            }
            "type" => &mut self.type_,
            "from" => &mut self.from,
            "instantiation" => &mut self.instantiation,
            "inherits" => &mut self.inherits,
            "setting_id" => &mut self.setting_id,
            "filament_id" => &mut self.filament_id,
            "description" => &mut self.description,
            "renamed_from" => &mut self.renamed_from,
            "version" => &mut self.version,
            "url" => &mut self.url,
            "is_custom_defined" => &mut self.is_custom_defined,
            _ => unreachable!("not a metadata key: {key}"),
        };
        *slot = Some(value);
    }

    pub fn inherits(&self) -> Option<&str> {
        self.inherits.as_deref().filter(|parent| !parent.is_empty())
    }
}

/// A process, filament or machine preset file listed in a vendor index.
#[derive(Debug, Clone)]
pub struct Preset {
    pub id: PresetId,
    pub vendor: VendorId,
    pub kind: PresetKind,
    /// The `name` of the manifest entry; lookups use [`Preset::name`] instead.
    pub list_name: String,
    /// Path relative to the profiles directory.
    pub path: PathBuf,
    pub meta: PresetMeta,
    /// Config options in file order.
    pub params: IndexMap<String, RawValue>,
    pub had_bom: bool,
    pub parent: Option<PresetId>,
    /// `inherits` names a preset that could not be found when this one was loaded.
    pub inherits_unresolved: bool,
    /// The parent comes from OrcaFilamentLibrary rather than this preset's own vendor.
    pub inherits_from_library: bool,
}

impl Preset {
    pub fn name(&self) -> &str {
        &self.meta.name
    }

    /// Base presets with `instantiation: "false"` only exist to be inherited from.
    pub fn is_template(&self) -> bool {
        self.meta.instantiation.as_deref() == Some("false")
    }
}

/// The alias OrcaSlicer derives from a preset name: the part before the first `@`.
pub fn derived_alias(name: &str) -> &str {
    match name.find('@') {
        Some(at) => name[..at].trim_end(),
        None => name,
    }
}

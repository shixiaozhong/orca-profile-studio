//! Loading, inheritance resolution and validation for OrcaSlicer system profiles.
//!
//! The loading semantics mirror `PresetBundle::load_vendor_configs_from_json` in OrcaSlicer;
//! `docs/reference/profile_format_spec.md` describes the rules this crate implements.
//! Unlike OrcaSlicer, loading never aborts: every deviation becomes an [`Issue`].

mod check;
mod document;
mod edit;
mod issue;
mod json;
mod legacy;
mod model;
mod repository;
mod schema;
mod stats;

pub use document::{Document, DocumentError};
pub use edit::{Change, EditSession, SaveError};
pub use issue::{Issue, IssueKind, Severity};
pub use model::{MachineModel, Preset, PresetId, PresetKind, PresetMeta, RawValue, Vendor, VendorId, derived_alias};
pub use repository::{FILAMENT_LIBRARY, KeyTrace, LoadError, Origin, Repository, ResolvedValue};
pub use schema::{DefaultPreset, OptionDef, OptionMode, OptionType, SCHEMA_FORMAT_VERSION, Schema, SchemaError};
pub use stats::{KindStats, Stats};

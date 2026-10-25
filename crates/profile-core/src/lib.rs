//! Loading, inheritance resolution and validation for OrcaSlicer system profiles.
//!
//! The loading semantics mirror `PresetBundle::load_vendor_configs_from_json` in OrcaSlicer;
//! `docs/reference/profile_format_spec.md` describes the rules this crate implements.
//! Unlike OrcaSlicer, loading never aborts: every deviation becomes an [`Issue`].

mod issue;
mod json;
mod model;
mod repository;
mod stats;

pub use issue::{Issue, IssueKind, Severity};
pub use model::{MachineModel, Preset, PresetId, PresetKind, PresetMeta, RawValue, Vendor, VendorId, derived_alias};
pub use repository::{FILAMENT_LIBRARY, LoadError, Repository, ResolvedValue};
pub use stats::{KindStats, Stats};

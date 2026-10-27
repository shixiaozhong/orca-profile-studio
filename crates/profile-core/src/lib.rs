//! Loading, inheritance resolution and validation for OrcaSlicer system profiles.
//!
//! The loading semantics mirror `PresetBundle::load_vendor_configs_from_json` in OrcaSlicer;
//! `docs/reference/profile_format_spec.md` describes the rules this crate implements.
//! Unlike OrcaSlicer, loading never aborts: every deviation becomes an [`Issue`].

mod check;
mod compat;
mod condition;
mod diff;
mod document;
mod edit;
mod issue;
mod json;
mod legacy;
mod manage;
mod model;
mod po;
mod repository;
mod schema;
mod search;
mod stats;

pub use compat::{CompatibilityIndex, Verdict};
pub use condition::{Condition, ConditionError, Value, Variable};
pub use diff::{DiffError, KeyDiff};
pub use document::{Document, DocumentError};
pub use edit::{Change, EditSession, SaveError};
pub use issue::{Issue, IssueKind, Severity};
pub use manage::{
    FileChange, ManageError, ManagementPlan, ModelDraft, ModelPlan, NameRule, PlanNotice, PresetDraft, preset_file_name,
};
pub use model::{MachineModel, Preset, PresetId, PresetKind, PresetMeta, RawValue, Vendor, VendorId, derived_alias};
pub use po::Translations;
pub use repository::{FILAMENT_LIBRARY, KeyTrace, LoadError, Origin, Repository, ResolvedValue};
pub use schema::{DefaultPreset, OptionDef, OptionMode, OptionType, SCHEMA_FORMAT_VERSION, Schema, SchemaError};
pub use search::{OptionHit, SearchIndex, SearchResults, ValueHit};
pub use stats::{KindStats, Stats};

//! Loading, inheritance resolution and structural checks for OrcaSlicer system profiles.
//!
//! Only the profile files are analysed: option types, defaults and labels live in OrcaSlicer's
//! C++ sources, which this crate deliberately does not depend on.
//!
//! The loading semantics mirror `PresetBundle::load_vendor_configs_from_json` in OrcaSlicer;
//! `docs/reference/profile_format_spec.md` describes the rules this crate implements.
//! Unlike OrcaSlicer, loading never aborts: every deviation becomes an [`Issue`].

mod compat;
mod condition;
mod diff;
mod document;
mod edit;
mod issue;
mod json;
mod manage;
mod model;
mod promote;
mod repository;
mod search;
mod stats;
mod sub_path;

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
pub use promote::{PromotionError, PromotionImpact, PromotionPlan};
pub use repository::{
    FILAMENT_LIBRARY, KeyTrace, LoadError, Origin, Repository, ResolvedValue, find_profiles_dir, is_profiles_dir,
};
pub use search::{OptionHit, SearchIndex, SearchResults, ValueHit};
pub use stats::{KindStats, Stats};

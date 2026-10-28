use std::fmt;
use std::path::PathBuf;

use crate::model::{PresetId, VendorId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth knowing, no effect on loading.
    Info,
    /// Loads, but probably not what the author intended.
    Warning,
    /// OrcaSlicer logs an error and keeps going.
    Error,
    /// OrcaSlicer refuses to load the whole vendor.
    Fatal,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    // Files
    FileUnreadable(String),
    ListedFileMissing,
    Utf8Bom,
    OrphanFile,
    // Vendor index
    InvalidVendorVersion(Option<String>),
    InvalidManifestEntry(String),
    // Preset contents
    MissingName,
    InvalidMetaType {
        key: String,
    },
    InvalidValueType {
        key: String,
    },
    InvalidInstantiation(Option<String>),
    TypeMismatch {
        declared: String,
    },
    UnexpectedFrom(Option<String>),
    ListNameMismatch {
        list_name: String,
    },
    // Inheritance
    ParentNotFound {
        parent: String,
    },
    ParentListedAfterChild {
        parent: String,
    },
    // Identity
    DuplicateInVendor,
    DuplicateAcrossVendors {
        first_vendor: String,
    },
    FilamentIdMissing,
    // Machines
    ModelWithoutVariants,
    PrinterModelMissing,
    PrinterModelUnknown {
        model: String,
    },
    PrinterVariantUnknown {
        model: String,
        variant: String,
    },
    // Compatibility
    DanglingCompatiblePrinter {
        printer: String,
    },
    /// An own value exactly matches the value inherited through the parent.
    RedundantOverride {
        key: String,
    },
    // Compatibility conditions; OrcaSlicer treats a failing condition as "compatible with everything"
    ConditionInvalid {
        key: String,
        message: String,
    },
    /// `message` describes the first failure; `failures` counts the presets it failed for.
    ConditionFailed {
        key: String,
        message: String,
        failures: usize,
    },
}

impl IssueKind {
    pub fn severity(&self) -> Severity {
        use IssueKind::*;
        match self {
            FileUnreadable(_)
            | ListedFileMissing
            | InvalidVendorVersion(_)
            | MissingName
            | ParentNotFound { .. }
            | ParentListedAfterChild { .. }
            | DuplicateInVendor
            | FilamentIdMissing
            | PrinterModelMissing
            | PrinterModelUnknown { .. }
            | PrinterVariantUnknown { .. } => Severity::Fatal,
            InvalidManifestEntry(_)
            | InvalidMetaType { .. }
            | InvalidValueType { .. }
            | InvalidInstantiation(_)
            | DuplicateAcrossVendors { .. }
            | ConditionInvalid { .. }
            | ConditionFailed { .. } => Severity::Error,
            Utf8Bom
            | TypeMismatch { .. }
            | UnexpectedFrom(Some(_))
            | ModelWithoutVariants
            | DanglingCompatiblePrinter { .. } => Severity::Warning,
            OrphanFile | UnexpectedFrom(None) | ListNameMismatch { .. } | RedundantOverride { .. } => Severity::Info,
        }
    }

    /// The config key a value issue is about.
    pub fn key(&self) -> Option<&str> {
        use IssueKind::*;
        match self {
            RedundantOverride { key }
            | ConditionInvalid { key, .. }
            | ConditionFailed { key, .. }
            | InvalidValueType { key } => Some(key),
            DanglingCompatiblePrinter { .. } => Some("compatible_printers"),
            _ => None,
        }
    }

    /// Stable identifier for grouping and filtering.
    pub fn code(&self) -> &'static str {
        use IssueKind::*;
        match self {
            FileUnreadable(_) => "file-unreadable",
            ListedFileMissing => "listed-file-missing",
            Utf8Bom => "utf8-bom",
            OrphanFile => "orphan-file",
            InvalidVendorVersion(_) => "invalid-vendor-version",
            InvalidManifestEntry(_) => "invalid-manifest-entry",
            MissingName => "missing-name",
            InvalidMetaType { .. } => "invalid-meta-type",
            InvalidValueType { .. } => "invalid-value-type",
            InvalidInstantiation(_) => "invalid-instantiation",
            TypeMismatch { .. } => "type-mismatch",
            UnexpectedFrom(_) => "unexpected-from",
            ListNameMismatch { .. } => "list-name-mismatch",
            ParentNotFound { .. } => "parent-not-found",
            ParentListedAfterChild { .. } => "parent-listed-after-child",
            DuplicateInVendor => "duplicate-in-vendor",
            DuplicateAcrossVendors { .. } => "duplicate-across-vendors",
            FilamentIdMissing => "filament-id-missing",
            ModelWithoutVariants => "model-without-variants",
            PrinterModelMissing => "printer-model-missing",
            PrinterModelUnknown { .. } => "printer-model-unknown",
            PrinterVariantUnknown { .. } => "printer-variant-unknown",
            DanglingCompatiblePrinter { .. } => "dangling-compatible-printer",
            RedundantOverride { .. } => "redundant-override",
            ConditionInvalid { .. } => "condition-invalid",
            ConditionFailed { .. } => "condition-failed",
        }
    }
}

impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use IssueKind::*;
        match self {
            RedundantOverride { key } => {
                write!(f, "`{key}` is identical to its inherited value; the own override is redundant")
            }
            FileUnreadable(reason) => write!(f, "cannot load file: {reason}"),
            ListedFileMissing => f.write_str("listed in the vendor index but the file does not exist"),
            Utf8Bom => f.write_str("file starts with a UTF-8 BOM"),
            OrphanFile => f.write_str("JSON file is not listed in any vendor index and is never loaded"),
            InvalidVendorVersion(version) => write!(f, "vendor version {version:?} is not a dotted number"),
            InvalidManifestEntry(reason) => write!(f, "invalid vendor index entry: {reason}"),
            MissingName => f.write_str("preset has no `name`"),
            InvalidMetaType { key } => write!(f, "`{key}` must be a string"),
            InvalidValueType { key } => {
                write!(f, "`{key}` must be a string or an array of strings; the key is dropped")
            }
            InvalidInstantiation(value) => write!(f, "`instantiation` must be \"true\" or \"false\", found {value:?}"),
            TypeMismatch { declared } => write!(f, "`type` is {declared:?} but the file is listed in another section"),
            UnexpectedFrom(value) => write!(f, "`from` is {value:?}, expected \"system\""),
            ListNameMismatch { list_name } => {
                write!(f, "listed as {list_name:?} but the file names itself differently")
            }
            ParentNotFound { parent } => write!(f, "inherits {parent:?}, which does not exist"),
            ParentListedAfterChild { parent } => {
                write!(f, "inherits {parent:?}, which is listed after this preset and cannot be resolved")
            }
            DuplicateInVendor => f.write_str("another preset of this vendor has the same name"),
            DuplicateAcrossVendors { first_vendor } => {
                write!(f, "a preset with the same name was already loaded from vendor {first_vendor:?}")
            }
            FilamentIdMissing => f.write_str("no `filament_id` (own, inherited or from `setting_id`)"),
            ModelWithoutVariants => f.write_str("`nozzle_diameter` lists no variants; OrcaSlicer ignores the model"),
            PrinterModelMissing => f.write_str("`printer_model` or `printer_variant` is empty"),
            PrinterModelUnknown { model } => write!(f, "`printer_model` {model:?} is not a model of this vendor"),
            PrinterVariantUnknown { model, variant } => {
                write!(f, "`printer_variant` {variant:?} is not a nozzle variant of {model:?}")
            }
            DanglingCompatiblePrinter { printer } => {
                write!(f, "`compatible_printers` names {printer:?}, which is not a selectable printer")
            }
            ConditionInvalid { key, message } => {
                write!(f, "`{key}` cannot be parsed ({message}); OrcaSlicer treats it as compatible with everything")
            }
            ConditionFailed { key, message, failures } => {
                write!(
                    f,
                    "`{key}` fails for {failures} preset(s), e.g. {message}; OrcaSlicer treats those as compatible"
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub vendor: VendorId,
    /// File the issue is about, relative to the profiles directory.
    pub path: PathBuf,
    pub preset: Option<PresetId>,
}

impl Issue {
    pub fn severity(&self) -> Severity {
        self.kind.severity()
    }
}

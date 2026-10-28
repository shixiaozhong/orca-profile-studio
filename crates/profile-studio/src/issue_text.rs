//! Localized presentation of core diagnostics; CLI Display remains English.
use profile_core::IssueKind;
pub fn issue(issue: &IssueKind) -> String {
    use IssueKind::*;
    match issue {
        RedundantOverride { key } => t!("issue.redundant_override", key = key),
        FileUnreadable(reason) => t!("issue.file_unreadable", reason = reason),
        ListedFileMissing => t!("issue.listed_missing").to_owned(),
        Utf8Bom => t!("issue.bom").to_owned(),
        OrphanFile => t!("issue.orphan").to_owned(),
        InvalidVendorVersion(version) => t!("issue.vendor_version", version = format!("{version:?}")),
        InvalidManifestEntry(reason) => t!("issue.manifest_entry", reason = reason),
        MissingName => t!("issue.missing_name").to_owned(),
        InvalidMetaType { key } => t!("issue.meta_type", key = key),
        InvalidValueType { key } => {
            t!("issue.value_type", key = key)
        }
        InvalidInstantiation(value) => t!("issue.instantiation", value = format!("{value:?}")),
        TypeMismatch { declared } => t!("issue.type", declared = format!("{declared:?}")),
        UnexpectedFrom(value) => t!("issue.from", value = format!("{value:?}")),
        ListNameMismatch { list_name } => {
            t!("issue.list_name", list_name = format!("{list_name:?}"))
        }
        ParentNotFound { parent } => t!("issue.parent", parent = format!("{parent:?}")),
        ParentListedAfterChild { parent } => {
            t!("issue.parent_order", parent = format!("{parent:?}"))
        }
        DuplicateInVendor => t!("issue.duplicate").to_owned(),
        DuplicateAcrossVendors { first_vendor } => {
            t!("issue.cross_vendor", first_vendor = format!("{first_vendor:?}"))
        }
        FilamentIdMissing => t!("issue.filament_id").to_owned(),
        ModelWithoutVariants => t!("issue.variants").to_owned(),
        PrinterModelMissing => t!("issue.model_missing").to_owned(),
        PrinterModelUnknown { model } => t!("issue.model_unknown", model = format!("{model:?}")),
        PrinterVariantUnknown { model, variant } => {
            t!("issue.variant_unknown", variant = format!("{variant:?}"), model = format!("{model:?}"))
        }
        DanglingCompatiblePrinter { printer } => {
            t!("issue.dangling_printer", printer = format!("{printer:?}"))
        }
        ConditionInvalid { key, message } => {
            t!("issue.condition_invalid", key = key, message = message)
        }
        ConditionFailed { key, message, failures } => {
            t!("issue.condition_failed", key = key, failures = failures, message = message)
        }
    }
}

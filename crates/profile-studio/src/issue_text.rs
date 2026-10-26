//! Localized presentation of core diagnostics; CLI Display remains English.
use profile_core::IssueKind;
pub fn issue(issue: &IssueKind) -> String {
    use IssueKind::*;
    match issue {
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
        ObsoleteKey { key } => t!("issue.obsolete", key = key),
        RenamedKey { key, current } => t!("issue.renamed", key = key, current = current),
        UnknownKey { key } => t!("issue.unknown", key = key),
        KeyNotForKind { key, reported: true } => {
            t!("issue.wrong_kind_removed", key = key)
        }
        KeyNotForKind { key, reported: false } => {
            t!("issue.wrong_kind_kept", key = key)
        }
        ScalarGivenList { key, collapsed: true } => {
            t!("issue.scalar_equal", key = key)
        }
        ScalarGivenList { key, collapsed: false } => {
            t!("issue.scalar_list", key = key)
        }
        NilNotNullable { key } => t!("issue.nil", key = key),
        ValueUnparsable { key, value } => {
            t!("issue.unparsable", key = key, value = format!("{value:?}"))
        }
        ItemUnparsable { key, item } => t!("issue.item_unparsable", key = key, item = format!("{item:?}")),
        ValueSubstituted { key, value } => {
            t!("issue.substituted", key = key, value = format!("{value:?}"))
        }
        ValueTrailingText { key, value } => {
            t!("issue.trailing", key = key, value = format!("{value:?}"))
        }
        ValueOutOfRange { key, value, min, max } => {
            let bound = |bound: &Option<i64>| bound.map_or_else(|| "-".to_owned(), |bound| bound.to_string());
            t!("issue.range", key = key, value = value, arg0 = bound(min), arg1 = bound(max))
        }
        ConditionInvalid { key, message } => {
            t!("issue.condition_invalid", key = key, message = message)
        }
        ConditionFailed { key, message, failures } => {
            t!("issue.condition_failed", key = key, failures = failures, message = message)
        }
    }
}

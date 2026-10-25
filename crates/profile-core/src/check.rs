//! Checks a preset's own values against the schema, the way `ConfigBase::load_from_json`
//! (`src/libslic3r/Config.cpp`) and the option deserializers in `Config.hpp` would read them.

use indexmap::IndexMap;

use crate::issue::IssueKind;
use crate::legacy::handle_legacy;
use crate::model::{PresetKind, RawValue};
use crate::schema::{OptionDef, OptionType, Schema};

/// Checks every value a preset file sets.
///
/// `instantiated` presets go through `Preset::remove_invalid_keys`; `inherited` tells whether an
/// ancestor sets a key, in which case OrcaSlicer keeps a misplaced key without reporting it.
pub(crate) fn check_params(
    schema: &Schema,
    kind: PresetKind,
    params: &IndexMap<String, RawValue>,
    instantiated: bool,
    inherited: impl Fn(&str) -> bool,
) -> Vec<IssueKind> {
    let mut issues = Vec::new();
    for (key, raw) in params {
        let Some((current, value)) = effective_value(schema, key, raw, &mut issues) else { continue };
        if current != *key {
            issues.push(IssueKind::RenamedKey { key: key.clone(), current: current.clone() });
        }
        let Some(def) = schema.option(&current) else {
            issues.push(IssueKind::UnknownKey { key: key.clone() });
            continue;
        };
        if !schema.belongs_to(kind, &current) {
            let reported = instantiated && !inherited(&current);
            issues.push(IssueKind::KeyNotForKind { key: current.clone(), reported });
        }
        check_value(def, &current, &value, &mut issues);
    }
    issues
}

/// The key and the single string OrcaSlicer deserializes, or `None` when the key is dropped.
fn effective_value(
    schema: &Schema,
    key: &str,
    raw: &RawValue,
    issues: &mut Vec<IssueKind>,
) -> Option<(String, String)> {
    let obsolete = |issues: &mut Vec<IssueKind>| issues.push(IssueKind::ObsoleteKey { key: key.to_owned() });
    let items = match raw {
        RawValue::Str(value) => {
            let result = handle_legacy(key, value);
            if result.is_none() {
                obsolete(issues);
            }
            return result;
        }
        RawValue::List(items) => items,
    };
    // Arrays run through handle_legacy twice: first with an empty value to find the key,
    // then with the joined value inside set_deserialize().
    let Some((current, mut value)) = handle_legacy(key, "") else {
        obsolete(issues);
        return None;
    };
    let Some(def) = schema.option(&current) else {
        // Unknown; the caller reports it.
        return Some((current, items.join(",")));
    };
    if !def.type_.is_vector() && !matches!(def.type_, OptionType::Point | OptionType::Point3) {
        match items.as_slice() {
            [single] => value = single.clone(),
            [first, rest @ ..] => {
                let collapsed = rest.iter().all(|item| item == first);
                if collapsed {
                    value = first.clone();
                }
                issues.push(IssueKind::ScalarGivenList { key: key.to_owned(), collapsed });
            }
            [] => {}
        }
    }
    // String lists are joined as C-style quoted items; no legacy rule or check looks at them,
    // so skip building that (possibly long G-code) string.
    if value.is_empty() && def.type_ != OptionType::Strings {
        value = items.join(",");
    }
    let result = handle_legacy(&current, &value);
    if result.is_none() {
        obsolete(issues);
    }
    result
}

fn check_value(def: &OptionDef, key: &str, value: &str, issues: &mut Vec<IssueKind>) {
    use OptionType::*;
    match def.type_ {
        // Strings are unescaped, points parse leniently; neither fails in practice.
        String | Strings | Point | Points | Point3 | None => {}
        vector if vector.is_vector() => {
            let element = vector.element();
            // getline(',') yields no item for an empty string or after a trailing comma.
            let mut items: Vec<&str> = value.split(',').collect();
            if items.last() == Some(&"") {
                items.pop();
            }
            for item in items {
                let item = item.trim();
                if item == "nil" {
                    if !def.nullable {
                        issues.push(IssueKind::NilNotNullable { key: key.to_owned() });
                    }
                    continue;
                }
                check_element(def, element, key, item, true, issues);
            }
        }
        scalar => check_element(def, scalar, key, value, false, issues),
    }
}

fn check_element(
    def: &OptionDef,
    type_: OptionType,
    key: &str,
    text: &str,
    in_vector: bool,
    issues: &mut Vec<IssueKind>,
) {
    let issue_value = || (key.to_owned(), text.to_owned());
    match type_ {
        OptionType::Bool => {
            if text == "0" || text == "1" {
                return;
            }
            let (key, value) = issue_value();
            // A bool vector only substitutes values that look like an enum name.
            issues.push(if !in_vector || looks_like_enum_value(text) {
                IssueKind::ValueSubstituted { key, value }
            } else {
                IssueKind::ValueUnparsable { key, value }
            });
        }
        OptionType::Enum => {
            if !def.enum_keys.iter().any(|known| known == text) {
                let (key, value) = issue_value();
                issues.push(IssueKind::ValueSubstituted { key, value });
            }
        }
        numeric => {
            let parsed = if numeric == OptionType::Int {
                leading_int(text).map(|(number, rest)| (number as f64, rest))
            } else {
                leading_float(text)
            };
            let Some((number, rest)) = parsed else {
                let (key, value) = issue_value();
                // Vector deserializers ignore the stream state and store 0; scalars fail.
                issues.push(if in_vector {
                    IssueKind::ItemUnparsable { key, item: value }
                } else {
                    IssueKind::ValueUnparsable { key, value }
                });
                return;
            };
            let percent_allowed = matches!(numeric, OptionType::Percent | OptionType::FloatOrPercent);
            let rest = rest.trim();
            let is_percent = percent_allowed && rest == "%";
            if !rest.is_empty() && !is_percent {
                let (key, value) = issue_value();
                issues.push(IssueKind::ValueTrailingText { key, value });
            }
            let absolute = !(numeric == OptionType::FloatOrPercent && is_percent);
            let below = def.min.is_some_and(|min| number < min as f64);
            let above = def.max.is_some_and(|max| number > max as f64);
            if absolute && (below || above) {
                let (key, value) = issue_value();
                issues.push(IssueKind::ValueOutOfRange { key, value, min: def.min, max: def.max });
            }
        }
    }
}

/// `ConfigHelpers::looks_like_enum_value`.
fn looks_like_enum_value(text: &str) -> bool {
    let text = text.trim();
    text.len() <= 64
        && text.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Approximates `std::istringstream >> double`: skips leading whitespace and reads the longest
/// decimal prefix. Returns the number and the unread rest, or `None` when there is no number.
fn leading_float(text: &str) -> Option<(f64, &str)> {
    let body = text.trim_start();
    let bytes = body.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end += 1;
    }
    let digits_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    let mut digits = end - digits_start;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let fraction_start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        digits += end - fraction_start;
    }
    if digits == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let exponent_digits = exponent;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
        }
        if exponent > exponent_digits {
            end = exponent;
        }
    }
    let number = body[..end].parse().ok()?;
    Some((number, &body[end..]))
}

/// Approximates `std::istringstream >> int`, failing on overflow like the stream does.
fn leading_int(text: &str) -> Option<(i32, &str)> {
    let body = text.trim_start();
    let bytes = body.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let digits = bytes[sign..].iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let end = sign + digits;
    let number = body[..end].parse().ok()?;
    Some((number, &body[end..]))
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::{check_params, leading_float, leading_int};
    use crate::issue::IssueKind;
    use crate::model::{PresetKind, RawValue};
    use crate::schema::Schema;

    fn check(kind: PresetKind, entries: &[(&str, RawValue)]) -> Vec<IssueKind> {
        let params: IndexMap<String, RawValue> =
            entries.iter().map(|(key, value)| (key.to_string(), value.clone())).collect();
        check_params(&Schema::bundled(), kind, &params, true, |_| false)
    }

    fn s(text: &str) -> RawValue {
        RawValue::Str(text.into())
    }

    fn list(items: &[&str]) -> RawValue {
        RawValue::List(items.iter().map(|item| item.to_string()).collect())
    }

    #[test]
    fn parses_like_a_stream() {
        assert_eq!(leading_float(" 0.2mm"), Some((0.2, "mm")));
        assert_eq!(leading_float("-.5e3"), Some((-500.0, "")));
        assert_eq!(leading_float("1e"), Some((1.0, "e")));
        assert_eq!(leading_float("abc"), None);
        assert_eq!(leading_int("1.5"), Some((1, ".5")));
        assert_eq!(leading_int("99999999999"), None);
    }

    #[test]
    fn valid_values_pass() {
        let issues = check(
            PresetKind::Process,
            &[
                ("wall_loops", s("3")),
                ("layer_height", list(&["0.2"])),
                ("sparse_infill_density", s("15%")),
                ("support_type", s("tree(auto)")),
                ("enable_support", s("1")),
            ],
        );
        assert_eq!(issues, Vec::new());
    }

    #[test]
    fn reports_bad_values() {
        let issues = check(
            PresetKind::Process,
            &[
                ("wall_loops", s("three")),
                ("layer_height", s("0.2mm")),
                ("support_type", s("sideways")),
                ("enable_support", s("yes")),
                ("wall_loops_typo", s("3")),
                ("silent_mode", s("0")),
                ("bridge_fan_speed", s("50")),
            ],
        );
        let codes: Vec<&str> = issues.iter().map(IssueKind::code).collect();
        assert_eq!(
            codes,
            [
                "value-unparsable",
                "value-trailing-text",
                "value-substituted",
                "value-substituted",
                "unknown-key",
                "obsolete-key",
                "renamed-key",
                "key-not-for-kind",
            ]
        );
    }

    #[test]
    fn vectors_and_scalars_given_lists() {
        let issues = check(
            PresetKind::Filament,
            &[
                ("nozzle_temperature", list(&["220", "abc"])),
                ("filament_retraction_length", list(&["nil"])),
                ("filament_diameter", list(&["nil"])),
                ("filament_type", list(&["PLA", "PETG"])),
            ],
        );
        assert_eq!(
            issues,
            [
                IssueKind::ItemUnparsable { key: "nozzle_temperature".into(), item: "abc".into() },
                IssueKind::NilNotNullable { key: "filament_diameter".into() },
            ]
        );

        let issues = check(
            PresetKind::Machine,
            &[("printer_model", list(&["One", "One"])), ("printer_variant", list(&["0.4", "0.6"]))],
        );
        assert_eq!(
            issues,
            [
                IssueKind::ScalarGivenList { key: "printer_model".into(), collapsed: true },
                IssueKind::ScalarGivenList { key: "printer_variant".into(), collapsed: false },
            ]
        );
    }
}

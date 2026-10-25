//! Port of `PrintConfigDef::handle_legacy` (`src/libslic3r/PrintConfig.cpp`), which renames
//! obsolete keys and rewrites obsolete values before OrcaSlicer parses a value.
//!
//! The rules are code, not data, so the exported schema cannot carry them. Keep this file in
//! sync with the C++ function when the schema is regenerated.

/// Keys renamed without touching the value.
const RENAMED: &[(&str, &str)] = &[
    ("enable_wipe_tower", "enable_prime_tower"),
    ("wipe_tower_width", "prime_tower_width"),
    ("wiping_volume", "prime_volume"),
    ("wipe_tower_brim_width", "prime_tower_brim_width"),
    ("tool_change_gcode", "change_filament_gcode"),
    ("bridge_fan_speed", "overhang_fan_speed"),
    ("infill_extruder", "sparse_infill_filament"),
    ("solid_infill_extruder", "solid_infill_filament"),
    ("perimeter_extruder", "wall_filament"),
    ("wipe_tower_extruder", "wipe_tower_filament"),
    ("support_material_extruder", "support_filament"),
    ("support_material_interface_extruder", "support_interface_filament"),
    ("support_material_angle", "support_angle"),
    ("support_material_enforce_layers", "enforce_support_layers"),
    ("inherits_cummulative", "inherits_group"),
    ("compatible_printers_condition_cummulative", "compatible_machine_expression_group"),
    ("compatible_prints_condition_cummulative", "compatible_process_expression_group"),
    ("cooling", "slow_down_for_layer_cooling"),
    ("timelapse_no_toolhead", "timelapse_type"),
    ("sparse_infill_anchor", "infill_anchor"),
    ("sparse_infill_anchor_max", "infill_anchor_max"),
    ("chamber_temperatures", "chamber_temperature"),
    ("thumbnail_size", "thumbnails"),
    ("initial_layer_flow_ratio", "bottom_solid_infill_flow_ratio"),
    ("ironing_direction", "ironing_angle"),
    ("counterbole_hole_bridging", "counterbore_hole_bridging"),
];

/// Options that used to accept percentages; a percentage value drops the key.
const NO_LONGER_PERCENT: &[&str] = &[
    "initial_layer_print_height",
    "initial_layer_speed",
    "internal_solid_infill_speed",
    "top_surface_speed",
    "support_interface_speed",
    "outer_wall_speed",
    "support_object_xy_distance",
];

/// Patterns whose `zig-zag` value became `rectilinear`.
const ZIG_ZAG_PATTERNS: &[&str] = &[
    "sparse_infill_pattern",
    "top_surface_pattern",
    "bottom_surface_pattern",
    "internal_solid_infill_pattern",
    "ironing_pattern",
    "support_ironing_pattern",
];

/// Obsolete keys OrcaSlicer silently ignores.
const IGNORED: &[&str] = &[
    "acceleration",
    "scale",
    "rotate",
    "duplicate",
    "duplicate_grid",
    "bed_size",
    "print_center",
    "g0",
    "wipe_tower_per_color_wipe",
    "support_sharp_tails",
    "support_remove_small_overhangs",
    "support_with_sheath",
    "tree_support_collision_resolution",
    "tree_support_with_infill",
    "max_volumetric_speed",
    "max_print_speed",
    "support_closing_radius",
    "remove_freq_sweep",
    "remove_bed_leveling",
    "remove_extrusion_calibration",
    "support_transition_line_width",
    "bed_temperature",
    "bed_temperature_initial_layer",
    "can_switch_nozzle_type",
    "can_add_auxiliary_fan",
    "extra_flush_volume",
    "spaghetti_detector",
    "adaptive_layer_height",
    "z_hop_type",
    "z_lift_type",
    "bed_temperature_difference",
    "long_retraction_when_cut",
    "retraction_distance_when_cut",
    "extruder_type",
    "internal_bridge_support_thickness",
    "extruder_clearance_max_radius",
    "top_area_threshold",
    "reduce_wall_solid_infill",
    "filament_load_time",
    "filament_unload_time",
    "smooth_coefficient",
    "overhang_totally_speed",
    "silent_mode",
];

/// Applies the legacy rules to one key and value. `None` means OrcaSlicer drops the key.
///
/// The C++ function also drops keys `print_config_def` does not know; callers check that
/// against the schema so they can tell obsolete keys from unknown ones.
pub(crate) fn handle_legacy(key: &str, value: &str) -> Option<(String, String)> {
    let mut key = key.to_owned();
    let mut value = value.to_owned();
    // An if/else chain in C++: only the first matching rule applies.
    if let Some(&(_, current)) = RENAMED.iter().find(|(old, _)| *old == key) {
        key = current.to_owned();
    } else if NO_LONGER_PERCENT.contains(&key.as_str()) && value.contains('%') {
        return None;
    } else {
        let rewritten = match (key.as_str(), value.as_str()) {
            ("timelapse_type", "2") => Some("0"),
            ("support_type", "normal") => Some("normal(manual)"),
            ("support_type", "tree") => Some("tree(manual)"),
            ("support_type", "hybrid(auto)") => Some("tree(auto)"),
            ("support_base_pattern", "none") => Some("hollow"),
            ("overhang_fan_threshold", "5%") => Some("10%"),
            ("ensure_vertical_shell_thickness", "1") => Some("ensure_all"),
            ("ensure_vertical_shell_thickness", "0") => Some("ensure_moderate"),
            ("draft_shield", "limited") => Some("disabled"),
            (pattern, "zig-zag") if ZIG_ZAG_PATTERNS.contains(&pattern) => Some("rectilinear"),
            _ => None,
        };
        if let Some(rewritten) = rewritten {
            value = rewritten.to_owned();
        } else if key == "wall_infill_order" {
            key = "wall_sequence".to_owned();
            let sequence = match value.as_str() {
                "inner wall/outer wall/infill" | "infill/inner wall/outer wall" => Some("inner wall/outer wall"),
                "outer wall/inner wall/infill" | "infill/outer wall/inner wall" => Some("outer wall/inner wall"),
                "inner-outer-inner wall/infill" => Some("inner-outer-inner wall"),
                _ => None,
            };
            if let Some(sequence) = sequence {
                value = sequence.to_owned();
            }
        } else if key == "rotate_solid_infill_direction" {
            key = "solid_infill_rotate_template".to_owned();
            if value == "1" {
                value = "0,90".to_owned();
            }
        } else if key == "top_one_wall_type" && value != "none" {
            key = "only_one_wall_top".to_owned();
            value = "1".to_owned();
        }
    }
    if IGNORED.contains(&key.as_str()) {
        return None;
    }
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::handle_legacy;

    fn apply(key: &str, value: &str) -> Option<(String, String)> {
        handle_legacy(key, value)
    }

    #[test]
    fn renames_and_rewrites() {
        assert_eq!(apply("bridge_fan_speed", "50"), Some(("overhang_fan_speed".into(), "50".into())));
        assert_eq!(apply("support_type", "tree"), Some(("support_type".into(), "tree(manual)".into())));
        assert_eq!(
            apply("wall_infill_order", "infill/outer wall/inner wall"),
            Some(("wall_sequence".into(), "outer wall/inner wall".into()))
        );
        assert_eq!(apply("wall_loops", "3"), Some(("wall_loops".into(), "3".into())));
    }

    #[test]
    fn drops_obsolete_keys() {
        assert_eq!(apply("silent_mode", "0"), None);
        assert_eq!(apply("outer_wall_speed", "50%"), None);
        assert_eq!(apply("outer_wall_speed", "50"), Some(("outer_wall_speed".into(), "50".into())));
    }
}

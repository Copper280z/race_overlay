//! Compatibility facade for pure overlay policies.
//!
//! The editor modules import these names explicitly. Keeping the re-exports in
//! one place avoids coupling feature code to the physical policy split while
//! retaining a small, private compatibility boundary.
pub(super) use super::appearance_policy::{
    appearance_color_editor, appearance_for_preset, appearance_number, appearance_preset,
    appearance_preset_label, normalize_appearance, parse_unit_system, remove_style,
    reset_widget_appearance, set_appearance_number, set_appearance_string, set_style, style_string,
    unit_name, widget_appearance_ui,
};
pub(super) use super::export_policy::{codec_display_name, export_surface, format_time};
pub(super) use super::source_policy::{
    correlation_candidate_offsets, correlation_lag_window, dataset_duration, forward_axis_label,
    forward_axis_vector, prepare_loaded_dataset, set_source_low_pass_settings,
    source_low_pass_settings,
};
pub(super) use super::widget_policy::{
    apply_unbound_widget_unit_default, coordinate_pair_label, coordinate_value_at,
    default_display_unit, default_widgets_for, is_latitude_channel, is_longitude_channel,
    make_widget, normalized_to_screen, point_in, preferred_channels, retarget_widget_units,
    select_gps_coordinate_channels, style_bool, style_number, track_map_mode_label,
    widget_channel_binding, widget_suggested_range,
};

#[cfg(test)]
pub(super) use super::source_policy::{apply_low_pass_to_dataset, remove_source_from_project};

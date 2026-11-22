//! Multi-monitor output discovery, arrangement, fractional scaling, rotation, mirroring,
//! and safe hotplug disconnection management for **NEON DE**.

use std::collections::HashMap;

use neon_window_manager::geometry::{Point, Rect, Size};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Stable internal monitor identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputId(pub u64);

/// Physical display mode (resolution + refresh rate in millihertz).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

/// Output rotation / reflection transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTransform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl OutputTransform {
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

/// Complete tracked state of a connected display output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputRecord {
    pub id: OutputId,
    pub name: String,
    pub make: String,
    pub model: String,
    pub available_modes: Vec<DisplayMode>,
    pub current_mode: DisplayMode,
    pub position: Point,
    /// Fractional or integer HiDPI scale factor (clamped to `0.5..=4.0`).
    pub scale: f64,
    pub transform: OutputTransform,
    pub primary: bool,
    pub enabled: bool,
    /// Optional target output ID when display mirroring is active.
    pub mirror_of: Option<OutputId>,
}

impl OutputRecord {
    /// Computes the logical output size after applying rotation and fractional scale.
    pub fn logical_size(&self) -> Size {
        let scale = self.scale.clamp(0.5, 4.0);
        let (pw, ph) = if self.transform.swaps_axes() {
            (self.current_mode.height, self.current_mode.width)
        } else {
            (self.current_mode.width, self.current_mode.height)
        };
        Size::new(
            ((pw as f64) / scale).round().max(1.0) as i32,
            ((ph as f64) / scale).round().max(1.0) as i32,
        )
    }

    /// Computes the logical bounding rectangle in global compositor coordinates.
    pub fn logical_rect(&self) -> Rect {
        Rect::new(self.position, self.logical_size())
    }
}

/// Partial configuration update applied to an existing output (e.g., via Settings IPC).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputConfigPatch {
    pub mode: Option<DisplayMode>,
    pub position: Option<Point>,
    pub scale: Option<f64>,
    pub transform: Option<OutputTransform>,
    pub primary: Option<bool>,
    pub enabled: Option<bool>,
    pub mirror_of: Option<Option<String>>,
}

/// Result returned when an output is disconnected so the compositor can safely migrate windows.
#[derive(Debug, Clone, PartialEq)]
pub struct DisconnectOutcome {
    pub removed: OutputRecord,
    pub fallback_output: Option<OutputId>,
}

/// Multi-monitor output manager with persistent per-connector preferences and safe hotplugging.
#[derive(Debug, Default)]
pub struct OutputManager {
    next_id: u64,
    outputs: HashMap<OutputId, OutputRecord>,
    order: Vec<OutputId>,
    saved_configs: HashMap<String, OutputConfigPatch>,
}

impl OutputManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a saved persistent layout preference for connector `name` (e.g. `"DP-1"`).
    pub fn save_connector_config(&mut self, name: impl Into<String>, patch: OutputConfigPatch) {
        self.saved_configs.insert(name.into(), patch);
    }

    /// Registers a newly discovered display output, applying saved preferences or horizontal auto-layout.
    pub fn register_output(
        &mut self,
        name: String,
        make: String,
        model: String,
        available_modes: Vec<DisplayMode>,
        preferred_mode: DisplayMode,
        scale: f64,
    ) -> OutputRecord {
        self.next_id += 1;
        let id = OutputId(self.next_id);

        let default_x = self
            .enabled_outputs()
            .map(|o| {
                let r = o.logical_rect();
                r.loc.x + r.size.w
            })
            .max()
            .unwrap_or(0);

        let is_first = self.primary_output().is_none();
        let mut record = OutputRecord {
            id,
            name: name.clone(),
            make,
            model,
            available_modes,
            current_mode: preferred_mode,
            position: Point::new(default_x, 0),
            scale: scale.clamp(0.5, 4.0),
            transform: OutputTransform::Normal,
            primary: is_first,
            enabled: true,
            mirror_of: None,
        };

        if let Some(saved) = self.saved_configs.get(&name).cloned() {
            apply_patch_to_record(&mut record, &saved, &self.outputs);
        }

        if record.primary {
            for other in self.outputs.values_mut() {
                other.primary = false;
            }
        }

        self.outputs.insert(id, record.clone());
        self.order.push(id);
        self.ensure_primary_exists();

        let final_record = self.outputs.get(&id).unwrap().clone();
        info!(
            ?id,
            name = %final_record.name,
            pos = ?final_record.position,
            scale = final_record.scale,
            primary = final_record.primary,
            "Registered display output"
        );
        final_record
    }

    /// Safely handles monitor hotplug disconnection, reassigning primary to a surviving display.
    pub fn disconnect_output(&mut self, id: OutputId) -> Option<DisconnectOutcome> {
        let removed = self.outputs.remove(&id)?;
        self.order.retain(|&o| o != id);

        // Break any mirror references pointing at the disconnected output.
        for other in self.outputs.values_mut() {
            if other.mirror_of == Some(id) {
                other.mirror_of = None;
            }
        }

        self.ensure_primary_exists();
        let fallback_output = self.primary_output().map(|o| o.id);
        info!(
            ?id,
            name = %removed.name,
            ?fallback_output,
            "Disconnected display output cleanly"
        );

        Some(DisconnectOutcome {
            removed,
            fallback_output,
        })
    }

    /// Updates mode, scale, fractional scale, rotation, position, primary, or mirroring by connector name.
    pub fn configure_by_name(
        &mut self,
        name: &str,
        patch: &OutputConfigPatch,
    ) -> Option<OutputRecord> {
        let target_id = self
            .outputs
            .values()
            .find(|o| o.name == name)
            .map(|o| o.id)?;

        // Resolve mirror target ID before mutably borrowing target record.
        let resolved_mirror = patch.mirror_of.as_ref().map(|opt_name| {
            opt_name.as_ref().and_then(|src_name| {
                self.outputs
                    .values()
                    .find(|o| o.name == *src_name && o.id != target_id)
                    .map(|o| (o.id, o.position))
            })
        });

        let make_primary = patch.primary == Some(true);
        if make_primary {
            for other in self.outputs.values_mut() {
                other.primary = other.id == target_id;
            }
        }

        let record = self.outputs.get_mut(&target_id)?;
        if let Some(mode) = patch.mode {
            record.current_mode = mode;
        }
        if let Some(pos) = patch.position {
            record.position = pos;
        }
        if let Some(scale) = patch.scale {
            record.scale = scale.clamp(0.5, 4.0);
        }
        if let Some(transform) = patch.transform {
            record.transform = transform;
        }
        if let Some(enabled) = patch.enabled {
            record.enabled = enabled;
        }
        if let Some(mirror_target) = resolved_mirror {
            match mirror_target {
                Some((src_id, src_pos)) => {
                    record.mirror_of = Some(src_id);
                    record.position = src_pos;
                }
                None => {
                    record.mirror_of = None;
                }
            }
        }

        let snapshot = record.clone();
        self.saved_configs.insert(name.to_string(), patch.clone());
        self.ensure_primary_exists();
        debug!(name, pos = ?snapshot.position, scale = snapshot.scale, "Updated output configuration");
        self.outputs.get(&target_id).cloned()
    }

    pub fn get(&self, id: OutputId) -> Option<&OutputRecord> {
        self.outputs.get(&id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&OutputRecord> {
        self.outputs.values().find(|o| o.name == name)
    }

    pub fn all_outputs(&self) -> Vec<&OutputRecord> {
        self.order
            .iter()
            .filter_map(|id| self.outputs.get(id))
            .collect()
    }

    pub fn enabled_outputs(&self) -> impl Iterator<Item = &OutputRecord> {
        self.order
            .iter()
            .filter_map(|id| self.outputs.get(id))
            .filter(|o| o.enabled)
    }

    pub fn primary_output(&self) -> Option<&OutputRecord> {
        self.enabled_outputs()
            .find(|o| o.primary)
            .or_else(|| self.enabled_outputs().next())
    }

    /// Finds the enabled output whose logical rectangle contains `(x, y)`.
    pub fn output_at_point(&self, x: i32, y: i32) -> Option<&OutputRecord> {
        self.enabled_outputs().find(|o| {
            let r = o.logical_rect();
            x >= r.loc.x && x < r.loc.x + r.size.w && y >= r.loc.y && y < r.loc.y + r.size.h
        })
    }

    /// Ensures exactly one enabled output is marked primary whenever at least one is enabled.
    fn ensure_primary_exists(&mut self) {
        let has_enabled_primary = self.outputs.values().any(|o| o.enabled && o.primary);
        if has_enabled_primary {
            return;
        }
        if let Some(first_id) = self
            .order
            .iter()
            .copied()
            .find(|id| self.outputs.get(id).map(|o| o.enabled).unwrap_or(false))
            && let Some(rec) = self.outputs.get_mut(&first_id)
        {
            rec.primary = true;
        }
    }
}

fn apply_patch_to_record(
    record: &mut OutputRecord,
    patch: &OutputConfigPatch,
    existing: &HashMap<OutputId, OutputRecord>,
) {
    if let Some(mode) = patch.mode {
        record.current_mode = mode;
    }
    if let Some(pos) = patch.position {
        record.position = pos;
    }
    if let Some(scale) = patch.scale {
        record.scale = scale.clamp(0.5, 4.0);
    }
    if let Some(transform) = patch.transform {
        record.transform = transform;
    }
    if let Some(primary) = patch.primary {
        record.primary = primary;
    }
    if let Some(enabled) = patch.enabled {
        record.enabled = enabled;
    }
    if let Some(Some(src_name)) = &patch.mirror_of
        && let Some(src) = existing.values().find(|o| o.name == *src_name)
    {
        record.mirror_of = Some(src.id);
        record.position = src.position;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_monitor_auto_layout_fractional_scale_rotation_and_hotplug_disconnect() {
        let mut mgr = OutputManager::new();
        let mode_1080 = DisplayMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        };

        let laptop = mgr.register_output(
            "eDP-1".into(),
            "BOE".into(),
            "Laptop Panel".into(),
            vec![mode_1080],
            mode_1080,
            1.5,
        );
        assert!(laptop.primary);
        assert_eq!(laptop.position, Point::new(0, 0));
        // 1920 / 1.5 = 1280, 1080 / 1.5 = 720
        assert_eq!(laptop.logical_size(), Size::new(1280, 720));

        // Hotplug second monitor -> placed automatically to the right of eDP-1 at x=1280
        let ext = mgr.register_output(
            "HDMI-A-1".into(),
            "Dell".into(),
            "External 1080p".into(),
            vec![mode_1080],
            mode_1080,
            1.0,
        );
        assert!(!ext.primary);
        assert_eq!(ext.position, Point::new(1280, 0));

        // Rotate HDMI-A-1 90 degrees -> axes swap
        let rotated = mgr
            .configure_by_name(
                "HDMI-A-1",
                &OutputConfigPatch {
                    transform: Some(OutputTransform::Rotate90),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(rotated.logical_size(), Size::new(1080, 1920));

        // Disconnect primary laptop panel -> HDMI-A-1 safely becomes primary fallback
        let outcome = mgr.disconnect_output(laptop.id).unwrap();
        assert_eq!(outcome.fallback_output, Some(ext.id));
        assert!(mgr.get(ext.id).unwrap().primary);
    }

    #[test]
    fn display_mirroring_aligns_origin_and_clears_on_source_disconnect() {
        let mut mgr = OutputManager::new();
        let mode = DisplayMode {
            width: 1280,
            height: 800,
            refresh_mhz: 60_000,
        };
        let a = mgr.register_output(
            "DP-1".into(),
            "NEON".into(),
            "A".into(),
            vec![mode],
            mode,
            1.0,
        );
        let b = mgr.register_output(
            "DP-2".into(),
            "NEON".into(),
            "B".into(),
            vec![mode],
            mode,
            1.0,
        );

        let mirrored = mgr
            .configure_by_name(
                "DP-2",
                &OutputConfigPatch {
                    mirror_of: Some(Some("DP-1".into())),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(mirrored.mirror_of, Some(a.id));
        assert_eq!(mirrored.position, a.position);

        mgr.disconnect_output(a.id);
        assert_eq!(mgr.get(b.id).unwrap().mirror_of, None);
    }
}

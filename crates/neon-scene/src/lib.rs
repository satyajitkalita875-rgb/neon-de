//! Retained scene graph for **NEON DE**.
//!
//! Represents the complete desktop hierarchy in explicit Z-ordered layers:
//! `Background` -> `Windows` (Shadow -> Border -> Client Surface) -> `Panels` -> `Dock`
//! -> `Overview` -> `Notifications` -> `LockScreen` -> `Cursor`.
//!
//! Tracks dirty regions whenever nodes are added, updated, restacked, animated, or removed
//! so the rendering backend avoids unnecessary full-screen work when idle.

use std::collections::HashMap;

use neon_window_manager::{
    WindowId,
    geometry::{Point, Rect, Size},
};

/// Stable monotonic identifier for a node in the retained [`SceneGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

/// Explicit Z-order desktop layers from bottom (`Background`) to top (`Cursor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SceneLayer {
    Background = 0,
    Windows = 1,
    Panels = 2,
    Dock = 3,
    Overview = 4,
    Notifications = 5,
    LockScreen = 6,
    Cursor = 7,
}

/// Standard logical height (in px) of NEON DE's macOS server-side window titlebar.
pub const MACOS_TITLEBAR_HEIGHT: i32 = 28;

/// Visual parameters for a macOS-style server-side titlebar with traffic-light buttons.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitlebarStyle {
    pub height: i32,
    pub corner_radius: f32,
    pub focused: bool,
    pub light_theme: bool,
    /// `true` when the pointer hovers over the traffic-light button cluster (reveals ×, −, + glyphs).
    pub hover_controls: bool,
    /// `false` = Left (macOS: Close, Minimize, Maximize); `true` = Right (Standard: Minimize, Maximize, Close).
    pub buttons_right: bool,
}

/// Visual drop-shadow parameters attached to a floating window node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowStyle {
    pub spread: f32,
    pub offset_y: f32,
    pub corner_radius: f32,
    pub color: [f32; 4],
}

impl ShadowStyle {
    /// NEON DE soft shadow for focused / unfocused floating windows (`focus_progress` in `[0, 1]`).
    pub fn for_window(focus_progress: f32, corner_radius: f32) -> Self {
        let f = focus_progress.clamp(0.0, 1.0);
        Self {
            spread: 18.0 + 10.0 * f,
            offset_y: 6.0 + 4.0 * f,
            corner_radius,
            color: [0.0, 0.0, 0.0, 0.28 + 0.20 * f],
        }
    }
}

/// Visual border / focus-ring parameters attached to a window node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyle {
    pub width: f32,
    pub corner_radius: f32,
    pub color: [f32; 4],
}

impl BorderStyle {
    /// NEON DE subtle rim border that transitions smoothly to electric cyan-blue on focus.
    pub fn for_window(focus_progress: f32, corner_radius: f32) -> Self {
        let f = focus_progress.clamp(0.0, 1.0);
        // Unfocused: subtle cool slate rim; Focused: NEON accent (`#38BDF8`)
        let r = 0.28 * (1.0 - f) + 0.22 * f;
        let g = 0.32 * (1.0 - f) + 0.74 * f;
        let b = 0.40 * (1.0 - f) + 0.97 * f;
        let a = 0.35 * (1.0 - f) + 0.85 * f;
        Self {
            width: 1.5 + 0.5 * f,
            corner_radius,
            color: [r, g, b, a],
        }
    }
}

/// Payload identifying what visual content a [`SceneNode`] represents.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeContent {
    /// Compositor desktop backdrop / wallpaper canvas.
    Backdrop {
        primary_color: [f32; 4],
        accent_color: [f32; 4],
    },
    /// Managed Wayland toplevel window subtree (shadow + border + client surface tree).
    Window {
        window_id: WindowId,
        focused: bool,
        focus_progress: f32,
        fullscreen: bool,
    },
    /// Generic solid colored rectangle (used for overlays, panels, or debug elements).
    SolidRect { color: [f32; 4] },
}

/// A single retained visual node in [`SceneGraph`].
#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub id: NodeId,
    pub layer: SceneLayer,
    /// Relative stacking order within `layer` (higher value renders in front).
    pub z_index: i32,
    pub bounds: Rect,
    pub visible: bool,
    pub opacity: f32,
    pub offset: (f64, f64),
    pub scale: (f64, f64),
    pub clip: Option<Rect>,
    pub shadow: Option<ShadowStyle>,
    pub border: Option<BorderStyle>,
    pub titlebar: Option<TitlebarStyle>,
    pub corner_radius: f32,
    pub content: NodeContent,
}

impl SceneNode {
    /// Returns the animated logical center point of this node (`bounds.center + offset`).
    pub fn effective_center(&self) -> (f64, f64) {
        (
            self.bounds.loc.x as f64 + self.offset.0 + self.bounds.size.w as f64 * 0.5,
            self.bounds.loc.y as f64 + self.offset.1 + self.bounds.size.h as f64 * 0.5,
        )
    }

    /// Computes the animated logical bounding box (bounds shifted by offset and scaled around center).
    pub fn effective_bounds(&self) -> Rect {
        let (sx, sy) = self.scale;
        if (sx - 1.0).abs() <= 0.001 && (sy - 1.0).abs() <= 0.001 {
            return Rect::new(
                Point::new(
                    self.bounds.loc.x + self.offset.0.round() as i32,
                    self.bounds.loc.y + self.offset.1.round() as i32,
                ),
                self.bounds.size,
            );
        }
        let (cx, cy) = self.effective_center();
        let w = (self.bounds.size.w as f64 * sx).round().max(1.0) as i32;
        let h = (self.bounds.size.h as f64 * sy).round().max(1.0) as i32;
        Rect::new(
            Point::new((cx - w as f64 * 0.5).round() as i32, (cy - h as f64 * 0.5).round() as i32),
            Size::new(w, h),
        )
    }

    /// Computes the outer damage envelope including shadow spread, border thickness, and SSD titlebar.
    pub fn damage_bounds(&self) -> Rect {
        let base = self.effective_bounds();
        let tb_h = self.titlebar.map(|t| t.height).unwrap_or(0);
        let pad = self
            .shadow
            .map(|s| (s.spread.ceil() as i32) + (s.offset_y.abs().ceil() as i32) + 4)
            .unwrap_or(0)
            .max(self.border.map(|b| b.width.ceil() as i32 + 2).unwrap_or(0));
        Rect::new(
            Point::new(base.loc.x - pad, base.loc.y - tb_h - pad),
            Size::new(base.size.w + pad * 2, base.size.h + tb_h + pad * 2),
        )
    }
}

/// Active full-desktop horizontal workspace slide offsets in logical pixels.
///
/// Both the departing desktop (wallpaper + windows + dock) and incoming desktop
/// (wallpaper + windows + dock) pan together horizontally across the display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceSlide {
    pub outgoing_dx: f64,
    pub incoming_dx: f64,
}

/// Retained desktop scene graph with dirty tracking and deterministic Z-order traversal.
#[derive(Debug)]
pub struct SceneGraph {
    next_node_id: u64,
    nodes: HashMap<NodeId, SceneNode>,
    window_nodes: HashMap<WindowId, NodeId>,
    backdrop_node: Option<NodeId>,
    workspace_slide: Option<WorkspaceSlide>,
    dirty: bool,
    dirty_regions: Vec<Rect>,
}

impl Default for SceneGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneGraph {
    pub fn new() -> Self {
        Self {
            next_node_id: 0,
            nodes: HashMap::new(),
            window_nodes: HashMap::new(),
            backdrop_node: None,
            workspace_slide: None,
            dirty: true,
            dirty_regions: Vec::new(),
        }
    }

    pub fn set_workspace_slide(&mut self, slide: Option<WorkspaceSlide>) {
        if self.workspace_slide != slide {
            self.workspace_slide = slide;
            self.dirty = true;
        }
    }

    pub fn workspace_slide(&self) -> Option<WorkspaceSlide> {
        self.workspace_slide
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Consumes and returns accumulated logical dirty regions since the previous frame.
    pub fn take_dirty_regions(&mut self) -> Vec<Rect> {
        self.dirty = false;
        std::mem::take(&mut self.dirty_regions)
    }

    /// Ensures the desktop backdrop node exists and matches the current output bounds.
    pub fn set_backdrop(&mut self, output_size: Size) -> NodeId {
        let bounds = Rect::from_size(output_size);
        let content = NodeContent::Backdrop {
            primary_color: [0.047, 0.059, 0.090, 1.0],
            accent_color: [0.090, 0.145, 0.245, 1.0],
        };

        if let Some(id) = self.backdrop_node
            && let Some(node) = self.nodes.get_mut(&id)
        {
            if node.bounds != bounds {
                self.dirty = true;
                self.dirty_regions.push(bounds);
                node.bounds = bounds;
            }
            return id;
        }

        let id = self.alloc_id();
        self.nodes.insert(
            id,
            SceneNode {
                id,
                layer: SceneLayer::Background,
                z_index: 0,
                bounds,
                visible: true,
                opacity: 1.0,
                offset: (0.0, 0.0),
                scale: (1.0, 1.0),
                clip: None,
                shadow: None,
                border: None,
                titlebar: None,
                corner_radius: 0.0,
                content,
            },
        );
        self.backdrop_node = Some(id);
        self.dirty = true;
        self.dirty_regions.push(bounds);
        id
    }

    /// Inserts or updates the retained scene node for a managed window.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_window(
        &mut self,
        window_id: WindowId,
        bounds: Rect,
        z_index: i32,
        visible: bool,
        opacity: f32,
        offset: (f64, f64),
        scale: (f64, f64),
        focused: bool,
        focus_progress: f32,
        fullscreen: bool,
        shadow: Option<ShadowStyle>,
        border: Option<BorderStyle>,
    ) -> NodeId {
        let (existing_titlebar, existing_corner_radius) = self
            .window_nodes
            .get(&window_id)
            .and_then(|nid| self.nodes.get(nid))
            .map(|n| (n.titlebar, n.corner_radius))
            .unwrap_or((None, 0.0));
        let new_node = |id| SceneNode {
            id,
            layer: SceneLayer::Windows,
            z_index,
            bounds,
            visible,
            opacity: opacity.clamp(0.0, 1.0),
            offset,
            scale: (scale.0.clamp(0.05, 4.0), scale.1.clamp(0.05, 4.0)),
            clip: None,
            shadow: if fullscreen { None } else { shadow },
            border: if fullscreen { None } else { border },
            titlebar: if fullscreen { None } else { existing_titlebar },
            corner_radius: if fullscreen { 0.0 } else { existing_corner_radius },
            content: NodeContent::Window {
                window_id,
                focused,
                focus_progress,
                fullscreen,
            },
        };

        if let Some(&id) = self.window_nodes.get(&window_id) {
            let candidate = new_node(id);
            if let Some(existing) = self.nodes.get_mut(&id)
                && *existing != candidate
            {
                let old_dmg = existing.damage_bounds();
                let new_dmg = candidate.damage_bounds();
                *existing = candidate;
                self.dirty = true;
                self.dirty_regions.push(old_dmg);
                self.dirty_regions.push(new_dmg);
            }
            return id;
        }

        let id = self.alloc_id();
        let node = new_node(id);
        let dmg = node.damage_bounds();
        self.nodes.insert(id, node);
        self.window_nodes.insert(window_id, id);
        self.dirty = true;
        self.dirty_regions.push(dmg);
        id
    }

    /// Updates the optional macOS server-side titlebar for an existing managed window node.
    pub fn set_window_titlebar(&mut self, window_id: WindowId, titlebar: Option<TitlebarStyle>) {
        if let Some(&id) = self.window_nodes.get(&window_id)
            && let Some(node) = self.nodes.get_mut(&id)
        {
            let target = match &node.content {
                NodeContent::Window { fullscreen: true, .. } => None,
                _ => titlebar,
            };
            if node.titlebar != target {
                let old_dmg = node.damage_bounds();
                node.titlebar = target;
                let new_dmg = node.damage_bounds();
                self.dirty = true;
                self.dirty_regions.push(old_dmg);
                self.dirty_regions.push(new_dmg);
            }
        }
    }

    /// Updates the GPU rounded-corner clip radius (in logical pixels) for a managed window node.
    pub fn set_window_corner_radius(&mut self, window_id: WindowId, corner_radius: f32) {
        if let Some(&id) = self.window_nodes.get(&window_id)
            && let Some(node) = self.nodes.get_mut(&id)
        {
            let target = match &node.content {
                NodeContent::Window { fullscreen: true, .. } => 0.0,
                _ => corner_radius.max(0.0),
            };
            if (node.corner_radius - target).abs() > 1e-4 {
                node.corner_radius = target;
                self.dirty = true;
                self.dirty_regions.push(node.damage_bounds());
            }
        }
    }

    /// Removes the retained node for a destroyed or unmapped window.
    pub fn remove_window(&mut self, window_id: WindowId) -> bool {
        let Some(node_id) = self.window_nodes.remove(&window_id) else {
            return false;
        };
        if let Some(removed) = self.nodes.remove(&node_id) {
            self.dirty = true;
            self.dirty_regions.push(removed.damage_bounds());
            true
        } else {
            false
        }
    }

    /// Retains only window nodes whose [`WindowId`] is present in `live_ids`.
    pub fn retain_windows<F>(&mut self, mut keep: F)
    where
        F: FnMut(WindowId) -> bool,
    {
        let to_remove: Vec<WindowId> = self
            .window_nodes
            .keys()
            .copied()
            .filter(|&wid| !keep(wid))
            .collect();
        for wid in to_remove {
            self.remove_window(wid);
        }
    }

    /// Inserts a generic custom node (panels, dock, overview, notifications, cursor).
    pub fn insert_node(&mut self, mut node: SceneNode) -> NodeId {
        let id = self.alloc_id();
        node.id = id;
        let dmg = node.damage_bounds();
        self.nodes.insert(id, node);
        self.dirty = true;
        self.dirty_regions.push(dmg);
        id
    }

    /// Collects all visible nodes in deterministic **front-to-back** order for composition
    /// (highest `SceneLayer` first, then highest `z_index`, tie-broken by `NodeId`).
    pub fn collect_front_to_back(&self) -> Vec<&SceneNode> {
        let mut list: Vec<&SceneNode> = self
            .nodes
            .values()
            .filter(|n| n.visible && n.opacity > 0.001)
            .collect();
        list.sort_by(|a, b| {
            b.layer
                .cmp(&a.layer)
                .then_with(|| b.z_index.cmp(&a.z_index))
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        list
    }

    fn alloc_id(&mut self) -> NodeId {
        self.next_node_id += 1;
        NodeId(self.next_node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_graph_orders_layers_and_windows_front_to_back() {
        let mut scene = SceneGraph::new();
        let bg = scene.set_backdrop(Size::new(1280, 800));
        let w1 = scene.upsert_window(
            WindowId(1),
            Rect::new(Point::new(50, 50), Size::new(400, 300)),
            1,
            true,
            1.0,
            (0.0, 0.0),
            (1.0, 1.0),
            false,
            0.0,
            false,
            Some(ShadowStyle::for_window(0.0, 12.0)),
            Some(BorderStyle::for_window(0.0, 12.0)),
        );
        let w2 = scene.upsert_window(
            WindowId(2),
            Rect::new(Point::new(100, 100), Size::new(400, 300)),
            2,
            true,
            1.0,
            (0.0, 0.0),
            (1.0, 1.0),
            true,
            1.0,
            false,
            Some(ShadowStyle::for_window(1.0, 12.0)),
            Some(BorderStyle::for_window(1.0, 12.0)),
        );
        let panel = scene.insert_node(SceneNode {
            id: NodeId(0),
            layer: SceneLayer::Panels,
            z_index: 0,
            bounds: Rect::from_size(Size::new(1280, 32)),
            visible: true,
            opacity: 1.0,
            offset: (0.0, 0.0),
            scale: (1.0, 1.0),
            clip: None,
            shadow: None,
            border: None,
            titlebar: None,
            corner_radius: 0.0,
            content: NodeContent::SolidRect {
                color: [0.1, 0.1, 0.1, 0.9],
            },
        });

        let ordered: Vec<NodeId> = scene
            .collect_front_to_back()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ordered, vec![panel, w2, w1, bg]);
    }

    #[test]
    fn scene_graph_tracks_dirty_regions_and_skips_unchanged_updates() {
        let mut scene = SceneGraph::new();
        scene.set_backdrop(Size::new(1280, 800));
        assert!(scene.is_dirty());
        let regions = scene.take_dirty_regions();
        assert_eq!(regions.len(), 1);
        assert!(!scene.is_dirty());

        // Identical backdrop size does not re-dirty the scene
        scene.set_backdrop(Size::new(1280, 800));
        assert!(!scene.is_dirty());

        // Fullscreen window strips shadows and borders automatically
        scene.upsert_window(
            WindowId(7),
            Rect::from_size(Size::new(1280, 800)),
            1,
            true,
            1.0,
            (0.0, 0.0),
            (1.0, 1.0),
            true,
            1.0,
            true,
            Some(ShadowStyle::for_window(1.0, 12.0)),
            Some(BorderStyle::for_window(1.0, 12.0)),
        );
        let node = scene
            .collect_front_to_back()
            .into_iter()
            .find(|n| {
                matches!(
                    n.content,
                    NodeContent::Window {
                        window_id: WindowId(7),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(node.shadow.is_none());
        assert!(node.border.is_none());
    }
}

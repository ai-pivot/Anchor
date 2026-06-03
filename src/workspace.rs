//! Workspace management — per-workspace window lists, focus tracking, and unified render order.
//!
//! Each workspace maintains its own set of Wayland toplevels and X11 surfaces,
//! along with a unified `window_order` that determines rendering/focus sequence.

use crate::layout::{LayoutPreset, SplitDir};
use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::shell::xdg::ToplevelSurface,
};

/// Total number of workspaces (1-9, matching Super+1..9 keybindings).
pub const NUM_WORKSPACES: usize = 9;

/// Identifies a window in the unified window list.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowSlot {
    /// Wayland toplevel (index into `Workspace::tops`)
    Wl(usize),
    /// X11 surface (index into `Workspace::x11_surfaces`)
    X11(usize),
}

/// A single workspace holding both Wayland and X11 windows.
pub struct Workspace {
    /// Wayland toplevel surfaces on this workspace.
    pub tops: Vec<ToplevelSurface>,
    /// Currently focused surface (if any).
    pub focus: Option<WlSurface>,
    /// Index into `effective_order()` of the fullscreen window, if any.
    pub fullscreen: Option<usize>,
    /// Active layout preset for this workspace.
    pub layout: LayoutPreset,
    /// Tiling direction for new windows.
    pub split: SplitDir,
    /// One-shot split direction for the next new window (consumed on use).
    pub pending_split: Option<SplitDir>,
    /// X11 (XWayland) surfaces on this workspace.
    pub x11_surfaces: Vec<smithay::xwayland::X11Surface>,
    /// Unified rendering/focus order. Maps flat index → window slot.
    /// When empty, defaults to `[Wl(0), Wl(1), ..., X11(0), X11(1), ...]`.
    pub window_order: Vec<WindowSlot>,
    /// Minimized windows (removed from rendering but not closed).
    pub minimized: Vec<WindowSlot>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            tops: Vec::new(),
            focus: None,
            fullscreen: Option::None,
            layout: LayoutPreset::default(),
            split: SplitDir::Horizontal,
            pending_split: Option::None,
            x11_surfaces: Vec::new(),
            window_order: Vec::new(),
            minimized: Vec::new(),
        }
    }

    /// Get the unified window order. If `window_order` is empty, generate
    /// the default order (all Wayland toplevels first, then all X11 surfaces).
    pub fn effective_order(&self) -> Vec<WindowSlot> {
        if self.window_order.is_empty() {
            let mut order: Vec<WindowSlot> = (0..self.tops.len()).map(WindowSlot::Wl).collect();
            order.extend((0..self.x11_surfaces.len()).map(WindowSlot::X11));
            order
        } else {
            // Filter out invalid entries (windows that were closed)
            self.window_order
                .iter()
                .filter(|s| match s {
                    WindowSlot::Wl(i) => *i < self.tops.len(),
                    WindowSlot::X11(i) => *i < self.x11_surfaces.len(),
                })
                .cloned()
                .collect()
        }
    }

    /// Rebuild `window_order` to match current windows.
    /// Preserves existing order, appends new windows at the end.
    pub fn rebuild_order(&mut self) {
        let n_wl = self.tops.len();
        let n_x11 = self.x11_surfaces.len();
        let mut new_order = Vec::new();
        let mut wl_used = vec![false; n_wl];
        let mut x11_used = vec![false; n_x11];

        // Preserve existing order for windows that still exist
        for slot in &self.window_order {
            match slot {
                WindowSlot::Wl(i) if *i < n_wl && !wl_used[*i] => {
                    new_order.push(slot.clone());
                    wl_used[*i] = true;
                }
                WindowSlot::X11(i) if *i < n_x11 && !x11_used[*i] => {
                    new_order.push(slot.clone());
                    x11_used[*i] = true;
                }
                _ => {}
            }
        }

        // Append any new windows not yet in the order
        for (i, used) in wl_used.iter().enumerate() {
            if !used {
                new_order.push(WindowSlot::Wl(i));
            }
        }
        for (i, used) in x11_used.iter().enumerate() {
            if !used {
                new_order.push(WindowSlot::X11(i));
            }
        }
        self.window_order = new_order;
    }

    /// Insert a new window slot next to the focused window based on split direction.
    /// If `pending_split` is set, uses that (and consumes it).
    /// Otherwise falls back to `self.split`.
    /// If no focused window or only 1 window, appends at end.
    ///
    /// `new_slot`: the WindowSlot to insert (e.g. Wl(idx) or X11(idx))
    pub fn insert_next_to_focus(&mut self, new_slot: WindowSlot) {
        self.rebuild_order();

        let split = self.pending_split.take().unwrap_or(self.split);

        let n = self.window_order.len();
        if n <= 1 {
            // Empty or single window → just append
            self.window_order.push(new_slot);
            return;
        }

        // Find focused window index in window_order
        let fi = self.focus_idx();
        match fi {
            None => {
                // No focus → append at end
                self.window_order.push(new_slot);
            }
            Some(fi) => {
                // Insert relative to focused window
                match split {
                    SplitDir::Vertical => {
                        // Vertical split: new window BELOW focused → insert after focused
                        let insert_at = (fi + 1).min(self.window_order.len());
                        self.window_order.insert(insert_at, new_slot);
                    }
                    SplitDir::Horizontal => {
                        // Horizontal split: new window to the RIGHT of focused → insert after focused
                        let insert_at = (fi + 1).min(self.window_order.len());
                        self.window_order.insert(insert_at, new_slot);
                    }
                }
            }
        }
    }

    /// Find the index in effective_order() of the focused surface.
    fn focus_idx(&self) -> Option<usize> {
        let focus_surf = self.focus.as_ref()?;
        let order = self.effective_order();
        for (i, slot) in order.iter().enumerate() {
            let matches = match slot {
                WindowSlot::Wl(idx) => self
                    .tops
                    .get(*idx)
                    .map(|tl| tl.wl_surface() == focus_surf)
                    .unwrap_or(false),
                WindowSlot::X11(idx) => self
                    .x11_surfaces
                    .get(*idx)
                    .and_then(|xs| xs.wl_surface())
                    .map(|wl| &wl == focus_surf)
                    .unwrap_or(false),
            };
            if matches {
                return Some(i);
            }
        }
        None
    }
}

//! Quake-style dropdown terminal (scratchpad).
//!
//! The scratchpad is a floating terminal overlay toggled with Super+`.
//! The first invocation launches the terminal; subsequent toggles hide/show it.

use tracing::info;

/// Scratchpad state, held inside `App`.
pub struct ScratchpadState {
    /// Running terminal process (kept alive across toggles).
    pub process: Option<std::process::Child>,
    /// Whether the scratchpad overlay is currently visible.
    pub visible: bool,
    /// Wayland toplevel surface for the scratchpad terminal.
    pub surface: Option<smithay::wayland::shell::xdg::ToplevelSurface>,
    /// Flag indicating the next new toplevel should be intercepted as scratchpad.
    pub pending: bool,
}

impl ScratchpadState {
    pub fn new() -> Self {
        Self {
            process: None,
            visible: false,
            surface: None,
            pending: false,
        }
    }

    /// Toggle scratchpad visibility. On first toggle, launches the terminal.
    /// Returns a notification message to display.
    pub fn toggle(&mut self, terminal_cmd: &str, xdisplay: Option<u32>) -> &'static str {
        if self.visible {
            // Hide: keep the terminal running, just stop rendering
            self.visible = false;
            "Scratchpad hidden"
        } else if self.process.is_some() {
            // Show: already have a running terminal, just toggle visibility
            self.visible = true;
            "Scratchpad"
        } else {
            // First time: launch terminal
            self.pending = true;
            let uid = unsafe { libc::getuid() };
            let mut cmd = std::process::Command::new(terminal_cmd);
            cmd.env("WAYLAND_DISPLAY", "wayland-anchor")
                .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
            if let Some(d) = xdisplay {
                cmd.env("DISPLAY", format!(":{}", d));
            }
            match cmd.spawn() {
                Ok(child) => {
                    self.process = Some(child);
                    self.visible = true;
                    info!("Scratchpad launched: {}", terminal_cmd);
                }
                Err(e) => {
                    self.pending = false;
                    info!("Failed to launch scratchpad: {}", e);
                }
            }
            "Scratchpad launched"
        }
    }

    /// Check if the scratchpad should intercept the given new toplevel.
    /// Returns `true` if the toplevel was consumed as scratchpad.
    pub fn intercept_toplevel(
        &mut self,
        tl: smithay::wayland::shell::xdg::ToplevelSurface,
    ) -> bool {
        if self.pending {
            self.surface = Some(tl);
            self.pending = false;
            true
        } else {
            false
        }
    }
}

//! Built-in application launcher.
//!
//! Scans XDG `.desktop` files, provides search filtering, and spawns selected apps.

use tracing::info;

/// Launcher state, held inside `App`.
pub struct LauncherState {
    /// Whether the launcher overlay is currently visible.
    pub visible: bool,
    /// Current search query string.
    pub query: String,
    /// All discovered applications: (display_name, exec_command).
    pub apps: Vec<(String, String)>,
    /// Index of the currently highlighted item in the filtered list.
    pub selected: usize,
}

impl LauncherState {
    pub fn new() -> Self {
        Self {
            visible: false,
            query: String::new(),
            apps: Vec::new(),
            selected: 0,
        }
    }

    /// Scan system and user XDG desktop files to discover applications.
    pub fn load_apps(terminal_cmd: &str) -> Vec<(String, String)> {
        let mut apps = Vec::new();
        let dirs = [
            "/usr/share/applications".to_string(),
            format!(
                "{}/.local/share/applications",
                std::env::var("HOME").unwrap_or_default()
            ),
        ];
        for dir in &dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().to_str() {
                        if name.ends_with(".desktop") {
                            if let Ok(content) = std::fs::read_to_string(name) {
                                let mut app_name = String::new();
                                let mut app_exec = String::new();
                                let mut is_terminal = false;
                                let mut no_display = false;
                                for line in content.lines() {
                                    if line.starts_with("Name=") && app_name.is_empty() {
                                        app_name = line[5..].to_string();
                                    }
                                    if line.starts_with("Exec=") && app_exec.is_empty() {
                                        let exec = &line[5..];
                                        // Remove % parameter placeholders
                                        app_exec = exec
                                            .split_whitespace()
                                            .next()
                                            .unwrap_or(exec)
                                            .to_string();
                                    }
                                    if line.starts_with("Terminal=true") {
                                        is_terminal = true;
                                    }
                                    if line.starts_with("NoDisplay=true") {
                                        no_display = true;
                                    }
                                }
                                if !app_name.is_empty() && !app_exec.is_empty() && !no_display {
                                    if is_terminal {
                                        app_exec = format!("{} {}", terminal_cmd, app_exec);
                                    }
                                    apps.push((app_name, app_exec));
                                }
                            }
                        }
                    }
                }
            }
        }
        apps.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        apps.dedup_by(|a, b| a.0 == b.0);
        apps
    }

    /// Toggle launcher visibility on/off.
    pub fn toggle(&mut self, terminal_cmd: &str) {
        if self.visible {
            self.visible = false;
            self.query.clear();
            self.apps.clear();
        } else {
            self.apps = Self::load_apps(terminal_cmd);
            self.query.clear();
            self.selected = 0;
            self.visible = true;
        }
    }

    /// Return filtered apps matching the current query (for rendering).
    pub fn filtered(&self) -> Vec<(usize, &(String, String))> {
        let q = self.query.to_lowercase();
        self.apps
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| name.to_lowercase().contains(&q))
            .collect()
    }

    /// Move selection up in the filtered list.
    pub fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down in the filtered list.
    pub fn select_down(&mut self) {
        let max = self.filtered().len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// Execute the currently selected application.
    /// Returns `true` if an app was launched.
    pub fn select_and_launch(&mut self, xdisplay: Option<u32>) -> bool {
        let filtered = self.filtered();
        if let Some((_, (_, exec))) = filtered.get(self.selected) {
            let exec_cmd = exec.clone();
            info!("🚀 启动器: {}", exec_cmd);
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&exec_cmd);
            // 继承 anchor 的完整环境（含 GPU 变量）
            cmd.env_clear();
            for (k, v) in std::env::vars() {
                cmd.env(k, v);
            }
            cmd.env("WAYLAND_DISPLAY", "wayland-anchor")
                .env(
                    "XDG_RUNTIME_DIR",
                    format!("/run/user/{}", unsafe { libc::getuid() }),
                );
            if let Some(d) = xdisplay {
                cmd.env("DISPLAY", format!(":{}", d));
            }
            cmd.spawn().ok();
        }
        self.visible = false;
        self.query.clear();
        true
    }

    /// Handle a printable character key in the launcher.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    /// Handle backspace in the launcher.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Close the launcher (Escape).
    pub fn close(&mut self) {
        self.visible = false;
        self.query.clear();
    }
}

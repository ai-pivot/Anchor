use crate::config::Config;
use crate::layout::LayoutPreset;
use crate::screenshot::ScreenshotRequest;
use crate::workspace::{WindowSlot, NUM_WORKSPACES};
use crate::App;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub workspace: usize,
    pub slot_index: usize,
    pub kind: String,
    pub title: String,
    pub app_id: String,
    pub focused: bool,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub index: usize,
    pub active: bool,
    pub layout: String,
    pub window_count: usize,
    pub focused_window: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopStatus {
    pub active_workspace: usize,
    pub focused_output: usize,
    pub locked: bool,
    pub recording: bool,
    pub launcher_visible: bool,
    pub scratchpad_visible: bool,
    pub overview_visible: bool,
    pub settings_visible: bool,
    pub gpu_vendor: String,
    pub gpu_device: Option<String>,
    pub focused_window: Option<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub outputs: Vec<OutputInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputInfo {
    pub index: usize,
    pub focused: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub workspace: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStateInfo {
    pub active_workspace: usize,
    pub focused_output: usize,
    pub locked: bool,
    pub recording: bool,
    pub launcher_visible: bool,
    pub scratchpad_visible: bool,
    pub overview_visible: bool,
    pub settings_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DesktopEvent {
    WorkspaceChanged {
        from: usize,
        to: usize,
    },
    LayoutChanged {
        workspace: usize,
        layout: String,
    },
    WindowFocused {
        workspace: usize,
        title: String,
        app_id: String,
        kind: String,
    },
    WindowOpened {
        workspace: usize,
        title: String,
        app_id: String,
        kind: String,
    },
    WindowClosed {
        workspace: usize,
        title: String,
        app_id: String,
        kind: String,
    },
    WindowMoved {
        from_workspace: usize,
        to_workspace: usize,
        title: String,
        app_id: String,
        kind: String,
    },
    WindowFullscreenChanged {
        workspace: usize,
        fullscreen: bool,
    },
    ScratchpadChanged {
        visible: bool,
    },
    OverviewChanged {
        visible: bool,
    },
    SettingsChanged {
        visible: bool,
    },
    OutputFocusedChanged {
        output: usize,
    },
    LockChanged {
        locked: bool,
    },
    ScreenshotCompleted {
        path: String,
    },
    ScreenshotFailed {
        reason: String,
    },
    RecordChanged {
        recording: bool,
    },
    AppLaunched {
        app: String,
    },
    ConfigReloaded,
}

#[derive(Debug, Clone)]
pub enum DesktopAction {
    SwitchWorkspace {
        index: usize,
    },
    SwitchWorkspaceRelative {
        delta: i32,
    },
    FocusWindow {
        app_id: Option<String>,
        title: Option<String>,
        workspace: Option<usize>,
    },
    MoveWindow {
        app_id: Option<String>,
        title: Option<String>,
        workspace: Option<usize>,
        target_workspace: usize,
    },
    SetWindowFullscreen {
        app_id: Option<String>,
        title: Option<String>,
        workspace: Option<usize>,
    },
    CloseWindow {
        app_id: Option<String>,
        title: Option<String>,
        workspace: Option<usize>,
    },
    SetLayout {
        preset: LayoutPreset,
    },
    CycleLayout,
    ToggleScratchpad,
    ShowOverview,
    HideOverview,
    ShowSettings,
    HideSettings,
    Lock,
    ReloadConfig,
    ToggleFullscreen,
    CloseFocusedWindow,
    ScreenshotFull,
    ScreenshotArea {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    RecordStart,
    RecordStop,
    LaunchApp {
        app: String,
    },
    Notify {
        text: String,
    },
}

pub fn dispatch_action(app: &mut App, action: DesktopAction) -> Result<Vec<DesktopEvent>, String> {
    match action {
        DesktopAction::SwitchWorkspace { index } => {
            if index >= NUM_WORKSPACES {
                return Err(format!("workspace {} out of range", index));
            }
            let from = app.active_ws;
            app.switch_workspace(index);
            Ok(vec![DesktopEvent::WorkspaceChanged {
                from,
                to: app.active_ws,
            }])
        }
        DesktopAction::SwitchWorkspaceRelative { delta } => {
            let from = app.active_ws;
            app.switch_workspace_direction(delta);
            Ok(vec![DesktopEvent::WorkspaceChanged {
                from,
                to: app.active_ws,
            }])
        }
        DesktopAction::FocusWindow {
            app_id,
            title,
            workspace,
        } => {
            let target = find_window(app, &app_id, &title, workspace)?;
            let target_ws = target.workspace;
            app.switch_workspace(target_ws);

            let ws = &app.workspaces[target_ws];
            let target_surface = ws
                .effective_order()
                .get(target.slot_index)
                .and_then(|slot| match slot {
                    WindowSlot::Wl(i) => ws.tops.get(*i).map(|tl| tl.wl_surface().clone()),
                    WindowSlot::X11(i) => ws.x11_surfaces.get(*i).and_then(|xs| xs.wl_surface()),
                })
                .ok_or_else(|| "matching window surface not available".to_string())?;

            app.workspaces[target_ws].focus = Some(target_surface.clone());
            let kbd = app.kbd.clone();
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            kbd.set_focus(app, Some(target_surface), serial);

            Ok(focused_window_event(app).into_iter().collect())
        }
        DesktopAction::MoveWindow {
            app_id,
            title,
            workspace,
            target_workspace,
        } => {
            if target_workspace >= NUM_WORKSPACES {
                return Err(format!("workspace {} out of range", target_workspace));
            }
            let target = find_window(app, &app_id, &title, workspace)?;
            let current_ws = app.active_ws;
            dispatch_action(
                app,
                DesktopAction::FocusWindow {
                    app_id: Some(target.app_id.clone()),
                    title: Some(target.title.clone()),
                    workspace: Some(target.workspace),
                },
            )?;
            app.move_window_to_workspace(target_workspace);
            if current_ws != app.active_ws {
                app.switch_workspace(current_ws);
            }
            Ok(vec![DesktopEvent::WindowMoved {
                from_workspace: target.workspace,
                to_workspace: target_workspace,
                title: target.title,
                app_id: target.app_id,
                kind: target.kind,
            }])
        }
        DesktopAction::SetWindowFullscreen {
            app_id,
            title,
            workspace,
        } => {
            let target = find_window(app, &app_id, &title, workspace)?;
            let current_ws = app.active_ws;
            dispatch_action(
                app,
                DesktopAction::FocusWindow {
                    app_id: Some(target.app_id.clone()),
                    title: Some(target.title.clone()),
                    workspace: Some(target.workspace),
                },
            )?;
            app.toggle_fullscreen();
            let fullscreen = app.workspaces[app.active_ws].fullscreen.is_some();
            if current_ws != app.active_ws {
                app.switch_workspace(current_ws);
            }
            Ok(vec![DesktopEvent::WindowFullscreenChanged {
                workspace: target.workspace,
                fullscreen,
            }])
        }
        DesktopAction::CloseWindow {
            app_id,
            title,
            workspace,
        } => {
            let target = find_window(app, &app_id, &title, workspace)?;
            let current_ws = app.active_ws;
            dispatch_action(
                app,
                DesktopAction::FocusWindow {
                    app_id: Some(target.app_id.clone()),
                    title: Some(target.title.clone()),
                    workspace: Some(target.workspace),
                },
            )?;
            app.close_focused_window()?;
            if current_ws != app.active_ws {
                app.switch_workspace(current_ws);
            }
            Ok(vec![])
        }
        DesktopAction::SetLayout { preset } => {
            let ws = app.active_ws;
            app.workspaces[ws].layout = preset;
            app.do_layout_animated();
            app.dirty = true;
            Ok(vec![DesktopEvent::LayoutChanged {
                workspace: ws,
                layout: format!("{:?}", app.workspaces[ws].layout),
            }])
        }
        DesktopAction::CycleLayout => {
            let ws = app.active_ws;
            app.workspaces[ws].layout = app.workspaces[ws].layout.next();
            app.do_layout_animated();
            app.dirty = true;
            Ok(vec![DesktopEvent::LayoutChanged {
                workspace: ws,
                layout: format!("{:?}", app.workspaces[ws].layout),
            }])
        }
        DesktopAction::ToggleScratchpad => {
            let xdisplay = app.xdisplay;
            app.scratchpad.toggle(&app.cfg.terminal.command, xdisplay);
            app.dirty = true;
            Ok(vec![DesktopEvent::ScratchpadChanged {
                visible: app.scratchpad.visible,
            }])
        }
        DesktopAction::ShowOverview => {
            let focus = app.focus_idx().unwrap_or(0);
            let total = app.workspaces[app.active_ws].effective_order().len();
            app.overview.open_expose(total, focus);
            app.dirty = true;
            Ok(vec![DesktopEvent::OverviewChanged { visible: true }])
        }
        DesktopAction::HideOverview => {
            app.overview.close();
            app.dirty = true;
            Ok(vec![DesktopEvent::OverviewChanged { visible: false }])
        }
        DesktopAction::ShowSettings => {
            let cfg = app.cfg.clone();
            app.settings.open(&cfg);
            app.dirty = true;
            Ok(vec![DesktopEvent::SettingsChanged { visible: true }])
        }
        DesktopAction::HideSettings => {
            app.settings.close();
            app.dirty = true;
            Ok(vec![DesktopEvent::SettingsChanged { visible: false }])
        }
        DesktopAction::Lock => {
            app.lock_state.lock(app.pointer_pos.0);
            app.dirty = true;
            Ok(vec![DesktopEvent::LockChanged {
                locked: app.lock_state.locked,
            }])
        }
        DesktopAction::ReloadConfig => {
            app.reload_config();
            Ok(vec![DesktopEvent::ConfigReloaded])
        }
        DesktopAction::ToggleFullscreen => {
            app.toggle_fullscreen();
            let fullscreen = app.workspaces[app.active_ws].fullscreen.is_some();
            Ok(vec![DesktopEvent::WindowFullscreenChanged {
                workspace: app.active_ws,
                fullscreen,
            }])
        }
        DesktopAction::CloseFocusedWindow => {
            app.close_focused_window()?;
            Ok(vec![])
        }
        DesktopAction::ScreenshotFull => {
            app.pending_screenshot = Some(ScreenshotRequest::Full);
            app.dirty = true;
            Ok(vec![])
        }
        DesktopAction::ScreenshotArea { x, y, w, h } => {
            if w <= 0 || h <= 0 {
                return Err("invalid screenshot area".into());
            }
            app.pending_screenshot = Some(ScreenshotRequest::Area(x, y, w, h));
            app.dirty = true;
            Ok(vec![])
        }
        DesktopAction::RecordStart => {
            let (w, h) = (app.osize.w.max(1) as u32, app.osize.h.max(1) as u32);
            app.record_state.start(w, h);
            app.dirty = true;
            Ok(vec![DesktopEvent::RecordChanged {
                recording: app.record_state.recording,
            }])
        }
        DesktopAction::RecordStop => {
            app.record_state.stop();
            app.dirty = true;
            Ok(vec![DesktopEvent::RecordChanged {
                recording: app.record_state.recording,
            }])
        }
        DesktopAction::LaunchApp { app: target } => {
            let xdisplay = app.xdisplay;
            if app
                .launcher
                .launch_by_name(&target, xdisplay, &app.cfg.terminal.command)
            {
                Ok(vec![DesktopEvent::AppLaunched { app: target }])
            } else {
                Err(format!("app not found: {}", target))
            }
        }
        DesktopAction::Notify { text } => {
            app.notify(text);
            app.dirty = true;
            Ok(vec![])
        }
    }
}

fn find_window(
    app: &App,
    app_id: &Option<String>,
    title: &Option<String>,
    workspace: Option<usize>,
) -> Result<WindowInfo, String> {
    if app_id.is_none() && title.is_none() && workspace.is_none() {
        return Err("at least one of app_id, title, or workspace must be specified".to_string());
    }
    query_windows(app)
        .into_iter()
        .find(|w| {
            if let Some(ws) = workspace {
                if w.workspace != ws {
                    return false;
                }
            }
            if let Some(app_id_filter) = app_id {
                if &w.app_id != app_id_filter {
                    return false;
                }
            }
            if let Some(title_filter) = title {
                if !w.title.contains(title_filter) {
                    return false;
                }
            }
            true
        })
        .ok_or_else(|| "no matching window".to_string())
}

pub fn query_status(app: &App) -> DesktopStatus {
    DesktopStatus {
        active_workspace: app.active_ws,
        focused_output: app.focused_output,
        locked: app.lock_state.locked,
        recording: app.record_state.recording,
        launcher_visible: app.launcher.visible,
        scratchpad_visible: app.scratchpad.visible,
        overview_visible: app.overview.is_active(),
        settings_visible: app.settings.is_active(),
        gpu_vendor: app.current_gpu_vendor(),
        gpu_device: std::env::var("TITAN_DRM_DEV").ok(),
        focused_window: query_focused_window(app),
        workspaces: query_workspaces(app),
        outputs: query_outputs(app),
    }
}

pub fn query_workspaces(app: &App) -> Vec<WorkspaceInfo> {
    app.workspaces
        .iter()
        .enumerate()
        .map(|(idx, ws)| WorkspaceInfo {
            index: idx,
            active: idx == app.active_ws,
            layout: format!("{:?}", ws.layout),
            window_count: ws.tops.len() + ws.x11_surfaces.len(),
            focused_window: ws.focus.as_ref().map(|s| format!("{:?}", s)),
        })
        .collect()
}

pub fn query_outputs(app: &App) -> Vec<OutputInfo> {
    app.output_sizes
        .iter()
        .enumerate()
        .map(|(idx, (x, y, w, h))| OutputInfo {
            index: idx,
            focused: idx == app.focused_output,
            x: *x,
            y: *y,
            width: *w,
            height: *h,
            workspace: app.output_active_ws[idx],
        })
        .collect()
}

pub fn query_runtime_state(app: &App) -> RuntimeStateInfo {
    RuntimeStateInfo {
        active_workspace: app.active_ws,
        focused_output: app.focused_output,
        locked: app.lock_state.locked,
        recording: app.record_state.recording,
        launcher_visible: app.launcher.visible,
        scratchpad_visible: app.scratchpad.visible,
        overview_visible: app.overview.is_active(),
        settings_visible: app.settings.is_active(),
    }
}

pub fn query_windows(app: &App) -> Vec<WindowInfo> {
    let mut out = Vec::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        let order = ws.effective_order();
        for (slot_idx, slot) in order.iter().enumerate() {
            match slot {
                WindowSlot::Wl(i) => {
                    if let Some(tl) = ws.tops.get(*i) {
                        let surf = tl.wl_surface();
                        let focused = ws.focus.as_ref().map(|f| f == surf).unwrap_or(false);
                        out.push(WindowInfo {
                            workspace: ws_idx,
                            slot_index: slot_idx,
                            kind: "wayland".into(),
                            title: app.title_for_surface(surf),
                            app_id: app.app_id_for_surface(surf),
                            focused,
                            fullscreen: ws.fullscreen == Some(slot_idx),
                        });
                    }
                }
                WindowSlot::X11(i) => {
                    if let Some(xs) = ws.x11_surfaces.get(*i) {
                        let wl = xs.wl_surface();
                        let focused = wl
                            .as_ref()
                            .and_then(|surf| ws.focus.as_ref().map(|f| f == surf))
                            .unwrap_or(false);
                        let title = xs.title();
                        let app_id = xs.class();
                        out.push(WindowInfo {
                            workspace: ws_idx,
                            slot_index: slot_idx,
                            kind: "x11".into(),
                            title,
                            app_id,
                            focused,
                            fullscreen: ws.fullscreen == Some(slot_idx),
                        });
                    }
                }
            }
        }
    }
    out
}

pub fn query_focused_window(app: &App) -> Option<WindowInfo> {
    query_windows(app).into_iter().find(|w| w.focused)
}

pub fn query_config(app: &App) -> Config {
    app.cfg.clone()
}

pub fn focused_window_event(app: &App) -> Option<DesktopEvent> {
    query_focused_window(app).map(|w| DesktopEvent::WindowFocused {
        workspace: w.workspace,
        title: w.title,
        app_id: w.app_id,
        kind: w.kind,
    })
}

pub fn emit_event(app: &mut App, event: DesktopEvent) {
    if let Some(ipc) = app.ipc.as_mut() {
        ipc.broadcast_event(&event);
    }
}

pub fn emit_output_focused_changed(app: &mut App) {
    emit_event(
        app,
        DesktopEvent::OutputFocusedChanged {
            output: app.focused_output,
        },
    );
}

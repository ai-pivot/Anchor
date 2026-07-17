use crate::appctl::{self, DesktopAction, DesktopEvent};
use crate::layout::LayoutPreset;
use crate::App;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: Option<u64>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    subscriptions: HashSet<String>,
}

pub struct IpcServer {
    listener: UnixListener,
    clients: HashMap<u64, Client>,
    next_client_id: u64,
    socket_path: String,
}

impl IpcServer {
    pub fn bind_default() -> std::io::Result<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
        let dir = format!("{}/anchor", runtime);
        fs::create_dir_all(&dir)?;
        let path = format!("{}/ctl.sock", dir);
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            clients: HashMap::new(),
            next_client_id: 1,
            socket_path: path,
        })
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    pub fn poll(&mut self, app: &mut App) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if self.clients.len() >= 8 {
                        let _ = write_json_line(
                            &stream,
                            &Response {
                                id: None,
                                result: None,
                                error: Some(ErrorBody {
                                    code: "too_many_clients".into(),
                                    message: "too many clients".into(),
                                }),
                            },
                        );
                        continue;
                    }
                    if !verify_client(&stream) {
                        let _ = write_json_line(
                            &stream,
                            &Response {
                                id: None,
                                result: None,
                                error: Some(ErrorBody {
                                    code: "unauthorized".into(),
                                    message: "unauthorized client".into(),
                                }),
                            },
                        );
                        continue;
                    }
                    let _ = stream.set_nonblocking(true);
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    self.clients.insert(
                        id,
                        Client {
                            stream,
                            buffer: Vec::new(),
                            subscriptions: HashSet::new(),
                        },
                    );
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let ids: Vec<u64> = self.clients.keys().copied().collect();
        let mut dead = Vec::new();
        let mut outgoing: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut broadcast_events: Vec<DesktopEvent> = Vec::new();
        for id in ids {
            let mut requests = Vec::new();
            let mut should_close = false;
            if let Some(client) = self.clients.get_mut(&id) {
                let mut tmp = [0u8; 8192];
                loop {
                    match client.stream.read(&mut tmp) {
                        Ok(0) => {
                            should_close = true;
                            break;
                        }
                        Ok(n) => {
                            client.buffer.extend_from_slice(&tmp[..n]);
                            while let Some(pos) = client.buffer.iter().position(|b| *b == b'\n') {
                                let line = client.buffer.drain(..=pos).collect::<Vec<_>>();
                                let line = String::from_utf8_lossy(&line);
                                let line = line.trim();
                                if !line.is_empty() {
                                    requests.push(line.to_string());
                                }
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {
                            should_close = true;
                            break;
                        }
                    }
                }
            }
            if should_close {
                dead.push(id);
                continue;
            }
            for raw in requests {
                let (resp, events, direct_event) = self.handle_request(id, app, &raw);
                if let Some(resp) = resp {
                    if let Some(bytes) = serialize_line(&resp) {
                        outgoing.push((id, bytes));
                    }
                }
                broadcast_events.extend(events);
                if let Some(event) = direct_event {
                    if let Ok(bytes) =
                        serde_json::to_vec(&json!({"method": event_name(&event), "params": event}))
                    {
                        let mut bytes = bytes;
                        bytes.push(b'\n');
                        outgoing.push((id, bytes));
                    }
                }
            }
        }
        for (id, bytes) in outgoing {
            if let Some(client) = self.clients.get_mut(&id) {
                let _ = client.stream.write_all(&bytes);
            }
        }
        for event in broadcast_events {
            self.broadcast_event(&event);
        }
        for id in dead {
            self.clients.remove(&id);
        }
    }

    fn handle_request(
        &mut self,
        client_id: u64,
        app: &mut App,
        raw: &str,
    ) -> (Option<Response>, Vec<DesktopEvent>, Option<DesktopEvent>) {
        let req: Request = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                return (
                    Some(Response {
                        id: None,
                        result: None,
                        error: Some(ErrorBody {
                            code: "bad_request".into(),
                            message: e.to_string(),
                        }),
                    }),
                    vec![],
                    None,
                )
            }
        };

        macro_rules! early_err {
            ($msg:expr) => {
                return (
                    Some(Response {
                        id: req.id,
                        result: None,
                        error: Some(ErrorBody {
                            code: "command_error".into(),
                            message: $msg,
                        }),
                    }),
                    vec![],
                    None,
                )
            };
        }
        macro_rules! must {
            ($expr:expr) => {
                match $expr {
                    Ok(v) => v,
                    Err(e) => early_err!(e.to_string()),
                }
            };
        }
        macro_rules! need {
            ($opt:expr, $msg:expr) => {
                match $opt {
                    Some(v) => v,
                    None => early_err!($msg.to_string()),
                }
            };
        }

        let result: Result<(Value, Vec<DesktopEvent>), String> = match req.method.as_str() {
            "system.status" => Ok((json!(appctl::query_status(app)), vec![])),
            "system.state" => Ok((json!(appctl::query_runtime_state(app)), vec![])),
            "output.list" => Ok((json!(appctl::query_outputs(app)), vec![])),
            "workspace.list" => Ok((json!(appctl::query_workspaces(app)), vec![])),
            "window.list" => {
                let app_id = req
                    .params
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let title = req
                    .params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = req
                    .params
                    .get("workspace")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let windows = appctl::query_windows(app)
                    .into_iter()
                    .filter(|w| workspace.map(|ws| w.workspace == ws).unwrap_or(true))
                    .filter(|w| app_id.as_ref().map(|x| &w.app_id == x).unwrap_or(true))
                    .filter(|w| title.as_ref().map(|x| w.title.contains(x)).unwrap_or(true))
                    .collect::<Vec<_>>();
                Ok((json!(windows), vec![]))
            }
            "window.focused" => Ok((json!(appctl::query_focused_window(app)), vec![])),
            "config.get" => Ok((json!(appctl::query_config(app)), vec![])),
            "workspace.switch" => {
                let index = need!(
                    req.params.get("index").and_then(|v| v.as_u64()),
                    "missing index"
                ) as usize;
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::SwitchWorkspace { index }
                ));
                Ok((
                    json!({"ok": true, "active_workspace": app.active_ws}),
                    events,
                ))
            }
            "workspace.next" => {
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::SwitchWorkspaceRelative { delta: 1 }
                ));
                Ok((
                    json!({"ok": true, "active_workspace": app.active_ws}),
                    events,
                ))
            }
            "workspace.prev" => {
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::SwitchWorkspaceRelative { delta: -1 }
                ));
                Ok((
                    json!({"ok": true, "active_workspace": app.active_ws}),
                    events,
                ))
            }
            "window.focus" => {
                let app_id = req
                    .params
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let title = req
                    .params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = req
                    .params
                    .get("workspace")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::FocusWindow {
                        app_id,
                        title,
                        workspace
                    }
                ));
                Ok((json!({"ok": true}), events))
            }
            "window.move_to_workspace" => {
                let target_workspace = need!(
                    req.params.get("workspace").and_then(|v| v.as_u64()),
                    "missing workspace"
                ) as usize;
                let app_id = req
                    .params
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let title = req
                    .params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = req
                    .params
                    .get("source_workspace")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::MoveWindow {
                        app_id,
                        title,
                        workspace,
                        target_workspace
                    }
                ));
                Ok((json!({"ok": true}), events))
            }
            "layout.set" => {
                let preset_name = need!(
                    req.params.get("preset").and_then(|v| v.as_str()),
                    "missing preset"
                );
                let preset = must!(parse_layout(preset_name));
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::SetLayout { preset }
                ));
                Ok((json!({"ok": true}), events))
            }
            "layout.cycle" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::CycleLayout));
                Ok((json!({"ok": true}), events))
            }
            "scratchpad.toggle" => {
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::ToggleScratchpad
                ));
                Ok((json!({"ok": true}), events))
            }
            "overview.show" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::ShowOverview));
                Ok((json!({"ok": true}), events))
            }
            "overview.hide" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::HideOverview));
                Ok((json!({"ok": true}), events))
            }
            "settings.show" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::ShowSettings));
                Ok((json!({"ok": true}), events))
            }
            "settings.hide" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::HideSettings));
                Ok((json!({"ok": true}), events))
            }
            "lock.activate" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::Lock));
                Ok((json!({"ok": true, "locked": app.lock_state.locked}), events))
            }
            "config.reload" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::ReloadConfig));
                Ok((json!({"ok": true}), events))
            }
            "window.fullscreen.toggle" => {
                let app_id = req
                    .params
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let title = req
                    .params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = req
                    .params
                    .get("workspace")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::SetWindowFullscreen {
                        app_id,
                        title,
                        workspace
                    }
                ));
                Ok((json!({"ok": true}), events))
            }
            "window.close" => {
                let app_id = req
                    .params
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let title = req
                    .params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = req
                    .params
                    .get("workspace")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::CloseWindow {
                        app_id,
                        title,
                        workspace
                    }
                ));
                Ok((json!({"ok": true}), events))
            }
            "notify.send" => {
                let text = need!(
                    req.params.get("text").and_then(|v| v.as_str()),
                    "missing text"
                )
                .to_string();
                let events = must!(appctl::dispatch_action(app, DesktopAction::Notify { text }));
                Ok((json!({"ok": true}), events))
            }
            "screenshot.capture" => {
                let mode = req
                    .params
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("full");
                let events = if mode == "area" {
                    let x = need!(req.params.get("x").and_then(|v| v.as_i64()), "missing x") as i32;
                    let y = need!(req.params.get("y").and_then(|v| v.as_i64()), "missing y") as i32;
                    let w = need!(req.params.get("w").and_then(|v| v.as_i64()), "missing w") as i32;
                    let h = need!(req.params.get("h").and_then(|v| v.as_i64()), "missing h") as i32;
                    must!(appctl::dispatch_action(
                        app,
                        DesktopAction::ScreenshotArea { x, y, w, h }
                    ))
                } else {
                    must!(appctl::dispatch_action(app, DesktopAction::ScreenshotFull))
                };
                Ok((json!({"accepted": true}), events))
            }
            "record.start" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::RecordStart));
                Ok((
                    json!({"ok": true, "recording": app.record_state.recording}),
                    events,
                ))
            }
            "record.stop" => {
                let events = must!(appctl::dispatch_action(app, DesktopAction::RecordStop));
                Ok((
                    json!({"ok": true, "recording": app.record_state.recording}),
                    events,
                ))
            }
            "app.launch" => {
                let app_name = need!(
                    req.params.get("app").and_then(|v| v.as_str()),
                    "missing app"
                )
                .to_string();
                let events = must!(appctl::dispatch_action(
                    app,
                    DesktopAction::LaunchApp { app: app_name }
                ));
                Ok((json!({"ok": true}), events))
            }
            "event.subscribe" => {
                let events_array = need!(
                    req.params.get("events").and_then(|v| v.as_array()),
                    "missing events"
                );
                let names = events_array
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect::<HashSet<_>>();
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.subscriptions.extend(names);
                }
                Ok((json!({"ok": true}), vec![]))
            }
            _ => Err(format!("unknown method: {}", req.method)),
        };

        match result {
            Ok((value, events)) => (
                Some(Response {
                    id: req.id,
                    result: Some(value),
                    error: None,
                }),
                events,
                None,
            ),
            Err(msg) => (
                Some(Response {
                    id: req.id,
                    result: None,
                    error: Some(ErrorBody {
                        code: "command_error".into(),
                        message: msg,
                    }),
                }),
                vec![],
                None,
            ),
        }
    }

    pub fn broadcast_event(&mut self, event: &DesktopEvent) {
        let name = event_name(event);
        let payload = json!({"method": name, "params": event});
        if let Some(bytes) = serialize_line(&payload) {
            for client in self.clients.values_mut() {
                if client.subscriptions.contains(name) {
                    let _ = client.stream.write_all(&bytes);
                }
            }
        }
    }
}

fn event_name(event: &DesktopEvent) -> &'static str {
    match event {
        DesktopEvent::WorkspaceChanged { .. } => "workspace.changed",
        DesktopEvent::LayoutChanged { .. } => "layout.changed",
        DesktopEvent::WindowFocused { .. } => "window.focused",
        DesktopEvent::WindowOpened { .. } => "window.opened",
        DesktopEvent::WindowClosed { .. } => "window.closed",
        DesktopEvent::WindowMoved { .. } => "window.moved",
        DesktopEvent::WindowFullscreenChanged { .. } => "window.fullscreen.changed",
        DesktopEvent::ScratchpadChanged { .. } => "scratchpad.changed",
        DesktopEvent::OverviewChanged { .. } => "overview.changed",
        DesktopEvent::SettingsChanged { .. } => "settings.changed",
        DesktopEvent::OutputFocusedChanged { .. } => "output.focused.changed",
        DesktopEvent::LockChanged { .. } => "lock.changed",
        DesktopEvent::ScreenshotCompleted { .. } => "screenshot.completed",
        DesktopEvent::ScreenshotFailed { .. } => "screenshot.failed",
        DesktopEvent::RecordChanged { .. } => "record.changed",
        DesktopEvent::AppLaunched { .. } => "app.launched",
        DesktopEvent::ConfigReloaded => "config.reloaded",
    }
}

fn parse_layout(s: &str) -> Result<LayoutPreset, String> {
    match s.to_ascii_lowercase().as_str() {
        "master-stack" | "masterstack" => Ok(LayoutPreset::MasterStack),
        "columns" => Ok(LayoutPreset::Columns),
        "center" => Ok(LayoutPreset::Center),
        "grid" => Ok(LayoutPreset::Grid),
        _ => Err(format!("unknown layout: {}", s)),
    }
}

fn verify_client(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;

    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let ok = libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        ) == 0;
        ok && cred.uid == libc::getuid()
    }
}

fn serialize_line<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    serde_json::to_vec(value).ok().map(|mut bytes| {
        bytes.push(b'\n');
        bytes
    })
}

fn write_json_line<T: Serialize>(stream: &UnixStream, value: &T) -> std::io::Result<()> {
    let bytes = serialize_line(value)
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidData, "serialize json line failed"))?;
    (&*stream).write_all(&bytes)
}

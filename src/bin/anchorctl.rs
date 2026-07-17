use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "anchorctl",
    version,
    about = "Agent-native CLI for the Anchor desktop"
)]
struct Cli {
    #[arg(long, global = true, help = "Print the full JSON response")]
    json: bool,
    #[arg(long, global = true, help = "Print event stream as NDJSON")]
    ndjson: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "Full desktop status snapshot (recommended for agents)")]
    Status,
    #[command(about = "Compact runtime state summary")]
    State,
    #[command(about = "Query output topology and focused output")]
    Output(OutputCmd),
    Workspace(WorkspaceCmd),
    Layout(LayoutCmd),
    Window(WindowCmd),
    Scratchpad,
    Overview(ShowHideCmd),
    Settings(ShowHideCmd),
    Lock,
    Config(ConfigCmd),
    Notify(NotifyCmd),
    Screenshot(ScreenshotCmd),
    Record(RecordCmd),
    App(AppCmd),
    Events(EventsCmd),
    Exec(ExecCmd),
}

#[derive(Args, Debug)]
struct OutputCmd {
    #[command(subcommand)]
    action: OutputAction,
}

#[derive(Subcommand, Debug)]
enum OutputAction {
    List,
}

#[derive(Args, Debug)]
struct WorkspaceCmd {
    #[command(subcommand)]
    action: WorkspaceAction,
}

#[derive(Subcommand, Debug)]
enum WorkspaceAction {
    List,
    Switch { index: usize },
    Next,
    Prev,
}

#[derive(Args, Debug)]
struct LayoutCmd {
    #[command(subcommand)]
    action: LayoutAction,
}

#[derive(Subcommand, Debug)]
enum LayoutAction {
    Set { preset: LayoutPresetArg },
    Cycle,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LayoutPresetArg {
    #[value(name = "master-stack")]
    MasterStack,
    Columns,
    Center,
    Grid,
}

#[derive(Args, Debug)]
struct WindowCmd {
    #[command(subcommand)]
    action: WindowAction,
}

#[derive(Subcommand, Debug)]
enum WindowAction {
    List(WindowListArgs),
    Focused,
    Focus(WindowSelectArgs),
    Close(WindowSelectArgs),
    Fullscreen(WindowFullscreenCmd),
    Move(WindowMoveArgs),
}

#[derive(Args, Debug, Default)]
struct WindowSelectArgs {
    #[arg(long)]
    app_id: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    workspace: Option<usize>,
}

#[derive(Args, Debug, Default)]
struct WindowListArgs {
    #[arg(long)]
    app_id: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    workspace: Option<usize>,
}

#[derive(Args, Debug)]
struct WindowMoveArgs {
    #[arg(long = "to", help = "Target workspace (1-based)")]
    to: usize,
    #[arg(long = "from-workspace", help = "Source workspace filter (1-based)")]
    from_workspace: Option<usize>,
    #[arg(long)]
    app_id: Option<String>,
    #[arg(long)]
    title: Option<String>,
}

#[derive(Args, Debug)]
struct WindowFullscreenCmd {
    #[command(subcommand)]
    action: WindowFullscreenAction,
    #[command(flatten)]
    select: WindowSelectArgs,
}

#[derive(Subcommand, Debug)]
enum WindowFullscreenAction {
    Toggle,
}

#[derive(Args, Debug)]
struct ShowHideCmd {
    #[command(subcommand)]
    action: ShowHideAction,
}

#[derive(Subcommand, Debug)]
enum ShowHideAction {
    Show,
    Hide,
}

#[derive(Args, Debug)]
struct ConfigCmd {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    Reload,
    Get,
}

#[derive(Args, Debug)]
struct NotifyCmd {
    #[command(subcommand)]
    action: NotifyAction,
}

#[derive(Subcommand, Debug)]
enum NotifyAction {
    Send { text: String },
}

#[derive(Args, Debug)]
struct ScreenshotCmd {
    #[command(subcommand)]
    action: ScreenshotAction,
}

#[derive(Subcommand, Debug)]
enum ScreenshotAction {
    Full(WaitArgs),
    Area(ScreenshotAreaArgs),
}

#[derive(Args, Debug, Clone)]
struct WaitArgs {
    #[arg(long, help = "Wait for the corresponding event")]
    wait: bool,
    #[arg(long, default_value_t = 10, help = "Wait timeout in seconds")]
    timeout: u64,
    #[arg(long, help = "Only accept events whose method matches this value")]
    wait_for: Option<String>,
    #[arg(
        long = "match",
        help = "Filter event payload by key=value, e.g. app_id=firefox",
        value_name = "KEY=VALUE"
    )]
    matches: Vec<String>,
}

#[derive(Args, Debug)]
struct ScreenshotAreaArgs {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    #[command(flatten)]
    wait: WaitArgs,
}

#[derive(Args, Debug)]
struct RecordCmd {
    #[command(subcommand)]
    action: RecordAction,
}

#[derive(Subcommand, Debug)]
enum RecordAction {
    Start,
    Stop,
}

#[derive(Args, Debug)]
struct AppCmd {
    #[command(subcommand)]
    action: AppAction,
}

#[derive(Subcommand, Debug)]
enum AppAction {
    Launch(AppLaunchArgs),
}

#[derive(Args, Debug)]
struct AppLaunchArgs {
    app: String,
    #[command(flatten)]
    wait: WaitArgs,
}

#[derive(Args, Debug)]
struct EventsCmd {
    #[arg(required = true, help = "Event names to subscribe to")]
    events: Vec<String>,
}

#[derive(Args, Debug)]
struct ExecCmd {
    #[arg(help = "Raw JSON request")]
    request: String,
}

fn socket_path() -> String {
    let runtime = env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    format!("{}/anchor/ctl.sock", runtime)
}

fn serialize_line(value: &Value) -> Result<Vec<u8>, String> {
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    Ok(line)
}

fn print_json(v: &Value, ndjson: bool) {
    if ndjson {
        println!("{}", v);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        );
    }
}

fn response_ok(resp: &Value) -> bool {
    resp.get("error").is_none()
}

fn print_response(resp: &Value, json_out: bool, ndjson: bool) -> i32 {
    if json_out {
        print_json(resp, ndjson);
        return if response_ok(resp) { 0 } else { 1 };
    }
    if let Some(result) = resp.get("result") {
        if result.is_object() || result.is_array() {
            print_json(result, ndjson);
        } else {
            println!("{}", result);
        }
        return 0;
    }
    if let Some(error) = resp.get("error") {
        if ndjson {
            println!("{}", error);
        } else {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(error).unwrap_or_else(|_| error.to_string())
            );
        }
        return 1;
    }
    0
}

struct Subscription {
    reader: BufReader<UnixStream>,
}

impl Subscription {
    fn connect(events: &[impl AsRef<str>], timeout: Option<Duration>) -> Result<Self, String> {
        let mut stream = UnixStream::connect(socket_path()).map_err(|e| e.to_string())?;
        let names = events
            .iter()
            .map(|s| s.as_ref().to_string())
            .collect::<Vec<_>>();
        let req = json!({"id":1,"method":"event.subscribe","params":{"events": names}});
        let line = serialize_line(&req)?;
        stream.write_all(&line).map_err(|e| e.to_string())?;
        let _ = stream.set_read_timeout(timeout);
        let mut reader = BufReader::new(stream);
        let mut ack = String::new();
        reader.read_line(&mut ack).map_err(|e| e.to_string())?;
        let ack_value: Value = serde_json::from_str(&ack).map_err(|e| e.to_string())?;
        if ack_value.get("error").is_some() {
            return Err(format!("subscribe failed: {}", ack.trim()));
        }
        Ok(Self { reader })
    }

    fn next_event(&mut self) -> Result<Option<Value>, String> {
        loop {
            let mut buf = String::new();
            match self.reader.read_line(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    if buf.trim().is_empty() {
                        continue;
                    }
                    let value: Value = serde_json::from_str(&buf).map_err(|e| e.to_string())?;
                    if value.get("method").is_some() {
                        return Ok(Some(value));
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

fn event_matches(value: &Value, method: Option<&str>, matchers: &[String]) -> bool {
    if let Some(expected) = method {
        if value.get("method").and_then(|v| v.as_str()) != Some(expected) {
            return false;
        }
    }
    let params = match value.get("params") {
        Some(v) => v,
        None => return matchers.is_empty(),
    };
    for matcher in matchers {
        let Some((key, expected)) = matcher.split_once('=') else {
            return false;
        };
        let actual = params.get(key).map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });
        if actual.as_deref() != Some(expected) {
            return false;
        }
    }
    true
}

fn send_request(req: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket_path()).map_err(|e| e.to_string())?;
    let line = serialize_line(&req)?;
    stream.write_all(&line).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_str(&buf).map_err(|e| e.to_string())
}

fn send_request_with_subscription(
    req: Value,
    events: &[&str],
    timeout: Duration,
    wait_for: Option<&str>,
    matchers: &[String],
) -> Result<(Value, Option<Value>), String> {
    let mut sub = Subscription::connect(events, Some(timeout))?;
    let resp = send_request(req)?;
    if !response_ok(&resp) {
        return Ok((resp, None));
    }
    loop {
        let event = sub.next_event()?;
        match event {
            Some(value) if event_matches(&value, wait_for, matchers) => {
                return Ok((resp, Some(value)))
            }
            Some(_) => continue,
            None => return Ok((resp, None)),
        }
    }
}

fn subscribe_forever(events: &[String], ndjson: bool) -> Result<(), String> {
    let mut sub = Subscription::connect(events, None)?;
    while let Some(event) = sub.next_event()? {
        print_json(&event, ndjson);
    }
    Ok(())
}

fn layout_name(preset: LayoutPresetArg) -> &'static str {
    match preset {
        LayoutPresetArg::MasterStack => "master-stack",
        LayoutPresetArg::Columns => "columns",
        LayoutPresetArg::Center => "center",
        LayoutPresetArg::Grid => "grid",
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let exit = match cli.command {
        Command::Status => print_response(
            &send_request(json!({"id":1,"method":"system.status","params":{}}))?,
            cli.json,
            cli.ndjson,
        ),
        Command::State => print_response(
            &send_request(json!({"id":1,"method":"system.state","params":{}}))?,
            cli.json,
            cli.ndjson,
        ),
        Command::Output(cmd) => {
            let req = match cmd.action {
                OutputAction::List => json!({"id":1,"method":"output.list","params":{}}),
            };
            print_response(&send_request(req)?, cli.json, cli.ndjson)
        }
        Command::Workspace(cmd) => {
            let req = match cmd.action {
                WorkspaceAction::List => json!({"id":1,"method":"workspace.list","params":{}}),
                WorkspaceAction::Switch { index } => {
                    if index == 0 {
                        return Err("workspace index starts from 1".into());
                    }
                    json!({"id":1,"method":"workspace.switch","params":{"index": index - 1}})
                }
                WorkspaceAction::Next => json!({"id":1,"method":"workspace.next","params":{}}),
                WorkspaceAction::Prev => json!({"id":1,"method":"workspace.prev","params":{}}),
            };
            print_response(&send_request(req)?, cli.json, cli.ndjson)
        }
        Command::Layout(cmd) => {
            let req = match cmd.action {
                LayoutAction::Set { preset } => {
                    json!({"id":1,"method":"layout.set","params":{"preset": layout_name(preset)}})
                }
                LayoutAction::Cycle => json!({"id":1,"method":"layout.cycle","params":{}}),
            };
            print_response(&send_request(req)?, cli.json, cli.ndjson)
        }
        Command::Window(cmd) => {
            let req = match cmd.action {
                WindowAction::List(args) => {
                    let workspace = match args.workspace {
                        Some(0) => return Err("workspace index starts from 1".into()),
                        Some(w) => Some(w - 1),
                        None => None,
                    };
                    json!({"id":1,"method":"window.list","params":{"app_id": args.app_id, "title": args.title, "workspace": workspace}})
                }
                WindowAction::Focused => json!({"id":1,"method":"window.focused","params":{}}),
                WindowAction::Focus(args) => {
                    let workspace = match args.workspace {
                        Some(0) => return Err("workspace index starts from 1".into()),
                        Some(w) => Some(w - 1),
                        None => None,
                    };
                    json!({"id":1,"method":"window.focus","params":{"app_id": args.app_id, "title": args.title, "workspace": workspace}})
                }
                WindowAction::Close(args) => {
                    let workspace = match args.workspace {
                        Some(0) => return Err("workspace index starts from 1".into()),
                        Some(w) => Some(w - 1),
                        None => None,
                    };
                    json!({"id":1,"method":"window.close","params":{"app_id": args.app_id, "title": args.title, "workspace": workspace}})
                }
                WindowAction::Fullscreen(fs) => match fs.action {
                    WindowFullscreenAction::Toggle => {
                        let workspace = match fs.select.workspace {
                            Some(0) => return Err("workspace index starts from 1".into()),
                            Some(w) => Some(w - 1),
                            None => None,
                        };
                        json!({"id":1,"method":"window.fullscreen.toggle","params":{"app_id": fs.select.app_id, "title": fs.select.title, "workspace": workspace}})
                    }
                },
                WindowAction::Move(args) => {
                    if args.to == 0 {
                        return Err("workspace index starts from 1".into());
                    }
                    let source_workspace = match args.from_workspace {
                        Some(0) => return Err("workspace index starts from 1".into()),
                        Some(w) => Some(w - 1),
                        None => None,
                    };
                    json!({"id":1,"method":"window.move_to_workspace","params":{"workspace": args.to - 1, "app_id": args.app_id, "title": args.title, "source_workspace": source_workspace}})
                }
            };
            print_response(&send_request(req)?, cli.json, cli.ndjson)
        }
        Command::Scratchpad => print_response(
            &send_request(json!({"id":1,"method":"scratchpad.toggle","params":{}}))?,
            cli.json,
            cli.ndjson,
        ),
        Command::Overview(cmd) => {
            let method = match cmd.action {
                ShowHideAction::Show => "overview.show",
                ShowHideAction::Hide => "overview.hide",
            };
            print_response(
                &send_request(json!({"id":1,"method":method,"params":{}}))?,
                cli.json,
                cli.ndjson,
            )
        }
        Command::Settings(cmd) => {
            let method = match cmd.action {
                ShowHideAction::Show => "settings.show",
                ShowHideAction::Hide => "settings.hide",
            };
            print_response(
                &send_request(json!({"id":1,"method":method,"params":{}}))?,
                cli.json,
                cli.ndjson,
            )
        }
        Command::Lock => print_response(
            &send_request(json!({"id":1,"method":"lock.activate","params":{}}))?,
            cli.json,
            cli.ndjson,
        ),
        Command::Config(cmd) => {
            let method = match cmd.action {
                ConfigAction::Reload => "config.reload",
                ConfigAction::Get => "config.get",
            };
            print_response(
                &send_request(json!({"id":1,"method":method,"params":{}}))?,
                cli.json,
                cli.ndjson,
            )
        }
        Command::Notify(cmd) => {
            let NotifyAction::Send { text } = cmd.action;
            print_response(
                &send_request(json!({"id":1,"method":"notify.send","params":{"text": text}}))?,
                cli.json,
                cli.ndjson,
            )
        }
        Command::Screenshot(cmd) => match cmd.action {
            ScreenshotAction::Full(wait_args) => {
                let req = json!({"id":1,"method":"screenshot.capture","params":{"mode":"full"}});
                if wait_args.wait {
                    let wait_for = wait_args.wait_for.as_deref();
                    let (resp, event) = send_request_with_subscription(
                        req,
                        &["screenshot.completed", "screenshot.failed"],
                        Duration::from_secs(wait_args.timeout),
                        wait_for,
                        &wait_args.matches,
                    )?;
                    let code = print_response(&resp, cli.json, cli.ndjson);
                    if let Some(event) = event {
                        print_json(&event, true);
                    }
                    code
                } else {
                    print_response(&send_request(req)?, cli.json, cli.ndjson)
                }
            }
            ScreenshotAction::Area(args) => {
                let req = json!({"id":1,"method":"screenshot.capture","params":{"mode":"area","x":args.x,"y":args.y,"w":args.w,"h":args.h}});
                if args.wait.wait {
                    let wait_for = args.wait.wait_for.as_deref();
                    let (resp, event) = send_request_with_subscription(
                        req,
                        &["screenshot.completed", "screenshot.failed"],
                        Duration::from_secs(args.wait.timeout),
                        wait_for,
                        &args.wait.matches,
                    )?;
                    let code = print_response(&resp, cli.json, cli.ndjson);
                    if let Some(event) = event {
                        print_json(&event, true);
                    }
                    code
                } else {
                    print_response(&send_request(req)?, cli.json, cli.ndjson)
                }
            }
        },
        Command::Record(cmd) => {
            let method = match cmd.action {
                RecordAction::Start => "record.start",
                RecordAction::Stop => "record.stop",
            };
            print_response(
                &send_request(json!({"id":1,"method":method,"params":{}}))?,
                cli.json,
                cli.ndjson,
            )
        }
        Command::App(cmd) => match cmd.action {
            AppAction::Launch(args) => {
                let req = json!({"id":1,"method":"app.launch","params":{"app": args.app}});
                if args.wait.wait {
                    let wait_for = args.wait.wait_for.as_deref();
                    let (resp, event) = send_request_with_subscription(
                        req,
                        &["app.launched", "window.opened", "window.focused"],
                        Duration::from_secs(args.wait.timeout),
                        wait_for,
                        &args.wait.matches,
                    )?;
                    let code = print_response(&resp, cli.json, cli.ndjson);
                    if let Some(event) = event {
                        print_json(&event, true);
                    }
                    code
                } else {
                    print_response(&send_request(req)?, cli.json, cli.ndjson)
                }
            }
        },
        Command::Events(cmd) => {
            subscribe_forever(&cmd.events, true)?;
            0
        }
        Command::Exec(cmd) => {
            let req: Value = serde_json::from_str(&cmd.request)?;
            print_response(&send_request(req)?, true, cli.ndjson)
        }
    };

    std::process::exit(exit);
}

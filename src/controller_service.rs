use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use log::{error, info, warn};
use serde::Deserialize;
use serde_json::json;
use serialport::{available_ports, SerialPort};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::{
    consts, controller_commands,
    shared_serial::SharedSerialPort,
    startup::SharedStartupState,
    types::{ControllerMetrics, ControllerMode, ControllerTelemetrySnapshot, MetricsMsg},
};

const INDEX_HTML: &str = include_str!("controller_ui.html");

type SharedState = Arc<Mutex<ControllerState>>;

#[derive(Clone)]
pub struct ControllerHandle {
    state: SharedState,
}

impl ControllerHandle {
    pub fn new(shared_port: SharedSerialPort) -> Self {
        Self {
            state: Arc::new(Mutex::new(ControllerState::new(shared_port))),
        }
    }
}

impl ModeTarget {
    fn keyword(self) -> &'static str {
        match self {
            ModeTarget::Steering => controller_commands::MODE_TARGET_STEERING,
            ModeTarget::Throttle => controller_commands::MODE_TARGET_THROTTLE,
            ModeTarget::Both => controller_commands::MODE_TARGET_BOTH,
        }
    }
}

struct ControllerState {
    log_writer: Option<BufWriter<File>>,
    stop_flag: Option<Arc<AtomicBool>>,
    snapshot: ControllerTelemetrySnapshot,
    shared_port: SharedSerialPort,
}

impl ControllerState {
    fn new(shared_port: SharedSerialPort) -> Self {
        Self {
            log_writer: None,
            stop_flag: None,
            snapshot: ControllerTelemetrySnapshot::default(),
            shared_port,
        }
    }
}

#[derive(Deserialize)]
struct ConnectRequest {
    port: String,
    baud: Option<u32>,
}

#[derive(Deserialize)]
struct CommandRequest {
    command: Option<String>,
    commands: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SettingsRequest {
    steering_sensitivity: f32,
    throttle_sensitivity: f32,
}

#[derive(Deserialize)]
struct ModeRequest {
    mode: ControllerMode,
    #[serde(default)]
    target: ModeTarget,
}

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum ModeTarget {
    Steering,
    Throttle,
    #[default]
    Both,
}

struct ParsedTelemetry {
    timestamp_us: u64,
    steer_us: i32,
    throttle_us: i32,
    speed_mps: f32,
}

struct ControllerLogRow {
    timestamp_us: u64,
    event: &'static str,
    steer_us: Option<i32>,
    throttle_us: Option<i32>,
    setpoint_mps: Option<f32>,
    error_value: Option<f32>,
    wall_left_correction_deg: Option<f32>,
    wall_right_correction_deg: Option<f32>,
    wall_combined_correction_deg: Option<f32>,
    delta_t_us: Option<u32>,
    kalman0: Option<f32>,
    kalman1: Option<f32>,
    kalman2: Option<f32>,
    kalman3: Option<f32>,
    distance0_cm: Option<u32>,
    distance1_cm: Option<u32>,
    distance2_cm: Option<u32>,
    /// 0 = Startup, 1 = Straight, 2 = Turning
    drive_mode: Option<u8>,
}

enum TelemetryParseResult {
    Parsed {
        snapshot: Option<ParsedTelemetry>,
        log_row: ControllerLogRow,
    },
    Ignore,
    Invalid,
}

pub fn run_controller_service(
    tx_metrics: Sender<MetricsMsg>,
    controller: ControllerHandle,
    startup_state: SharedStartupState,
) -> Result<()> {
    fs::create_dir_all(controller_log_dir())?;

    let state = controller.state.clone();
    publish_metrics(&tx_metrics, &ControllerTelemetrySnapshot::default());

    let server = tiny_http::Server::http(consts::CONTROLLER_HTTP_BIND)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    info!(
        "[controller] UI/API endpoint -> http://{}",
        consts::CONTROLLER_HTTP_BIND
    );

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request, &state, &startup_state, &tx_metrics) {
            error!("[controller] request handling failed: {error:#}");
        }
    }

    Ok(())
}

fn handle_request(
    request: Request,
    state: &SharedState,
    startup_state: &SharedStartupState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    let path = request.url().split('?').next().unwrap_or("/").to_string();

    match (request.method(), path.as_str()) {
        (&Method::Get, "/") => respond_html(request, INDEX_HTML),
        (&Method::Get, "/api/ports") => handle_list_ports(request),
        (&Method::Get, "/api/telemetry") => handle_telemetry(request, state, startup_state),
        (&Method::Get, "/api/logs") => handle_logs(request),
        (&Method::Get, "/api/startup") => handle_startup_status(request, startup_state),
        (&Method::Post, "/api/connect") => handle_connect(request, state, tx_metrics),
        (&Method::Post, "/api/disconnect") => handle_disconnect(request, state, tx_metrics),
        (&Method::Post, "/api/command") => handle_command(request, state, tx_metrics),
        (&Method::Post, "/api/settings") => handle_settings(request, state, tx_metrics),
        (&Method::Post, "/api/mode") => handle_mode(request, state, tx_metrics),
        (&Method::Post, "/api/startup/reset") => handle_startup_reset(request, startup_state),
        (&Method::Get, _) if path.starts_with("/api/logs/") => handle_download_log(request, &path),
        _ => respond_json(
            request,
            StatusCode(404),
            &json!({"error": "not found"}).to_string(),
        ),
    }
}

fn handle_list_ports(request: Request) -> Result<()> {
    let ports = available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|port| {
            json!({
                "device": port.port_name,
                "description": describe_port(&port),
            })
        })
        .collect::<Vec<_>>();

    respond_json(request, StatusCode(200), &serde_json::to_string(&ports)?)
}

#[derive(serde::Serialize)]
struct TelemetryResponse {
    #[serde(flatten)]
    snapshot: ControllerTelemetrySnapshot,
    startup: crate::types::StartupStatus,
}

fn handle_telemetry(
    request: Request,
    state: &SharedState,
    startup_state: &SharedStartupState,
) -> Result<()> {
    let snapshot = { lock_state(state)?.snapshot.clone() };
    let response = TelemetryResponse {
        snapshot,
        startup: crate::startup::snapshot(startup_state),
    };
    respond_json(request, StatusCode(200), &serde_json::to_string(&response)?)
}

fn handle_startup_status(request: Request, startup_state: &SharedStartupState) -> Result<()> {
    respond_json(
        request,
        StatusCode(200),
        &serde_json::to_string(&crate::startup::snapshot(startup_state))?,
    )
}

fn handle_startup_reset(request: Request, startup_state: &SharedStartupState) -> Result<()> {
    crate::startup::reset(startup_state);
    respond_json(request, StatusCode(200), &json!({"ok": true}).to_string())
}

fn handle_logs(request: Request) -> Result<()> {
    let mut files = Vec::new();
    for entry in fs::read_dir(controller_log_dir())? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("csv") {
            continue;
        }
        let metadata = entry.metadata()?;
        files.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "size": metadata.len(),
        }));
    }
    files.sort_by(|a, b| {
        b.get("name")
            .and_then(|value| value.as_str())
            .cmp(&a.get("name").and_then(|value| value.as_str()))
    });

    respond_json(request, StatusCode(200), &serde_json::to_string(&files)?)
}

fn handle_connect(
    mut request: Request,
    state: &SharedState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    let connect: ConnectRequest = read_json_body(&mut request)?;
    if connect.port.trim().is_empty() {
        return respond_json(
            request,
            StatusCode(400),
            &json!({"error": "port required"}).to_string(),
        );
    }

    {
        let state = lock_state(state)?;
        if state.snapshot.connected {
            return respond_json(
                request,
                StatusCode(400),
                &json!({"error": "already connected"}).to_string(),
            );
        }
    }

    let baud = connect.baud.unwrap_or(consts::SERIAL_BAUD_RATE);
    let mut writer = serialport::new(&connect.port, baud)
        .timeout(Duration::from_millis(consts::CONTROLLER_SERIAL_TIMEOUT_MS))
        .open()
        .with_context(|| format!("failed to open serial port {}", connect.port))?;
    writer.write_data_terminal_ready(true).ok();
    let reader = writer.try_clone().context("failed to clone serial port")?;

    let log_path = new_log_path();
    let mut log_writer = BufWriter::new(File::create(&log_path)?);
    writeln!(
        log_writer,
        "timestamp_us,event,steer_us,throttle_us,setpoint_mps,error,delta_t_us,kalman0,kalman1,kalman2,kalman3,distance0_cm,distance1_cm,distance2_cm,drive_mode"
    )?;
    log_writer.flush()?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let snapshot = {
        let mut state = lock_state(state)?;
        state.snapshot.connected = true;
        state.snapshot.timestamp_us = 0;
        state.snapshot.last_update_us = 0;
        state.snapshot.parse_errors = 0;
        state.snapshot.serial_errors = 0;
        state.snapshot.selected_port = Some(connect.port.clone());
        state.log_writer = Some(log_writer);
        state.stop_flag = Some(stop_flag.clone());
        // Set the port in the shared handle for alignment and command writing
        state.shared_port.set_port(writer);
        state.snapshot.clone()
    };

    publish_metrics(tx_metrics, &snapshot);
    spawn_reader(reader, state.clone(), stop_flag, tx_metrics.clone());

    respond_json(
        request,
        StatusCode(200),
        &json!({
            "ok": true,
            "log": log_path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
        })
        .to_string(),
    )
}

fn handle_disconnect(
    request: Request,
    state: &SharedState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    disconnect_locked(state)?;
    let snapshot = { lock_state(state)?.snapshot.clone() };
    publish_metrics(tx_metrics, &snapshot);
    respond_json(request, StatusCode(200), &json!({"ok": true}).to_string())
}

fn handle_command(
    mut request: Request,
    state: &SharedState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    let payload: CommandRequest = read_json_body(&mut request)?;
    let commands = parse_command_request(&payload)?;
    if commands.is_empty() {
        return respond_json(
            request,
            StatusCode(400),
            &json!({"error": "command required"}).to_string(),
        );
    }

    match send_serial_commands(state, &commands) {
        Ok(snapshot) => {
            publish_metrics(tx_metrics, &snapshot);
            respond_json(
                request,
                StatusCode(200),
                &json!({"ok": true, "sent": commands.len()}).to_string(),
            )
        }
        Err(error) => respond_json(
            request,
            StatusCode(400),
            &json!({"error": error.to_string()}).to_string(),
        ),
    }
}

fn parse_command_request(payload: &CommandRequest) -> Result<Vec<String>> {
    let mut commands = Vec::new();

    if let Some(command) = payload.command.as_deref() {
        let command = command.trim();
        if !command.is_empty() {
            commands.push(command.to_string());
        }
    }

    if let Some(list) = payload.commands.as_ref() {
        for command in list {
            let command = command.trim();
            if !command.is_empty() {
                commands.push(command.to_string());
            }
        }
    }

    for command in &commands {
        validate_command(command)?;
    }

    Ok(commands)
}

fn validate_command(command: &str) -> Result<()> {
    if command.starts_with(controller_commands::CONST_PREFIX) {
        let mut parts = command.split_whitespace();
        let _ = parts.next();
        let Some(name) = parts.next() else {
            anyhow::bail!("const command must be: const <name> <value>");
        };
        let Some(_value) = parts.next() else {
            anyhow::bail!("const command must be: const <name> <value>");
        };

        if name.is_empty() {
            anyhow::bail!("const name cannot be empty");
        }
    } else if command.starts_with(controller_commands::MODE_PREFIX)
        && parse_mode_command(command).is_none()
    {
        anyhow::bail!(
            "mode command must be: mode <manual|auto> or mode <steering|throttle|both> <manual|auto>"
        );
    }

    Ok(())
}

fn send_serial_commands(
    state: &SharedState,
    commands: &[String],
) -> Result<ControllerTelemetrySnapshot> {
    let mut last_snapshot = None;
    for command in commands {
        last_snapshot = Some(
            send_serial_command(state, command)
                .with_context(|| format!("failed to send command: {command}"))?,
        );
    }

    last_snapshot.context("no commands to send")
}

fn handle_settings(
    mut request: Request,
    state: &SharedState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    let payload: SettingsRequest = read_json_body(&mut request)?;
    let steering = payload.steering_sensitivity.clamp(0.1, 2.0);
    let throttle = payload.throttle_sensitivity.clamp(0.1, 2.0);

    let snapshot = {
        let mut state = lock_state(state)?;
        state.snapshot.steering_sensitivity = steering;
        state.snapshot.throttle_sensitivity = throttle;
        state.snapshot.clone()
    };
    publish_metrics(tx_metrics, &snapshot);

    respond_json(request, StatusCode(200), &json!({"ok": true}).to_string())
}

fn handle_mode(
    mut request: Request,
    state: &SharedState,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<()> {
    let payload: ModeRequest = read_json_body(&mut request)?;
    let command =
        controller_commands::format_mode_command(Some(payload.target.keyword()), payload.mode);

    match send_serial_command(state, &command) {
        Ok(snapshot) => {
            publish_metrics(tx_metrics, &snapshot);
            respond_json(request, StatusCode(200), &json!({"ok": true}).to_string())
        }
        Err(error) => respond_json(
            request,
            StatusCode(400),
            &json!({"error": error.to_string()}).to_string(),
        ),
    }
}

pub fn send_mode(
    controller: &ControllerHandle,
    mode: ControllerMode,
    tx_metrics: &Sender<MetricsMsg>,
) -> Result<ControllerTelemetrySnapshot> {
    let command = controller_commands::format_mode_command(None, mode);

    let snapshot = send_serial_command(&controller.state, &command)?;
    publish_metrics(tx_metrics, &snapshot);
    Ok(snapshot)
}

fn handle_download_log(request: Request, path: &str) -> Result<()> {
    let filename = path.trim_start_matches("/api/logs/");
    let safe_name = Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| value.ends_with(".csv"));

    let Some(safe_name) = safe_name else {
        return respond_json(
            request,
            StatusCode(400),
            &json!({"error": "invalid filename"}).to_string(),
        );
    };

    let path = controller_log_dir().join(safe_name);
    if !path.exists() {
        return respond_json(
            request,
            StatusCode(404),
            &json!({"error": "not found"}).to_string(),
        );
    }

    let file = File::open(&path)?;
    let response = Response::from_file(file)
        .with_status_code(StatusCode(200))
        .with_header(content_type("text/csv; charset=utf-8"))
        .with_header(content_disposition(safe_name));
    request.respond(response)?;
    Ok(())
}

fn send_serial_command(state: &SharedState, command: &str) -> Result<ControllerTelemetrySnapshot> {
    let mut state = lock_state(state)?;
    if !state.snapshot.connected {
        anyhow::bail!("not connected");
    }

    if !command_allowed(&state.snapshot, command) {
        anyhow::bail!("manual PWM commands are disabled while auto mode is active");
    }

    let payload = format!("{}\n", command);
    state.shared_port.write_str(&payload);

    apply_command_snapshot(&mut state.snapshot, command);
    Ok(state.snapshot.clone())
}

fn apply_command_snapshot(snapshot: &mut ControllerTelemetrySnapshot, command: &str) {
    if let Some(value) =
        controller_commands::strip_value(command, controller_commands::PWM_STEERING_PREFIX)
    {
        if let Ok(steer_us) = value.trim().parse::<i32>() {
            snapshot.steer_us = steer_us;
        }
    } else if let Some(value) =
        controller_commands::strip_value(command, controller_commands::PWM_THROTTLE_PREFIX)
    {
        if let Ok(throttle_us) = value.trim().parse::<i32>() {
            snapshot.throttle_us = throttle_us;
        }
    } else if let Some(mode) = parse_mode_command(command) {
        snapshot.mode = mode;
    }
}

fn parse_mode_command(command: &str) -> Option<ControllerMode> {
    let rest = controller_commands::strip_value(command, controller_commands::MODE_PREFIX)?;
    let mut parts = rest.split_ascii_whitespace();
    let first = parts.next()?;
    let second = parts.next();

    if parts.next().is_some() {
        return None;
    }

    let mode = second.unwrap_or(first);
    match mode {
        controller_commands::MODE_MANUAL => Some(ControllerMode::Manual),
        controller_commands::MODE_AUTO => Some(ControllerMode::Auto),
        _ => None,
    }
}

fn command_allowed(snapshot: &ControllerTelemetrySnapshot, command: &str) -> bool {
    if snapshot.mode == ControllerMode::Manual
        || command.starts_with(controller_commands::MODE_PREFIX)
    {
        return true;
    }

    if let Some(value) =
        controller_commands::strip_value(command, controller_commands::PWM_STEERING_PREFIX)
    {
        return value.trim() == "1500";
    }
    if let Some(value) =
        controller_commands::strip_value(command, controller_commands::PWM_THROTTLE_PREFIX)
    {
        return value.trim() == "1500";
    }

    true
}

fn spawn_reader(
    reader: Box<dyn SerialPort>,
    state: SharedState,
    stop_flag: Arc<AtomicBool>,
    tx_metrics: Sender<MetricsMsg>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let mut consecutive_errors = 0_u32;

        while !stop_flag.load(Ordering::Relaxed) {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => continue,
                Ok(_) => {
                    consecutive_errors = 0;
                    process_telemetry_line(line.trim(), &state, &tx_metrics);
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => {
                    consecutive_errors += 1;
                    warn!("[controller] serial read error: {error}");
                    if let Ok(mut state) = lock_state(&state) {
                        state.snapshot.serial_errors += 1;
                        publish_metrics(&tx_metrics, &state.snapshot);
                    }

                    if consecutive_errors >= 10 {
                        if let Err(disconnect_error) = disconnect_locked(&state) {
                            error!(
                                "[controller] failed to close broken serial link: {disconnect_error:#}"
                            );
                        }
                        if let Ok(state) = lock_state(&state) {
                            publish_metrics(&tx_metrics, &state.snapshot);
                        }
                        break;
                    }
                }
            }
        }
    });
}

fn process_telemetry_line(line: &str, state: &SharedState, tx_metrics: &Sender<MetricsMsg>) {
    if line.is_empty()
        || line == "OK"
        || line.starts_with('#')
        || line.starts_with("ERR")
        || is_command_echo(line)
    {
        return;
    }

    let parsed = parse_telemetry_line(line);
    let mut state = match lock_state(state) {
        Ok(state) => state,
        Err(error) => {
            error!("[controller] state lock poisoned: {error:#}");
            return;
        }
    };

    match parsed {
        TelemetryParseResult::Parsed { snapshot, log_row } => {
            if let Some(parsed) = snapshot {
                state.snapshot.timestamp_us = parsed.timestamp_us;
                state.snapshot.last_update_us = parsed.timestamp_us;
                state.snapshot.steer_us = parsed.steer_us;
                state.snapshot.throttle_us = parsed.throttle_us;
                state.snapshot.speed_mps = parsed.speed_mps;
                state.snapshot.connected = true;
            }
            apply_log_row_to_snapshot(&mut state.snapshot, &log_row);
            if let Some(log_writer) = state.log_writer.as_mut() {
                if let Err(error) = write_log_row(log_writer, &log_row) {
                    warn!("[controller] failed to append log row: {error}");
                } else {
                    let _ = log_writer.flush();
                }
            }
        }
        TelemetryParseResult::Ignore => {}
        TelemetryParseResult::Invalid => {
            state.snapshot.parse_errors += 1;
        }
    }

    publish_metrics(tx_metrics, &state.snapshot);
}

fn parse_telemetry_line(line: &str) -> TelemetryParseResult {
    if line.contains(':') {
        let mut timestamp_us = None;
        let mut steer_us = None;
        let mut throttle_us = None;
        let mut setpoint_mps = None;
        let mut error_value = None;
        let mut kalman0 = None;
        let mut kalman1 = None;
        let mut kalman2 = None;
        let mut kalman3 = None;
        let mut delta_t_us = None;
        let mut distance0_cm = None;
        let mut distance1_cm = None;
        let mut distance2_cm = None;
        let mut drive_mode: Option<u8> = None;
        let mut wall_left_correction_deg = None;
        let mut wall_right_correction_deg = None;
        let mut wall_combined_correction_deg = None;
        let line = line.strip_prefix('>').unwrap_or(line);

        for pair in line.split(',') {
            let Some((key, value)) = pair.split_once(':') else {
                return TelemetryParseResult::Invalid;
            };
            let key = key.trim();
            let value = value.trim();
            if key.contains("time") {
                timestamp_us = value.parse().ok();
            } else if key.contains("steer") {
                steer_us = value.parse().ok();
            } else if key.contains("throttle") {
                throttle_us = value.parse().ok();
            } else if key.contains("setpoint") || key.contains("speed") {
                setpoint_mps = value.parse().ok();
            } else if key == "error" {
                error_value = value.parse().ok();
            } else if key == "kalman0" {
                kalman0 = value.parse().ok();
            } else if key == "kalman1" {
                kalman1 = value.parse().ok();
            } else if key == "kalman2" {
                kalman2 = value.parse().ok();
            } else if key == "kalman3" {
                kalman3 = value.parse().ok();
            } else if key == "delta_t_us" {
                delta_t_us = value.parse().ok();
            } else if key == "distance0_cm" {
                distance0_cm = value.parse().ok();
            } else if key == "distance1_cm" {
                distance1_cm = value.parse().ok();
            } else if key == "distance2_cm" {
                distance2_cm = value.parse().ok();
            } else if key == "drive_mode" {
                drive_mode = value.parse().ok();
            } else if key == "wall_left_deg" {
                wall_left_correction_deg = value.parse().ok();
            } else if key == "wall_right_deg" {
                wall_right_correction_deg = value.parse().ok();
            } else if key == "wall_combined_deg" {
                wall_combined_correction_deg = value.parse().ok();
            }
        }

        let Some(timestamp_us) = timestamp_us else {
            return TelemetryParseResult::Invalid;
        };

        if steer_us.is_some() || throttle_us.is_some() || setpoint_mps.is_some() {
            return TelemetryParseResult::Parsed {
                snapshot: Some(ParsedTelemetry {
                    timestamp_us,
                    steer_us: steer_us.unwrap_or(1500),
                    throttle_us: throttle_us.unwrap_or(1500),
                    speed_mps: setpoint_mps.unwrap_or(0.0),
                }),
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "controller",
                    steer_us,
                    throttle_us,
                    setpoint_mps,
                    error_value,
                    wall_left_correction_deg,
                    wall_right_correction_deg,
                    wall_combined_correction_deg,
                    delta_t_us: None,
                    kalman0,
                    kalman1,
                    kalman2,
                    kalman3,
                    distance0_cm: None,
                    distance1_cm: None,
                    distance2_cm: None,
                    drive_mode,
                },
            };
        }

        if delta_t_us.is_some() {
            return TelemetryParseResult::Parsed {
                snapshot: None,
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "hall_delta_t",
                    steer_us: None,
                    throttle_us: None,
                    setpoint_mps: None,
                    error_value: None,
                    wall_left_correction_deg: None,
                    wall_right_correction_deg: None,
                    wall_combined_correction_deg: None,
                    delta_t_us,
                    kalman0: None,
                    kalman1: None,
                    kalman2: None,
                    kalman3: None,
                    distance0_cm: None,
                    distance1_cm: None,
                    distance2_cm: None,
                    drive_mode: None,
                },
            };
        }

        if distance0_cm.is_some() || distance1_cm.is_some() || distance2_cm.is_some() {
            return TelemetryParseResult::Parsed {
                snapshot: None,
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "ultrasound",
                    steer_us: None,
                    throttle_us: None,
                    setpoint_mps: None,
                    error_value: None,
                    wall_left_correction_deg: None,
                    wall_right_correction_deg: None,
                    wall_combined_correction_deg: None,
                    delta_t_us: None,
                    kalman0: None,
                    kalman1: None,
                    kalman2: None,
                    kalman3: None,
                    distance0_cm,
                    distance1_cm,
                    distance2_cm,
                    drive_mode: None,
                },
            };
        }

        return TelemetryParseResult::Ignore;
    }

    let parts = line.split(',').collect::<Vec<_>>();
    if parts.len() == 4 {
        let parsed = (
            parts[0].trim().parse::<u64>(),
            parts[1].trim().parse::<i32>(),
            parts[2].trim().parse::<i32>(),
            parts[3].trim().parse::<f32>(),
        );

        return match parsed {
            (Ok(timestamp_us), Ok(steer_us), Ok(throttle_us), Ok(speed_mps)) => {
                TelemetryParseResult::Parsed {
                    snapshot: Some(ParsedTelemetry {
                        timestamp_us,
                        steer_us,
                        throttle_us,
                        speed_mps,
                    }),
                    log_row: ControllerLogRow {
                        timestamp_us,
                        event: "controller",
                        steer_us: Some(steer_us),
                        throttle_us: Some(throttle_us),
                        setpoint_mps: Some(speed_mps),
                        error_value: None,
                        wall_left_correction_deg: None,
                        wall_right_correction_deg: None,
                        wall_combined_correction_deg: None,
                        delta_t_us: None,
                        kalman0: None,
                        kalman1: None,
                        kalman2: None,
                        kalman3: None,
                        distance0_cm: None,
                        distance1_cm: None,
                        distance2_cm: None,
                        drive_mode: None,
                    },
                }
            }
            _ => TelemetryParseResult::Invalid,
        };
    }

    if parts.len() == 14 || parts.len() == 15 {
        let Ok(timestamp_us) = parts[0].trim().parse::<u64>() else {
            return TelemetryParseResult::Invalid;
        };
        // Column 14 (index 14) is drive_mode, present only in the new 15-column format.
        let csv_drive_mode: Option<u8> = if parts.len() == 15 {
            parse_csv_u32(parts[14]).map(|v| v as u8)
        } else {
            None
        };

        return match parts[1].trim() {
            "controller" => {
                let parsed = (
                    parts[2].trim().parse::<i32>(),
                    parts[3].trim().parse::<i32>(),
                    parts[4].trim().parse::<f32>(),
                    parse_csv_f32(parts[5]),
                    parse_csv_f32(parts[7]),
                    parse_csv_f32(parts[8]),
                    parse_csv_f32(parts[9]),
                    parse_csv_f32(parts[10]),
                );

                match parsed {
                    (
                        Ok(steer_us),
                        Ok(throttle_us),
                        Ok(speed_mps),
                        error_value,
                        kalman0,
                        kalman1,
                        kalman2,
                        kalman3,
                    ) => TelemetryParseResult::Parsed {
                        snapshot: Some(ParsedTelemetry {
                            timestamp_us,
                            steer_us,
                            throttle_us,
                            speed_mps,
                        }),
                        log_row: ControllerLogRow {
                            timestamp_us,
                            event: "controller",
                            steer_us: Some(steer_us),
                            throttle_us: Some(throttle_us),
                            setpoint_mps: Some(speed_mps),
                            error_value,
                            wall_left_correction_deg: None,
                            wall_right_correction_deg: None,
                            wall_combined_correction_deg: None,
                            delta_t_us: None,
                            kalman0,
                            kalman1,
                            kalman2,
                            kalman3,
                            distance0_cm: parse_csv_u32(parts[11]),
                            distance1_cm: parse_csv_u32(parts[12]),
                            distance2_cm: parse_csv_u32(parts[13]),
                            drive_mode: csv_drive_mode,
                        },
                    },
                    _ => TelemetryParseResult::Invalid,
                }
            }
            "ultrasound" => TelemetryParseResult::Parsed {
                snapshot: None,
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "ultrasound",
                    steer_us: None,
                    throttle_us: None,
                    setpoint_mps: None,
                    error_value: None,
                    wall_left_correction_deg: None,
                    wall_right_correction_deg: None,
                    wall_combined_correction_deg: None,
                    delta_t_us: None,
                    kalman0: None,
                    kalman1: None,
                    kalman2: None,
                    kalman3: None,
                    distance0_cm: parse_csv_u32(parts[11]),
                    distance1_cm: parse_csv_u32(parts[12]),
                    distance2_cm: parse_csv_u32(parts[13]),
                    drive_mode: None,
                },
            },
            "hall_delta_t" => TelemetryParseResult::Parsed {
                snapshot: None,
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "hall_delta_t",
                    steer_us: None,
                    throttle_us: None,
                    setpoint_mps: None,
                    error_value: None,
                    wall_left_correction_deg: None,
                    wall_right_correction_deg: None,
                    wall_combined_correction_deg: None,
                    delta_t_us: parse_csv_u32(parts[6]),
                    kalman0: None,
                    kalman1: None,
                    kalman2: None,
                    kalman3: None,
                    distance0_cm: None,
                    distance1_cm: None,
                    distance2_cm: None,
                    drive_mode: None,
                },
            },
            _ => TelemetryParseResult::Invalid,
        };
    }

    // New 18-column controller format and 17-column hall_delta_t format.
    // controller (18):  ts,controller,steer,throttle,setpoint,error,wall_left,wall_right,wall_combined,null,k0,k1,k2,k3,null,null,null,drive_mode
    // hall_delta_t (17): ts,hall_delta_t,null×7,delta_t,null×7
    if parts.len() == 17 || parts.len() == 18 {
        let Ok(timestamp_us) = parts[0].trim().parse::<u64>() else {
            return TelemetryParseResult::Invalid;
        };

        return match parts[1].trim() {
            "controller" if parts.len() == 18 => {
                let parsed = (
                    parts[2].trim().parse::<i32>(),
                    parts[3].trim().parse::<i32>(),
                    parts[4].trim().parse::<f32>(),
                    parse_csv_f32(parts[5]),
                    parse_csv_f32(parts[6]),
                    parse_csv_f32(parts[7]),
                    parse_csv_f32(parts[8]),
                    parse_csv_f32(parts[10]),
                    parse_csv_f32(parts[11]),
                    parse_csv_f32(parts[12]),
                    parse_csv_f32(parts[13]),
                    parse_csv_u32(parts[17]).map(|v| v as u8),
                );

                match parsed {
                    (
                        Ok(steer_us),
                        Ok(throttle_us),
                        Ok(speed_mps),
                        error_value,
                        wall_left,
                        wall_right,
                        wall_combined,
                        kalman0,
                        kalman1,
                        kalman2,
                        kalman3,
                        csv_drive_mode,
                    ) => TelemetryParseResult::Parsed {
                        snapshot: Some(ParsedTelemetry {
                            timestamp_us,
                            steer_us,
                            throttle_us,
                            speed_mps,
                        }),
                        log_row: ControllerLogRow {
                            timestamp_us,
                            event: "controller",
                            steer_us: Some(steer_us),
                            throttle_us: Some(throttle_us),
                            setpoint_mps: Some(speed_mps),
                            error_value,
                            wall_left_correction_deg: wall_left,
                            wall_right_correction_deg: wall_right,
                            wall_combined_correction_deg: wall_combined,
                            delta_t_us: None,
                            kalman0,
                            kalman1,
                            kalman2,
                            kalman3,
                            distance0_cm: None,
                            distance1_cm: None,
                            distance2_cm: None,
                            drive_mode: csv_drive_mode,
                        },
                    },
                    _ => TelemetryParseResult::Invalid,
                }
            }
            "hall_delta_t" if parts.len() == 17 => TelemetryParseResult::Parsed {
                snapshot: None,
                log_row: ControllerLogRow {
                    timestamp_us,
                    event: "hall_delta_t",
                    steer_us: None,
                    throttle_us: None,
                    setpoint_mps: None,
                    error_value: None,
                    wall_left_correction_deg: None,
                    wall_right_correction_deg: None,
                    wall_combined_correction_deg: None,
                    delta_t_us: parse_csv_u32(parts[9]),
                    kalman0: None,
                    kalman1: None,
                    kalman2: None,
                    kalman3: None,
                    distance0_cm: None,
                    distance1_cm: None,
                    distance2_cm: None,
                    drive_mode: None,
                },
            },
            _ => TelemetryParseResult::Invalid,
        };
    }

    TelemetryParseResult::Invalid
}

fn apply_log_row_to_snapshot(
    snapshot: &mut ControllerTelemetrySnapshot,
    log_row: &ControllerLogRow,
) {
    snapshot.timestamp_us = log_row.timestamp_us;
    snapshot.last_update_us = log_row.timestamp_us;

    if let Some(steer_us) = log_row.steer_us {
        snapshot.steer_us = steer_us;
    }
    if let Some(throttle_us) = log_row.throttle_us {
        snapshot.throttle_us = throttle_us;
    }
    if let Some(setpoint_mps) = log_row.setpoint_mps {
        snapshot.speed_mps = setpoint_mps;
        snapshot.setpoint_mps = Some(setpoint_mps);
    }
    if let Some(error_value) = log_row.error_value {
        snapshot.error_value = Some(error_value);
    }
    if let Some(delta_t_us) = log_row.delta_t_us {
        snapshot.hall_delta_t_us = Some(delta_t_us);
    }
    if let Some(kalman0) = log_row.kalman0 {
        snapshot.kalman0 = Some(kalman0);
    }
    if let Some(kalman1) = log_row.kalman1 {
        snapshot.kalman1 = Some(kalman1);
    }
    if let Some(kalman2) = log_row.kalman2 {
        snapshot.kalman2 = Some(kalman2);
    }
    if let Some(kalman3) = log_row.kalman3 {
        snapshot.kalman3 = Some(kalman3);
    }
    if let Some(distance0_cm) = log_row.distance0_cm {
        snapshot.distance0_cm = Some(distance0_cm);
    }
    if let Some(distance1_cm) = log_row.distance1_cm {
        snapshot.distance1_cm = Some(distance1_cm);
    }
    if let Some(distance2_cm) = log_row.distance2_cm {
        snapshot.distance2_cm = Some(distance2_cm);
    }
    if let Some(drive_mode) = log_row.drive_mode {
        snapshot.drive_mode = Some(drive_mode);
    }
    if let Some(v) = log_row.wall_left_correction_deg {
        snapshot.wall_left_correction_deg = Some(v);
    }
    if let Some(v) = log_row.wall_right_correction_deg {
        snapshot.wall_right_correction_deg = Some(v);
    }
    if let Some(v) = log_row.wall_combined_correction_deg {
        snapshot.wall_combined_correction_deg = Some(v);
    }
}

fn is_command_echo(line: &str) -> bool {
    controller_commands::is_known_command_echo(line)
}

fn parse_csv_f32(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("null") || value.is_empty() {
        None
    } else {
        value.parse::<f32>().ok()
    }
}

fn parse_csv_u32(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("null") || value.is_empty() {
        None
    } else {
        value.parse::<u32>().ok()
    }
}

fn write_log_row(log_writer: &mut BufWriter<File>, row: &ControllerLogRow) -> std::io::Result<()> {
    writeln!(
        log_writer,
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        row.timestamp_us,
        row.event,
        format_csv_opt_i32(row.steer_us),
        format_csv_opt_i32(row.throttle_us),
        format_csv_opt_f32(row.setpoint_mps),
        format_csv_opt_f32(row.error_value),
        format_csv_opt_f32(row.wall_left_correction_deg),
        format_csv_opt_f32(row.wall_right_correction_deg),
        format_csv_opt_f32(row.wall_combined_correction_deg),
        format_csv_opt_u32(row.delta_t_us),
        format_csv_opt_f32(row.kalman0),
        format_csv_opt_f32(row.kalman1),
        format_csv_opt_f32(row.kalman2),
        format_csv_opt_f32(row.kalman3),
        format_csv_opt_u32(row.distance0_cm),
        format_csv_opt_u32(row.distance1_cm),
        format_csv_opt_u32(row.distance2_cm),
        format_csv_opt_u8(row.drive_mode),
    )
}

fn format_csv_opt_i32(value: Option<i32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn format_csv_opt_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn format_csv_opt_u8(value: Option<u8>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn format_csv_opt_f32(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.4}"))
        .unwrap_or_else(|| "null".to_string())
}

fn disconnect_locked(state: &SharedState) -> Result<()> {
    let mut state = lock_state(state)?;
    if let Some(stop_flag) = state.stop_flag.take() {
        stop_flag.store(true, Ordering::Relaxed);
    }

    state.shared_port.clear_port();
    if let Some(mut log_writer) = state.log_writer.take() {
        log_writer.flush().ok();
    }
    state.snapshot.connected = false;
    state.snapshot.selected_port = None;
    Ok(())
}

fn publish_metrics(tx_metrics: &Sender<MetricsMsg>, snapshot: &ControllerTelemetrySnapshot) {
    tx_metrics
        .try_send(MetricsMsg {
            stage: "controller",
            real_us: 0,
            cpu_us: 0,
            lines: None,
            camera: None,
            controller: Some(ControllerMetrics::from(snapshot)),
        })
        .ok();
}

fn controller_log_dir() -> PathBuf {
    PathBuf::from(consts::CONTROLLER_LOG_DIR)
}

fn new_log_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    controller_log_dir().join(format!("controller_{millis}.csv"))
}

fn lock_state(state: &SharedState) -> Result<std::sync::MutexGuard<'_, ControllerState>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("controller state lock poisoned"))
}

fn describe_port(port: &serialport::SerialPortInfo) -> String {
    match &port.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            let manufacturer = usb.manufacturer.as_deref().unwrap_or("USB serial");
            let product = usb.product.as_deref().unwrap_or("");
            format!("{} {}", manufacturer, product).trim().to_string()
        }
        serialport::SerialPortType::BluetoothPort => "Bluetooth".to_string(),
        serialport::SerialPortType::PciPort => "PCI".to_string(),
        serialport::SerialPortType::Unknown => "Unknown".to_string(),
    }
}

fn read_json_body<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T> {
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body)?;
    Ok(serde_json::from_str(&body)?)
}

fn respond_html(request: Request, body: &str) -> Result<()> {
    let response = Response::from_string(body)
        .with_status_code(StatusCode(200))
        .with_header(content_type("text/html; charset=utf-8"));
    request.respond(response)?;
    Ok(())
}

fn respond_json(request: Request, status: StatusCode, body: &str) -> Result<()> {
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(content_type("application/json; charset=utf-8"));
    request.respond(response)?;
    Ok(())
}

fn content_type(value: &str) -> Header {
    Header::from_bytes(b"Content-Type".as_slice(), value.as_bytes()).expect("valid header")
}

fn content_disposition(filename: &str) -> Header {
    Header::from_bytes(
        b"Content-Disposition".as_slice(),
        format!("attachment; filename=\"{filename}\"").into_bytes(),
    )
    .expect("valid header")
}

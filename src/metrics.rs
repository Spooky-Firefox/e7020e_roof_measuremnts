use anyhow::Result;
use crossbeam_channel::Receiver;
use log::{error, info};
use prometheus::{CounterVec, Encoder, Gauge, GaugeVec, Opts, TextEncoder};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::thread;

use crate::{
    consts,
    types::{ControllerMetrics, MetricsMsg},
};

const WINDOW: usize = 300;

struct QuantileTracker {
    buf: VecDeque<f64>,
}

impl QuantileTracker {
    fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(WINDOW),
        }
    }

    fn push(&mut self, value: f64) {
        if self.buf.len() >= WINDOW {
            self.buf.pop_front();
        }
        self.buf.push_back(value);
    }

    fn quantile(&self, q: f64) -> f64 {
        if self.buf.is_empty() {
            return 0.0;
        }
        let mut sorted = self.buf.iter().copied().collect::<Vec<_>>();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((q * (sorted.len() - 1) as f64).round() as usize).min(sorted.len() - 1);
        sorted[idx]
    }
}

struct StageTrackers {
    real: QuantileTracker,
    cpu: QuantileTracker,
    wait: QuantileTracker,
}

impl StageTrackers {
    fn new() -> Self {
        Self {
            real: QuantileTracker::new(),
            cpu: QuantileTracker::new(),
            wait: QuantileTracker::new(),
        }
    }
}

struct Metrics {
    stage_real: GaugeVec,
    stage_cpu: GaugeVec,
    stage_wait: GaugeVec,
    camera_setting: GaugeVec,
    capture_actual_fps: Gauge,
    capture_frame_period_ms: Gauge,
    frames_total: CounterVec,
    total_lines: Gauge,
    vertical_lines: Gauge,
    horizontal_lines: Gauge,
    outlier_lines: Gauge,
    angle_from_vertical: Gauge,
    alignment_confidence: Gauge,
    vertical_spread: Gauge,
    horizontal_spread: Gauge,
    min_angle: Gauge,
    max_angle: Gauge,
    controller_connected: Gauge,
    controller_steer_us: Gauge,
    controller_throttle_us: Gauge,
    controller_speed_mps: Gauge,
    controller_setpoint_mps: Gauge,
    controller_error: Gauge,
    controller_hall_delta_t_us: Gauge,
    controller_kalman0: Gauge,
    controller_kalman1: Gauge,
    controller_kalman2: Gauge,
    controller_kalman3: Gauge,
    controller_distance0_cm: Gauge,
    controller_distance1_cm: Gauge,
    controller_distance2_cm: Gauge,
    /// Drive mode: 0 = Startup, 1 = Straight, 2 = Turning
    controller_drive_mode: Gauge,
    controller_wall_left_correction_deg: Gauge,
    controller_wall_right_correction_deg: Gauge,
    controller_wall_combined_correction_deg: Gauge,
    controller_last_update_us: Gauge,
    controller_parse_errors: Gauge,
    controller_serial_errors: Gauge,
    controller_mode_manual: Gauge,
    controller_steering_sensitivity: Gauge,
    controller_throttle_sensitivity: Gauge,
    trackers: HashMap<&'static str, StageTrackers>,
}

impl Metrics {
    fn new() -> Result<Self> {
        let stage_real = GaugeVec::new(
            Opts::new(
                "stage_real_seconds",
                "Sliding-window quantile of wall-clock processing time per stage",
            )
            .namespace(consts::METRICS_NAMESPACE),
            &["stage", "quantile"],
        )?;
        let stage_cpu = GaugeVec::new(
            Opts::new(
                "stage_cpu_seconds",
                "Sliding-window quantile of CPU processing time per stage",
            )
            .namespace(consts::METRICS_NAMESPACE),
            &["stage", "quantile"],
        )?;
        let stage_wait = GaugeVec::new(
            Opts::new(
                "stage_wait_seconds",
                "Sliding-window quantile of wall-clock time not spent on CPU per stage",
            )
            .namespace(consts::METRICS_NAMESPACE),
            &["stage", "quantile"],
        )?;
        let frames_total = CounterVec::new(
            Opts::new("frames_total", "Total frames processed per pipeline stage")
                .namespace(consts::METRICS_NAMESPACE),
            &["stage"],
        )?;
        let camera_setting = GaugeVec::new(
            Opts::new(
                "camera_setting",
                "Current camera configuration and driver-applied values",
            )
            .namespace(consts::METRICS_NAMESPACE),
            &["setting"],
        )?;
        let capture_actual_fps = Gauge::with_opts(
            Opts::new(
                "capture_actual_fps",
                "Observed capture frame rate from inter-frame timing",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let capture_frame_period_ms = Gauge::with_opts(
            Opts::new(
                "capture_frame_period_ms",
                "Observed capture frame period in milliseconds",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let total_lines = Gauge::with_opts(
            Opts::new("lines_total", "Total Hough lines in the most recent frame")
                .namespace(consts::METRICS_NAMESPACE),
        )?;
        let vertical_lines = Gauge::with_opts(
            Opts::new("vertical_lines", "Vertical lines in the most recent frame")
                .namespace(consts::METRICS_NAMESPACE),
        )?;
        let horizontal_lines = Gauge::with_opts(
            Opts::new(
                "horizontal_lines",
                "Horizontal lines in the most recent frame",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let outlier_lines = Gauge::with_opts(
            Opts::new("outlier_lines", "Outlier lines in the most recent frame")
                .namespace(consts::METRICS_NAMESPACE),
        )?;
        let angle_from_vertical = Gauge::with_opts(
            Opts::new(
                "angle_from_vertical_deg",
                "Estimated signed camera misalignment relative to the roof grid vertical axis",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let alignment_confidence = Gauge::with_opts(
            Opts::new(
                "alignment_confidence",
                "Confidence score of the chosen grid angle",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let vertical_spread = Gauge::with_opts(
            Opts::new(
                "vertical_stddev_deg",
                "Stddev of vertical line angle residuals",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let horizontal_spread = Gauge::with_opts(
            Opts::new(
                "horizontal_stddev_deg",
                "Stddev of horizontal line angle residuals",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let min_angle = Gauge::with_opts(
            Opts::new(
                "min_angle_deg",
                "Minimum raw line angle detected in the most recent frame",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let max_angle = Gauge::with_opts(
            Opts::new(
                "max_angle_deg",
                "Maximum raw line angle detected in the most recent frame",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_connected = Gauge::with_opts(
            Opts::new(
                "controller_connected",
                "Whether the RP2350 controller serial link is connected",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_steer_us = Gauge::with_opts(
            Opts::new(
                "controller_steer_pwm_us",
                "Latest controller steering PWM command in microseconds",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_throttle_us = Gauge::with_opts(
            Opts::new(
                "controller_throttle_pwm_us",
                "Latest controller throttle PWM command in microseconds",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_speed_mps = Gauge::with_opts(
            Opts::new(
                "controller_speed_mps",
                "Latest controller speed telemetry in meters per second",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_setpoint_mps = Gauge::with_opts(
            Opts::new(
                "controller_setpoint_mps",
                "Latest controller setpoint received from serial in meters per second",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_error = Gauge::with_opts(
            Opts::new(
                "controller_error",
                "Latest controller error term received from serial",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_hall_delta_t_us = Gauge::with_opts(
            Opts::new(
                "controller_hall_delta_t_us",
                "Latest hall sensor delta-t received from serial in microseconds",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_kalman0 = Gauge::with_opts(
            Opts::new(
                "controller_kalman0",
                "Latest controller Kalman state 0 from serial",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_kalman1 = Gauge::with_opts(
            Opts::new(
                "controller_kalman1",
                "Latest controller Kalman state 1 from serial",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_kalman2 = Gauge::with_opts(
            Opts::new(
                "controller_kalman2",
                "Latest controller Kalman state 2 from serial",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_kalman3 = Gauge::with_opts(
            Opts::new(
                "controller_kalman3",
                "Latest controller Kalman state 3 from serial",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_distance0_cm = Gauge::with_opts(
            Opts::new(
                "controller_distance0_cm",
                "Latest distance sensor 0 reading from serial in centimeters",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_distance1_cm = Gauge::with_opts(
            Opts::new(
                "controller_distance1_cm",
                "Latest distance sensor 1 reading from serial in centimeters",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_distance2_cm = Gauge::with_opts(
            Opts::new(
                "controller_distance2_cm",
                "Latest distance sensor 2 reading from serial in centimeters",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_drive_mode = Gauge::with_opts(
            Opts::new(
                "controller_drive_mode",
                "Controller drive mode: 0=Startup, 1=Straight, 2=Turning",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_wall_left_correction_deg = Gauge::with_opts(
            Opts::new(
                "controller_wall_left_correction_deg",
                "Left-wall centering correction from controller telemetry in degrees",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_wall_right_correction_deg = Gauge::with_opts(
            Opts::new(
                "controller_wall_right_correction_deg",
                "Right-wall centering correction from controller telemetry in degrees",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_wall_combined_correction_deg = Gauge::with_opts(
            Opts::new(
                "controller_wall_combined_correction_deg",
                "Combined wall-centering correction from controller telemetry in degrees",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_last_update_us = Gauge::with_opts(
            Opts::new(
                "controller_last_update_us",
                "Latest controller telemetry timestamp in microseconds",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_parse_errors = Gauge::with_opts(
            Opts::new(
                "controller_parse_errors",
                "Controller telemetry parse errors observed by the host",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_serial_errors = Gauge::with_opts(
            Opts::new(
                "controller_serial_errors",
                "Controller serial I/O errors observed by the host",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_mode_manual = Gauge::with_opts(
            Opts::new(
                "controller_mode_manual",
                "1 when host mode is manual, 0 when auto",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_steering_sensitivity = Gauge::with_opts(
            Opts::new(
                "controller_steering_sensitivity",
                "Current UI steering sensitivity factor",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;
        let controller_throttle_sensitivity = Gauge::with_opts(
            Opts::new(
                "controller_throttle_sensitivity",
                "Current UI throttle sensitivity factor",
            )
            .namespace(consts::METRICS_NAMESPACE),
        )?;

        prometheus::register(Box::new(stage_real.clone()))?;
        prometheus::register(Box::new(stage_cpu.clone()))?;
        prometheus::register(Box::new(stage_wait.clone()))?;
        prometheus::register(Box::new(camera_setting.clone()))?;
        prometheus::register(Box::new(capture_actual_fps.clone()))?;
        prometheus::register(Box::new(capture_frame_period_ms.clone()))?;
        prometheus::register(Box::new(frames_total.clone()))?;
        prometheus::register(Box::new(total_lines.clone()))?;
        prometheus::register(Box::new(vertical_lines.clone()))?;
        prometheus::register(Box::new(horizontal_lines.clone()))?;
        prometheus::register(Box::new(outlier_lines.clone()))?;
        prometheus::register(Box::new(angle_from_vertical.clone()))?;
        prometheus::register(Box::new(alignment_confidence.clone()))?;
        prometheus::register(Box::new(vertical_spread.clone()))?;
        prometheus::register(Box::new(horizontal_spread.clone()))?;
        prometheus::register(Box::new(min_angle.clone()))?;
        prometheus::register(Box::new(max_angle.clone()))?;
        prometheus::register(Box::new(controller_connected.clone()))?;
        prometheus::register(Box::new(controller_steer_us.clone()))?;
        prometheus::register(Box::new(controller_throttle_us.clone()))?;
        prometheus::register(Box::new(controller_speed_mps.clone()))?;
        prometheus::register(Box::new(controller_setpoint_mps.clone()))?;
        prometheus::register(Box::new(controller_error.clone()))?;
        prometheus::register(Box::new(controller_hall_delta_t_us.clone()))?;
        prometheus::register(Box::new(controller_kalman0.clone()))?;
        prometheus::register(Box::new(controller_kalman1.clone()))?;
        prometheus::register(Box::new(controller_kalman2.clone()))?;
        prometheus::register(Box::new(controller_kalman3.clone()))?;
        prometheus::register(Box::new(controller_distance0_cm.clone()))?;
        prometheus::register(Box::new(controller_distance1_cm.clone()))?;
        prometheus::register(Box::new(controller_distance2_cm.clone()))?;
        prometheus::register(Box::new(controller_drive_mode.clone()))?;
        prometheus::register(Box::new(controller_wall_left_correction_deg.clone()))?;
        prometheus::register(Box::new(controller_wall_right_correction_deg.clone()))?;
        prometheus::register(Box::new(controller_wall_combined_correction_deg.clone()))?;
        prometheus::register(Box::new(controller_last_update_us.clone()))?;
        prometheus::register(Box::new(controller_parse_errors.clone()))?;
        prometheus::register(Box::new(controller_serial_errors.clone()))?;
        prometheus::register(Box::new(controller_mode_manual.clone()))?;
        prometheus::register(Box::new(controller_steering_sensitivity.clone()))?;
        prometheus::register(Box::new(controller_throttle_sensitivity.clone()))?;

        Ok(Self {
            stage_real,
            stage_cpu,
            stage_wait,
            camera_setting,
            capture_actual_fps,
            capture_frame_period_ms,
            frames_total,
            total_lines,
            vertical_lines,
            horizontal_lines,
            outlier_lines,
            angle_from_vertical,
            alignment_confidence,
            vertical_spread,
            horizontal_spread,
            min_angle,
            max_angle,
            controller_connected,
            controller_steer_us,
            controller_throttle_us,
            controller_speed_mps,
            controller_setpoint_mps,
            controller_error,
            controller_hall_delta_t_us,
            controller_kalman0,
            controller_kalman1,
            controller_kalman2,
            controller_kalman3,
            controller_distance0_cm,
            controller_distance1_cm,
            controller_distance2_cm,
            controller_drive_mode,
            controller_wall_left_correction_deg,
            controller_wall_right_correction_deg,
            controller_wall_combined_correction_deg,
            controller_last_update_us,
            controller_parse_errors,
            controller_serial_errors,
            controller_mode_manual,
            controller_steering_sensitivity,
            controller_throttle_sensitivity,
            trackers: HashMap::new(),
        })
    }

    fn update_controller(&self, controller: ControllerMetrics) {
        self.controller_connected.set(controller.connected);
        self.controller_steer_us.set(controller.steer_us);
        self.controller_throttle_us.set(controller.throttle_us);
        self.controller_speed_mps.set(controller.speed_mps);
        self.controller_setpoint_mps.set(controller.setpoint_mps);
        self.controller_error.set(controller.error_value);
        self.controller_hall_delta_t_us
            .set(controller.hall_delta_t_us);
        self.controller_kalman0.set(controller.kalman0);
        self.controller_kalman1.set(controller.kalman1);
        self.controller_kalman2.set(controller.kalman2);
        self.controller_kalman3.set(controller.kalman3);
        self.controller_distance0_cm.set(controller.distance0_cm);
        self.controller_distance1_cm.set(controller.distance1_cm);
        self.controller_distance2_cm.set(controller.distance2_cm);
        if controller.drive_mode.is_finite() {
            self.controller_drive_mode.set(controller.drive_mode);
        }
        if controller.wall_left_correction_deg.is_finite() {
            self.controller_wall_left_correction_deg
                .set(controller.wall_left_correction_deg);
        }
        if controller.wall_right_correction_deg.is_finite() {
            self.controller_wall_right_correction_deg
                .set(controller.wall_right_correction_deg);
        }
        if controller.wall_combined_correction_deg.is_finite() {
            self.controller_wall_combined_correction_deg
                .set(controller.wall_combined_correction_deg);
        }
        self.controller_last_update_us
            .set(controller.last_update_us);
        self.controller_parse_errors.set(controller.parse_errors);
        self.controller_serial_errors.set(controller.serial_errors);
        self.controller_mode_manual.set(controller.mode_manual);
        self.controller_steering_sensitivity
            .set(controller.steering_sensitivity);
        self.controller_throttle_sensitivity
            .set(controller.throttle_sensitivity);
    }

    fn set_camera_setting(&self, setting: &'static str, value: Option<f64>) {
        if let Some(value) = value {
            self.camera_setting.with_label_values(&[setting]).set(value);
        }
    }

    fn update(&mut self, msg: MetricsMsg) {
        if msg.real_us > 0 || msg.cpu_us > 0 {
            let real_s = msg.real_us as f64 / 1_000_000.0;
            let cpu_s = msg.cpu_us as f64 / 1_000_000.0;
            let wait_s = (real_s - cpu_s).max(0.0);
            let trackers = self
                .trackers
                .entry(msg.stage)
                .or_insert_with(StageTrackers::new);
            trackers.real.push(real_s);
            trackers.cpu.push(cpu_s);
            trackers.wait.push(wait_s);

            self.stage_real
                .with_label_values(&[msg.stage, "p01"])
                .set(trackers.real.quantile(0.01));
            self.stage_real
                .with_label_values(&[msg.stage, "p99"])
                .set(trackers.real.quantile(0.99));
            self.stage_cpu
                .with_label_values(&[msg.stage, "p01"])
                .set(trackers.cpu.quantile(0.01));
            self.stage_cpu
                .with_label_values(&[msg.stage, "p99"])
                .set(trackers.cpu.quantile(0.99));
            self.stage_wait
                .with_label_values(&[msg.stage, "p01"])
                .set(trackers.wait.quantile(0.01));
            self.stage_wait
                .with_label_values(&[msg.stage, "p99"])
                .set(trackers.wait.quantile(0.99));
            self.frames_total.with_label_values(&[msg.stage]).inc();
        }

        if let Some(camera) = msg.camera {
            self.set_camera_setting("requested_width", camera.requested_width);
            self.set_camera_setting("requested_height", camera.requested_height);
            self.set_camera_setting("requested_fps", camera.requested_fps);
            self.set_camera_setting("applied_backend", camera.applied_backend);
            self.set_camera_setting("applied_width", camera.applied_width);
            self.set_camera_setting("applied_height", camera.applied_height);
            self.set_camera_setting("applied_fps", camera.applied_fps);
            self.set_camera_setting("auto_exposure", camera.auto_exposure);
            self.set_camera_setting("exposure", camera.exposure);
            self.set_camera_setting("buffer_size", camera.buffer_size);
            self.set_camera_setting("fourcc_code", camera.fourcc_code);
            if let Some(actual_fps) = camera.actual_fps {
                self.capture_actual_fps.set(actual_fps);
            }
            if let Some(frame_period_ms) = camera.frame_period_ms {
                self.capture_frame_period_ms.set(frame_period_ms);
            }
        }

        if let Some(lines) = msg.lines {
            self.total_lines.set(lines.total_lines as f64);
            self.vertical_lines.set(lines.vertical_lines as f64);
            self.horizontal_lines.set(lines.horizontal_lines as f64);
            self.outlier_lines.set(lines.outlier_lines as f64);
            self.angle_from_vertical
                .set(lines.angle_from_vertical_deg as f64);
            self.alignment_confidence.set(lines.confidence as f64);
            self.vertical_spread.set(lines.vertical_spread_deg as f64);
            self.horizontal_spread
                .set(lines.horizontal_spread_deg as f64);
            self.min_angle.set(lines.min_angle_deg as f64);
            self.max_angle.set(lines.max_angle_deg as f64);
        }

        if let Some(controller) = msg.controller {
            self.update_controller(controller);
        }
    }
}

fn serve_metrics(server: Arc<tiny_http::Server>) {
    let encoder = TextEncoder::new();
    for request in server.incoming_requests() {
        let families = prometheus::gather();
        let mut buf = Vec::with_capacity(4096);
        if let Err(error) = encoder.encode(&families, &mut buf) {
            error!("[metrics] encode error: {error}");
            continue;
        }
        let header = "Content-Type: text/plain; version=0.0.4; charset=utf-8"
            .parse::<tiny_http::Header>()
            .unwrap();
        let response = tiny_http::Response::from_data(buf).with_header(header);
        if let Err(error) = request.respond(response) {
            error!("[metrics] respond error: {error}");
        }
    }
}

pub fn run_metrics(rx: Receiver<MetricsMsg>) -> Result<()> {
    let mut metrics = Metrics::new()?;
    let server = Arc::new(
        tiny_http::Server::http(consts::METRICS_HTTP_BIND)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    );
    info!(
        "[metrics] Prometheus endpoint -> http://{}/metrics",
        consts::METRICS_HTTP_BIND
    );

    let server_clone = Arc::clone(&server);
    thread::spawn(move || serve_metrics(server_clone));

    for msg in rx {
        metrics.update(msg);
    }

    server.unblock();
    Ok(())
}

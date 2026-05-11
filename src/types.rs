use opencv::{core::Point, prelude::Mat};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisClass {
    Vertical,
    Horizontal,
    Outlier,
}

impl AxisClass {
    pub fn as_str(self) -> &'static str {
        match self {
            AxisClass::Vertical => "vertical",
            AxisClass::Horizontal => "horizontal",
            AxisClass::Outlier => "outlier",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RawLine {
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub start: Point,
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub end: Point,
    pub angle_deg: f32,
    pub length_px: f32,
}

#[derive(Clone, Debug)]
pub struct ClassifiedLine {
    pub raw: RawLine,
    pub axis: AxisClass,
    pub axis_error_deg: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AngleClusterStats {
    pub count: u32,
    pub mean_deg: f32,
    pub min_deg: f32,
    pub max_deg: f32,
    pub spread_deg: f32,
    pub stddev_deg: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct AlignmentReport {
    pub dominant_axis: AxisClass,
    pub angle_from_vertical_deg: f32,
    pub confidence: f32,
    pub vertical: AngleClusterStats,
    pub horizontal: AngleClusterStats,
    pub outlier_count: u32,
    pub total_lines: u32,
}

impl Default for AlignmentReport {
    fn default() -> Self {
        Self {
            dominant_axis: AxisClass::Outlier,
            angle_from_vertical_deg: 0.0,
            confidence: 0.0,
            vertical: AngleClusterStats::default(),
            horizontal: AngleClusterStats::default(),
            outlier_count: 0,
            total_lines: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LineMetrics {
    pub total_lines: u32,
    pub vertical_lines: u32,
    pub horizontal_lines: u32,
    pub outlier_lines: u32,
    pub angle_from_vertical_deg: f32,
    pub confidence: f32,
    pub vertical_spread_deg: f32,
    pub horizontal_spread_deg: f32,
    pub min_angle_deg: f32,
    pub max_angle_deg: f32,
}

#[derive(Clone, Debug, Default)]
pub struct CameraMetrics {
    pub requested_width: Option<f64>,
    pub requested_height: Option<f64>,
    pub requested_fps: Option<f64>,
    pub applied_backend: Option<f64>,
    pub applied_width: Option<f64>,
    pub applied_height: Option<f64>,
    pub applied_fps: Option<f64>,
    pub auto_exposure: Option<f64>,
    pub exposure: Option<f64>,
    pub buffer_size: Option<f64>,
    pub fourcc_code: Option<f64>,
    pub actual_fps: Option<f64>,
    pub frame_period_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Clone, Debug, Serialize)]
pub struct ControllerTelemetrySnapshot {
    pub timestamp_us: u64,
    pub steer_us: i32,
    pub throttle_us: i32,
    pub speed_mps: f32,
    pub setpoint_mps: Option<f32>,
    pub error_value: Option<f32>,
    pub hall_delta_t_us: Option<u32>,
    pub kalman0: Option<f32>,
    pub kalman1: Option<f32>,
    pub kalman2: Option<f32>,
    pub kalman3: Option<f32>,
    pub distance0_cm: Option<u32>,
    pub distance1_cm: Option<u32>,
    pub distance2_cm: Option<u32>,
    pub connected: bool,
    pub last_update_us: u64,
    pub parse_errors: u64,
    pub serial_errors: u64,
    pub mode: ControllerMode,
    pub steering_sensitivity: f32,
    pub throttle_sensitivity: f32,
    pub selected_port: Option<String>,
}

impl Default for ControllerTelemetrySnapshot {
    fn default() -> Self {
        Self {
            timestamp_us: 0,
            steer_us: 1500,
            throttle_us: 1500,
            speed_mps: 0.0,
            setpoint_mps: None,
            error_value: None,
            hall_delta_t_us: None,
            kalman0: None,
            kalman1: None,
            kalman2: None,
            kalman3: None,
            distance0_cm: None,
            distance1_cm: None,
            distance2_cm: None,
            connected: false,
            last_update_us: 0,
            parse_errors: 0,
            serial_errors: 0,
            mode: ControllerMode::Manual,
            steering_sensitivity: 1.0,
            throttle_sensitivity: 1.0,
            selected_port: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerMetrics {
    pub connected: f64,
    pub steer_us: f64,
    pub throttle_us: f64,
    pub speed_mps: f64,
    pub setpoint_mps: f64,
    pub error_value: f64,
    pub hall_delta_t_us: f64,
    pub kalman0: f64,
    pub kalman1: f64,
    pub kalman2: f64,
    pub kalman3: f64,
    pub distance0_cm: f64,
    pub distance1_cm: f64,
    pub distance2_cm: f64,
    pub last_update_us: f64,
    pub parse_errors: f64,
    pub serial_errors: f64,
    pub mode_manual: f64,
    pub steering_sensitivity: f64,
    pub throttle_sensitivity: f64,
}

impl From<&ControllerTelemetrySnapshot> for ControllerMetrics {
    fn from(value: &ControllerTelemetrySnapshot) -> Self {
        Self {
            connected: if value.connected { 1.0 } else { 0.0 },
            steer_us: value.steer_us as f64,
            throttle_us: value.throttle_us as f64,
            speed_mps: value.speed_mps as f64,
            setpoint_mps: opt_f32_metric(value.setpoint_mps),
            error_value: opt_f32_metric(value.error_value),
            hall_delta_t_us: opt_u32_metric(value.hall_delta_t_us),
            kalman0: opt_f32_metric(value.kalman0),
            kalman1: opt_f32_metric(value.kalman1),
            kalman2: opt_f32_metric(value.kalman2),
            kalman3: opt_f32_metric(value.kalman3),
            distance0_cm: opt_u32_metric(value.distance0_cm),
            distance1_cm: opt_u32_metric(value.distance1_cm),
            distance2_cm: opt_u32_metric(value.distance2_cm),
            last_update_us: value.last_update_us as f64,
            parse_errors: value.parse_errors as f64,
            serial_errors: value.serial_errors as f64,
            mode_manual: if value.mode == ControllerMode::Manual {
                1.0
            } else {
                0.0
            },
            steering_sensitivity: value.steering_sensitivity as f64,
            throttle_sensitivity: value.throttle_sensitivity as f64,
        }
    }
}

fn opt_f32_metric(value: Option<f32>) -> f64 {
    value.map(f64::from).unwrap_or(f64::NAN)
}

fn opt_u32_metric(value: Option<u32>) -> f64 {
    value.map(|value| value as f64).unwrap_or(f64::NAN)
}

pub struct MetricsMsg {
    pub stage: &'static str,
    pub real_us: u64,
    pub cpu_us: u64,
    pub lines: Option<LineMetrics>,
    pub camera: Option<CameraMetrics>,
    pub controller: Option<ControllerMetrics>,
}

pub struct EnhanceMsg {
    pub frame: Mat,
    pub gray_contrast: Mat,
    pub edges: Mat,
}

pub struct DetectMsg {
    pub frame: Mat,
    pub gray_contrast: Mat,
    pub edges: Mat,
    pub lines: Vec<RawLine>,
}

pub struct ClassifiedMsg {
    pub frame: Mat,
    pub gray_contrast: Mat,
    pub edges: Mat,
    pub lines: Vec<ClassifiedLine>,
    pub report: AlignmentReport,
}

pub struct AlignmentMsg {
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub frame: Mat,
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub gray_contrast: Mat,
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub edges: Mat,
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub lines: Vec<ClassifiedLine>,
    pub report: AlignmentReport,
    #[cfg_attr(feature = "no-display", allow(dead_code))]
    pub serial_frame: String,
}

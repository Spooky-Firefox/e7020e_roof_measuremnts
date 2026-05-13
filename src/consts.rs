#[cfg_attr(feature = "no-display", allow(dead_code))]
pub const WINDOW_WIDTH: i32 = 1440;
#[cfg_attr(feature = "no-display", allow(dead_code))]
pub const WINDOW_HEIGHT: i32 = 900;

pub const DEFAULT_CAMERA_INDEX: i32 = 0;
pub const DEFAULT_CAMERA_WIDTH: i32 = 1280;
pub const DEFAULT_CAMERA_HEIGHT: i32 = 720;
pub const DEFAULT_CAMERA_FPS: f64 = 30.0;
pub const DEFAULT_CAMERA_BUFFER_SIZE: i32 = 3;
pub const DEFAULT_CAMERA_FOURCC: &str = "MJPG";
pub const CAMERA_WARMUP_READS: usize = 30;
pub const CENTER_CROP_WIDTH_FRACTION: f64 = 1.0;
pub const CENTER_CROP_HEIGHT_FRACTION: f64 = 1.0;
pub const PROCESSING_DOWNSCALE: f64 = 0.25;

pub const CLAHE_CLIP_LIMIT: f64 = 2.4;
pub const CLAHE_TILE_SIZE: i32 = 8;
pub const GAUSSIAN_BLUR_KSIZE: i32 = 5;
pub const CANNY_LOW_THRESHOLD: f64 = 24.0;
pub const CANNY_HIGH_THRESHOLD: f64 = 72.0;
pub const EDGE_DILATE_KERNEL_SIZE: i32 = 3;
pub const PRIMARY_HOUGH_THRESHOLD: i32 = 80;
pub const PRIMARY_HOUGH_MIN_LINE_LENGTH: f64 = 160.0;
pub const PRIMARY_HOUGH_MAX_LINE_GAP: f64 = 10.0;

pub const HOUGH_RHO: f64 = 1.0;
pub const HOUGH_THETA_DEG: f64 = 1.0;

pub const CLASS_ANGLE_MARGIN_DEG: f32 = 30.0;
pub const MIN_CLASSIFIED_LINES: usize = 2;

pub const SERIAL_BAUD_RATE: u32 = 115_200;

pub const METRICS_HTTP_BIND: &str = "0.0.0.0:9090";
pub const CONTROLLER_HTTP_BIND: &str = "0.0.0.0:9091";
pub const METRICS_NAMESPACE: &str = "roof_alignment";
pub const CONTROLLER_LOG_DIR: &str = "controller_logs";
pub const CONTROLLER_SERIAL_TIMEOUT_MS: u64 = 100;

#[cfg_attr(feature = "no-display", allow(dead_code))]
pub const OVERLAY_TEXT_SCALE: f64 = 0.7;
#[cfg_attr(feature = "no-display", allow(dead_code))]
pub const OVERLAY_TEXT_THICKNESS: i32 = 2;

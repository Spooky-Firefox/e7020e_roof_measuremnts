mod capture;
mod classify;
mod consts;
mod controller_service;
mod decide;
mod detect;
mod display;
mod enhance;
mod metrics;
mod serial;
mod types;

use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use log::{info, warn};
use opencv::{prelude::*, videoio};
use std::thread;

#[cfg(not(feature = "no-display"))]
use opencv::highgui;

use types::{AlignmentMsg, CameraMetrics, ClassifiedMsg, DetectMsg, EnhanceMsg, MetricsMsg};

fn requested_camera_index() -> i32 {
    std::env::var("ROOF_CAMERA_INDEX")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(consts::DEFAULT_CAMERA_INDEX)
}

fn read_camera_prop(cam: &videoio::VideoCapture, prop: i32) -> f64 {
    cam.get(prop).unwrap_or(-1.0)
}

fn preferred_camera_backend() -> i32 {
    #[cfg(target_os = "linux")]
    {
        videoio::CAP_V4L2
    }

    #[cfg(not(target_os = "linux"))]
    {
        videoio::CAP_ANY
    }
}

fn fourcc_from_str(code: &str) -> i32 {
    let bytes = code.as_bytes();
    if bytes.len() != 4 {
        return 0;
    }

    i32::from(bytes[0])
        | (i32::from(bytes[1]) << 8)
        | (i32::from(bytes[2]) << 16)
        | (i32::from(bytes[3]) << 24)
}

fn fourcc_to_string(value: f64) -> String {
    let code = value.round() as u32;
    let bytes = [
        (code & 0xFF) as u8,
        ((code >> 8) & 0xFF) as u8,
        ((code >> 16) & 0xFF) as u8,
        ((code >> 24) & 0xFF) as u8,
    ];

    if bytes.iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("0x{code:08X}")
    }
}

fn camera_metrics_snapshot(cam: &videoio::VideoCapture) -> CameraMetrics {
    CameraMetrics {
        requested_width: Some(consts::DEFAULT_CAMERA_WIDTH as f64),
        requested_height: Some(consts::DEFAULT_CAMERA_HEIGHT as f64),
        requested_fps: Some(consts::DEFAULT_CAMERA_FPS),
        applied_backend: Some(read_camera_prop(cam, videoio::CAP_PROP_BACKEND)),
        applied_width: Some(read_camera_prop(cam, videoio::CAP_PROP_FRAME_WIDTH)),
        applied_height: Some(read_camera_prop(cam, videoio::CAP_PROP_FRAME_HEIGHT)),
        applied_fps: Some(read_camera_prop(cam, videoio::CAP_PROP_FPS)),
        auto_exposure: Some(read_camera_prop(cam, videoio::CAP_PROP_AUTO_EXPOSURE)),
        exposure: Some(read_camera_prop(cam, videoio::CAP_PROP_EXPOSURE)),
        buffer_size: Some(read_camera_prop(cam, videoio::CAP_PROP_BUFFERSIZE)),
        fourcc_code: Some(read_camera_prop(cam, videoio::CAP_PROP_FOURCC)),
        actual_fps: None,
        frame_period_ms: None,
    }
}

fn log_camera_settings(label: &str, cam: &videoio::VideoCapture) {
    let backend = read_camera_prop(cam, videoio::CAP_PROP_BACKEND);
    let width = read_camera_prop(cam, videoio::CAP_PROP_FRAME_WIDTH);
    let height = read_camera_prop(cam, videoio::CAP_PROP_FRAME_HEIGHT);
    let fps = read_camera_prop(cam, videoio::CAP_PROP_FPS);
    let auto_exposure = read_camera_prop(cam, videoio::CAP_PROP_AUTO_EXPOSURE);
    let exposure = read_camera_prop(cam, videoio::CAP_PROP_EXPOSURE);
    let buffer_size = read_camera_prop(cam, videoio::CAP_PROP_BUFFERSIZE);
    let fourcc = fourcc_to_string(read_camera_prop(cam, videoio::CAP_PROP_FOURCC));

    info!(
        "[camera:{label}] requested index={} backend={} width={} height={} fps={} fourcc={} buffersize={}",
        requested_camera_index(),
        preferred_camera_backend(),
        consts::DEFAULT_CAMERA_WIDTH,
        consts::DEFAULT_CAMERA_HEIGHT,
        consts::DEFAULT_CAMERA_FPS,
        consts::DEFAULT_CAMERA_FOURCC,
        consts::DEFAULT_CAMERA_BUFFER_SIZE,
    );
    info!(
        "[camera:{label}] applied backend={backend:.0} width={width:.0} height={height:.0} fps={fps:.2} fourcc={fourcc} auto_exposure={auto_exposure:.3} exposure={exposure:.3} buffersize={buffer_size:.0}",
    );

    if (width - consts::DEFAULT_CAMERA_WIDTH as f64).abs() > 1.0
        || (height - consts::DEFAULT_CAMERA_HEIGHT as f64).abs() > 1.0
        || (fps - consts::DEFAULT_CAMERA_FPS).abs() > 0.5
    {
        warn!(
            "[camera:{label}] applied mode differs from requested mode; this usually means the driver negotiated a different device stream or pixel format"
        );
    }
}

fn open_camera() -> Result<videoio::VideoCapture> {
    let camera_index = std::env::var("ROOF_CAMERA_INDEX")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(consts::DEFAULT_CAMERA_INDEX);

    let mut cam = videoio::VideoCapture::new(camera_index, preferred_camera_backend())
        .with_context(|| format!("failed to open camera index {camera_index}"))?;

    cam.set(
        videoio::CAP_PROP_FOURCC,
        fourcc_from_str(consts::DEFAULT_CAMERA_FOURCC) as f64,
    )
    .ok();
    cam.set(
        videoio::CAP_PROP_FRAME_WIDTH,
        consts::DEFAULT_CAMERA_WIDTH as f64,
    )
    .ok();
    cam.set(
        videoio::CAP_PROP_FRAME_HEIGHT,
        consts::DEFAULT_CAMERA_HEIGHT as f64,
    )
    .ok();
    cam.set(videoio::CAP_PROP_FPS, consts::DEFAULT_CAMERA_FPS).ok();
    cam.set(
        videoio::CAP_PROP_BUFFERSIZE,
        consts::DEFAULT_CAMERA_BUFFER_SIZE as f64,
    )
    .ok();

    if !cam.is_opened()? {
        anyhow::bail!("camera index {camera_index} did not open");
    }

    log_camera_settings("open", &cam);

    Ok(cam)
}

fn warmup_frame(cam: &mut videoio::VideoCapture) -> Result<opencv::core::Size> {
    let mut warmup = Mat::default();
    for _ in 0..consts::CAMERA_WARMUP_READS {
        cam.read(&mut warmup)?;
        if !warmup.empty() {
            return Ok(opencv::core::Size::new(warmup.cols(), warmup.rows()));
        }
    }

    anyhow::bail!("camera produced no frames during warm-up");
}

fn processing_frame_size(full_size: opencv::core::Size) -> opencv::core::Size {
    let width = ((full_size.width as f64) * consts::CENTER_CROP_WIDTH_FRACTION).round() as i32;
    let height = ((full_size.height as f64) * consts::CENTER_CROP_HEIGHT_FRACTION).round() as i32;
    let scaled_width = ((width as f64) * consts::PROCESSING_DOWNSCALE).round() as i32;
    let scaled_height = ((height as f64) * consts::PROCESSING_DOWNSCALE).round() as i32;
    opencv::core::Size::new(scaled_width.max(1), scaled_height.max(1))
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("e7020e_roof_control_hub=debug"),
    )
    .init();

    #[cfg(not(feature = "no-display"))]
    highgui::named_window("roof-alignment", highgui::WINDOW_NORMAL)?;
    #[cfg(not(feature = "no-display"))]
    highgui::resize_window(
        "roof-alignment",
        consts::WINDOW_WIDTH,
        consts::WINDOW_HEIGHT,
    )?;

    let mut cam = open_camera()?;
    let frame_size = processing_frame_size(warmup_frame(&mut cam)?);
    log_camera_settings("warmup", &cam);

    let (tx_cap, rx_cap) = bounded::<Mat>(2);
    let (tx_enh, rx_enh) = bounded::<EnhanceMsg>(2);
    let (tx_det, rx_det) = bounded::<DetectMsg>(2);
    let (tx_cls, rx_cls) = bounded::<ClassifiedMsg>(2);
    let (tx_align, rx_align) = bounded::<AlignmentMsg>(2);
    let (tx_metrics, rx_metrics) = bounded::<MetricsMsg>(128);

    tx_metrics
        .try_send(MetricsMsg {
            stage: "camera",
            real_us: 0,
            cpu_us: 0,
            lines: None,
            camera: Some(camera_metrics_snapshot(&cam)),
            controller: None,
        })
        .ok();

    let tx_m1 = tx_metrics.clone();
    let tx_m2 = tx_metrics.clone();
    let tx_m3 = tx_metrics.clone();
    let tx_m4 = tx_metrics.clone();
    let tx_m5 = tx_metrics;
    let tx_controller = tx_m5.clone();

    let t1 = thread::spawn(move || capture::run_capture(cam, tx_cap, tx_m1));
    let t2 = thread::spawn(move || enhance::run_enhance(rx_cap, tx_enh, tx_m2));
    let t3 = thread::spawn(move || detect::run_detect(rx_enh, tx_det, tx_m3));
    let t4 = thread::spawn(move || classify::run_classify(rx_det, tx_cls, tx_m4));
    let t5 = thread::spawn(move || decide::run_decide(rx_cls, tx_align, tx_m5));
    let t6 = thread::spawn(move || metrics::run_metrics(rx_metrics));
    let t7 = thread::spawn(move || controller_service::run_controller_service(tx_controller));

    display::run_display(rx_align, frame_size)?;

    for res in [t1.join(), t2.join(), t3.join(), t4.join(), t5.join()] {
        if let Ok(Err(error)) = res {
            eprintln!("Pipeline thread error: {error}");
        }
    }
    if let Ok(Err(error)) = t6.join() {
        eprintln!("Metrics thread error: {error}");
    }
    if let Ok(Err(error)) = t7.join() {
        eprintln!("Controller service thread error: {error}");
    }

    Ok(())
}

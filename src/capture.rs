use anyhow::Result;
use cpu_time::ThreadTime;
use crossbeam_channel::Sender;
use log::info;
use std::time::Instant;

use crate::types::{CameraMetrics, MetricsMsg};

#[cfg(feature = "camera-support")]
use opencv::{prelude::*, videoio};

#[cfg(feature = "camera-support")]
pub fn run_capture(
    mut cam: videoio::VideoCapture,
    tx: Sender<Mat>,
    tx_metrics: Sender<MetricsMsg>,
) -> Result<()> {
    let mut previous_frame_at: Option<Instant> = None;
    let mut log_started = Instant::now();
    let mut frames_since_log = 0u64;
    loop {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();

        let mut frame = Mat::default();
        cam.read(&mut frame)?;
        if frame.empty() {
            continue;
        }

        let captured_at = Instant::now();
        let (actual_fps, frame_period_ms) = if let Some(previous) = previous_frame_at {
            let period_s = captured_at.duration_since(previous).as_secs_f64();
            let fps = if period_s > 0.0 { 1.0 / period_s } else { 0.0 };
            (Some(fps), Some(period_s * 1000.0))
        } else {
            (None, None)
        };
        previous_frame_at = Some(captured_at);
        frames_since_log += 1;

        if log_started.elapsed().as_secs_f64() >= 5.0 {
            let elapsed = log_started.elapsed().as_secs_f64();
            let avg_fps = if elapsed > 0.0 {
                frames_since_log as f64 / elapsed
            } else {
                0.0
            };
            info!(
                "[capture] avg_fps={avg_fps:.2} instant_fps={:.2} frame_period_ms={:.1}",
                actual_fps.unwrap_or(0.0),
                frame_period_ms.unwrap_or(0.0)
            );
            log_started = Instant::now();
            frames_since_log = 0;
        }

        tx_metrics
            .try_send(MetricsMsg {
                stage: "capture",
                real_us: t_real.elapsed().as_micros() as u64,
                cpu_us: t_cpu.elapsed().as_micros() as u64,
                lines: None,
                camera: Some(CameraMetrics {
                    actual_fps,
                    frame_period_ms,
                    ..CameraMetrics::default()
                }),
                controller: None,
            })
            .ok();

        if tx.send(frame).is_err() {
            break;
        }
    }

    Ok(())
}

#[cfg(not(feature = "camera-support"))]
pub fn run_capture(tx: Sender<opencv::prelude::Mat>, tx_metrics: Sender<MetricsMsg>) -> Result<()> {
    use opencv::core;
    use opencv::prelude::*;
    use std::thread;

    info!("[capture] Running in no-camera mode - generating dummy frames");

    let mut previous_frame_at: Option<Instant> = None;
    let mut log_started = Instant::now();
    let mut frames_since_log = 0u64;
    let frame_interval = std::time::Duration::from_millis(33); // ~30 FPS

    loop {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();

        // Create a dummy frame (gray mat with alternating patterns)
        let frame = {
            let width = crate::consts::DEFAULT_CAMERA_WIDTH;
            let height = crate::consts::DEFAULT_CAMERA_HEIGHT;
            let mut dummy = Mat::zeros(height as i32, width as i32, core::CV_8U)?.to_mat()?;

            // Add some simple patterns to make it non-trivial
            let frame_count = frames_since_log as i32;
            let val = ((frame_count * 5) % 256) as u8;
            let mask = Mat::default();
            dummy.set_to(&core::Scalar::new(val as f64, 0.0, 0.0, 0.0), &mask)?;

            dummy
        };

        let captured_at = Instant::now();
        let (actual_fps, frame_period_ms) = if let Some(previous) = previous_frame_at {
            let period_s = captured_at.duration_since(previous).as_secs_f64();
            let fps = if period_s > 0.0 { 1.0 / period_s } else { 0.0 };
            (Some(fps), Some(period_s * 1000.0))
        } else {
            (None, None)
        };
        previous_frame_at = Some(captured_at);
        frames_since_log += 1;

        if log_started.elapsed().as_secs_f64() >= 5.0 {
            let elapsed = log_started.elapsed().as_secs_f64();
            let avg_fps = if elapsed > 0.0 {
                frames_since_log as f64 / elapsed
            } else {
                0.0
            };
            info!(
                "[capture-dummy] avg_fps={avg_fps:.2} instant_fps={:.2} frame_period_ms={:.1}",
                actual_fps.unwrap_or(0.0),
                frame_period_ms.unwrap_or(0.0)
            );
            log_started = Instant::now();
            frames_since_log = 0;
        }

        tx_metrics
            .try_send(MetricsMsg {
                stage: "capture",
                real_us: t_real.elapsed().as_micros() as u64,
                cpu_us: t_cpu.elapsed().as_micros() as u64,
                lines: None,
                camera: Some(CameraMetrics {
                    actual_fps,
                    frame_period_ms,
                    ..CameraMetrics::default()
                }),
                controller: None,
            })
            .ok();

        if tx.send(frame).is_err() {
            break;
        }

        // Sleep to maintain frame rate
        if frame_interval > t_real.elapsed() {
            thread::sleep(frame_interval - t_real.elapsed());
        }
    }

    Ok(())
}

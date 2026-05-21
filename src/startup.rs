use std::f32::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use cpu_time::ThreadTime;
use crossbeam_channel::{Receiver, Sender};
use log::{info, warn};
#[cfg(has_opencv_algorithm_hint)]
use opencv::core::AlgorithmHint;
use opencv::{core, imgproc, prelude::*};

use crate::{
    consts, controller_service,
    types::{
        ControllerMode, DisplayMsg, MetricsMsg, StartupCircle, StartupDisplayMsg,
        StartupPhase, StartupStatus,
    },
};

pub type SharedStartupState = Arc<Mutex<StartupStatus>>;

pub fn new_shared_state() -> SharedStartupState {
    Arc::new(Mutex::new(StartupStatus::default()))
}

pub fn snapshot(state: &SharedStartupState) -> StartupStatus {
    state.lock().map(|guard| guard.clone()).unwrap_or_default()
}

pub fn reset(state: &SharedStartupState) {
    if let Ok(mut guard) = state.lock() {
        let handoff_count = guard.handoff_count;
        *guard = StartupStatus::default();
        guard.handoff_count = handoff_count;
    }
}

fn update_search_status(state: &SharedStartupState, detected: &DetectionResult) {
    if let Ok(mut guard) = state.lock() {
        guard.phase = StartupPhase::SearchGreen;
        guard.green_detected = detected.green_detected;
        guard.green_fraction = detected.green_fraction;
        guard.green_ema = detected.green_ema;
        guard.best_circle = detected.best_circle.clone();
        guard.last_error = None;
    }
}

fn mark_handoff(state: &SharedStartupState, detected: &DetectionResult) {
    if let Ok(mut guard) = state.lock() {
        guard.phase = StartupPhase::RoofAlignment;
        guard.green_detected = detected.green_detected;
        guard.green_fraction = detected.green_fraction;
        guard.green_ema = detected.green_ema;
        guard.best_circle = detected.best_circle.clone();
        guard.handoff_count += 1;
        guard.last_error = None;
    }
}

fn set_error(state: &SharedStartupState, error: String) {
    if let Ok(mut guard) = state.lock() {
        guard.last_error = Some(error);
    }
}

#[derive(Clone, Debug)]
struct DetectionResult {
    green_detected: bool,
    green_fraction: f32,
    green_ema: f32,
    best_circle: Option<StartupCircle>,
    trigger_auto: bool,
}

pub struct StartupDetector {
    hsv: Mat,
    mask: Mat,
    blurred_mask: Mat,
    circles: Mat,
    circle_mask: Mat,
    scratch: Mat,
    green_ema: f32,
}

impl StartupDetector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            hsv: Mat::default(),
            mask: Mat::default(),
            blurred_mask: Mat::default(),
            circles: Mat::default(),
            circle_mask: Mat::default(),
            scratch: Mat::default(),
            green_ema: 0.0,
        })
    }

    pub fn reset(&mut self) {
        self.green_ema = 0.0;
        self.circles = Mat::default();
    }

    fn ensure_circle_mask(&mut self, rows: i32, cols: i32) -> Result<()> {
        if self.circle_mask.rows() == rows && self.circle_mask.cols() == cols {
            return Ok(());
        }

        self.circle_mask = Mat::zeros(rows, cols, core::CV_8UC1)?.to_mat()?;
        Ok(())
    }

    fn update_ema(&mut self, score: f32) -> f32 {
        if score >= consts::STARTUP_MIN_GREEN_FRACTION {
            self.green_ema +=
                consts::STARTUP_DECIDE_EMA_ALPHA * (score - self.green_ema);
        }
        self.green_ema
    }

    fn process(&mut self, frame: &Mat) -> Result<DetectionResult> {
        #[cfg(has_opencv_algorithm_hint)]
        imgproc::cvt_color(
            frame,
            &mut self.hsv,
            imgproc::COLOR_BGR2HSV,
            0,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::cvt_color(frame, &mut self.hsv, imgproc::COLOR_BGR2HSV, 0)?;

        core::in_range(
            &self.hsv,
            &core::Scalar::new(
                consts::STARTUP_GREEN_H_MIN,
                consts::STARTUP_GREEN_S_MIN,
                consts::STARTUP_GREEN_V_MIN,
                0.0,
            ),
            &core::Scalar::new(
                consts::STARTUP_GREEN_H_MAX,
                consts::STARTUP_GREEN_S_MAX,
                consts::STARTUP_GREEN_V_MAX,
                0.0,
            ),
            &mut self.mask,
        )?;

        #[cfg(has_opencv_algorithm_hint)]
        imgproc::gaussian_blur(
            &self.mask,
            &mut self.blurred_mask,
            core::Size::new(
                consts::STARTUP_MASK_BLUR_KSIZE,
                consts::STARTUP_MASK_BLUR_KSIZE,
            ),
            0.0,
            0.0,
            core::BORDER_DEFAULT,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::gaussian_blur(
            &self.mask,
            &mut self.blurred_mask,
            core::Size::new(
                consts::STARTUP_MASK_BLUR_KSIZE,
                consts::STARTUP_MASK_BLUR_KSIZE,
            ),
            0.0,
            0.0,
            core::BORDER_DEFAULT,
        )?;

        self.circles = Mat::default();
        imgproc::hough_circles(
            &self.blurred_mask,
            &mut self.circles,
            imgproc::HOUGH_GRADIENT,
            consts::STARTUP_HOUGH_DP,
            consts::STARTUP_HOUGH_MIN_DIST,
            consts::STARTUP_HOUGH_PARAM1,
            consts::STARTUP_HOUGH_PARAM2,
            consts::STARTUP_HOUGH_MIN_RADIUS,
            consts::STARTUP_HOUGH_MAX_RADIUS,
        )?;

        self.ensure_circle_mask(self.mask.rows(), self.mask.cols())?;
        let no_mask = Mat::default();
        let mut best_circle = None;
        let mut best_fraction = 0.0_f32;

        for (_, entry) in self.circles.iter::<core::Vec3f>()? {
            let center_x = entry[0].round() as i32;
            let center_y = entry[1].round() as i32;
            let radius = entry[2].round() as i32;
            if radius <= 0 {
                continue;
            }

            self.circle_mask
                .set_to(&core::Scalar::all(0.0), &no_mask)?;
            imgproc::circle(
                &mut self.circle_mask,
                core::Point::new(center_x, center_y),
                radius,
                core::Scalar::all(255.0),
                -1,
                imgproc::LINE_8,
                0,
            )?;
            core::bitwise_and(&self.mask, &self.circle_mask, &mut self.scratch, &no_mask)?;

            let green_pixels = core::count_non_zero(&self.scratch)? as f32;
            let area = (PI * (radius as f32) * (radius as f32)).max(1.0);
            let green_fraction = green_pixels / area;
            if green_fraction <= best_fraction {
                continue;
            }

            best_fraction = green_fraction;
            best_circle = Some(StartupCircle {
                center_x,
                center_y,
                radius,
                green_fraction,
            });
        }

        let green_ema = self.update_ema(best_fraction);
        Ok(DetectionResult {
            green_detected: best_fraction >= consts::STARTUP_MIN_GREEN_FRACTION,
            green_fraction: best_fraction,
            green_ema,
            best_circle,
            trigger_auto: green_ema >= consts::STARTUP_DECIDE_AUTO_THRESHOLD,
        })
    }

    fn display_mask(&self) -> Mat {
        self.mask.clone()
    }
}

pub fn run_startup_gate(
    rx: Receiver<(Mat, Instant)>,
    tx: Sender<(Mat, Instant)>,
    tx_display: Sender<DisplayMsg>,
    tx_metrics: Sender<MetricsMsg>,
    startup_state: SharedStartupState,
    controller: controller_service::ControllerHandle,
) -> Result<()> {
    let mut detector = StartupDetector::new()?;
    let mut previous_phase = StartupPhase::SearchGreen;

    for (frame, captured_at) in rx {
        let phase = snapshot(&startup_state).phase;
        if previous_phase == StartupPhase::RoofAlignment && phase == StartupPhase::SearchGreen {
            detector.reset();
        }
        previous_phase = phase;

        if phase == StartupPhase::RoofAlignment {
            if tx.send((frame, captured_at)).is_err() {
                break;
            }
            continue;
        }

        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let detected = detector.process(&frame)?;
        update_search_status(&startup_state, &detected);

        tx_metrics
            .try_send(MetricsMsg {
                stage: "startup",
                real_us: t_real.elapsed().as_micros() as u64,
                cpu_us: t_cpu.elapsed().as_micros() as u64,
                lines: None,
                camera: None,
                controller: None,
            })
            .ok();

        if !detected.trigger_auto {
            continue;
        }

        match controller_service::send_mode(
            &controller,
            ControllerMode::Auto,
            &tx_metrics,
        ) {
            Ok(_) => {
                info!(
                    "[startup] green circle confirmed; sending mode auto and switching to roof alignment"
                );
                mark_handoff(&startup_state, &detected);
                if tx.send((frame, captured_at)).is_err() {
                    break;
                }
                continue;
            }
            Err(error) => {
                warn!("[startup] failed to send mode auto: {error:#}");
                set_error(&startup_state, error.to_string());
            }
        }

        let display_msg = StartupDisplayMsg {
            frame,
            mask: detector.display_mask(),
            status: snapshot(&startup_state),
        };
        if tx_display.send(DisplayMsg::Startup(display_msg)).is_err() {
            break;
        }
    }

    Ok(())
}
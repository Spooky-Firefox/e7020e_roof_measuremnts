use anyhow::Result;
use cpu_time::ThreadTime;
use crossbeam_channel::{Receiver, Sender};
#[cfg(has_opencv_algorithm_hint)]
use opencv::core::AlgorithmHint;
use opencv::{core, imgproc, prelude::*};
use std::time::Instant;

use crate::{
    consts,
    types::{EnhanceMsg, MetricsMsg},
};

pub struct EnhanceStage {
    cropped_frame: Mat,
    lab: Mat,
    gray: Mat,
    gray_blurred: Mat,
    gray_contrast: Mat,
    edges: Mat,
    dilate_kernel: Mat,
    clahe: opencv::core::Ptr<imgproc::CLAHE>,
}

impl EnhanceStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cropped_frame: Mat::default(),
            lab: Mat::default(),
            gray: Mat::default(),
            gray_blurred: Mat::default(),
            gray_contrast: Mat::default(),
            edges: Mat::default(),
            dilate_kernel: imgproc::get_structuring_element(
                imgproc::MORPH_RECT,
                core::Size::new(
                    consts::EDGE_DILATE_KERNEL_SIZE,
                    consts::EDGE_DILATE_KERNEL_SIZE,
                ),
                core::Point::new(-1, -1),
            )?,
            clahe: imgproc::create_clahe(
                consts::CLAHE_CLIP_LIMIT,
                opencv::core::Size::new(consts::CLAHE_TILE_SIZE, consts::CLAHE_TILE_SIZE),
            )?,
        })
    }

    fn maybe_downscale(src: &Mat, dst: &mut Mat) -> Result<()> {
        if consts::PROCESSING_DOWNSCALE >= 0.999 {
            src.copy_to(dst)?;
            return Ok(());
        }

        imgproc::resize(
            src,
            dst,
            core::Size::new(0, 0),
            consts::PROCESSING_DOWNSCALE,
            consts::PROCESSING_DOWNSCALE,
            imgproc::INTER_AREA,
        )?;
        Ok(())
    }

    fn crop_center(frame: &Mat) -> Result<Mat> {
        let width = ((frame.cols() as f64) * consts::CENTER_CROP_WIDTH_FRACTION).round() as i32;
        let height = ((frame.rows() as f64) * consts::CENTER_CROP_HEIGHT_FRACTION).round() as i32;
        let width = width.max(1).min(frame.cols());
        let height = height.max(1).min(frame.rows());
        let x = ((frame.cols() - width) / 2).max(0);
        let y = ((frame.rows() - height) / 2).max(0);
        let roi = core::Rect::new(x, y, width, height);
        let view = Mat::roi(frame, roi)?;
        let mut cropped = Mat::default();
        view.copy_to(&mut cropped)?;
        Ok(cropped)
    }

    fn blur_and_canny(
        src: &Mat,
        blurred: &mut Mat,
        edges: &mut Mat,
        blur_ksize: i32,
    ) -> Result<()> {
        #[cfg(has_opencv_algorithm_hint)]
        imgproc::gaussian_blur(
            src,
            blurred,
            opencv::core::Size::new(blur_ksize, blur_ksize),
            0.0,
            0.0,
            opencv::core::BORDER_DEFAULT,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::gaussian_blur(
            src,
            blurred,
            opencv::core::Size::new(blur_ksize, blur_ksize),
            0.0,
            0.0,
            opencv::core::BORDER_DEFAULT,
        )?;
        imgproc::canny(
            blurred,
            edges,
            consts::CANNY_LOW_THRESHOLD,
            consts::CANNY_HIGH_THRESHOLD,
            3,
            false,
        )?;
        Ok(())
    }

    pub fn process(&mut self, frame: Mat, captured_at: Instant) -> Result<EnhanceMsg> {
        let cropped = Self::crop_center(&frame)?;
        Self::maybe_downscale(&cropped, &mut self.cropped_frame)?;
        #[cfg(has_opencv_algorithm_hint)]
        imgproc::cvt_color(
            &self.cropped_frame,
            &mut self.lab,
            imgproc::COLOR_BGR2Lab,
            0,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::cvt_color(
            &self.cropped_frame,
            &mut self.lab,
            imgproc::COLOR_BGR2Lab,
            0,
        )?;
        core::extract_channel(&self.lab, &mut self.gray, 0)?;
        self.clahe.apply(&self.gray, &mut self.gray_contrast)?;

        Self::blur_and_canny(
            &self.gray_contrast,
            &mut self.gray_blurred,
            &mut self.edges,
            consts::GAUSSIAN_BLUR_KSIZE,
        )?;
        let mut dilated_edges = Mat::default();
        imgproc::dilate(
            &self.edges,
            &mut dilated_edges,
            &self.dilate_kernel,
            core::Point::new(-1, -1),
            1,
            core::BORDER_DEFAULT,
            imgproc::morphology_default_border_value()?,
        )?;
        self.edges = dilated_edges;

        Ok(EnhanceMsg {
            frame: std::mem::take(&mut self.cropped_frame),
            gray_contrast: std::mem::take(&mut self.gray_contrast),
            edges: std::mem::take(&mut self.edges),
            captured_at,
        })
    }
}

pub fn run_enhance(
    rx: Receiver<(Mat, Instant)>,
    tx: Sender<EnhanceMsg>,
    tx_metrics: Sender<MetricsMsg>,
) -> Result<()> {
    let mut stage = EnhanceStage::new()?;
    for (frame, captured_at) in rx {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let out = stage.process(frame, captured_at)?;
        tx_metrics
            .try_send(MetricsMsg {
                stage: "enhance",
                real_us: t_real.elapsed().as_micros() as u64,
                cpu_us: t_cpu.elapsed().as_micros() as u64,
                lines: None,
                camera: None,
                controller: None,
            })
            .ok();
        if tx.send(out).is_err() {
            break;
        }
    }

    Ok(())
}

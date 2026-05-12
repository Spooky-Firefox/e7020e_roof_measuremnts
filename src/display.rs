use anyhow::Result;
use crossbeam_channel::Receiver;

use crate::types::AlignmentMsg;

#[cfg(not(feature = "no-display"))]
use crate::{consts, types::AxisClass};

#[cfg(not(feature = "no-display"))]
#[cfg(has_opencv_algorithm_hint)]
use opencv::core::AlgorithmHint;

#[cfg(not(feature = "no-display"))]
use opencv::{
    core::{self},
    highgui, imgproc,
    prelude::*,
};

#[cfg(not(feature = "no-display"))]
pub struct DisplayStage {
    gray_bgr: Mat,
    edges_bgr: Mat,
    annotated: Mat,
    canvas: Mat,
    frame_size: core::Size,
}

#[cfg(not(feature = "no-display"))]
impl DisplayStage {
    pub fn new(frame_size: core::Size) -> Result<Self> {
        Ok(Self {
            gray_bgr: Mat::default(),
            edges_bgr: Mat::default(),
            annotated: Mat::default(),
            canvas: Mat::zeros(frame_size.height * 2, frame_size.width * 2, core::CV_8UC3)?
                .to_mat()?,
            frame_size,
        })
    }

    fn line_color(axis: AxisClass) -> core::Scalar {
        match axis {
            AxisClass::Vertical => core::Scalar::new(0.0, 220.0, 0.0, 0.0),
            AxisClass::Horizontal => core::Scalar::new(255.0, 180.0, 0.0, 0.0),
            AxisClass::Outlier => core::Scalar::new(0.0, 0.0, 255.0, 0.0),
        }
    }

    pub fn render(&mut self, msg: AlignmentMsg) -> Result<()> {
        #[cfg(has_opencv_algorithm_hint)]
        imgproc::cvt_color(
            &msg.gray_contrast,
            &mut self.gray_bgr,
            imgproc::COLOR_GRAY2BGR,
            0,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::cvt_color(
            &msg.gray_contrast,
            &mut self.gray_bgr,
            imgproc::COLOR_GRAY2BGR,
            0,
        )?;
        #[cfg(has_opencv_algorithm_hint)]
        imgproc::cvt_color(
            &msg.edges,
            &mut self.edges_bgr,
            imgproc::COLOR_GRAY2BGR,
            0,
            AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        #[cfg(not(has_opencv_algorithm_hint))]
        imgproc::cvt_color(&msg.edges, &mut self.edges_bgr, imgproc::COLOR_GRAY2BGR, 0)?;
        msg.frame.copy_to(&mut self.annotated)?;

        for line in &msg.lines {
            let color = Self::line_color(line.axis);
            imgproc::line(
                &mut self.annotated,
                line.raw.start,
                line.raw.end,
                color,
                2,
                imgproc::LINE_AA,
                0,
            )?;
        }

        let overlay = format!(
            "angle={:.2} deg conf={:.2} v={} h={} o={} serial={} ",
            msg.report.angle_from_vertical_deg,
            msg.report.confidence,
            msg.report.vertical.count,
            msg.report.horizontal.count,
            msg.report.outlier_count,
            msg.serial_frame.trim_end(),
        );
        imgproc::put_text(
            &mut self.annotated,
            &overlay,
            core::Point::new(20, 30),
            imgproc::FONT_HERSHEY_SIMPLEX,
            consts::OVERLAY_TEXT_SCALE,
            core::Scalar::new(255.0, 255.0, 255.0, 0.0),
            consts::OVERLAY_TEXT_THICKNESS,
            imgproc::LINE_AA,
            false,
        )?;

        let stats = format!(
            "v[min={:.1} max={:.1} spread={:.1} std={:.1}] h[min={:.1} max={:.1} spread={:.1} std={:.1}]",
            msg.report.vertical.min_deg,
            msg.report.vertical.max_deg,
            msg.report.vertical.spread_deg,
            msg.report.vertical.stddev_deg,
            msg.report.horizontal.min_deg,
            msg.report.horizontal.max_deg,
            msg.report.horizontal.spread_deg,
            msg.report.horizontal.stddev_deg,
        );
        imgproc::put_text(
            &mut self.annotated,
            &stats,
            core::Point::new(20, 60),
            imgproc::FONT_HERSHEY_SIMPLEX,
            0.55,
            core::Scalar::new(180.0, 255.0, 255.0, 0.0),
            1,
            imgproc::LINE_AA,
            false,
        )?;

        let w = self.frame_size.width;
        let h = self.frame_size.height;
        msg.frame.copy_to(&mut Mat::roi_mut(
            &mut self.canvas,
            core::Rect::new(0, 0, w, h),
        )?)?;
        self.gray_bgr.copy_to(&mut Mat::roi_mut(
            &mut self.canvas,
            core::Rect::new(w, 0, w, h),
        )?)?;
        self.edges_bgr.copy_to(&mut Mat::roi_mut(
            &mut self.canvas,
            core::Rect::new(0, h, w, h),
        )?)?;
        self.annotated.copy_to(&mut Mat::roi_mut(
            &mut self.canvas,
            core::Rect::new(w, h, w, h),
        )?)?;

        highgui::imshow("roof-alignment", &self.canvas)?;
        Ok(())
    }
}

#[cfg(feature = "no-display")]
pub fn run_display(rx: Receiver<AlignmentMsg>, _frame_size: opencv::core::Size) -> Result<()> {
    for _ in rx {}
    Ok(())
}

#[cfg(not(feature = "no-display"))]
pub fn run_display(rx: Receiver<AlignmentMsg>, frame_size: opencv::core::Size) -> Result<()> {
    let mut stage = DisplayStage::new(frame_size)?;
    loop {
        let msg = match rx.recv() {
            Ok(msg) => msg,
            Err(_) => break,
        };
        stage.render(msg)?;
        if highgui::wait_key(1)? == 113 {
            break;
        }
    }
    Ok(())
}

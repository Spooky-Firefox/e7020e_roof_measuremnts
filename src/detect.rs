use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use cpu_time::ThreadTime;
use log::debug;
use opencv::{core, imgproc, prelude::*};
use std::time::Instant;

use crate::{
    consts,
    types::{DetectMsg, EnhanceMsg, MetricsMsg, RawLine},
};

pub struct DetectStage {
    lines: core::Vector<core::Vec4i>,
}

impl DetectStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            lines: core::Vector::new(),
        })
    }

    fn wrap_angle_180(angle_deg: f32) -> f32 {
        let mut wrapped = angle_deg % 180.0;
        if wrapped < 0.0 {
            wrapped += 180.0;
        }
        wrapped
    }

    fn to_raw_line(line: core::Vec4i) -> RawLine {
        let start = core::Point::new(line[0], line[1]);
        let end = core::Point::new(line[2], line[3]);
        let dx = (end.x - start.x) as f32;
        let dy = (end.y - start.y) as f32;
        let angle_deg = Self::wrap_angle_180(dy.atan2(dx).to_degrees());
        let length_px = dx.hypot(dy);

        RawLine {
            start,
            end,
            angle_deg,
            length_px,
        }
    }

    fn scaled_length(value: f64) -> f64 {
        (value * consts::PROCESSING_DOWNSCALE).max(8.0)
    }

    fn scaled_gap(value: f64) -> f64 {
        (value * consts::PROCESSING_DOWNSCALE).max(3.0)
    }

    fn scaled_threshold(value: i32) -> i32 {
        (((value as f64) * consts::PROCESSING_DOWNSCALE).round() as i32).max(8)
    }

    fn detect_lines(
        &mut self,
        edge_image: &Mat,
        threshold: i32,
        min_line_length: f64,
        max_line_gap: f64,
    ) -> Result<Vec<RawLine>> {
        self.lines.clear();
        imgproc::hough_lines_p(
            edge_image,
            &mut self.lines,
            consts::HOUGH_RHO,
            consts::HOUGH_THETA_DEG.to_radians(),
            threshold,
            min_line_length,
            max_line_gap,
        )?;

        Ok(self.lines.iter().map(Self::to_raw_line).collect())
    }

    pub fn process(&mut self, msg: EnhanceMsg) -> Result<DetectMsg> {
        let EnhanceMsg {
            frame,
            gray_contrast,
            edges,
        } = msg;

        let lines = self.detect_lines(
            &edges,
            Self::scaled_threshold(consts::PRIMARY_HOUGH_THRESHOLD),
            Self::scaled_length(consts::PRIMARY_HOUGH_MIN_LINE_LENGTH),
            Self::scaled_gap(consts::PRIMARY_HOUGH_MAX_LINE_GAP),
        )?;
        debug!("[detect] primary pass found {} line candidates", lines.len());

        Ok(DetectMsg {
            frame,
            gray_contrast,
            edges,
            lines,
        })
    }
}

pub fn run_detect(
    rx: Receiver<EnhanceMsg>,
    tx: Sender<DetectMsg>,
    tx_metrics: Sender<MetricsMsg>,
) -> Result<()> {
    let mut stage = DetectStage::new()?;
    for msg in rx {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let out = stage.process(msg)?;
        tx_metrics
            .try_send(MetricsMsg {
                stage: "detect",
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
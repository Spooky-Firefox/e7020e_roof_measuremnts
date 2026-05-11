use anyhow::Result;
use cpu_time::ThreadTime;
use crossbeam_channel::{Receiver, Sender};
use std::time::Instant;

use crate::{
    consts,
    types::{
        AlignmentReport, AngleClusterStats, AxisClass, ClassifiedLine, ClassifiedMsg, DetectMsg,
        MetricsMsg, RawLine,
    },
};

pub struct ClassifyStage;

impl ClassifyStage {
    pub fn new() -> Self {
        Self
    }

    fn signed_axis_delta(angle_deg: f32, target_deg: f32) -> f32 {
        let mut delta = angle_deg - target_deg;
        while delta >= 90.0 {
            delta -= 180.0;
        }
        while delta < -90.0 {
            delta += 180.0;
        }
        delta
    }

    fn classify_line(line: RawLine) -> ClassifiedLine {
        let vertical_error = Self::signed_axis_delta(line.angle_deg, 90.0);
        let horizontal_error = Self::signed_axis_delta(line.angle_deg, 0.0);

        let axis = if vertical_error.abs() <= consts::CLASS_ANGLE_MARGIN_DEG {
            AxisClass::Vertical
        } else if horizontal_error.abs() <= consts::CLASS_ANGLE_MARGIN_DEG {
            AxisClass::Horizontal
        } else {
            AxisClass::Outlier
        };

        let axis_error_deg = match axis {
            AxisClass::Vertical => vertical_error,
            AxisClass::Horizontal => horizontal_error,
            AxisClass::Outlier => {
                if vertical_error.abs() <= horizontal_error.abs() {
                    vertical_error
                } else {
                    horizontal_error
                }
            }
        };

        ClassifiedLine {
            raw: line,
            axis,
            axis_error_deg,
        }
    }

    fn cluster_stats(lines: &[&ClassifiedLine]) -> AngleClusterStats {
        if lines.is_empty() {
            return AngleClusterStats::default();
        }

        let count = lines.len() as u32;
        let mut weighted_sum = 0.0_f32;
        let mut total_weight = 0.0_f32;
        let mut min_deg = f32::INFINITY;
        let mut max_deg = f32::NEG_INFINITY;

        for line in lines {
            let weight = line.raw.length_px.max(1.0);
            weighted_sum += line.axis_error_deg * weight;
            total_weight += weight;
            min_deg = min_deg.min(line.axis_error_deg);
            max_deg = max_deg.max(line.axis_error_deg);
        }

        let mean_deg = if total_weight > 0.0 {
            weighted_sum / total_weight
        } else {
            0.0
        };

        let mut variance = 0.0_f32;
        for line in lines {
            let weight = line.raw.length_px.max(1.0);
            variance += weight * (line.axis_error_deg - mean_deg).powi(2);
        }
        let stddev_deg = if total_weight > 0.0 {
            (variance / total_weight).sqrt()
        } else {
            0.0
        };

        AngleClusterStats {
            count,
            mean_deg,
            min_deg,
            max_deg,
            spread_deg: max_deg - min_deg,
            stddev_deg,
        }
    }

    fn build_report(lines: &[ClassifiedLine]) -> AlignmentReport {
        let vertical = lines
            .iter()
            .filter(|line| matches!(line.axis, AxisClass::Vertical))
            .collect::<Vec<_>>();
        let horizontal = lines
            .iter()
            .filter(|line| matches!(line.axis, AxisClass::Horizontal))
            .collect::<Vec<_>>();
        let outlier_count = lines
            .iter()
            .filter(|line| matches!(line.axis, AxisClass::Outlier))
            .count() as u32;

        let vertical_stats = Self::cluster_stats(&vertical);
        let horizontal_stats = Self::cluster_stats(&horizontal);

        let dominant_axis = if vertical_stats.count >= consts::MIN_CLASSIFIED_LINES as u32 {
            AxisClass::Vertical
        } else if horizontal_stats.count >= consts::MIN_CLASSIFIED_LINES as u32 {
            AxisClass::Horizontal
        } else {
            AxisClass::Outlier
        };

        let angle_from_vertical_deg = match dominant_axis {
            AxisClass::Vertical => vertical_stats.mean_deg,
            AxisClass::Horizontal => horizontal_stats.mean_deg,
            AxisClass::Outlier => 0.0,
        };

        let spread = match dominant_axis {
            AxisClass::Vertical => vertical_stats.stddev_deg,
            AxisClass::Horizontal => horizontal_stats.stddev_deg,
            AxisClass::Outlier => 90.0,
        };
        let inlier_count = match dominant_axis {
            AxisClass::Vertical => vertical_stats.count,
            AxisClass::Horizontal => horizontal_stats.count,
            AxisClass::Outlier => 0,
        } as f32;
        let total_lines = lines.len() as u32;
        let inlier_ratio = if total_lines > 0 {
            inlier_count / total_lines as f32
        } else {
            0.0
        };
        let spread_score =
            (1.0 - (spread / consts::CLASS_ANGLE_MARGIN_DEG).clamp(0.0, 1.0)).max(0.0);
        let confidence = (inlier_ratio * spread_score).clamp(0.0, 1.0);

        AlignmentReport {
            dominant_axis,
            angle_from_vertical_deg,
            confidence,
            vertical: vertical_stats,
            horizontal: horizontal_stats,
            outlier_count,
            total_lines,
        }
    }

    pub fn process(&mut self, msg: DetectMsg) -> ClassifiedMsg {
        let lines = msg
            .lines
            .into_iter()
            .map(Self::classify_line)
            .collect::<Vec<_>>();
        let report = Self::build_report(&lines);

        ClassifiedMsg {
            frame: msg.frame,
            gray_contrast: msg.gray_contrast,
            edges: msg.edges,
            lines,
            report,
        }
    }
}

pub fn run_classify(
    rx: Receiver<DetectMsg>,
    tx: Sender<ClassifiedMsg>,
    tx_metrics: Sender<MetricsMsg>,
) -> Result<()> {
    let mut stage = ClassifyStage::new();
    for msg in rx {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let out = stage.process(msg);

        let min_angle_deg = out
            .lines
            .iter()
            .map(|line| line.raw.angle_deg)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0.0);
        let max_angle_deg = out
            .lines
            .iter()
            .map(|line| line.raw.angle_deg)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0.0);

        tx_metrics
            .try_send(MetricsMsg {
                stage: "classify",
                real_us: t_real.elapsed().as_micros() as u64,
                cpu_us: t_cpu.elapsed().as_micros() as u64,
                lines: Some(crate::types::LineMetrics {
                    total_lines: out.report.total_lines,
                    vertical_lines: out.report.vertical.count,
                    horizontal_lines: out.report.horizontal.count,
                    outlier_lines: out.report.outlier_count,
                    angle_from_vertical_deg: out.report.angle_from_vertical_deg,
                    confidence: out.report.confidence,
                    vertical_spread_deg: out.report.vertical.stddev_deg,
                    horizontal_spread_deg: out.report.horizontal.stddev_deg,
                    min_angle_deg,
                    max_angle_deg,
                }),
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

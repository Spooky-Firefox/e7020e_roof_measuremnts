use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use cpu_time::ThreadTime;
use log::{debug, info};
use std::time::Instant;

use crate::{
    consts,
    serial::{encode_alignment_csv, SerialOutput},
    types::{AlignmentMsg, AxisClass, ClassifiedMsg, MetricsMsg},
};

pub struct DecideStage {
    serial: SerialOutput,
}

impl DecideStage {
    pub fn new() -> Self {
        Self {
            serial: SerialOutput::from_env(),
        }
    }

    pub fn process(&mut self, msg: ClassifiedMsg) -> AlignmentMsg {
        let serial_frame = encode_alignment_csv(&msg.report);
        if msg.report.confidence >= consts::MIN_ALIGNMENT_CONFIDENCE {
            self.serial.send(&serial_frame);
        }

        let axis = match msg.report.dominant_axis {
            AxisClass::Outlier => "none",
            other => other.as_str(),
        };
        info!(
            "[align] axis={} angle={:.2}deg confidence={:.3} total={} vertical={} horizontal={} outliers={} v_range=[{:.2},{:.2}] h_range=[{:.2},{:.2}] v_spread={:.2} h_spread={:.2} v_stddev={:.2} h_stddev={:.2}",
            axis,
            msg.report.angle_from_vertical_deg,
            msg.report.confidence,
            msg.report.total_lines,
            msg.report.vertical.count,
            msg.report.horizontal.count,
            msg.report.outlier_count,
            msg.report.vertical.min_deg,
            msg.report.vertical.max_deg,
            msg.report.horizontal.min_deg,
            msg.report.horizontal.max_deg,
            msg.report.vertical.spread_deg,
            msg.report.horizontal.spread_deg,
            msg.report.vertical.stddev_deg,
            msg.report.horizontal.stddev_deg,
        );
        debug!("[align] serial={}", serial_frame.trim_end());

        AlignmentMsg {
            frame: msg.frame,
            gray_contrast: msg.gray_contrast,
            edges: msg.edges,
            lines: msg.lines,
            report: msg.report,
            serial_frame,
        }
    }
}

pub fn run_decide(
    rx: Receiver<ClassifiedMsg>,
    tx: Sender<AlignmentMsg>,
    tx_metrics: Sender<MetricsMsg>,
) -> Result<()> {
    let mut stage = DecideStage::new();
    for msg in rx {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let out = stage.process(msg);

        tx_metrics
            .try_send(MetricsMsg {
                stage: "decide",
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
                    min_angle_deg: out.report.vertical.min_deg.min(out.report.horizontal.min_deg),
                    max_angle_deg: out.report.vertical.max_deg.max(out.report.horizontal.max_deg),
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
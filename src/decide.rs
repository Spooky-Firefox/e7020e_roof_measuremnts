use anyhow::Result;
use cpu_time::ThreadTime;
use crossbeam_channel::{Receiver, Sender};
use log::{debug, info, warn};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{
    consts,
    serial::send_alignment,
    shared_serial::SharedSerialPort,
    types::{AlignmentMsg, ClassifiedMsg, MetricsMsg},
};

pub struct DecideStage {
    shared_port: SharedSerialPort,
}

impl DecideStage {
    pub fn new(shared_port: SharedSerialPort) -> Self {
        Self { shared_port }
    }

    pub fn process(&mut self, msg: ClassifiedMsg) -> AlignmentMsg {
        send_alignment(&self.shared_port, &msg.report);

        let axis = msg.report.dominant_axis.as_str();
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
        debug!(
            "[align] command=align {:.6} {:.4}",
                msg.report.angle_from_vertical_deg, msg.report.confidence,
        );

        AlignmentMsg {
            frame: msg.frame,
            gray_contrast: msg.gray_contrast,
            edges: msg.edges,
            lines: msg.lines,
            report: msg.report,
            serial_frame: String::new(),
        }
    }
}

struct CameraCsvLogger {
    writer: BufWriter<File>,
}

impl CameraCsvLogger {
    fn new() -> Result<Self> {
        fs::create_dir_all(consts::CONTROLLER_LOG_DIR)?;
        let file = File::create(new_camera_log_path())?;
        let mut writer = BufWriter::new(file);
        writeln!(
            writer,
            "timestamp_us,axis,angle_from_vertical_deg,confidence,total_lines,vertical_lines,horizontal_lines,outlier_lines,vertical_mean_deg,horizontal_mean_deg,vertical_stddev_deg,horizontal_stddev_deg"
        )?;
        writer.flush()?;
        Ok(Self { writer })
    }

    fn write_report(&mut self, report: &crate::types::AlignmentReport) -> Result<()> {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        writeln!(
            self.writer,
            "{},{},{:.6},{:.4},{},{},{},{},{:.6},{:.6},{:.6},{:.6}",
            now_us,
            report.dominant_axis.as_str(),
            report.angle_from_vertical_deg,
            report.confidence,
            report.total_lines,
            report.vertical.count,
            report.horizontal.count,
            report.outlier_count,
            report.vertical.mean_deg,
            report.horizontal.mean_deg,
            report.vertical.stddev_deg,
            report.horizontal.stddev_deg,
        )?;
        self.writer.flush()?;
        Ok(())
    }
}

fn new_camera_log_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    PathBuf::from(consts::CONTROLLER_LOG_DIR).join(format!("camera_alignment_{millis}.csv"))
}

pub fn run_decide(
    rx: Receiver<ClassifiedMsg>,
    tx: Sender<AlignmentMsg>,
    tx_metrics: Sender<MetricsMsg>,
    shared_port: SharedSerialPort,
) -> Result<()> {
    let mut stage = DecideStage::new(shared_port);
    let mut camera_csv = match CameraCsvLogger::new() {
        Ok(logger) => Some(logger),
        Err(error) => {
            warn!("[camera-csv] failed to initialize log file: {error:#}");
            None
        }
    };

    for msg in rx {
        let t_real = Instant::now();
        let t_cpu = ThreadTime::now();
        let out = stage.process(msg);

        if let Some(logger) = camera_csv.as_mut() {
            if let Err(error) = logger.write_report(&out.report) {
                warn!("[camera-csv] failed to write row: {error}");
            }
        }

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
                    min_angle_deg: out
                        .report
                        .vertical
                        .min_deg
                        .min(out.report.horizontal.min_deg),
                    max_angle_deg: out
                        .report
                        .vertical
                        .max_deg
                        .max(out.report.horizontal.max_deg),
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

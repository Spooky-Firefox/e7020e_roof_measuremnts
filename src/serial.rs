use anyhow::{Context, Result};
use log::{debug, warn};
use serialport::SerialPort;
use std::{io::Write, time::Duration};

use crate::{consts, types::AlignmentReport};

pub struct SerialOutput {
    port_name: Option<String>,
    port: Option<Box<dyn SerialPort>>,
}

impl SerialOutput {
    pub fn from_env() -> Self {
        let port_name = std::env::var("ROOF_SERIAL_PORT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        Self {
            port_name,
            port: None,
        }
    }

    fn ensure_open(&mut self) -> Result<()> {
        if self.port.is_some() || self.port_name.is_none() {
            return Ok(());
        }

        let port_name = self.port_name.clone().unwrap_or_default();
        let port = serialport::new(&port_name, consts::SERIAL_BAUD_RATE)
            .timeout(Duration::from_millis(consts::SERIAL_WRITE_TIMEOUT_MS))
            .open()
            .with_context(|| format!("failed to open serial port {port_name}"))?;
        self.port = Some(port);
        Ok(())
    }

    pub fn send(&mut self, payload: &str) {
        if let Err(error) = self.ensure_open() {
            warn!("[serial] {error}");
            return;
        }

        if let Some(port) = self.port.as_mut() {
            if let Err(error) = port.write_all(payload.as_bytes()) {
                warn!("[serial] write failed: {error}");
                self.port = None;
                return;
            }
            if let Err(error) = port.flush() {
                warn!("[serial] flush failed: {error}");
                self.port = None;
            }
        } else {
            debug!("[serial] disabled payload={payload}");
        }
    }
}

pub fn encode_alignment_csv(report: &AlignmentReport) -> String {
    let cross_check = report.horizontal_cross_check_deg.unwrap_or(0.0);
    format!(
        "ALIGN,{:.2},{:.3},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
        report.angle_from_vertical_deg,
        report.confidence,
        report.total_lines,
        report.vertical.count,
        report.horizontal.count,
        report.vertical.min_deg,
        report.vertical.max_deg,
        report.vertical.stddev_deg,
        report.horizontal.stddev_deg,
        cross_check,
    )
}
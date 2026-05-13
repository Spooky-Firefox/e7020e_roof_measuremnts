use log::debug;

use crate::controller_commands;
use crate::shared_serial::SharedSerialPort;
use crate::types::AlignmentReport;

/// Format alignment data as a command string for the RP2350.
///
/// Format: `align <angle_deg> <confidence>\n`
/// Angle is in degrees, confidence in [0, 1].
pub fn encode_alignment_command(report: &AlignmentReport) -> String {
    controller_commands::format_alignment_command(
        report.angle_from_vertical_deg,
        report.confidence,
    )
}

/// Send alignment data to the shared serial port if one is open.
pub fn send_alignment(shared_port: &SharedSerialPort, report: &AlignmentReport) {
    if !shared_port.is_open() {
        debug!("[align] no port open, skipping");
        return;
    }

    let command = encode_alignment_command(report);
    shared_port.write_str(&command);
}

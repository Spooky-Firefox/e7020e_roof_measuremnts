use crate::types::ControllerMode;

pub const PWM_STEERING_PREFIX: &str = "pwm-a ";
pub const PWM_THROTTLE_PREFIX: &str = "pwm-b ";
pub const SPEED_PREFIX: &str = "speed ";
pub const CONST_PREFIX: &str = "const ";
pub const ALIGN_PREFIX: &str = "align ";
pub const MODE_PREFIX: &str = "mode ";

pub const MODE_MANUAL: &str = "manual";
pub const MODE_AUTO: &str = "auto";
pub const MODE_TARGET_STEERING: &str = "steering";
pub const MODE_TARGET_THROTTLE: &str = "throttle";
pub const MODE_TARGET_BOTH: &str = "both";

pub const KNOWN_ECHO_PREFIXES: [&str; 5] = [
    PWM_STEERING_PREFIX,
    PWM_THROTTLE_PREFIX,
    SPEED_PREFIX,
    CONST_PREFIX,
    MODE_PREFIX,
];

pub fn strip_value<'a>(command: &'a str, prefix: &str) -> Option<&'a str> {
    command.strip_prefix(prefix).map(str::trim)
}

pub fn is_known_command_echo(line: &str) -> bool {
    let line = line.trim();
    KNOWN_ECHO_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

pub fn mode_keyword(mode: ControllerMode) -> &'static str {
    match mode {
        ControllerMode::Manual => MODE_MANUAL,
        ControllerMode::Auto => MODE_AUTO,
    }
}

pub fn format_mode_command(target: Option<&str>, mode: ControllerMode) -> String {
    match target {
        Some(target) if !target.is_empty() && target != MODE_TARGET_BOTH => {
            format!("{MODE_PREFIX}{target} {}", mode_keyword(mode))
        }
        _ => format!("{MODE_PREFIX}{}", mode_keyword(mode)),
    }
}

pub fn format_alignment_command(angle_deg: f32, confidence: f32) -> String {
    format!("{ALIGN_PREFIX}{angle_deg:.6} {confidence:.4}\n")
}
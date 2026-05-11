//! Shared serial port manager for both controller and alignment services.
//!
//! Both the controller service and alignment writer can send data to the same port
//! through a mutex-protected handle. The controller service opens/closes the port
//! based on UI requests, and the alignment writer checks if a port is available.

use std::io::Write;
use std::sync::{Arc, Mutex};

use log::warn;
use serialport::SerialPort;

/// Shared serial port wrapped in Arc<Mutex<>> for safe multi-threaded access.
#[derive(Clone)]
pub struct SharedSerialPort {
    inner: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
}

impl SharedSerialPort {
    /// Create a new empty shared port handle (no port opened yet).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the port to a newly opened handle. Closes any existing port first.
    pub fn set_port(&self, port: Box<dyn SerialPort>) {
        // Acquire the lock; old port is dropped when this scope exits
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some(port);
        }
    }

    /// Close the current port.
    pub fn clear_port(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }

    /// Send a string over the port if one is currently open.
    /// Logs a warning and closes the port on write/flush errors.
    pub fn write_str(&self, payload: &str) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => {
                warn!("[shared_serial] poisoned mutex");
                return;
            }
        };

        let Some(port) = guard.as_mut() else {
            return;
        };

        if let Err(error) = port.write_all(payload.as_bytes()) {
            warn!("[shared_serial] write failed: {error}");
            *guard = None;
            return;
        }

        if let Err(error) = port.flush() {
            warn!("[shared_serial] flush failed: {error}");
            *guard = None;
        }
    }

    /// Check if a port is currently open.
    pub fn is_open(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

impl Default for SharedSerialPort {
    fn default() -> Self {
        Self::new()
    }
}

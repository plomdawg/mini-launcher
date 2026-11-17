//! Real transport: a PN532 in HSU mode behind a USB-UART adapter.

use std::time::Duration;

use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};

use super::driver::Pn532;

/// PN532 HSU default baud rate.
pub const BAUD_RATE: u32 = 115_200;
/// Per-read port timeout. Short, so the driver's own per-command deadline
/// (see [`Pn532::set_timeout`]) decides when to give up.
pub const READ_TIMEOUT: Duration = Duration::from_millis(50);

/// Opens `path` (e.g. `COM7` or `/dev/ttyUSB0`) at 115200 8N1 with no flow
/// control and wraps it in a [`Pn532`] driver.
pub fn open(path: &str) -> serialport::Result<Pn532<Box<dyn SerialPort>>> {
    let port = serialport::new(path, BAUD_RATE)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(READ_TIMEOUT)
        .open()?;
    Ok(Pn532::new(port))
}

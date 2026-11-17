//! PN532 support over the HSU (high-speed UART) interface.
//!
//! [`frame`] is the pure byte-level codec; it performs no I/O. [`driver`] is
//! the command layer over any `Read + Write` stream, and [`serial`] opens a
//! real serial port for it.

pub mod driver;
pub mod frame;
pub mod serial;

pub use driver::{FirmwareVersion, Pn532, Pn532Error, Target};

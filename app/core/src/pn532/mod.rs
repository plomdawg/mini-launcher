//! PN532 support over the HSU (high-speed UART) interface.
//!
//! [`frame`] is the pure byte-level codec; it performs no I/O. [`driver`] is
//! the command layer over any [`Transport`] (`Read + Write`), [`serial`] opens a
//! real serial port for it, and [`reader`] polls it as a [`crate::reader::Reader`].

pub mod driver;
pub mod frame;
pub mod reader;
pub mod serial;
pub mod write;

pub use driver::{FirmwareVersion, Pn532, Pn532Error, Target, Transport};
pub use reader::Pn532Hsu;

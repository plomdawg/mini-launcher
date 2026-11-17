//! Mini Launcher's settings window: the retro theme and its screens.
//!
//! The screens are plain egui so they build and test anywhere; the native
//! window (eframe + wgpu) is behind the `window` feature.

pub mod mock;
pub mod theme;
#[cfg(feature = "window")]
pub mod window;

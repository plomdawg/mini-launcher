//! Mini Launcher's settings window: the retro theme and its screens.
//!
//! The screens are plain egui so they build and test anywhere; the native
//! window (eframe + wgpu) is behind the `window` feature.

pub mod library;
pub mod settings;
pub mod theme;
#[cfg(feature = "window")]
pub mod window;
pub mod wizard;
pub mod wizard_view;

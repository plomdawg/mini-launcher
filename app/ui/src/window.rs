//! The native settings window (eframe, wgpu backend).

use crate::mock::{self, Screen, Settings};
use crate::theme;

const ICON: &[u8] = include_bytes!("../../assets/icon/mini-launcher.png");

/// Opens the settings window and blocks until it is closed.
///
/// eframe keeps the winit event loop between calls (`run_and_return`), but
/// the window, its wgpu surface and device belong to the app and are dropped
/// when this returns, so the caller goes back to its idle footprint.
pub fn open() -> Result<(), eframe::Error> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Mini Launcher")
        .with_inner_size([640.0, 440.0])
        .with_min_inner_size([560.0, 400.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        run_and_return: true,
        ..Default::default()
    };
    eframe::run_native(
        "mini-launcher-settings",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App(Settings::new(Screen::Library))))
        }),
    )
}

struct App(Settings);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        mock::show(ui, &mut self.0);
    }
}

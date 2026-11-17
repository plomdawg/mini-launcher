//! The native settings window (eframe, wgpu backend) on its own thread.
//!
//! The tray keeps its own Win32 message loop on the main thread; the window
//! runs a winit event loop on a dedicated thread, so neither blocks the
//! other (Quit, Pause and tag launches keep working while the window is
//! open). winit allows only one event loop per process, so the thread lives
//! as long as the [`SettingsWindow`] and reuses eframe's kept loop
//! (`run_and_return`) for every opening. The window, its wgpu surface and
//! device belong to one opening and are dropped when it closes, so the
//! process goes back to its idle footprint.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::settings::backup::Request;
use crate::settings::{self, Host, Settings};
use crate::theme;

const ICON: &[u8] = include_bytes!("../../assets/icon/mini-launcher.png");

/// What the window thread reports back.
#[derive(Debug)]
pub enum Event {
    Opened,
    /// The window closed; `Err` if it could not run.
    Closed(Result<(), String>),
}

/// A handle to the settings window thread. Dropping it closes the window
/// (if open) and ends the thread.
///
/// Running the event loop off the main thread is only enabled on Windows
/// (`with_any_thread`); on other platforms winit would refuse it, so a
/// Linux port must add the X11/Wayland equivalents first.
pub struct SettingsWindow {
    requests: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    /// Set from an open request until the window has closed again.
    open: Arc<AtomicBool>,
    /// The open window's context, to focus or close it from another thread.
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl SettingsWindow {
    /// Starts the (idle) window thread; `events` receives its [`Event`]s.
    pub fn spawn(host: Arc<dyn Host>, events: mpsc::Sender<Event>) -> std::io::Result<Self> {
        let (requests, rx) = mpsc::channel::<()>();
        let open = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(Mutex::new(None));
        let thread = thread::Builder::new()
            .name("settings-window".into())
            .spawn({
                let (open, ctx) = (open.clone(), ctx.clone());
                move || {
                    while rx.recv().is_ok() {
                        let _ = events.send(Event::Opened);
                        let result = run(&ctx, &host).map_err(|e| e.to_string());
                        *lock(&ctx) = None;
                        open.store(false, Ordering::SeqCst);
                        let _ = events.send(Event::Closed(result));
                    }
                }
            })?;
        Ok(SettingsWindow {
            requests: Some(requests),
            thread: Some(thread),
            open,
            ctx,
        })
    }

    /// Opens the window, or brings the open one to the front.
    pub fn show(&self) {
        if self.open.swap(true, Ordering::SeqCst) {
            if let Some(ctx) = lock(&self.ctx).as_ref() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        } else if let Some(requests) = &self.requests {
            let _ = requests.send(());
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// Redraws the open window (e.g. after the reader state changed).
    pub fn repaint(&self) {
        if let Some(ctx) = lock(&self.ctx).as_ref() {
            ctx.request_repaint();
        }
    }

    /// Asks the open window (if any) to close; returns at once.
    pub fn close(&self) {
        if let Some(ctx) = lock(&self.ctx).as_ref() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Closes the window and waits up to `timeout` for the thread to end.
    /// Returns whether it did.
    pub fn shut_down(mut self, timeout: Duration) -> bool {
        self.stop(timeout)
    }

    fn stop(&mut self, timeout: Duration) -> bool {
        self.requests = None;
        let Some(thread) = self.thread.take() else {
            return true;
        };
        let start = Instant::now();
        // An open request may still be on its way to becoming a window.
        while !thread.is_finished() {
            self.close();
            if start.elapsed() > timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = thread.join();
        true
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        self.stop(Duration::from_secs(3));
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs one opening of the window on this thread until it is closed.
fn run(
    ctx_slot: &Arc<Mutex<Option<egui::Context>>>,
    host: &Arc<dyn Host>,
) -> Result<(), eframe::Error> {
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
        // Only used when the thread's event loop is first created.
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(windows)]
            {
                use winit::platform::windows::EventLoopBuilderExtWindows;
                builder.with_any_thread(true);
            }
            #[cfg(not(windows))]
            let _ = builder;
        })),
        ..Default::default()
    };
    let (ctx_slot, host) = (ctx_slot.clone(), host.clone());
    eframe::run_native(
        "mini-launcher-settings",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            *lock(&ctx_slot) = Some(cc.egui_ctx.clone());
            Ok(Box::new(App(Settings::new(host))))
        }),
    )
}

struct App(Settings);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        settings::show(ui, &mut self.0);
        if let Some((pick, start)) = self.0.take_browse() {
            if let Some(path) = browse(frame, pick, start) {
                self.0.picked(pick, path);
            }
        }
        if let Some(request) = self.0.take_request() {
            if let Some(path) = file_dialog(frame, &request) {
                match request {
                    Request::Export(_) => self.0.export_to(path),
                    Request::Import => self.0.import_from(path),
                    Request::Folder => self.0.move_to(path),
                }
            }
        }
    }
}

/// The native dialog for an export / import / folder [`Request`].
#[cfg(windows)]
fn file_dialog(frame: &eframe::Frame, request: &Request) -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new().set_parent(frame);
    let toml = |d: rfd::FileDialog| {
        d.add_filter("Mini Launcher config", &["toml"])
            .add_filter("All files", &["*"])
    };
    match request {
        Request::Export(name) => {
            toml(dialog.set_title("Export the library").set_file_name(name)).save_file()
        }
        Request::Import => toml(dialog.set_title("Import a library")).pick_file(),
        Request::Folder => dialog.set_title("Choose the config folder").pick_folder(),
    }
}

#[cfg(not(windows))]
fn file_dialog(_: &eframe::Frame, _: &Request) -> Option<std::path::PathBuf> {
    None
}

/// The native file dialog, modal to the settings window.
#[cfg(windows)]
fn browse(
    frame: &eframe::Frame,
    pick: settings::Pick,
    start: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_parent(frame);
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    match pick {
        settings::Pick::Exe => dialog
            .set_title("Choose the program")
            .add_filter("Programs", &["exe", "bat", "cmd"])
            .add_filter("All files", &["*"])
            .pick_file(),
        settings::Pick::Cwd => dialog.set_title("Choose the working folder").pick_folder(),
    }
}

#[cfg(not(windows))]
fn browse(
    _: &eframe::Frame,
    _: settings::Pick,
    _: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    None
}

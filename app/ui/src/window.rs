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
//!
//! Closing: eframe drops the window on `CloseRequested` and only leaves the
//! loop on a later window event (`Destroyed`). winit queues events that
//! arrive while one is being handled and delivers them on the next wake-up,
//! so with `ControlFlow::Wait` and nothing else coming, the loop could sleep
//! forever on a destroyed window (#88). [`App::on_exit`] therefore starts a
//! [`Nudge`] that wakes the thread until the loop has returned, and the
//! thread logs how many wake-ups that took when it needed any (#90).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
    nudge: Nudge,
}

/// Wakes the window thread's event loop while a closed window's loop has
/// not returned yet.
#[derive(Clone, Default)]
struct Nudge {
    /// Set by `on_exit` (the window is gone) until the loop has returned.
    closing: Arc<AtomicBool>,
    /// Bumped by every `start`, so a helper left over from an earlier
    /// close stops instead of running alongside the new one.
    generation: Arc<AtomicU64>,
    /// Wake-ups posted since the last `start`.
    wakes: Arc<AtomicU32>,
    /// The window thread's id (Win32), 0 until it started.
    #[cfg_attr(not(windows), allow(dead_code))]
    thread_id: Arc<AtomicU32>,
}

impl Nudge {
    const EVERY: Duration = Duration::from_millis(50);
    const GIVE_UP: Duration = Duration::from_secs(10);

    /// Wakes the loop until it returns (or for [`Self::GIVE_UP`]).
    fn start(&self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.wakes.store(0, Ordering::SeqCst);
        self.closing.store(true, Ordering::SeqCst);
        let nudge = self.clone();
        let spawned = thread::Builder::new()
            .name("settings-window-nudge".into())
            .spawn(move || {
                let start = Instant::now();
                loop {
                    thread::sleep(Self::EVERY);
                    // Checked after the sleep, so no wake-up is posted once
                    // the loop has returned.
                    if !nudge.closing.load(Ordering::SeqCst)
                        || nudge.generation.load(Ordering::SeqCst) != generation
                        || start.elapsed() >= Self::GIVE_UP
                    {
                        break;
                    }
                    nudge.wake();
                }
            });
        if spawned.is_err() {
            self.wake();
        }
    }

    /// The loop has returned: stops the helper and logs the wake-ups it
    /// took, if any (the close would otherwise have hung, #88).
    fn finish(&self) {
        self.closing.store(false, Ordering::SeqCst);
        let wakes = self.wakes.swap(0, Ordering::SeqCst);
        if wakes > 0 {
            tracing::info!("settings window close needed {wakes} wake-ups");
        }
    }

    /// Posts an empty thread message: winit wakes up and delivers the
    /// events it queued (and the dispatcher ignores the message itself).
    fn wake(&self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_NULL};
            let id = self.thread_id.load(Ordering::SeqCst);
            if id != 0 {
                // SAFETY: plain message post; fails harmlessly if the thread is gone.
                if unsafe { PostThreadMessageW(id, WM_NULL, 0, 0) } != 0 {
                    self.wakes.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }
}

impl SettingsWindow {
    /// Starts the (idle) window thread; `events` receives its [`Event`]s.
    pub fn spawn(host: Arc<dyn Host>, events: mpsc::Sender<Event>) -> std::io::Result<Self> {
        let (requests, rx) = mpsc::channel::<()>();
        let open = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(Mutex::new(None));
        let nudge = Nudge::default();
        let thread = thread::Builder::new()
            .name("settings-window".into())
            .spawn({
                let (open, ctx, nudge) = (open.clone(), ctx.clone(), nudge.clone());
                move || {
                    #[cfg(windows)]
                    nudge.thread_id.store(
                        // SAFETY: no preconditions.
                        unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
                        Ordering::SeqCst,
                    );
                    while rx.recv().is_ok() {
                        let _ = events.send(Event::Opened);
                        let result = run(&ctx, &host, &nudge).map_err(|e| e.to_string());
                        *lock(&ctx) = None;
                        nudge.finish();
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
            nudge,
        })
    }

    /// Opens the window, or brings the open one to the front.
    pub fn show(&self) {
        if self.open.swap(true, Ordering::SeqCst) {
            // Still closing: make sure the loop gets to finish.
            if self.nudge.closing.load(Ordering::SeqCst) {
                self.nudge.wake();
            }
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
    nudge: &Nudge,
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
    let (ctx_slot, host, nudge) = (ctx_slot.clone(), host.clone(), nudge.clone());
    eframe::run_native(
        "mini-launcher-settings",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            *lock(&ctx_slot) = Some(cc.egui_ctx.clone());
            Ok(Box::new(App(Settings::new(host), nudge)))
        }),
    )
}

struct App(Settings, Nudge);

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

    /// The window is being dropped: see the module docs.
    fn on_exit(&mut self) {
        self.1.start();
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
        settings::Pick::Exe | settings::Pick::EmulatorExe => dialog
            .set_title("Choose the program")
            .add_filter("Programs", &["exe", "bat", "cmd"])
            .add_filter("All files", &["*"])
            .pick_file(),
        settings::Pick::Cwd => dialog.set_title("Choose the working folder").pick_folder(),
        settings::Pick::Rom => dialog.set_title("Choose the game file").pick_file(),
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

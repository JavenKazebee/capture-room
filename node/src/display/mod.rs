//! The node's screens, for display outputs: a channel's program fullscreen
//! on an attached screen (monitor, projector, HDMI→SDI converter).
//!
//! Windows must live on the main thread on macOS (AppKit), so `main` runs a
//! winit loop there and the node's runtime on another thread (see [`run`]);
//! everything else talks to it through [`Displays`]. Without a window system,
//! [`Displays::none`] lists nothing.
//!
//! Output windows are borderless, on top, cursor hidden. If the screen offers
//! the channel's exact format (or double the rate for 29.97), the window goes
//! exclusive in that mode, which sets the screen to it (what an HDMI→SDI
//! converter needs); otherwise it fills the screen and the video is scaled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use anyhow::{anyhow, Result};
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::monitor::{MonitorHandle, VideoModeHandle};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(not(target_os = "macos"))]
use winit::window::{Fullscreen, WindowLevel};
use winit::window::{Window, WindowId};

use crate::api::types::{DisplayDto, DisplayModeDto};
use crate::power;

#[cfg(target_os = "macos")]
mod mac;

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// A program format a window should show, to pick the screen mode by.
#[derive(Debug, Clone, Copy)]
pub struct WantedFormat {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

pub struct DisplayWindow {
    proxy: EventLoopProxy<Command>,
    id: WindowId,
    /// The native handle a video sink draws into (`NSView`, `HWND`, or X11 window id).
    pub handle: usize,
    /// The screen's mode as shown on the channel, noted when it isn't the channel's format.
    pub mode: String,
    /// Whether its screen is plugged in (the window hides while it isn't).
    connected: Arc<AtomicBool>,
}

impl DisplayWindow {
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Show or hide the window (it stays open, in its mode).
    pub fn set_visible(&self, visible: bool) {
        let _ = self.proxy.send_event(Command::Show(self.id, visible));
    }
}

impl Drop for DisplayWindow {
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Command::Close(self.id));
    }
}

/// An opened window: id, native handle, mode description, screen connected.
type Opened = (WindowId, usize, String, Arc<AtomicBool>);

enum Command {
    List(mpsc::SyncSender<Vec<DisplayDto>>),
    Open {
        display: String,
        format: WantedFormat,
        visible: bool,
        reply: mpsc::SyncSender<Result<Opened, String>>,
    },
    Show(WindowId, bool),
    Close(WindowId),
    /// The node's status, for the menu bar icon (none on Linux yet).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Tray(crate::tray::Status),
    /// A menu bar item was picked.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Menu(tray_icon::menu::MenuId),
    Quit,
}

#[derive(Clone)]
pub struct Displays {
    proxy: Option<EventLoopProxy<Command>>,
}

impl Displays {
    /// No window system: nothing to show on.
    pub fn none() -> Self {
        Self { proxy: None }
    }

    /// Show the node's status on its menu bar icon, if it has one.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn show_status(&self, status: crate::tray::Status) {
        if let Some(proxy) = &self.proxy {
            let _ = proxy.send_event(Command::Tray(status));
        }
    }

    pub fn available(&self) -> bool {
        self.proxy.is_some()
    }

    pub fn list(&self) -> Vec<DisplayDto> {
        let Some(proxy) = &self.proxy else {
            return Vec::new();
        };
        // A sleeping screen isn't listed.
        power::wake_screens();
        let (tx, rx) = mpsc::sync_channel(1);
        if proxy.send_event(Command::List(tx)).is_err() {
            return Vec::new();
        }
        rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_default()
    }

    /// Open an output window on `display` (a [`DisplayDto::id`]) in the mode
    /// that best fits `format`; shown at once if `visible`.
    pub fn open(
        &self,
        display: &str,
        format: WantedFormat,
        visible: bool,
    ) -> Result<DisplayWindow> {
        let proxy = self
            .proxy
            .clone()
            .ok_or_else(|| anyhow!("this node has no window system to show a display on"))?;
        power::wake_screens();
        let (tx, rx) = mpsc::sync_channel(1);
        proxy
            .send_event(Command::Open {
                display: display.to_string(),
                format,
                visible,
                reply: tx,
            })
            .map_err(|_| anyhow!("the window loop has stopped"))?;
        let (id, handle, mode, connected) = rx
            .recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| anyhow!("the window loop didn't answer"))?
            .map_err(|e| anyhow!(e))?;
        Ok(DisplayWindow {
            proxy,
            id,
            handle,
            mode,
            connected,
        })
    }
}

/// Run `node` with a window loop on this (main) thread when there's a window
/// system; otherwise just run it here.
pub fn run<T: Send + 'static>(
    tray: Option<crate::tray::Config>,
    node: impl FnOnce(Displays) -> T + Send + 'static,
) -> T {
    let event_loop = match build_event_loop() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            info!(reason = %e, "no window system: display outputs unavailable");
            return node(Displays::none());
        }
    };
    let proxy = event_loop.create_proxy();
    let displays = Displays {
        proxy: Some(proxy.clone()),
    };
    let node = std::thread::Builder::new()
        .name("node".into())
        .spawn(move || {
            let result = node(displays);
            let _ = proxy.send_event(Command::Quit);
            result
        })
        .expect("start the node thread");
    let mut window_loop = WindowLoop {
        tray_config: tray,
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        proxy: Some(event_loop.create_proxy()),
        ..WindowLoop::default()
    };
    if let Err(e) = event_loop.run_app(&mut window_loop) {
        warn!(error = %e, "window loop failed");
    }
    match node.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn build_event_loop() -> Result<EventLoop<Command>, winit::error::EventLoopError> {
    let mut builder = EventLoop::<Command>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // No Dock icon or menu bar: the node is a background service.
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        // X11 even under Wayland (via XWayland): glimagesink renders into a
        // wl_surface only through the wl_display that owns it, shared as a GL
        // display context; given a bare surface it opens its own connection.
        builder.with_x11();
    }
    builder.build()
}

/// An open output window, and the screen mode to put back when it closes.
struct OutputWindow {
    // Dropped first: the window closes before the screen's mode changes back.
    window: Window,
    handle: usize,
    /// Whether it should show (some outputs hide while their channel is idle).
    visible: bool,
    /// Whether its screen is plugged in, shared with its [`DisplayWindow`].
    connected: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    display: u32,
    /// The screen's mode (pixels, refresh in mHz), to set again when replugged.
    #[cfg(target_os = "macos")]
    wanted_mode: Option<(u32, u32, u32)>,
    #[cfg(target_os = "macos")]
    mode: Option<mac::ModeRestore>,
}

#[derive(Default)]
struct WindowLoop {
    windows: HashMap<WindowId, OutputWindow>,
    /// Keeps the screens from sleeping while any output window is open.
    awake: Option<power::DisplayAwake>,
    /// The menu bar icon to show once the loop runs, if any.
    tray_config: Option<crate::tray::Config>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray: Option<crate::tray::Tray>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    proxy: Option<EventLoopProxy<Command>>,
}

impl ApplicationHandler<Command> for WindowLoop {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    // The menu bar icon can only be made once the application has started.
    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if !matches!(cause, winit::event::StartCause::Init) {
            return;
        }
        let Some(config) = self.tray_config.take() else {
            return;
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let proxy = std::sync::Mutex::new(self.proxy.clone());
            let on_menu = move |id| {
                if let Some(proxy) = proxy.lock().unwrap().as_ref() {
                    let _ = proxy.send_event(Command::Menu(id));
                }
            };
            match crate::tray::Tray::new(config, on_menu) {
                Ok(tray) => {
                    info!("menu bar icon shown");
                    self.tray = Some(tray);
                }
                Err(e) => warn!(error = %e, "couldn't show the menu bar icon"),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        match config {}
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::List(reply) => {
                let _ = reply.send(list(event_loop));
            }
            Command::Open {
                display,
                format,
                visible,
                reply,
            } => {
                let result = self.open(event_loop, &display, format, visible);
                let _ = reply.send(result);
            }
            Command::Show(id, visible) => {
                if let Some(output) = self.windows.get_mut(&id) {
                    output.visible = visible;
                    if output.connected.load(Ordering::Relaxed) {
                        show(output, visible);
                    }
                }
            }
            Command::Close(id) => {
                if let Some(output) = self.windows.remove(&id) {
                    output.window.set_fullscreen(None);
                    drop(output);
                    info!(window = ?id, "display window closed");
                }
                if self.windows.is_empty() {
                    self.awake = None;
                }
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Command::Tray(status) => {
                if let Some(tray) = &mut self.tray {
                    tray.show(status);
                }
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Command::Menu(id) => {
                if let Some(tray) = &mut self.tray {
                    tray.picked(&id);
                }
            }
            Command::Quit => event_loop.exit(),
        }
    }

    // Output windows close when their output stops, not when asked. An
    // unplugged screen's windows get moved by the OS onto another screen, so
    // check at once rather than cover the operator's screen until the next tick.
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::Moved(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
        ) {
            self.check_screens();
        }
    }

    // While windows are open, check their screens twice a second too.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        use winit::event_loop::ControlFlow;
        if self.windows.is_empty() {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            self.check_screens();
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + SCREEN_CHECK,
            ));
        }
    }
}

const SCREEN_CHECK: Duration = Duration::from_millis(500);

impl WindowLoop {
    /// Hide windows whose screen was unplugged (so they never land on
    /// another screen) and bring them back when it returns.
    #[cfg(target_os = "macos")]
    fn check_screens(&mut self) {
        for output in self.windows.values_mut() {
            let present = mac::online(output.display);
            let connected = output.connected.load(Ordering::Relaxed);
            if !present && connected {
                mac::show(output.handle, false);
                output.connected.store(false, Ordering::Relaxed);
                warn!(
                    display = output.display,
                    "output screen unplugged: window hidden"
                );
            } else if present && !connected {
                // The screen comes back in its default mode.
                if let Some((width, height, refresh)) = output.wanted_mode {
                    output.mode = None;
                    match mac::set_mode(output.display, width, height, refresh) {
                        Ok(restore) => output.mode = restore,
                        Err(e) => warn!(error = %e, "couldn't set the screen's mode again"),
                    }
                }
                match mac::cover(output.handle, output.display, output.visible) {
                    Ok(()) => {
                        output.connected.store(true, Ordering::Relaxed);
                        info!(display = output.display, "output screen back");
                    }
                    Err(e) => warn!(error = %e, "couldn't put the window back on its screen"),
                }
            }
        }
    }

    /// Elsewhere winit's fullscreen windows stay with their screen.
    #[cfg(not(target_os = "macos"))]
    fn check_screens(&mut self) {}

    fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        screen: &str,
        format: WantedFormat,
        visible: bool,
    ) -> Result<Opened, String> {
        let monitor = event_loop
            .available_monitors()
            .find(|m| display_id(m) == screen)
            .ok_or_else(|| format!("display {screen} isn't connected"))?;
        let mode = best_mode(&monitor, format);
        let attributes = Window::default_attributes()
            .with_title("Capture Room output")
            .with_decorations(false);
        let (output, mode_set) = place(event_loop, &monitor, mode.as_ref(), attributes, visible)
            .map_err(|e| format!("couldn't open a window on {screen}: {e}"))?;
        let description = match mode.filter(|_| mode_set) {
            Some(mode) => describe(
                mode.size().width,
                mode.size().height,
                mode.refresh_rate_millihertz(),
            ),
            None => {
                let size = monitor.size();
                let current = describe(
                    size.width,
                    size.height,
                    monitor.refresh_rate_millihertz().unwrap_or(0),
                );
                format!(
                    "{current} (no {}×{} mode at the channel's rate: scaled)",
                    format.width, format.height
                )
            }
        };
        // Named, where the OS gives a name: "LG HDR 4K · 1920×1080 at 60 Hz".
        let description = match screen_name(&monitor) {
            Some(name) if name != screen => format!("{name} · {description}"),
            _ => description,
        };
        output.window.set_cursor_visible(false);
        let handle = output.handle;
        let connected = output.connected.clone();
        let id = output.window.id();
        info!(screen, mode = %description, "display window open");
        self.windows.insert(id, output);
        if self.awake.is_none() {
            self.awake = Some(power::keep_display_awake());
        }
        Ok((id, handle, description, connected))
    }
}

/// Put the screen in `mode` (if any) and open a window covering it; returns
/// whether the mode was set. Not winit's fullscreen: macOS exclusive mode
/// captures the display, whose black shield covers the window, and native
/// fullscreen animates into its own Space. Instead the mode is set for this
/// process only and a borderless window covers the screen on every Space.
#[cfg(target_os = "macos")]
fn place(
    event_loop: &ActiveEventLoop,
    monitor: &MonitorHandle,
    mode: Option<&VideoModeHandle>,
    attributes: winit::window::WindowAttributes,
    visible: bool,
) -> Result<(OutputWindow, bool), String> {
    use winit::platform::macos::MonitorHandleExtMacOS;

    let display = monitor.native_id();
    let wanted_mode = mode.map(|m| (m.size().width, m.size().height, m.refresh_rate_millihertz()));
    let (restore, mode_set) = match mode {
        Some(m) => match mac::set_mode(
            display,
            m.size().width,
            m.size().height,
            m.refresh_rate_millihertz(),
        ) {
            Ok(restore) => (restore, true),
            Err(e) => {
                warn!(error = %e, "couldn't set the screen's mode: scaling instead");
                (None, false)
            }
        },
        None => (None, false),
    };
    // Hidden until `cover` has put it on its screen.
    let window = event_loop
        .create_window(attributes.with_visible(false))
        .map_err(|e| e.to_string())?;
    let handle = native_handle(&window)?;
    mac::cover(handle, display, visible)?;
    Ok((
        OutputWindow {
            window,
            handle,
            visible,
            connected: Arc::new(AtomicBool::new(true)),
            display,
            wanted_mode: wanted_mode.filter(|_| mode_set),
            mode: restore,
        },
        mode_set,
    ))
}

/// Elsewhere winit's fullscreen does it: exclusive in the mode, or borderless
/// at the screen's own mode.
#[cfg(not(target_os = "macos"))]
fn place(
    event_loop: &ActiveEventLoop,
    monitor: &MonitorHandle,
    mode: Option<&VideoModeHandle>,
    attributes: winit::window::WindowAttributes,
    visible: bool,
) -> Result<(OutputWindow, bool), String> {
    let fullscreen = match mode {
        Some(mode) => Fullscreen::Exclusive(mode.clone()),
        None => Fullscreen::Borderless(Some(monitor.clone())),
    };
    let window = event_loop
        .create_window(
            attributes
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_fullscreen(Some(fullscreen))
                .with_visible(visible),
        )
        .map_err(|e| e.to_string())?;
    let handle = native_handle(&window)?;
    Ok((
        OutputWindow {
            window,
            handle,
            visible,
            connected: Arc::new(AtomicBool::new(true)),
        },
        mode.is_some(),
    ))
}

/// Show or hide an output window without taking focus from the operator's app.
fn show(output: &OutputWindow, visible: bool) {
    #[cfg(target_os = "macos")]
    mac::show(output.handle, visible);
    #[cfg(not(target_os = "macos"))]
    output.window.set_visible(visible);
}

/// The handle a video sink draws into.
fn native_handle(window: &Window) -> Result<usize, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("no window handle: {e}"))?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => Ok(h.ns_view.as_ptr() as usize),
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as usize),
        RawWindowHandle::Xlib(h) => Ok(h.window as usize),
        RawWindowHandle::Xcb(h) => Ok(h.window.get() as usize),
        other => Err(format!("unsupported window type {other:?}")),
    }
}

/// A screen's id: its name, stable while connected (and across reconnects on
/// most systems).
fn display_id(monitor: &MonitorHandle) -> String {
    monitor.name().unwrap_or_else(|| "display".into())
}

/// The name the OS shows for a screen, where winit doesn't (on macOS it gives
/// `Monitor #<display id>`).
fn screen_name(monitor: &MonitorHandle) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::MonitorHandleExtMacOS;
        mac::screen_name(monitor.native_id())
    }
    #[cfg(not(target_os = "macos"))]
    {
        monitor.name()
    }
}

fn list(event_loop: &ActiveEventLoop) -> Vec<DisplayDto> {
    let primary = event_loop.primary_monitor().map(|m| display_id(&m));
    event_loop
        .available_monitors()
        .map(|m| {
            let id = display_id(&m);
            let mut modes: Vec<DisplayModeDto> = Vec::new();
            for mode in m.video_modes() {
                let mode = DisplayModeDto {
                    width: mode.size().width,
                    height: mode.size().height,
                    refresh_mhz: mode.refresh_rate_millihertz(),
                };
                // Modes repeat per bit depth.
                if !modes.contains(&mode) {
                    modes.push(mode);
                }
            }
            modes.sort_by(|a, b| {
                (b.width * b.height, b.refresh_mhz).cmp(&(a.width * a.height, a.refresh_mhz))
            });
            DisplayDto {
                name: screen_name(&m).unwrap_or_else(|| id.clone()),
                primary: primary.as_deref() == Some(id.as_str()),
                width: m.size().width,
                height: m.size().height,
                refresh_mhz: m.refresh_rate_millihertz().unwrap_or(0),
                modes,
                id,
            }
        })
        .collect()
}

/// Refresh rates that show `format` without judder, best first: its own, then double.
pub fn refresh_rates(format: WantedFormat) -> [u32; 2] {
    let mhz = (u64::from(format.fps_num) * 1000 / u64::from(format.fps_den.max(1))) as u32;
    [mhz, mhz * 2]
}

/// Whether a mode's refresh (mHz) is `wanted`: screens report 59.94 Hz as
/// 59940 or a few mHz either side.
fn same_rate(mode: u32, wanted: u32) -> bool {
    mode.abs_diff(wanted) <= 10
}

/// The screen's mode at `format`'s size and a rate that suits it.
fn best_mode(monitor: &MonitorHandle, format: WantedFormat) -> Option<VideoModeHandle> {
    let sized: Vec<VideoModeHandle> = monitor
        .video_modes()
        .filter(|m| m.size().width == format.width && m.size().height == format.height)
        .collect();
    refresh_rates(format).into_iter().find_map(|rate| {
        sized
            .iter()
            .filter(|m| same_rate(m.refresh_rate_millihertz(), rate))
            .max_by_key(|m| m.bit_depth())
            .cloned()
    })
}

/// `1920×1080 at 59.94 Hz`
pub fn describe(width: u32, height: u32, refresh_mhz: u32) -> String {
    let hz = f64::from(refresh_mhz) / 1000.0;
    let hz = format!("{hz:.3}");
    let hz = hz.trim_end_matches('0').trim_end_matches('.');
    format!("{width}×{height} at {hz} Hz")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(fps_num: u32, fps_den: u32) -> WantedFormat {
        WantedFormat {
            width: 1920,
            height: 1080,
            fps_num,
            fps_den,
        }
    }

    #[test]
    fn refresh_rates_fit_the_frame_rate() {
        assert_eq!(refresh_rates(fmt(30000, 1001)), [29970, 59940]);
        assert_eq!(refresh_rates(fmt(60000, 1001)), [59940, 119880]);
        assert_eq!(refresh_rates(fmt(25, 1)), [25000, 50000]);
        assert!(same_rate(59940, 59940));
        assert!(same_rate(59950, 59940));
        assert!(!same_rate(60000, 59940));
    }

    #[test]
    fn modes_read_naturally() {
        assert_eq!(describe(1920, 1080, 59940), "1920×1080 at 59.94 Hz");
        assert_eq!(describe(3840, 2160, 60000), "3840×2160 at 60 Hz");
        assert_eq!(describe(1280, 720, 50000), "1280×720 at 50 Hz");
    }
}

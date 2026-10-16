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
    /// The node's status, for the menu bar icon.
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
    awake: Option<power::KeepAwake>,
    /// The menu bar icon to show once the loop runs, if any.
    tray_config: Option<crate::tray::Config>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray: Option<crate::tray::Tray>,
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
        drop(config);
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
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            Command::Tray(_) => {}
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
            self.awake = power::KeepAwake::new();
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
        RawWindowHandle::Wayland(h) => Ok(h.surface.as_ptr() as usize),
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

/// macOS screen modes and output window placement (see [`place`]).
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use objc2::encode::{Encode, Encoding};
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    type ModeRef = *mut c_void;
    type CFTypeRef = *const c_void;

    /// `kCGConfigureForAppOnly`: undone when the process exits.
    const FOR_APP_ONLY: u32 = 0;
    /// `NSScreenSaverWindowLevel`: above the menu bar and the Dock.
    const LEVEL: isize = 1000;
    /// `canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary`
    const BEHAVIOR: usize = 1 | 1 << 4 | 1 << 6 | 1 << 8;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        static kCGDisplayShowDuplicateLowResolutionModes: CFTypeRef;
        fn CGDisplayCopyDisplayMode(display: u32) -> ModeRef;
        fn CGDisplayCopyAllDisplayModes(display: u32, options: CFTypeRef) -> CFTypeRef;
        fn CGDisplayModeGetPixelWidth(mode: ModeRef) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: ModeRef) -> usize;
        fn CGDisplayModeGetWidth(mode: ModeRef) -> usize;
        fn CGDisplayModeGetRefreshRate(mode: ModeRef) -> f64;
        fn CGDisplayModeRetain(mode: ModeRef) -> ModeRef;
        fn CGDisplayModeRelease(mode: ModeRef);
        fn CGBeginDisplayConfiguration(config: *mut *mut c_void) -> i32;
        fn CGConfigureDisplayWithDisplayMode(
            config: *mut c_void,
            display: u32,
            mode: ModeRef,
            options: CFTypeRef,
        ) -> i32;
        fn CGCompleteDisplayConfiguration(config: *mut c_void, option: u32) -> i32;
        fn CGCancelDisplayConfiguration(config: *mut c_void) -> i32;
        fn CGDisplayBounds(display: u32) -> Rect;
        fn CGGetOnlineDisplayList(max: u32, ids: *mut u32, count: *mut u32) -> i32;
        fn CGMainDisplayID() -> u32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        fn CFDictionaryCreate(
            alloc: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: isize,
            key_callbacks: *const u8,
            value_callbacks: *const u8,
        ) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }

    fn matches(mode: ModeRef, width: u32, height: u32, refresh_mhz: u32) -> bool {
        // SAFETY: a valid mode.
        unsafe {
            CGDisplayModeGetPixelWidth(mode) == width as usize
                && CGDisplayModeGetPixelHeight(mode) == height as usize
                && super::same_rate(
                    (CGDisplayModeGetRefreshRate(mode) * 1000.0).round() as u32,
                    refresh_mhz,
                )
        }
    }

    /// Switch `display` to a mode with these pixels and rate, for this process.
    /// `None` if it's already in one; otherwise how to put it back.
    pub fn set_mode(
        display: u32,
        width: u32,
        height: u32,
        refresh_mhz: u32,
    ) -> Result<Option<ModeRestore>, String> {
        // SAFETY: CoreGraphics calls on a display id, releasing what's copied.
        unsafe {
            let current = CGDisplayCopyDisplayMode(display);
            if current.is_null() {
                return Err(format!("display {display} has no mode"));
            }
            if matches(current, width, height, refresh_mhz) {
                CGDisplayModeRelease(current);
                return Ok(None);
            }
            let keys = [kCGDisplayShowDuplicateLowResolutionModes];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let modes = CGDisplayCopyAllDisplayModes(display, options);
            if !options.is_null() {
                CFRelease(options);
            }
            let mut found: ModeRef = std::ptr::null_mut();
            if !modes.is_null() {
                for i in 0..CFArrayGetCount(modes) {
                    let mode = CFArrayGetValueAtIndex(modes, i) as ModeRef;
                    if !matches(mode, width, height, refresh_mhz) {
                        continue;
                    }
                    // One pixel per point: a HiDPI mode of the same pixels
                    // draws the desktop at half size with no different signal.
                    let native = CGDisplayModeGetWidth(mode) == width as usize;
                    if found.is_null() || native {
                        found = mode;
                    }
                    if native {
                        break;
                    }
                }
            }
            let result = if found.is_null() {
                Err(format!(
                    "display {display} has no {width}×{height} mode at {refresh_mhz} mHz"
                ))
            } else {
                configure(display, found).map(|()| {
                    Some(ModeRestore {
                        display,
                        original: CGDisplayModeRetain(current),
                    })
                })
            };
            if !modes.is_null() {
                CFRelease(modes);
            }
            CGDisplayModeRelease(current);
            result
        }
    }

    /// SAFETY: `mode` is a valid mode of `display`.
    unsafe fn configure(display: u32, mode: ModeRef) -> Result<(), String> {
        let mut config = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) != 0 {
            return Err("couldn't start configuring the display".into());
        }
        let err = CGConfigureDisplayWithDisplayMode(config, display, mode, std::ptr::null());
        if err != 0 {
            CGCancelDisplayConfiguration(config);
            return Err(format!("couldn't set the display's mode (error {err})"));
        }
        match CGCompleteDisplayConfiguration(config, FOR_APP_ONLY) {
            0 => Ok(()),
            err => Err(format!("couldn't apply the display's mode (error {err})")),
        }
    }

    /// Puts a screen's mode back when dropped.
    pub struct ModeRestore {
        display: u32,
        original: ModeRef,
    }

    // SAFETY: only used on the window loop's thread; the mode is an
    // immutable CF object.
    unsafe impl Send for ModeRestore {}

    impl Drop for ModeRestore {
        fn drop(&mut self) {
            // SAFETY: the retained original mode, released once.
            unsafe {
                if let Err(e) = configure(self.display, self.original) {
                    tracing::warn!(error = %e, "couldn't put the screen's mode back");
                }
                CGDisplayModeRelease(self.original);
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        origin: Point,
        size: Size,
    }

    // SAFETY: the layouts of CGPoint, CGSize and CGRect.
    unsafe impl Encode for Point {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Size {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Rect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
    }

    /// Whether `display` is plugged in (asleep counts).
    pub fn online(display: u32) -> bool {
        let mut ids = [0u32; 32];
        let mut count = 0u32;
        // SAFETY: the buffer holds `ids.len()` entries.
        let err = unsafe { CGGetOnlineDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        err == 0 && ids[..count as usize].contains(&display)
    }

    /// The `NSScreen` showing `display`, if it's connected. Main thread only.
    fn screen(display: u32) -> *mut AnyObject {
        // SAFETY: AppKit class methods and NSArray / NSDictionary / NSNumber
        // accessors, on the main thread.
        unsafe {
            let (Some(screens_class), Some(string_class)) =
                (AnyClass::get(c"NSScreen"), AnyClass::get(c"NSString"))
            else {
                return std::ptr::null_mut();
            };
            let key: *mut AnyObject =
                msg_send![string_class, stringWithUTF8String: c"NSScreenNumber".as_ptr()];
            let screens: *mut AnyObject = msg_send![screens_class, screens];
            if screens.is_null() {
                return std::ptr::null_mut();
            }
            let count: usize = msg_send![screens, count];
            for i in 0..count {
                let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
                let description: *mut AnyObject = msg_send![screen, deviceDescription];
                let number: *mut AnyObject = msg_send![description, objectForKey: key];
                if number.is_null() {
                    continue;
                }
                let id: u32 = msg_send![number, unsignedIntValue];
                if id == display {
                    return screen;
                }
            }
            std::ptr::null_mut()
        }
    }

    /// The name macOS shows for `display` ("LG HDR 4K"). Main thread only.
    pub fn screen_name(display: u32) -> Option<String> {
        let screen = screen(display);
        if screen.is_null() {
            return None;
        }
        // SAFETY: an NSScreen and its NSString name, on the main thread.
        unsafe {
            let name: *mut AnyObject = msg_send![screen, localizedName];
            if name.is_null() {
                return None;
            }
            let utf8: *const std::ffi::c_char = msg_send![name, UTF8String];
            if utf8.is_null() {
                return None;
            }
            Some(
                std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    /// Make the window holding `ns_view` cover `display` above the menu bar and
    /// Dock on every Space. (winit places it on the wrong screen when scale
    /// factors differ.)
    pub fn cover(ns_view: usize, display: u32, visible: bool) -> Result<(), String> {
        let screen = screen(display);
        if screen.is_null() {
            return Err(format!("display {display} isn't connected"));
        }
        // SAFETY: a live NSView from winit and an NSScreen, on the main thread.
        unsafe {
            let view = ns_view as *mut AnyObject;
            let window: *mut AnyObject = msg_send![view, window];
            if window.is_null() {
                return Err("the output view has no window".into());
            }
            let _: () = msg_send![window, setLevel: LEVEL];
            let _: () = msg_send![window, setCollectionBehavior: BEHAVIOR];
            // CoreGraphics has the new size right after a mode change (NSScreen
            // catches up later). Its origin is the main screen's top left,
            // AppKit's the bottom left.
            let bounds = CGDisplayBounds(display);
            let main = CGDisplayBounds(CGMainDisplayID());
            let frame = Rect {
                origin: Point {
                    x: bounds.origin.x,
                    y: main.size.height - bounds.origin.y - bounds.size.height,
                },
                size: bounds.size,
            };
            let _: () = msg_send![window, setFrame: frame, display: true];
        }
        show(ns_view, visible);
        Ok(())
    }

    /// Show (in front, without activating the app or taking key focus) or hide
    /// the window holding `ns_view`.
    pub fn show(ns_view: usize, visible: bool) {
        // SAFETY: a live NSView from winit, on the main thread.
        unsafe {
            let view = ns_view as *mut AnyObject;
            let window: *mut AnyObject = msg_send![view, window];
            if window.is_null() {
                return;
            }
            if visible {
                let _: () = msg_send![window, orderFrontRegardless];
            } else {
                let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
            }
        }
    }
}

/// Screen sleep. A sleeping screen drops out of the screen list (and its
/// output goes dark), so screens are woken before listing or opening, and
/// kept awake while an output shows.
#[cfg(target_os = "macos")]
mod power {
    use std::ffi::{c_char, c_void, CString};
    use std::time::{Duration, Instant};

    use tracing::warn;

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ASSERTION_ON: u32 = 255;
    /// `kIOPMUserActiveLocal`
    const USER_ACTIVE_LOCAL: u32 = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionDeclareUserActivity(name: CFStringRef, kind: u32, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, ids: *mut u32, count: *mut u32) -> i32;
        fn CGGetOnlineDisplayList(max: u32, ids: *mut u32, count: *mut u32) -> i32;
    }

    struct CfString(CFStringRef);

    impl CfString {
        fn new(s: &str) -> Self {
            let c = CString::new(s).expect("no NUL");
            // SAFETY: a valid C string; the result is released in `drop`.
            Self(unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) })
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: created by us, released once.
                unsafe { CFRelease(self.0) }
            }
        }
    }

    const NAME: &str = "Capture Room display output";

    fn display_count(list: unsafe extern "C" fn(u32, *mut u32, *mut u32) -> i32) -> u32 {
        let mut ids = [0u32; 32];
        let mut count = 0u32;
        // SAFETY: the buffer holds `ids.len()` entries.
        let err = unsafe { list(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        if err == 0 {
            count
        } else {
            0
        }
    }

    /// Wake sleeping screens (as a key press would), waiting briefly for them.
    pub fn wake_screens() {
        let online = display_count(CGGetOnlineDisplayList);
        if display_count(CGGetActiveDisplayList) >= online {
            return;
        }
        let name = CfString::new(NAME);
        let mut id = 0u32;
        // SAFETY: valid string and out pointer.
        let err = unsafe { IOPMAssertionDeclareUserActivity(name.0, USER_ACTIVE_LOCAL, &mut id) };
        if err != 0 {
            warn!(error = err, "couldn't wake the screens");
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while display_count(CGGetActiveDisplayList) < online && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        // SAFETY: the assertion we just made.
        unsafe { IOPMAssertionRelease(id) };
    }

    /// Screens stay awake until dropped.
    pub struct KeepAwake(u32);

    impl KeepAwake {
        pub fn new() -> Option<Self> {
            let kind = CfString::new("PreventUserIdleDisplaySleep");
            let name = CfString::new(NAME);
            let mut id = 0u32;
            // SAFETY: valid strings and out pointer.
            let err = unsafe { IOPMAssertionCreateWithName(kind.0, ASSERTION_ON, name.0, &mut id) };
            if err != 0 {
                warn!(error = err, "couldn't keep the screens awake");
                return None;
            }
            Some(Self(id))
        }
    }

    impl Drop for KeepAwake {
        fn drop(&mut self) {
            // SAFETY: the assertion made in `new`, released once.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

/// Elsewhere screens are left to the OS for now.
#[cfg(not(target_os = "macos"))]
mod power {
    pub fn wake_screens() {}

    pub struct KeepAwake;

    impl KeepAwake {
        pub fn new() -> Option<Self> {
            None
        }
    }
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

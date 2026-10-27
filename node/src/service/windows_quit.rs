//! Asking a node on Windows to quit cleanly, so it finishes its recordings.
//!
//! A process started with `CREATE_NO_WINDOW` has a console no one can type
//! Ctrl+C into, so each node makes a named event, `Local\CaptureRoom.Quit.<pid>`,
//! and quits when it's set: [`request`] is how `service stop` and the
//! installer ask. The event is in the login session's namespace with the
//! node's default security, so only the same user in the same session can
//! open it.
//!
//! Signing out and shutting down reach a process that loads user32 as window
//! messages, not as console events (Windows only sends those to services), so
//! the node also keeps a hidden top-level window of its own for
//! `WM_ENDSESSION`. Its own, not winit's, so it's there whether or not the
//! display loop started. Windows ends the process once the message returns,
//! so the window holds it until the recordings are finished.

use std::ffi::c_void;
use std::ptr::null;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Result};

/// How long sign-out or shutdown waits for the node: longer than a
/// recording takes to finish, short enough not to hold Windows up for good.
const END_SESSION_TIMEOUT: Duration = Duration::from_secs(30);

const EVENT_MODIFY_STATE: u32 = 0x0002;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;
const WM_ENDSESSION: u32 = 0x0016;

type WndProc = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: isize,
    icon: isize,
    cursor: isize,
    background: isize,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: isize,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> isize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn SetEvent(event: isize) -> i32;
    fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetModuleHandleW(name: *const u16) -> isize;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        param: *const c_void,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn ShutdownBlockReasonCreate(hwnd: isize, reason: *const u16) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn event_name(pid: u32) -> Vec<u16> {
    wide(&format!(r"Local\CaptureRoom.Quit.{pid}"))
}

/// Ask the node with process id `pid` to quit. Fails if it isn't listening
/// (not running, or still starting, so it has nothing to finish yet).
pub fn request(pid: u32) -> Result<()> {
    let name = event_name(pid);
    // SAFETY: a null-terminated name; the handle is closed below.
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if event == 0 {
        bail!("the node isn't listening for a quit: {}", last_error());
    }
    // SAFETY: a handle opened above with the right to set it.
    let set = unsafe { SetEvent(event) };
    let error = last_error();
    // SAFETY: as above, and not used again.
    unsafe { CloseHandle(event) };
    if set == 0 {
        bail!("couldn't ask the node to quit: {error}");
    }
    Ok(())
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

static ON_QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Call `on_quit` when [`request`] asks this process to quit, or when the
/// user signs out or Windows shuts down. In the second case the process has
/// to exit before `on_quit`'s caller gives up waiting. Once per process.
pub fn watch(on_quit: impl Fn() + Send + Sync + 'static) {
    if ON_QUIT.set(Box::new(on_quit)).is_err() {
        return;
    }
    let name = event_name(std::process::id());
    // SAFETY: a null-terminated name; manual reset, so a second request
    // finds it set too. Never closed: the process waits on it until it ends.
    let event = unsafe { CreateEventW(null(), 1, 0, name.as_ptr()) };
    if event == 0 {
        tracing::warn!(error = %last_error(), "can't listen for a quit request");
    } else {
        let spawned = std::thread::Builder::new()
            .name("quit-request".into())
            .spawn(move || {
                // SAFETY: an event handle that stays open.
                if unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
                    tracing::info!("asked to quit");
                    quit();
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "can't listen for a quit request");
        }
    }
    let spawned = std::thread::Builder::new()
        .name("end-session".into())
        .spawn(|| {
            if let Err(e) = end_session_window() {
                tracing::warn!(error = %e, "won't finish recordings when Windows signs out");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "won't finish recordings when Windows signs out");
    }
}

fn quit() {
    if let Some(on_quit) = ON_QUIT.get() {
        on_quit();
    }
}

/// A hidden window, and its message loop: top-level, since Windows only
/// sends `WM_ENDSESSION` to top-level windows (not to message-only ones).
fn end_session_window() -> Result<()> {
    let class_name = wide("CaptureRoomEndSession");
    // SAFETY: a null name asks for this exe's own module.
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = WndClassW {
        style: 0,
        wnd_proc: window_proc,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: 0,
        cursor: 0,
        background: 0,
        menu_name: null(),
        class_name: class_name.as_ptr(),
    };
    // SAFETY: a filled-in class whose name outlives the window.
    if unsafe { RegisterClassW(&class) } == 0 {
        bail!("register the window class: {}", last_error());
    }
    // SAFETY: the class registered above; no parent, menu or creation data.
    // Style 0 and never shown, so it stays hidden.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            instance,
            null(),
        )
    };
    if hwnd == 0 {
        bail!("create the window: {}", last_error());
    }
    let mut msg = std::mem::MaybeUninit::<Msg>::zeroed();
    // SAFETY: GetMessageW fills `msg` before DispatchMessageW reads it; it
    // returns 0 on WM_QUIT and -1 on error, which end the loop.
    while unsafe { GetMessageW(msg.as_mut_ptr(), 0, 0, 0) } > 0 {
        unsafe { DispatchMessageW(msg.as_ptr()) };
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: isize,
    msg: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    // `w_param` is 0 when the session isn't ending after all (something else
    // refused); and WM_QUERYENDSESSION falls through, never refusing.
    if msg == WM_ENDSESSION && w_param != 0 {
        tracing::info!("Windows is signing out or shutting down");
        let reason = wide("Finishing recordings");
        // SAFETY: called on the window's own thread, as it must be. Shown
        // on the screen that lists what's holding up the sign-out.
        unsafe { ShutdownBlockReasonCreate(hwnd, reason.as_ptr()) };
        quit();
        // The quit ends the process from another thread; until then, keep
        // Windows from ending it first.
        std::thread::sleep(END_SESSION_TIMEOUT);
        return 0;
    }
    // SAFETY: the arguments Windows passed in.
    unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) }
}

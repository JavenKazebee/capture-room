//! Keeping the machine and its screens awake: the machine while anything
//! records or plays out ([`keep_system_awake`]), and the screens while a
//! display output shows ([`keep_display_awake`]). An awake screen keeps the
//! machine awake too.
//!
//! One thread holds the OS's locks, since Windows ties
//! `SetThreadExecutionState` to the thread that called it; callers only say
//! what they want, and never wait on it. Per OS:
//! - macOS: IOKit assertions (`PreventUserIdleSystemSleep`,
//!   `PreventUserIdleDisplaySleep`).
//! - Windows: `ES_SYSTEM_REQUIRED`, plus `ES_DISPLAY_REQUIRED` for screens.
//! - Linux: a logind `sleep:idle` inhibitor on the system bus, and
//!   `org.freedesktop.ScreenSaver.Inhibit` on the session bus for screens
//!   (GNOME and KDE both serve it). Without either bus (a container, CI) the
//!   machine is left to the OS, with a note in the log.
//!
//! A sleeping screen also drops out of the screen list (and its output goes
//! dark), so [`wake_screens`] wakes them before listing or opening.

use std::sync::{mpsc, Mutex, OnceLock};

use tracing::warn;

/// What the lock is called where the OS shows it (Windows doesn't).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const NAME: &str = "Capture Room";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const WHY_SYSTEM: &str = "Recording or playing out";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const WHY_DISPLAY: &str = "Showing a display output";

struct Wanted {
    system: bool,
    displays: usize,
}

impl Wanted {
    fn locks(&self) -> Locks {
        let display = self.displays > 0;
        Locks {
            system: self.system || display,
            display,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Locks {
    system: bool,
    display: bool,
}

static WANTED: Mutex<Wanted> = Mutex::new(Wanted {
    system: false,
    displays: 0,
});

/// Keep the machine from idle-sleeping, or let it again.
pub fn keep_system_awake(on: bool) {
    change(|w| w.system = on);
}

/// Screens stay awake while any of these is alive.
pub struct DisplayAwake(());

/// Keep the screens awake until the result is dropped.
pub fn keep_display_awake() -> DisplayAwake {
    change(|w| w.displays += 1);
    DisplayAwake(())
}

impl Drop for DisplayAwake {
    fn drop(&mut self) {
        change(|w| w.displays -= 1);
    }
}

fn change(f: impl FnOnce(&mut Wanted)) {
    // Sent under the lock, so the worker sees changes in order.
    let mut wanted = WANTED.lock().unwrap();
    let before = wanted.locks();
    f(&mut wanted);
    let after = wanted.locks();
    if after != before {
        let _ = worker().send(after);
    }
}

fn worker() -> &'static mpsc::Sender<Locks> {
    static TX: OnceLock<mpsc::Sender<Locks>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Locks>();
        std::thread::Builder::new()
            .name("power".into())
            .spawn(move || {
                let mut held = imp::Held::default();
                while let Ok(mut locks) = rx.recv() {
                    // Only the latest matters.
                    while let Ok(next) = rx.try_recv() {
                        locks = next;
                    }
                    held.set(locks);
                }
            })
            .expect("spawn the power thread");
        tx
    })
}

#[cfg(target_os = "macos")]
pub use imp::wake_screens;

#[cfg(not(target_os = "macos"))]
pub fn wake_screens() {}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CString};
    use std::time::{Duration, Instant};

    use super::{warn, Locks, NAME, WHY_DISPLAY, WHY_SYSTEM};

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

    struct Assertion(u32);

    impl Assertion {
        fn new(kind: &str, why: &str) -> Option<Self> {
            let kind = CfString::new(kind);
            let name = CfString::new(&format!("{NAME}: {why}"));
            let mut id = 0u32;
            // SAFETY: valid strings and out pointer.
            let err = unsafe { IOPMAssertionCreateWithName(kind.0, ASSERTION_ON, name.0, &mut id) };
            if err != 0 {
                warn!(error = err, why, "couldn't keep the machine awake");
                return None;
            }
            Some(Self(id))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            // SAFETY: the assertion made in `new`, released once.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }

    #[derive(Default)]
    pub struct Held {
        system: Option<Assertion>,
        display: Option<Assertion>,
    }

    impl Held {
        pub fn set(&mut self, locks: Locks) {
            if locks.system != self.system.is_some() {
                self.system = locks
                    .system
                    .then(|| Assertion::new("PreventUserIdleSystemSleep", WHY_SYSTEM))
                    .flatten();
            }
            if locks.display != self.display.is_some() {
                self.display = locks
                    .display
                    .then(|| Assertion::new("PreventUserIdleDisplaySleep", WHY_DISPLAY))
                    .flatten();
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{warn, Locks};

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    #[derive(Default)]
    pub struct Held(());

    impl Held {
        pub fn set(&mut self, locks: Locks) {
            let mut flags = ES_CONTINUOUS;
            if locks.system {
                flags |= ES_SYSTEM_REQUIRED;
            }
            if locks.display {
                flags |= ES_DISPLAY_REQUIRED;
            }
            // SAFETY: no pointers; the state holds until this thread changes it.
            if unsafe { SetThreadExecutionState(flags) } == 0 {
                warn!(flags, "couldn't keep the machine awake");
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use tracing::info;
    use zbus::zvariant::OwnedFd;
    use zbus::Connection;

    use super::{warn, Locks, NAME, WHY_DISPLAY, WHY_SYSTEM};

    #[derive(Default)]
    pub struct Held {
        /// zbus needs a runtime; this thread has none of its own.
        rt: Option<tokio::runtime::Runtime>,
        /// logind holds the lock while this is open.
        sleep: Option<OwnedFd>,
        /// The screensaver drops an inhibit when its caller leaves the bus,
        /// so closing this connection releases it.
        screensaver: Option<Connection>,
        /// Logged once each, so a machine without them isn't noisy.
        no_logind: bool,
        no_screensaver: bool,
    }

    impl Held {
        pub fn set(&mut self, locks: Locks) {
            let rt = match &self.rt {
                Some(rt) => rt,
                None => match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("power-dbus")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => self.rt.insert(rt),
                    Err(e) => {
                        warn!(error = %e, "couldn't keep the machine awake");
                        return;
                    }
                },
            };
            if locks.system != self.sleep.is_some() {
                self.sleep = None;
                if locks.system {
                    match rt.block_on(inhibit_sleep()) {
                        Ok(fd) => self.sleep = Some(fd),
                        Err(e) if !self.no_logind => {
                            self.no_logind = true;
                            info!(error = %e, "no logind to keep the machine awake; it may idle-sleep");
                        }
                        Err(_) => {}
                    }
                }
            }
            if locks.display != self.screensaver.is_some() {
                // Closed on the runtime its tasks run on.
                if let Some(conn) = self.screensaver.take() {
                    rt.block_on(async move { drop(conn) });
                }
                if locks.display {
                    match rt.block_on(inhibit_screensaver()) {
                        Ok(conn) => self.screensaver = Some(conn),
                        Err(e) if !self.no_screensaver => {
                            self.no_screensaver = true;
                            info!(error = %e, "no screensaver service to keep the screens awake");
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }

    async fn inhibit_sleep() -> zbus::Result<OwnedFd> {
        let conn = Connection::system().await?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Inhibit",
                &("sleep:idle", NAME, WHY_SYSTEM, "block"),
            )
            .await?;
        reply.body().deserialize()
    }

    async fn inhibit_screensaver() -> zbus::Result<Connection> {
        let conn = Connection::session().await?;
        conn.call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/org/freedesktop/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "Inhibit",
            &(NAME, WHY_DISPLAY),
        )
        .await?;
        Ok(conn)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::Locks;

    #[derive(Default)]
    pub struct Held(());

    impl Held {
        pub fn set(&mut self, _: Locks) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_awake_screen_keeps_the_machine_awake() {
        let locks = |system, displays| Wanted { system, displays }.locks();
        assert!(locks(false, 0) == Locks::default());
        assert!(
            locks(true, 0)
                == Locks {
                    system: true,
                    display: false
                }
        );
        assert!(
            locks(false, 2)
                == Locks {
                    system: true,
                    display: true
                }
        );
    }
}

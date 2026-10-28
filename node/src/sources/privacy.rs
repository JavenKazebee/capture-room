//! Privacy permissions for capture devices. `fallbacksrc` swallows
//! AVFoundation's denied error and retries forever, so check first to fail
//! with a reason. On Windows, the camera and microphone switches in Settings ›
//! Privacy & security; nothing is checked on Linux.

use anyhow::{bail, Result};

use crate::api::types::DeviceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Camera,
    Microphone,
    ScreenRecording,
}

impl Permission {
    /// Its pane under System Settings › Privacy & Security.
    pub fn pane(self) -> &'static str {
        match self {
            Self::Camera => "Camera",
            Self::Microphone => "Microphone",
            Self::ScreenRecording => "Screen & System Audio Recording",
        }
    }

    pub fn settings_url(self) -> &'static str {
        if cfg!(target_os = "windows") {
            return match self {
                Self::Camera => "ms-settings:privacy-webcam",
                Self::Microphone => "ms-settings:privacy-microphone",
                Self::ScreenRecording => "ms-settings:privacy",
            };
        }
        match self {
            Self::Camera => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
            Self::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Self::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
        }
    }

    fn of(kind: DeviceKind) -> Option<Self> {
        match kind {
            DeviceKind::Video => Some(Self::Camera),
            DeviceKind::Audio => Some(Self::Microphone),
            DeviceKind::Screen => Some(Self::ScreenRecording),
            DeviceKind::Output => None,
        }
    }
}

/// Reported on macOS and Windows; Windows can't tell [`Access::NotAsked`]
/// apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Granted,
    /// Denied, or restricted by a profile. For screen recording, also not
    /// asked yet: macOS doesn't tell the two apart.
    Denied,
    /// Not asked yet: opening a device asks.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotAsked,
}

/// Whether the OS lets the node use `permission`; `None` where it isn't
/// checked.
pub fn access(permission: Permission) -> Option<Access> {
    imp::access(permission)
}

/// Show macOS's prompt; it only appears the first time. Screen recording
/// applies from the node's next launch.
pub fn request(permission: Permission) {
    imp::request(permission)
}

pub const WHO: &str = "the app the node runs under (Terminal when started from a shell), \
                       then restart the node";

/// Fail if macOS won't let the node capture from a device of `kind`.
pub fn check(kind: DeviceKind, device: &str) -> Result<()> {
    let Some(permission) = Permission::of(kind) else {
        return Ok(());
    };
    if access(permission) != Some(Access::Denied) {
        return Ok(());
    }
    let pane = permission.pane();
    if cfg!(target_os = "windows") {
        bail!(
            "Windows privacy settings block {} access, needed for {device}: turn it on in \
             Settings › Privacy & security › {pane}, with Let desktop apps access your {}",
            pane.to_lowercase(),
            pane.to_lowercase()
        );
    }
    match permission {
        Permission::ScreenRecording => bail!(
            "macOS hasn't allowed screen recording, needed for {device}: allow it in System \
             Settings › Privacy & Security › {pane} for {WHO}"
        ),
        _ => bail!(
            "macOS has denied {} access, needed for {device}: allow it in System Settings › \
             Privacy & Security › {pane} for {WHO}",
            pane.to_lowercase()
        ),
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    use super::{Access, Permission};

    /// How long `access` waits for AVFoundation's answer: it comes at once
    /// when access is decided, and waits only on a prompt that's still open.
    const ANSWER_WAIT: Duration = Duration::from_millis(200);

    /// The node has shown the camera or microphone prompt since it started.
    static ASKED_CAMERA: AtomicBool = AtomicBool::new(false);
    static ASKED_MICROPHONE: AtomicBool = AtomicBool::new(false);

    fn asked(permission: Permission) -> Option<&'static AtomicBool> {
        match permission {
            Permission::Camera => Some(&ASKED_CAMERA),
            Permission::Microphone => Some(&ASKED_MICROPHONE),
            Permission::ScreenRecording => None,
        }
    }

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeVideo: &'static AnyObject;
        static AVMediaTypeAudio: &'static AnyObject;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// `AVAuthorizationStatus`
    const NOT_DETERMINED: isize = 0;
    const AUTHORIZED: isize = 3;

    fn media(permission: Permission) -> Option<&'static AnyObject> {
        match permission {
            Permission::Camera => Some(unsafe { AVMediaTypeVideo }),
            Permission::Microphone => Some(unsafe { AVMediaTypeAudio }),
            Permission::ScreenRecording => None,
        }
    }

    pub fn access(permission: Permission) -> Option<Access> {
        let Some(media) = media(permission) else {
            // SAFETY: no arguments; only reads the current state.
            let granted = unsafe { CGPreflightScreenCaptureAccess() };
            return Some(if granted {
                Access::Granted
            } else {
                Access::Denied
            });
        };
        let class = AnyClass::get(c"AVCaptureDevice")?;
        // SAFETY: a class method taking an AVMediaType, returning an NSInteger.
        let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: media] };
        let asked = asked(permission).is_some_and(|a| a.load(Ordering::Relaxed));
        match status {
            AUTHORIZED => return Some(Access::Granted),
            // Asking would show the prompt.
            NOT_DETERMINED if !asked => return Some(Access::NotAsked),
            _ => {}
        }
        // The status can miss a grant made outside AVFoundation: CoreAudio's
        // own prompt, or System Settings while the node runs. Asking again
        // answers from the real state, without a prompt once it's decided.
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        // SAFETY: as in `request`.
        let _: () = unsafe {
            msg_send![class, requestAccessForMediaType: media, completionHandler: &*done]
        };
        Some(match rx.recv_timeout(ANSWER_WAIT) {
            Ok(true) => Access::Granted,
            Ok(false) => Access::Denied,
            Err(_) => Access::NotAsked,
        })
    }

    pub fn request(permission: Permission) {
        let Some(media) = media(permission) else {
            // SAFETY: no arguments; shows the prompt the first time only.
            unsafe { CGRequestScreenCaptureAccess() };
            return;
        };
        if let Some(asked) = asked(permission) {
            asked.store(true, Ordering::Relaxed);
        }
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
            return;
        };
        // The answer is read back with `access`.
        let done = RcBlock::new(|_granted: Bool| {});
        // SAFETY: a class method taking an AVMediaType and a `void (^)(BOOL)`
        // handler, which AVFoundation copies.
        let _: () = unsafe {
            msg_send![class, requestAccessForMediaType: media, completionHandler: &*done]
        };
    }
}

/// Settings › Privacy & security › Camera (or Microphone) turns access off
/// for the whole device (HKLM) or for desktop apps (`NonPackaged`), which the
/// node is; unset means on. Its "Let apps access" switch isn't read: whether
/// it covers desktop apps differs between Windows versions.
#[cfg(target_os = "windows")]
mod imp {
    use super::{Access, Permission};
    use crate::registry::{self, Root};

    const STORE: &str =
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

    pub fn access(permission: Permission) -> Option<Access> {
        let capability = match permission {
            Permission::Camera => "webcam",
            Permission::Microphone => "microphone",
            Permission::ScreenRecording => return None,
        };
        let key = format!(r"{STORE}\{capability}");
        let denied = [
            (Root::LocalMachine, key.clone()),
            (Root::CurrentUser, format!(r"{key}\NonPackaged")),
        ]
        .iter()
        .any(|(root, key)| {
            registry::string(*root, key, "Value").is_some_and(|v| v.eq_ignore_ascii_case("Deny"))
        });
        Some(if denied {
            Access::Denied
        } else {
            Access::Granted
        })
    }

    /// Windows has no prompt for desktop apps.
    pub fn request(_: Permission) {}
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use super::{Access, Permission};

    pub fn access(_: Permission) -> Option<Access> {
        None
    }

    pub fn request(_: Permission) {}
}

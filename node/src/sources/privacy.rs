//! macOS privacy permissions for capture devices. `fallbacksrc` swallows
//! AVFoundation's denied error and retries forever, so check first to fail
//! with a reason. Nothing is checked off macOS.

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

/// Only macOS reports it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Granted,
    /// Denied, or restricted by a profile. For screen recording, also not
    /// asked yet: macOS doesn't tell the two apart.
    Denied,
    /// Not asked yet: opening a device asks.
    NotAsked,
}

/// Whether macOS lets the node use `permission`; `None` off macOS.
pub fn access(permission: Permission) -> Option<Access> {
    imp::access(permission)
}

/// Show macOS's camera or microphone prompt; it only appears the first time.
/// Not offered for screen recording: macOS can't say whether it has asked.
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
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    use super::{Access, Permission};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeVideo: &'static AnyObject;
        static AVMediaTypeAudio: &'static AnyObject;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
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
        Some(match status {
            AUTHORIZED => Access::Granted,
            NOT_DETERMINED => Access::NotAsked,
            _ => Access::Denied,
        })
    }

    pub fn request(permission: Permission) {
        let Some(media) = media(permission) else {
            return;
        };
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

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Access, Permission};

    pub fn access(_: Permission) -> Option<Access> {
        None
    }

    pub fn request(_: Permission) {}
}

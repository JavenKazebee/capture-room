//! macOS privacy permissions for capture devices. AVFoundation answers a
//! denied camera or microphone with an error that `fallbacksrc` swallows and
//! retries forever, so the source would just sit at "connecting"; checking
//! first lets it fail with a reason instead. Elsewhere nothing is checked.

use anyhow::Result;

use crate::api::types::DeviceKind;

/// Fail if macOS won't let the node capture from a device of `kind`.
pub fn check(kind: DeviceKind, device: &str) -> Result<()> {
    imp::check(kind, device)
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{bail, Result};
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    use crate::api::types::DeviceKind;

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
    const RESTRICTED: isize = 1;
    const DENIED: isize = 2;

    const WHO: &str = "the app the node runs under (Terminal when started from a shell), \
                       then restart the node";

    pub fn check(kind: DeviceKind, device: &str) -> Result<()> {
        let (media, pane) = match kind {
            DeviceKind::Video => (unsafe { AVMediaTypeVideo }, "Camera"),
            DeviceKind::Audio => (unsafe { AVMediaTypeAudio }, "Microphone"),
            DeviceKind::Screen => {
                // SAFETY: no arguments; only reads the current state.
                if !unsafe { CGPreflightScreenCaptureAccess() } {
                    bail!(
                        "macOS hasn't allowed screen recording, needed for {device}: allow it in \
                         System Settings › Privacy & Security › Screen & System Audio Recording \
                         for {WHO}"
                    );
                }
                return Ok(());
            }
            DeviceKind::Output => return Ok(()),
        };
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
            return Ok(());
        };
        // SAFETY: a class method taking an AVMediaType, returning an NSInteger.
        let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: media] };
        // Not yet asked: opening the device asks.
        if status == DENIED || status == RESTRICTED {
            bail!(
                "macOS has denied {} access, needed for {device}: allow it in System Settings › \
                 Privacy & Security › {pane} for {WHO}",
                pane.to_lowercase()
            );
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::Result;

    use crate::api::types::DeviceKind;

    pub fn check(_: DeviceKind, _: &str) -> Result<()> {
        Ok(())
    }
}

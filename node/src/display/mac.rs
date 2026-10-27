//! macOS screen modes and output window placement (see [`place`]).

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

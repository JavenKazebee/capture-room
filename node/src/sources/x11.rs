//! X11 screens as capture devices. GStreamer has no device provider for
//! `ximagesrc`, so [`register`] adds one: a device per RandR monitor (per X
//! screen without RandR 1.5), in the `Source/Monitor` class Windows' screens
//! have, so the device monitor lists, keys and opens them like any other.
//! Each opens `ximagesrc` cropped to its monitor.
//!
//! Offered only in an X11 session: under Wayland, `ximagesrc` reaches only
//! XWayland windows and captures black. While started, the provider re-reads
//! the monitors every few seconds, as RandR has them change when a screen is
//! plugged in or rearranged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use gstreamer::{self as gst, glib, prelude::*, subclass::prelude::*};
use tracing::warn;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;

const POLL: Duration = Duration::from_secs(2);

/// Rates offered: `ximagesrc` grabs on a timer, so any would do.
const RATES: &[(i32, i32)] = &[
    (60, 1),
    (60000, 1001),
    (50, 1),
    (30, 1),
    (30000, 1001),
    (25, 1),
];

/// One capturable area of the X display.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Screen {
    /// The RandR monitor's name (`DP-1`), which stays the same across
    /// restarts, or `screen-N` for a whole X screen.
    id: String,
    display: String,
    screen_num: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Whether this is an X11 session. `DISPLAY` alone isn't enough: Wayland
/// sessions set it for XWayland.
fn is_x11(session_type: Option<&str>, display: bool, wayland_display: bool) -> bool {
    match session_type {
        Some("x11") => true,
        Some("wayland") => false,
        _ => display && !wayland_display,
    }
}

fn x11_session() -> bool {
    is_x11(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("DISPLAY").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    )
}

/// The screens to offer now.
fn screens() -> Vec<Screen> {
    static UNREACHABLE: AtomicBool = AtomicBool::new(false);
    if !x11_session() || gst::ElementFactory::find("ximagesrc").is_none() {
        return Vec::new();
    }
    let conn = match x11rb::connect(None) {
        Ok((conn, _)) => conn,
        Err(e) => {
            if !UNREACHABLE.swap(true, Ordering::Relaxed) {
                warn!(error = %e, "can't reach the X server to list screens");
            }
            return Vec::new();
        }
    };
    UNREACHABLE.store(false, Ordering::Relaxed);
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let mut out = Vec::new();
    for (num, root) in conn.setup().roots.iter().enumerate() {
        let screen_num = num as u32;
        let monitors = conn
            .randr_get_monitors(root.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors)
            .unwrap_or_default();
        if monitors.is_empty() {
            out.push(Screen {
                id: format!("screen-{num}"),
                display: display.clone(),
                screen_num,
                x: 0,
                y: 0,
                width: root.width_in_pixels.into(),
                height: root.height_in_pixels.into(),
            });
            continue;
        }
        for m in monitors.into_iter().filter(|m| m.width > 0 && m.height > 0) {
            let name = conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_else(|| format!("monitor-{}", m.name));
            out.push(Screen {
                id: if num == 0 {
                    name
                } else {
                    format!("{num}/{name}")
                },
                display: display.clone(),
                screen_num,
                x: m.x.max(0) as u32,
                y: m.y.max(0) as u32,
                width: m.width.into(),
                height: m.height.into(),
            });
        }
    }
    out
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Provider {
        devices: Mutex<Vec<super::ScreenDevice>>,
        running: Mutex<Option<Arc<AtomicBool>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Provider {
        const NAME: &'static str = "CaptureRoomX11ScreenProvider";
        type Type = super::Provider;
        type ParentType = gst::DeviceProvider;
    }

    impl ObjectImpl for Provider {}
    impl GstObjectImpl for Provider {}

    impl DeviceProviderImpl for Provider {
        fn metadata() -> Option<&'static gst::subclass::DeviceProviderMetadata> {
            static METADATA: LazyLock<gst::subclass::DeviceProviderMetadata> =
                LazyLock::new(|| {
                    gst::subclass::DeviceProviderMetadata::new(
                        "X11 screens",
                        "Source/Monitor",
                        "Monitors of the X11 session, captured with ximagesrc",
                        "Capture Room",
                    )
                });
            Some(&*METADATA)
        }

        fn probe(&self) -> Vec<gst::Device> {
            screens()
                .iter()
                .map(|s| super::ScreenDevice::new(s).upcast())
                .collect()
        }

        fn start(&self) -> Result<(), gst::LoggableError> {
            self.update();
            let running = Arc::new(AtomicBool::new(true));
            *self.running.lock().unwrap() = Some(running.clone());
            let provider = self.obj().downgrade();
            std::thread::Builder::new()
                .name("x11-screens".into())
                .spawn(move || loop {
                    std::thread::sleep(POLL);
                    let Some(provider) = provider.upgrade() else {
                        return;
                    };
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                    provider.imp().update();
                })
                .map_err(|e| gst::loggable_error!(gst::CAT_RUST, "{e}"))?;
            Ok(())
        }

        fn stop(&self) {
            if let Some(running) = self.running.lock().unwrap().take() {
                running.store(false, Ordering::Relaxed);
            }
            self.devices.lock().unwrap().clear();
        }
    }

    impl Provider {
        /// Remove screens that are gone or changed, and add new ones.
        fn update(&self) {
            let now = screens();
            let obj = self.obj();
            let mut devices = self.devices.lock().unwrap();
            devices.retain(|d| {
                let keep = now.contains(d.screen());
                if !keep {
                    obj.device_remove(d);
                }
                keep
            });
            for screen in now {
                if !devices.iter().any(|d| *d.screen() == screen) {
                    let device = super::ScreenDevice::new(&screen);
                    obj.device_add(&device);
                    devices.push(device);
                }
            }
        }
    }

    #[derive(Default)]
    pub struct ScreenDevice {
        pub(super) screen: OnceLock<Screen>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ScreenDevice {
        const NAME: &'static str = "CaptureRoomX11Screen";
        type Type = super::ScreenDevice;
        type ParentType = gst::Device;
    }

    impl ObjectImpl for ScreenDevice {}
    impl GstObjectImpl for ScreenDevice {}

    impl DeviceImpl for ScreenDevice {
        fn create_element(&self, name: Option<&str>) -> Result<gst::Element, gst::LoggableError> {
            let s = self.screen.get().expect("set when the device is made");
            let mut builder = gst::ElementFactory::make("ximagesrc")
                .property("screen-num", s.screen_num)
                .property("startx", s.x)
                .property("starty", s.y)
                // Inclusive: the last column and row captured.
                .property("endx", s.x + s.width - 1)
                .property("endy", s.y + s.height - 1)
                // XDamage leaves stale tiles on some drivers; grab whole frames.
                .property("use-damage", false);
            if !s.display.is_empty() {
                builder = builder.property("display-name", &s.display);
            }
            if let Some(name) = name {
                builder = builder.name(name);
            }
            builder
                .build()
                .map_err(|e| gst::loggable_error!(gst::CAT_RUST, "{e}"))
        }
    }
}

glib::wrapper! {
    pub struct Provider(ObjectSubclass<imp::Provider>)
        @extends gst::DeviceProvider, gst::Object;
}

glib::wrapper! {
    pub struct ScreenDevice(ObjectSubclass<imp::ScreenDevice>)
        @extends gst::Device, gst::Object;
}

impl ScreenDevice {
    fn new(screen: &Screen) -> Self {
        let rates = gst::List::new(RATES.iter().map(|&(n, d)| gst::Fraction::new(n, d)));
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRx")
            .field("width", screen.width as i32)
            .field("height", screen.height as i32)
            .field("framerate", rates)
            .build();
        let properties = gst::Structure::builder("properties")
            .field("device.id", format!("x11:{}", screen.id))
            .field("device.api", "x11")
            .build();
        let device: Self = glib::Object::builder()
            .property("caps", caps)
            .property("display-name", &screen.id)
            .property("device-class", "Source/Monitor")
            .property("properties", properties)
            .build();
        let _ = device.imp().screen.set(screen.clone());
        device
    }

    fn screen(&self) -> &Screen {
        self.imp()
            .screen
            .get()
            .expect("set when the device is made")
    }
}

/// Add the provider to GStreamer's registry: after `gst::init`, before the
/// device monitor starts.
pub fn register() -> Result<(), glib::BoolError> {
    gst::DeviceProvider::register(
        None,
        "captureroomx11screens",
        gst::Rank::PRIMARY,
        Provider::static_type(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_x11_session_offers_screens() {
        assert!(is_x11(Some("x11"), true, false));
        assert!(!is_x11(Some("wayland"), true, true));
        // XWayland sets DISPLAY too.
        assert!(!is_x11(Some("wayland"), true, false));
        assert!(!is_x11(None, true, true));
        assert!(is_x11(None, true, false));
        assert!(!is_x11(Some("tty"), false, false));
    }

    #[test]
    fn a_screen_is_listed_as_a_keyed_screen_device() {
        gst::init().unwrap();
        let device = ScreenDevice::new(&Screen {
            id: "DP-1".into(),
            display: ":0".into(),
            screen_num: 0,
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        });
        let device = device.upcast::<gst::Device>();
        assert_eq!(device.device_class(), "Source/Monitor");
        let id = device
            .properties()
            .and_then(|p| p.get::<String>("device.id").ok());
        assert_eq!(id.as_deref(), Some("x11:DP-1"));
        let caps = device.caps().unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.get::<i32>("width").unwrap(), 2560);
        assert_eq!(s.get::<i32>("height").unwrap(), 1440);
    }
}

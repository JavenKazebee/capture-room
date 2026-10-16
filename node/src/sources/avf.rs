//! AVFoundation cameras. `avfvideosrc` opens a camera by its index in the
//! system's list, which shifts when another camera is plugged or unplugged,
//! and `fallbacksrc` restarts the same element. [`CameraBin`] looks its camera
//! up by unique ID each time it starts, so a restart opens the same camera or
//! fails (and `fallbacksrc` retries) while it's gone.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::OnceLock;

use gstreamer::{self as gst, glib, prelude::*, subclass::prelude::*};
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject};
use tracing::{info, warn};

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeVideo: &'static AnyObject;
}

/// The index `avfvideosrc` would open camera `unique_id` at now, if connected.
fn index(unique_id: &str) -> Option<i32> {
    let class = AnyClass::get(c"AVCaptureDevice")?;
    autoreleasepool(|_| {
        // SAFETY: the class method `avfvideosrc` itself lists cameras with
        // (so indices match), then NSArray / NSString accessors on its result.
        unsafe {
            let devices: *mut AnyObject = msg_send![class, devicesWithMediaType: AVMediaTypeVideo];
            if devices.is_null() {
                return None;
            }
            let count: usize = msg_send![devices, count];
            (0..count).find_map(|i| {
                let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];
                let id: *mut AnyObject = msg_send![device, uniqueID];
                if id.is_null() {
                    return None;
                }
                let utf8: *const std::ffi::c_char = msg_send![id, UTF8String];
                let matches = !utf8.is_null()
                    && std::ffi::CStr::from_ptr(utf8).to_bytes() == unique_id.as_bytes();
                matches.then_some(i as i32)
            })
        }
    })
}

mod imp {
    use super::*;

    pub struct Camera {
        pub element: gst::Element,
        pub unique_id: String,
        pub name: String,
    }

    #[derive(Default)]
    pub struct CameraBin {
        pub camera: OnceLock<Camera>,
        /// Last start found the camera missing: warn once, not every retry.
        pub missing: AtomicBool,
        pub index: AtomicI32,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CameraBin {
        const NAME: &'static str = "CaptureRoomCameraBin";
        type Type = super::CameraBin;
        type ParentType = gst::Bin;
    }

    impl ObjectImpl for CameraBin {}
    impl GstObjectImpl for CameraBin {}
    impl BinImpl for CameraBin {}

    impl ElementImpl for CameraBin {
        fn change_state(
            &self,
            transition: gst::StateChange,
        ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
            if transition == gst::StateChange::NullToReady {
                self.resolve()?;
            }
            self.parent_change_state(transition)
        }
    }

    impl CameraBin {
        fn resolve(&self) -> Result<(), gst::StateChangeError> {
            let Some(camera) = self.camera.get() else {
                return Ok(());
            };
            let Some(index) = index(&camera.unique_id) else {
                if !self.missing.swap(true, Ordering::Relaxed) {
                    warn!(camera = %camera.name, "camera isn't connected; retrying");
                }
                return Err(gst::StateChangeError);
            };
            self.missing.store(false, Ordering::Relaxed);
            if self.index.swap(index, Ordering::Relaxed) != index {
                info!(camera = %camera.name, index, "opening camera");
            }
            camera.element.set_property("device-index", index);
            Ok(())
        }
    }
}

glib::wrapper! {
    /// A bin holding `avfvideosrc` that re-resolves its camera on each start.
    pub struct CameraBin(ObjectSubclass<imp::CameraBin>)
        @extends gst::Bin, gst::Element, gst::Object;
}

impl CameraBin {
    /// `element` is the bin's `avfvideosrc`, added by the caller.
    pub fn new(name: &str, element: &gst::Element, unique_id: &str, camera: &str) -> Self {
        let bin: Self = glib::Object::builder().property("name", name).build();
        let imp = bin.imp();
        imp.index
            .store(element.property::<i32>("device-index"), Ordering::Relaxed);
        let _ = imp.camera.set(imp::Camera {
            element: element.clone(),
            unique_id: unique_id.to_string(),
            name: camera.to_string(),
        });
        bin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every listed camera resolves to the index its device provider gave.
    #[test]
    fn index_matches_the_device_provider() {
        gst::init().unwrap();
        let Some(factory) = gst::DeviceProviderFactory::find("avfdeviceprovider") else {
            return;
        };
        let provider = factory.get().unwrap();
        for device in provider.devices() {
            let Some(id) = device
                .properties()
                .and_then(|p| p.get::<String>("avf.unique_id").ok())
            else {
                continue;
            };
            let element = device.create_element(None).unwrap();
            assert_eq!(
                index(&id),
                Some(element.property::<i32>("device-index")),
                "{}",
                device.display_name()
            );
        }
        assert_eq!(index("not-a-camera"), None);
    }

    /// Plays the first camera through `fallbacksrc` until `secs` pass; how
    /// many restarts it took and frames arrived, with `unique_id` swapped in
    /// for the camera's own when given.
    fn play(unique_id: Option<&str>, secs: u64) -> (u32, u32) {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;

        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let provider = gst::DeviceProviderFactory::find("avfdeviceprovider")
            .unwrap()
            .get()
            .unwrap();
        let device = provider.devices().into_iter().next().expect("a camera");
        let own = device
            .properties()
            .and_then(|p| p.get::<String>("avf.unique_id").ok())
            .unwrap();
        let element = device.create_element(None).unwrap();
        let bin = CameraBin::new("cam", &element, unique_id.unwrap_or(&own), "test");
        let caps = crate::pipeline::capsfilter("caps", gst::Caps::new_empty_simple("video/x-raw"))
            .unwrap();
        bin.add_many([&element, &caps]).unwrap();
        element.link(&caps).unwrap();
        let pad = gst::GhostPad::with_target(&caps.static_pad("src").unwrap()).unwrap();
        bin.add_pad(&pad).unwrap();

        let pipeline = gst::Pipeline::new();
        let fb = super::super::live::fallback("fb", true, false).unwrap();
        fb.set_property("source", &bin);
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        pipeline.add_many([&fb, &sink]).unwrap();
        let frames = Arc::new(AtomicU32::new(0));
        let counted = frames.clone();
        let sink_pad = sink.static_pad("sink").unwrap();
        fb.connect_pad_added(move |_, pad| {
            if pad.name().starts_with("video") {
                pad.link(&sink_pad).unwrap();
            }
        });
        // Frames from the camera, not the fallback's black.
        caps.static_pad("src")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                counted.fetch_add(1, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            });
        pipeline.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let stats = fb.property::<gst::Structure>("statistics");
        pipeline.set_state(gst::State::Null).unwrap();
        let retries = stats.get::<u64>("num-retry").unwrap_or(0) as u32;
        (retries, frames.load(Ordering::Relaxed))
    }

    #[test]
    #[ignore = "opens a camera (needs camera access); run with --ignored"]
    fn camera_plays_through_fallbacksrc() {
        use crate::sources::privacy::{access, Access, Permission};
        assert_ne!(
            access(Permission::Camera),
            Some(Access::Denied),
            "macOS denies this process camera access"
        );
        let (_, frames) = play(None, 3);
        assert!(frames > 10, "{frames} frames");
    }

    /// A camera that isn't connected fails to start, and `fallbacksrc` keeps
    /// retrying on black rather than opening whatever has its index.
    #[test]
    #[ignore = "real-time (~8 s); run with --ignored"]
    fn missing_camera_retries() {
        let (retries, frames) = play(Some("not-a-camera"), 8);
        assert_eq!(frames, 0);
        assert!(retries >= 1, "{retries} retries");
    }
}

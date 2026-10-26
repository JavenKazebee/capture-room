//! RTSP outputs, each a mount at `rtsp://<node>:8554/<path>`. A mount builds
//! one shared encode when the first viewer connects and drops it when the
//! last leaves:
//!
//! ```text
//! appsrc(video) → videoconvert → H.264 → h264parse → rtph264pay (pay0)
//! appsrc(audio) → audioconvert → audioresample → AAC → rtpmp4gpay (pay1)
//! ```
//!
//! The server runs on its own GLib main loop, started with the first RTSP
//! output and kept for the life of the process.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_rtsp_server::{self as rtsp, prelude::*};
use gstreamer_utils::StreamProducer;
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::output::{self, OutputSink, Program, RunningOutput};

pub const RTSP_PORT: u16 = 8554;

/// Elements an RTSP output needs, beyond the server library (and one of the
/// H.264 encoders).
pub const RTSP_ELEMENTS: &[&str] = &["h264parse", "rtph264pay", "avenc_aac", "rtpmp4gpay"];

static SERVER: Mutex<Option<rtsp::RTSPServer>> = Mutex::new(None);

fn server() -> Result<rtsp::RTSPServer> {
    let mut slot = SERVER.lock().unwrap();
    if let Some(server) = slot.as_ref() {
        return Ok(server.clone());
    }
    let context = glib::MainContext::new();
    let server = rtsp::RTSPServer::new();
    server.set_service(&RTSP_PORT.to_string());
    server.attach(Some(&context)).map_err(|_| {
        anyhow!("couldn't listen on port {RTSP_PORT} (is something else using it?)")
    })?;
    // Sessions whose viewer went away without a TEARDOWN time out here.
    let pool = server
        .session_pool()
        .context("RTSP server has no session pool")?;
    glib::source::timeout_source_new(
        Duration::from_secs(2),
        Some("rtsp-session-cleanup"),
        glib::Priority::DEFAULT,
        move || {
            pool.cleanup();
            glib::ControlFlow::Continue
        },
    )
    .attach(Some(&context));
    std::thread::Builder::new()
        .name("rtsp-server".into())
        .spawn(move || glib::MainLoop::new(Some(&context), false).run())
        .context("start the RTSP server thread")?;
    info!(port = RTSP_PORT, "RTSP server listening");
    *slot = Some(server.clone());
    Ok(server)
}

/// `path`, or the channel's name, made URL-safe (lowercase alphanumerics,
/// `-`, `_`, `.`, and `/` between segments).
pub fn mount_path(path: Option<&str>, channel_name: &str) -> String {
    let wanted = path
        .map(str::trim)
        .filter(|p| !p.trim_matches('/').is_empty())
        .unwrap_or(channel_name);
    let segments: Vec<String> = wanted
        .split('/')
        .map(|segment| {
            let mut out = String::new();
            for c in segment.trim().chars().flat_map(char::to_lowercase) {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') {
                    out.push(c);
                } else if !out.ends_with('-') {
                    out.push('-');
                }
            }
            out.trim_matches('-').to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        "/channel".into()
    } else {
        format!("/{}", segments.join("/"))
    }
}

pub struct RtspOutput {
    pub path: String,
    pub bitrate_kbps: u32,
    pub program: Program,
}

impl RtspOutput {
    /// The appsrcs are connected to the monitor in `media-configure`.
    fn launch(&self) -> Result<String> {
        let (encoder, props) = output::live_h264(self.bitrate_kbps, self.program)?;
        let props: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Ok(format!(
            "( appsrc name=video-src ! videoconvert ! {encoder} {props} \
               ! h264parse config-interval=-1 ! rtph264pay name=pay0 pt=96 config-interval=-1 \
               appsrc name=audio-src ! audioconvert ! audioresample \
               ! avenc_aac bitrate={abr} ! rtpmp4gpay name=pay1 pt=97 )",
            props = props.join(" "),
            abr = output::aac_bitrate(self.program.audio_channels),
        ))
    }
}

impl OutputSink for RtspOutput {
    fn label(&self) -> String {
        format!("RTSP {}", self.path)
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        let server = server()?;
        let mounts = server
            .mount_points()
            .context("RTSP server has no mount points")?;
        let factory = rtsp::RTSPMediaFactory::new();
        factory.set_launch(&self.launch()?);
        // One encode for every viewer, stopped when the last one leaves.
        factory.set_shared(true);

        let media: Arc<Mutex<Vec<glib::WeakRef<rtsp::RTSPMedia>>>> = Arc::default();
        let monitor_ref = Arc::downgrade(monitor);
        let media_ref = media.clone();
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let error_ref = error.clone();
        let path = self.path.clone();
        factory.connect_media_configure(move |_, m| {
            if !note_feed(&error_ref, &path, connect_media(m, &monitor_ref)) {
                return;
            }
            let mut media = media_ref.lock().unwrap();
            media.retain(|m| m.upgrade().is_some());
            media.push(m.downgrade());
        });
        mounts.add_factory(&self.path, factory);
        Ok(Box::new(RtspMount {
            server,
            path: self.path.clone(),
            media,
            error,
        }))
    }
}

/// Feed a new media's appsrcs from the monitor, on its clock and base time,
/// until the media is unprepared.
fn connect_media(media: &rtsp::RTSPMedia, monitor: &Weak<MonitorPipeline>) -> Result<()> {
    let monitor = monitor.upgrade().context("the channel stopped")?;
    let bin = media
        .element()
        .downcast::<gst::Bin>()
        .map_err(|_| anyhow!("RTSP media isn't a bin"))?;
    if let (Some(clock), Some(_)) = monitor.timing() {
        media.set_clock(Some(&clock));
        if let Some(pipeline) = bin
            .parent()
            .and_then(|p| p.downcast::<gst::Pipeline>().ok())
        {
            monitor.share_timing(&pipeline);
        }
    }
    let appsrc = |name: &str| -> Result<gst_app::AppSrc> {
        let src = bin
            .by_name(name)
            .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            .with_context(|| format!("no {name} in the RTSP media"))?;
        StreamProducer::configure_consumer(&src);
        Ok(src)
    };
    let links = monitor.feed(&appsrc("video-src")?, &appsrc("audio-src")?)?;
    let links = Mutex::new(Some(links));
    media.connect_unprepared(move |_| {
        links.lock().unwrap().take();
    });
    Ok(())
}

/// Keep why `fed` failed in `error`, or clear it: whether it succeeded.
fn note_feed(error: &Mutex<Option<String>>, path: &str, fed: Result<()>) -> bool {
    match fed {
        Ok(()) => {
            error.lock().unwrap().take();
            true
        }
        Err(e) => {
            let e = format!("{e:#}");
            warn!(path = %path, error = %e, "RTSP viewer couldn't be fed");
            *error.lock().unwrap() = Some(format!("viewers get nothing: {e}"));
            false
        }
    }
}

/// Dropping it unmounts and stops its viewers' streams.
struct RtspMount {
    server: rtsp::RTSPServer,
    path: String,
    media: Arc<Mutex<Vec<glib::WeakRef<rtsp::RTSPMedia>>>>,
    /// Why the last media couldn't be fed from the monitor, until one is.
    error: Arc<Mutex<Option<String>>>,
}

impl RunningOutput for RtspMount {
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    fn receivers(&self) -> Option<u32> {
        let pool = self.server.session_pool()?;
        let sessions = pool.filter(None);
        Some(
            sessions
                .iter()
                .filter(|s| s.media(&self.path).0.is_some())
                .count() as u32,
        )
    }
}

impl Drop for RtspMount {
    fn drop(&mut self) {
        if let Some(mounts) = self.server.mount_points() {
            mounts.remove_factory(&self.path);
        }
        for media in self.media.lock().unwrap().drain(..) {
            if let Some(media) = media.upgrade() {
                let _ = media.unprepare();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_paths_are_url_safe() {
        assert_eq!(mount_path(None, "Studio A"), "/studio-a");
        assert_eq!(mount_path(Some(""), "Studio A"), "/studio-a");
        assert_eq!(
            mount_path(Some("  /live/Main Feed/ "), "x"),
            "/live/main-feed"
        );
        assert_eq!(mount_path(Some("cam_1.hd"), "x"), "/cam_1.hd");
        assert_eq!(mount_path(Some("Café!!"), "x"), "/caf");
        assert_eq!(mount_path(None, "!!!"), "/channel");
    }

    #[test]
    fn feed_errors_last_until_a_feed_works() {
        let error = Mutex::new(None);
        assert!(!note_feed(
            &error,
            "/x",
            Err(anyhow!("the channel stopped"))
        ));
        assert_eq!(
            error.lock().unwrap().as_deref(),
            Some("viewers get nothing: the channel stopped")
        );
        assert!(note_feed(&error, "/x", Ok(())));
        assert_eq!(*error.lock().unwrap(), None);
    }
}

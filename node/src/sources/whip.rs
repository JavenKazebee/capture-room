use std::sync::Arc;

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};

use super::device::LocalDevices;
use super::live::{self, LinkTracker, LiveInput};
use super::InputSource;
use crate::api::types::{LinkState, SourceCapabilitiesDto, SourceType, WhipSourceConfig};
use crate::pipeline::make_el;

/// A WHIP ingest: `whipserversrc` (gst-plugins-rs, statically linked)
/// serves `http://<node>:<port>/whip/endpoint` and decodes what's published. While nobody
/// publishes, the feed plays black and silence.
pub struct WhipSource {
    id: String,
    name: String,
    config: WhipSourceConfig,
    devices: Arc<LocalDevices>,
    tracker: Arc<LinkTracker>,
}

impl WhipSource {
    pub fn new(
        id: String,
        name: String,
        config: WhipSourceConfig,
        devices: Arc<LocalDevices>,
    ) -> Self {
        Self {
            id,
            name,
            config,
            devices,
            tracker: Arc::default(),
        }
    }
}

impl InputSource for WhipSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Whip
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        None
    }

    fn fingerprint(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_default()
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let cfg = &self.config;
        let server = make_el("whipserversrc", &format!("whip-{}", self.id))?;
        let signaller = server.property::<glib::Object>("signaller");
        signaller.set_property("host-addr", format!("http://0.0.0.0:{}", cfg.port));
        // LAN ingest: don't contact a public STUN server.
        signaller.set_property("stun-server", None::<String>);
        let bin = live::build_bin(LiveInput {
            id: &self.id,
            source: server,
            audio: self.devices.audio(&cfg.audio).context("audio")?,
            format: cfg.format.as_ref(),
            tracker: &self.tracker,
            direct: true,
        })?;
        Ok(bin.upcast())
    }

    fn timecode(&self) -> Option<String> {
        None
    }

    fn link(&self) -> Option<LinkState> {
        Some(self.tracker.state(true))
    }
}

/// Check a config before it's saved.
pub fn validate(cfg: &WhipSourceConfig) -> Result<(), String> {
    if cfg.port < 1024 {
        return Err("use a port from 1024 up".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::types::{AudioPlan, MonitorSettingsDto};
    use crate::pipeline::monitor::MonitorPipeline;

    /// `whipclientsink` blocks on its own runtime while changing state.
    fn off_runtime(p: &gst::Element, state: gst::State) {
        std::thread::scope(|s| {
            s.spawn(|| p.set_state(state).unwrap());
        });
    }

    /// Publish with `whipclientsink` (the same plugin), stop, and publish
    /// again: the source goes live, waits, and goes live again.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~25 s); run with --ignored"]
    async fn publish_stop_publish() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        gstrswebrtc::plugin_register_static().unwrap();
        let port = 7743;
        let src = WhipSource::new(
            "w".into(),
            "w".into(),
            WhipSourceConfig {
                port,
                audio: AudioPlan::default(),
                format: None,
            },
            Arc::new(LocalDevices::start()),
        );
        let mon = MonitorPipeline::new(
            &src,
            &MonitorSettingsDto::default(),
            &gst::SystemClock::obtain(),
        )
        .unwrap();
        let publish = || {
            let p = gst::parse::launch(&format!(
                "videotestsrc is-live=1 pattern=ball ! video/x-raw,width=640,height=360,framerate=30/1 ! queue ! ws. \
                 audiotestsrc is-live=1 ! queue ! ws. \
                 whipclientsink name=ws signaller::whip-endpoint=http://127.0.0.1:{port}/whip/endpoint"
            ))
            .unwrap();
            off_runtime(&p, gst::State::Playing);
            p
        };
        let sleep = |s| tokio::time::sleep(Duration::from_secs(s));

        sleep(2).await;
        assert_eq!(src.link(), Some(LinkState::Waiting));
        let p = publish();
        sleep(8).await;
        assert_eq!(src.link(), Some(LinkState::Live), "{:?}", mon.error());
        off_runtime(&p, gst::State::Null);
        drop(p);
        sleep(5).await;
        assert_eq!(src.link(), Some(LinkState::Waiting));
        let p = publish();
        sleep(10).await;
        assert_eq!(src.link(), Some(LinkState::Live), "{:?}", mon.error());
        off_runtime(&p, gst::State::Null);
        mon.stop().unwrap();
    }
}

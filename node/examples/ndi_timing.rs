//! How closely two NDI receivers of one sender agree on when each frame
//! happened, per `ndisrc` timestamp mode: what a capture node and a replay node
//! would each stamp the same camera frame with. Sends a test pattern over NDI,
//! receives it twice (the second receiver joining `JOIN_LATE` after the first),
//! and matches frames by the sender's NDI timestamp.
//!
//! `cargo run -p capture-room --release --example ndi_timing` (needs the NDI
//! Runtime: `NDI_RUNTIME_DIR_V6`, else where NDI Tools puts it on macOS).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;

/// Where NDI Tools installs the library on macOS.
const NDI_TOOLS_DIR: &str =
    "/Library/CoreMediaIO/Plug-Ins/DAL/NDIVideoOut.plugin/Contents/Frameworks";
const SENDER: &str = "capture-room timing";
const JOIN_LATE: Duration = Duration::from_secs(5);
const RUN: Duration = Duration::from_secs(20);
/// Within this of the first receiver, the late one has settled.
const SETTLED_MS: f64 = 2.0;
const MODES: [&str; 4] = [
    "auto",
    "receive-time",
    "receive-time-vs-timestamp",
    "timestamp",
];

/// Each frame's capture time on the clock, by the sender's NDI timestamp.
type Frames = Arc<Mutex<HashMap<u64, u64>>>;

fn main() {
    if std::env::var_os("NDI_RUNTIME_DIR_V6").is_none() {
        std::env::set_var("NDI_RUNTIME_DIR_V6", NDI_TOOLS_DIR);
    }
    gst::init().unwrap();
    gstndi::plugin_register_static().unwrap();
    let clock = gst::SystemClock::obtain();

    let sender = gst::parse::launch(&format!(
        "ndisinkcombiner name=c ! ndisink ndi-name=\"{SENDER}\" \
         videotestsrc is-live=true pattern=ball ! video/x-raw,format=UYVY,width=1280,height=720,framerate=30/1 ! c.video \
         audiotestsrc is-live=true ! audio/x-raw,format=F32LE,channels=2,rate=48000 ! c.audio"
    ))
    .unwrap();
    sender.set_state(gst::State::Playing).unwrap();
    let name = find_sender().expect("the test sender never showed up on NDI");
    println!("sender: {name}\n");
    println!(
        "{:<26} {:>7} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "mode", "frames", "mean ms", "min ms", "max ms", "drift ms", "settled s"
    );

    for mode in MODES {
        let a = Frames::default();
        let b = Frames::default();
        let first = receiver(&name, mode, &clock, &a);
        first.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(JOIN_LATE);
        let second = receiver(&name, mode, &clock, &b);
        second.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(RUN);
        first.set_state(gst::State::Null).unwrap();
        second.set_state(gst::State::Null).unwrap();
        report(mode, &a.lock().unwrap(), &b.lock().unwrap());
    }
    sender.set_state(gst::State::Null).unwrap();
}

/// The test sender's full NDI name (`HOST (name)`), once it's found.
fn find_sender() -> Option<String> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Source/Audio/Video/Network"), None);
    monitor.start().ok()?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        for device in monitor.devices() {
            let name: Option<String> = device.properties().and_then(|p| p.get("ndi-name").ok());
            if let Some(name) = name.filter(|n| n.ends_with(&format!("({SENDER})"))) {
                monitor.stop();
                return Some(name);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    monitor.stop();
    None
}

/// A receiver like a node's NDI source, on `clock`, noting each video frame's
/// capture time (running time plus base time, as a node maps it).
fn receiver(ndi_name: &str, mode: &str, clock: &gst::Clock, frames: &Frames) -> gst::Pipeline {
    let pipeline = gst::parse::launch(&format!(
        "ndisrc ndi-name=\"{ndi_name}\" timestamp-mode={mode} ! ndisrcdemux name=d \
         d.video ! queue ! appsink name=sink sync=false"
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    pipeline.use_clock(Some(clock));
    let sink = pipeline
        .by_name("sink")
        .unwrap()
        .downcast::<gst_app::AppSink>()
        .unwrap();
    let ndi_timestamp = gst::Caps::new_empty_simple("timestamp/x-ndi-timestamp");
    let frames = Arc::clone(frames);
    let weak = pipeline.downgrade();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let (Some(buffer), Some(segment), Some(pipeline)) =
                    (sample.buffer(), sample.segment(), weak.upgrade())
                else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let key = buffer
                    .iter_meta::<gst::ReferenceTimestampMeta>()
                    .find(|m| m.reference().can_intersect(&ndi_timestamp))
                    .map(|m| m.timestamp().nseconds());
                let running = buffer.pts().and_then(|pts| {
                    segment
                        .downcast_ref::<gst::ClockTime>()?
                        .to_running_time(pts)
                });
                if let (Some(key), Some(running), Some(base)) = (key, running, pipeline.base_time())
                {
                    frames
                        .lock()
                        .unwrap()
                        .insert(key, (running + base).nseconds());
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline
}

/// How far the late receiver's time for each frame is from the first's.
fn report(mode: &str, a: &HashMap<u64, u64>, b: &HashMap<u64, u64>) {
    let mut diffs: Vec<(u64, f64)> = b
        .iter()
        .filter_map(|(key, tb)| Some((*key, (*tb as f64 - *a.get(key)? as f64) / 1e6)))
        .collect();
    diffs.sort_by_key(|(key, _)| *key);
    if diffs.is_empty() {
        println!("{mode:<26} no frames matched");
        return;
    }
    let ms: Vec<f64> = diffs.iter().map(|(_, d)| *d).collect();
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    let min = ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = ms.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // The last second's mean against the first second's (30 frames each).
    let n = ms.len().min(30);
    let avg = |s: &[f64]| s.iter().sum::<f64>() / s.len() as f64;
    let drift = avg(&ms[ms.len() - n..]) - avg(&ms[..n]);
    // From the late receiver's first frame until it stays within SETTLED.
    let settled = ms
        .iter()
        .rposition(|d| d.abs() > SETTLED_MS)
        .map_or(0.0, |i| (diffs[i].0 - diffs[0].0) as f64 / 1e9);
    println!(
        "{mode:<26} {:>7} {mean:>10.2} {min:>10.2} {max:>10.2} {drift:>10.2} {settled:>10.1}",
        ms.len()
    );
}

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Instant;

use super::lanes::Pushed;
use super::validate::mount_paths;
use super::*;
use crate::api::types::{ClipEnd, MonitorSettingsDto, TransportState};
use crate::pipeline::monitor::MonitorPipeline;
use crate::pipeline::output;
use crate::sources::player::Kind;

const W: u32 = 1920;
const H: u32 = 1080;

fn with_outputs(outputs: Vec<OutputConfig>) -> ChannelConfig {
    ChannelConfig {
        format: LiveVideoFormat {
            width: W,
            height: H,
            fps_num: 30,
            fps_den: 1,
        },
        audio_channels: 2,
        outputs,
    }
}

fn srt(url: &str) -> OutputConfig {
    OutputConfig::Srt {
        url: url.into(),
        latency_ms: 200,
        bitrate_kbps: 8000,
        passphrase: None,
    }
}

fn check(cfg: &ChannelConfig) -> Result<(), String> {
    validate(cfg, "c", &[], |_| true)
}

#[test]
fn srt_outputs_are_validated() {
    assert!(check(&with_outputs(vec![srt("srt://:9000")])).is_ok());
    assert!(check(&with_outputs(vec![srt("srt://10.0.0.5:9000")])).is_ok());
    assert!(check(&with_outputs(vec![srt("srt://0.0.0.0:9000?mode=listener")])).is_ok());
    // Not SRT, no port.
    assert!(check(&with_outputs(vec![srt("rtsp://:9000")])).is_err());
    assert!(check(&with_outputs(vec![srt("srt://10.0.0.5")])).is_err());
    assert!(check(&with_outputs(vec![srt("srt://")])).is_err());
    // Two listeners on one port.
    let err = check(&with_outputs(vec![srt("srt://:9000"), srt("srt://:9000")])).unwrap_err();
    assert!(err.contains("9000"), "{err}");
    // Two callers to the same place are fine.
    assert!(check(&with_outputs(vec![
        srt("srt://h:9000"),
        srt("srt://h:9000")
    ]))
    .is_ok());

    let with = |passphrase: &str| {
        with_outputs(vec![OutputConfig::Srt {
            url: "srt://:9000".into(),
            latency_ms: 200,
            bitrate_kbps: 8000,
            passphrase: Some(passphrase.into()),
        }])
    };
    assert!(check(&with("")).is_ok());
    assert!(check(&with("short")).is_err());
    assert!(check(&with("long enough")).is_ok());

    let mut surround = with_outputs(vec![srt("srt://:9000")]);
    surround.audio_channels = 16;
    assert!(check(&surround).is_err());
}

#[test]
fn outputs_need_their_elements() {
    let cfg = with_outputs(vec![srt("srt://:9000")]);
    let err = validate(&cfg, "c", &[], |e| e != "srtsink").unwrap_err();
    assert!(err.contains("srtsink"), "{err}");
    let no_h264 = |e: &str| !output::LIVE_H264.contains(&e);
    assert!(validate(&cfg, "c", &[], no_h264).is_err());
    let ndi = with_outputs(vec![OutputConfig::Ndi { ndi_name: None }]);
    assert!(validate(&ndi, "c", &[], |e| e != "ndisink").is_err());
    // An SRT-only channel doesn't need NDI.
    assert!(validate(&cfg, "c", &[], |e| e != "ndisink").is_ok());
}

fn rtsp(path: Option<&str>) -> OutputConfig {
    OutputConfig::Rtsp {
        path: path.map(Into::into),
        bitrate_kbps: 8000,
    }
}

#[test]
fn rtsp_paths_are_unique() {
    // The default path is the channel's name.
    let a = with_outputs(vec![rtsp(None)]);
    assert_eq!(mount_paths(&a, "Studio A"), vec!["/studio-a"]);
    assert!(validate(&a, "Studio A", &[], |_| true).is_ok());
    let err = validate(&a, "studio a", &[("Studio A", &a)], |_| true).unwrap_err();
    assert!(
        err.contains("/studio-a") && err.contains("Studio A"),
        "{err}"
    );
    assert!(validate(&a, "Studio B", &[("Studio A", &a)], |_| true).is_ok());
    let twice = with_outputs(vec![rtsp(None), rtsp(Some("C"))]);
    assert!(validate(&twice, "c", &[], |_| true).is_err());
    let err = validate(&a, "c", &[], |e| e != "rtph264pay").unwrap_err();
    assert!(err.contains("rtph264pay"), "{err}");
}

#[test]
fn listen_ports_are_srt_listeners() {
    let cfg = with_outputs(vec![
        srt("srt://:9000"),
        srt("srt://h:9001"),
        OutputConfig::Ndi { ndi_name: None },
        rtsp(None),
    ]);
    assert_eq!(listen_ports(&cfg), vec![9000]);
}

/// A 5 s 720p30 clip: colour bars and a tone.
fn make_clip(path: &std::path::Path) {
    let p = gst::parse::launch(&format!(
        "videotestsrc num-buffers=150 pattern=smpte ! video/x-raw,width=1280,height=720,framerate=30/1 \
         ! x264enc key-int-max=30 ! mux. \
         audiotestsrc num-buffers=235 samplesperbuffer=1024 ! audio/x-raw,rate=48000,channels=2 \
         ! avenc_aac ! aacparse ! mux. \
         mp4mux name=mux ! filesink location={}",
        path.display()
    ))
    .unwrap();
    p.set_state(gst::State::Playing).unwrap();
    let bus = p.bus().unwrap();
    let msg = bus
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .expect("clip written");
    assert_eq!(msg.type_(), gst::MessageType::Eos, "{msg:?}");
    p.set_state(gst::State::Null).unwrap();
}

/// What the program's video producer delivers.
#[derive(Default)]
struct Program {
    /// Mean luma of the latest frame.
    luma: Mutex<f64>,
    last: Mutex<Option<Instant>>,
    stalls: Mutex<Vec<Duration>>,
}

fn watch_program(mon: &MonitorPipeline) -> Arc<Program> {
    let program = Arc::new(Program::default());
    let p = program.clone();
    mon.video.appsink().static_pad("sink").unwrap().add_probe(
        gst::PadProbeType::BUFFER,
        move |_, info| {
            let now = Instant::now();
            if let Some(prev) = p.last.lock().unwrap().replace(now) {
                if now - prev > Duration::from_millis(150) {
                    p.stalls.lock().unwrap().push(now - prev);
                }
            }
            if let Some(gst::PadProbeData::Buffer(b)) = &info.data {
                let map = b.map_readable().unwrap();
                let y = &map[..(W * H) as usize];
                let sum: u64 = y.iter().step_by(97).map(|&v| v as u64).sum();
                *p.luma.lock().unwrap() = sum as f64 / y.iter().step_by(97).count() as f64;
            }
            gst::PadProbeReturn::Ok
        },
    );
    program
}

fn clip(path: &std::path::Path, in_s: f64, out_s: Option<f64>, end: ClipEnd) -> ClipRequest {
    let t = |s: f64| gst::ClockTime::from_nseconds((s * 1e9) as u64);
    ClipRequest {
        media_id: "m".into(),
        name: "clip".into(),
        path: path.to_string_lossy().into(),
        in_point: t(in_s),
        out_point: out_s.map(t),
        start: None,
        end,
        rate: 1.0,
        replay: None,
    }
}

fn take_pushed(playout: &Playout) -> Vec<Pushed> {
    std::mem::take(&mut playout.lanes.inner.lock().unwrap().pushed)
}

/// Distinct program − clip offsets of played (not preroll) buffers.
fn offsets(pushed: &[Pushed], kind: Kind) -> BTreeSet<i128> {
    pushed
        .iter()
        .filter(|p| p.kind == kind && !p.preroll)
        .map(|p| p.program.nseconds() as i128 - p.clip.nseconds() as i128)
        .collect()
}

/// The whole transport on a running program (cue, play, pause, resume, stop,
/// loop, end): what the program shows, and producers never stall.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~15 s); run with --ignored"]
async fn transport_on_a_running_program() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join(format!("cr-channel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.mp4");
    make_clip(&file);

    let src = ChannelSource::new(
        "c".into(),
        "c".into(),
        ChannelConfig {
            format: LiveVideoFormat {
                width: W,
                height: H,
                fps_num: 30,
                fps_den: 1,
            },
            audio_channels: 2,
            outputs: vec![],
        },
    );
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(
        &src,
        &MonitorSettingsDto::default(),
        &gst::SystemClock::obtain(),
    )
    .unwrap();
    let program = watch_program(&mon);
    let luma = || *program.luma.lock().unwrap();
    let pos = || playout.status().position.unwrap().mseconds();
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    // Black is Y=16; bars average far above.
    let black = |l: f64| (l - 16.0).abs() < 2.0;

    sleep(1000).await;
    assert!(black(luma()), "idle is black: {}", luma());
    assert_eq!(playout.status().state, TransportState::Idle);

    // Cue: the first frame shows, and holds.
    playout
        .load(clip(&file, 0.0, None, ClipEnd::Hold), false)
        .await
        .unwrap();
    assert_eq!(playout.status().state, TransportState::Cued);
    sleep(500).await;
    assert!(!black(luma()), "cued frame shows: {}", luma());
    assert!(pos() < 40, "cued at the start: {}", pos());

    // Play, pause, resume.
    playout.play().await.unwrap();
    sleep(1500).await;
    let p1 = pos();
    assert!((1300..1700).contains(&p1), "plays in real time: {p1}");
    playout.pause().await.unwrap();
    let paused_at = pos();
    sleep(1000).await;
    assert_eq!(playout.status().state, TransportState::Paused);
    assert!(pos().abs_diff(paused_at) < 40, "pause holds position");
    assert!(!black(luma()), "pause holds the frame");
    playout.play().await.unwrap();
    sleep(1000).await;
    let pushed = take_pushed(&playout);
    let (v, a) = (offsets(&pushed, Kind::Video), offsets(&pushed, Kind::Audio));
    println!("video offsets {v:?}\naudio offsets {a:?}");
    assert_eq!(v.len(), 2, "one offset per play stretch: {v:?}");
    assert_eq!(a.len(), 2, "one offset per play stretch: {a:?}");
    // File timestamps can differ between streams (an edit list): a pause must
    // move both alike.
    let drift: Vec<i128> = v.iter().zip(&a).map(|(v, a)| v - a).collect();
    assert_eq!(drift[0], drift[1], "A/V drift across the pause");

    // Stop: back to black.
    playout.stop().await;
    sleep(500).await;
    assert!(black(luma()), "stop goes to black: {}", luma());
    assert_eq!(playout.status().state, TransportState::Idle);

    // Loop 1 s..2 s: accurate and gapless.
    playout
        .load(clip(&file, 1.0, Some(2.0), ClipEnd::Loop), false)
        .await
        .unwrap();
    take_pushed(&playout);
    playout.play().await.unwrap();
    sleep(3500).await;
    let pushed: Vec<_> = take_pushed(&playout)
        .into_iter()
        .filter(|p| p.kind == Kind::Video && !p.preroll)
        .collect();
    let first = pushed.first().unwrap().clip.mseconds();
    assert!(
        (1000..1034).contains(&first),
        "starts at the in point: {first}"
    );
    let loops = pushed.windows(2).filter(|w| w[1].clip < w[0].clip).count();
    assert!(loops >= 2, "looped: {loops}");
    for w in pushed.windows(2) {
        let gap = w[1].program.saturating_sub(w[0].program).mseconds();
        assert!(
            (30..=40).contains(&gap),
            "gap {gap} ms at clip {}",
            w[1].clip
        );
        assert!(
            (1000..2000).contains(&w[1].clip.mseconds()),
            "within in/out: {}",
            w[1].clip
        );
    }

    // End to black.
    playout
        .load(clip(&file, 4.0, None, ClipEnd::Black), false)
        .await
        .unwrap();
    playout.play().await.unwrap();
    sleep(2000).await;
    assert_eq!(playout.status().state, TransportState::Ended);
    assert!(black(luma()), "ends in black: {}", luma());

    // Replay from the in point after the end.
    playout.play().await.unwrap();
    sleep(500).await;
    assert_eq!(playout.status().state, TransportState::Playing);
    assert!(!black(luma()), "replays: {}", luma());

    mon.stop().unwrap();
    assert_eq!(
        *program.stalls.lock().unwrap(),
        Vec::<std::time::Duration>::new(),
        "program stalls"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn channel() -> ChannelSource {
    channel_with_audio(2)
}

fn channel_with_audio(audio_channels: u32) -> ChannelSource {
    ChannelSource::new(
        "c".into(),
        "c".into(),
        ChannelConfig {
            format: LiveVideoFormat {
                width: W,
                height: H,
                fps_num: 30,
                fps_den: 1,
            },
            audio_channels,
            outputs: vec![],
        },
    )
}

fn item(id: &str, clip: ClipRequest) -> PlaylistItem {
    PlaylistItem {
        id: id.into(),
        clip,
    }
}

/// Played (not preroll) buffers of `kind`.
fn played(pushed: &[Pushed], kind: Kind) -> Vec<Pushed> {
    pushed
        .iter()
        .filter(|p| p.kind == kind && !p.preroll)
        .copied()
        .collect()
}

/// A playlist plays through gaplessly: each handover lands on the next frame
/// at the next item's in point, audio included; taking the next item cuts cleanly.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~10 s); run with --ignored"]
async fn playlist_hands_over_without_a_gap() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join(format!("cr-playlist-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.mp4");
    make_clip(&file);

    let src = channel();
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(
        &src,
        &MonitorSettingsDto::default(),
        &gst::SystemClock::obtain(),
    )
    .unwrap();
    let program = watch_program(&mon);
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    sleep(500).await;

    // Auto-advance: 0–1 s, 0.5–1.5 s, 3–4 s, then hold.
    playout
        .set_playlist(
            vec![
                item("a", clip(&file, 0.0, Some(1.0), ClipEnd::Next)),
                item("b", clip(&file, 0.5, Some(1.5), ClipEnd::Next)),
                item("c", clip(&file, 3.0, Some(4.0), ClipEnd::Next)),
            ],
            false,
        )
        .await;
    playout.cue_item("a").await.unwrap();
    // The next item is cued in the background.
    sleep(500).await;
    let status = playout.status();
    assert_eq!(status.next_id.as_deref(), Some("b"));
    assert!(status.next_ready, "b cued in standby");
    take_pushed(&playout);
    playout.play().await.unwrap();
    sleep(3600).await;

    let status = playout.status();
    assert_eq!(status.state, TransportState::Ended, "holds after the last");
    assert_eq!(status.item_id.as_deref(), Some("c"));
    let pushed = take_pushed(&playout);
    let video = played(&pushed, Kind::Video);
    let starts: Vec<u64> = std::iter::once(&video[0])
        .chain(
            video
                .windows(2)
                .filter(|w| w[1].clip.mseconds().abs_diff(w[0].clip.mseconds()) > 40)
                .map(|w| &w[1]),
        )
        .map(|p| p.clip.mseconds())
        .collect();
    println!("item starts {starts:?}");
    assert_eq!(starts.len(), 3, "three items: {starts:?}");
    for (start, want) in starts.iter().zip([0, 500, 3000]) {
        assert!(
            (want..want + 34).contains(start),
            "starts at its in point: {start} vs {want}"
        );
    }
    for w in video.windows(2) {
        let gap = w[1].program.saturating_sub(w[0].program).mseconds();
        assert!(
            (30..=40).contains(&gap),
            "video gap {gap} ms at clip {}",
            w[1].clip
        );
    }
    let audio = played(&pushed, Kind::Audio);
    for w in audio.windows(2) {
        let gap = w[1].program.saturating_sub(w[0].program).mseconds();
        assert!(gap <= 25, "audio gap {gap} ms at clip {}", w[1].clip);
    }

    // Take: cut from a loop to the next item.
    playout
        .set_playlist(
            vec![
                item("loop", clip(&file, 1.0, Some(2.0), ClipEnd::Loop)),
                item("after", clip(&file, 4.0, None, ClipEnd::Hold)),
            ],
            false,
        )
        .await;
    playout.cue_item("loop").await.unwrap();
    playout.play().await.unwrap();
    sleep(1500).await;
    take_pushed(&playout);
    playout.next().await.unwrap();
    sleep(500).await;
    let status = playout.status();
    assert_eq!(status.item_id.as_deref(), Some("after"));
    assert_eq!(status.state, TransportState::Playing);
    assert_eq!(status.next_id, None, "nothing after the last");
    let video = played(&take_pushed(&playout), Kind::Video);
    assert!(
        video.iter().any(|p| p.clip.mseconds() >= 4000),
        "the next item plays"
    );
    for w in video.windows(2) {
        let gap = w[1].program.saturating_sub(w[0].program).mseconds();
        assert!((30..=40).contains(&gap), "gap {gap} ms at a cut");
    }

    mon.stop().unwrap();
    assert_eq!(
        *program.stalls.lock().unwrap(),
        Vec::<std::time::Duration>::new(),
        "program stalls"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The next item is cued while idle so Play starts it at once; an item plays
/// directly; cueing over a playing clip shows its first frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~8 s); run with --ignored"]
async fn plays_without_cueing() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join(format!("cr-direct-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.mp4");
    make_clip(&file);

    let src = channel();
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(
        &src,
        &MonitorSettingsDto::default(),
        &gst::SystemClock::obtain(),
    )
    .unwrap();
    let program = watch_program(&mon);
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    sleep(500).await;

    playout
        .set_playlist(
            vec![
                item("a", clip(&file, 0.0, None, ClipEnd::Hold)),
                item("b", clip(&file, 2.0, None, ClipEnd::Hold)),
                item("c", clip(&file, 3.0, None, ClipEnd::Hold)),
            ],
            false,
        )
        .await;
    for _ in 0..30 {
        if playout.status().next_ready {
            break;
        }
        sleep(100).await;
    }
    let status = playout.status();
    assert_eq!(status.state, TransportState::Idle);
    assert_eq!(status.next_id.as_deref(), Some("a"));
    assert!(
        status.next_ready,
        "idle, the first item is in standby: {:?}",
        status.error
    );
    assert!(take_pushed(&playout).is_empty(), "standby shows nothing");

    // Play from idle: the next item.
    playout.play().await.unwrap();
    sleep(500).await;
    let status = playout.status();
    assert_eq!(status.state, TransportState::Playing);
    assert_eq!(status.item_id.as_deref(), Some("a"));
    let video = played(&take_pushed(&playout), Kind::Video);
    assert!(video[0].clip.mseconds() < 34, "from its in point");

    // Play an item that isn't next: straight on, no gap.
    playout.play_item("c").await.unwrap();
    sleep(500).await;
    let status = playout.status();
    assert_eq!(status.item_id.as_deref(), Some("c"));
    assert_eq!(status.state, TransportState::Playing);
    let video = played(&take_pushed(&playout), Kind::Video);
    assert!(video.iter().any(|p| p.clip.mseconds() >= 3000), "c plays");
    for w in video.windows(2) {
        let gap = w[1].program.saturating_sub(w[0].program).mseconds();
        assert!((30..=40).contains(&gap), "gap {gap} ms at the cut");
    }

    // Cue over a clip playing: its first frame shows.
    playout.cue_item("b").await.unwrap();
    sleep(300).await;
    let status = playout.status();
    assert_eq!(status.state, TransportState::Cued);
    assert_eq!(status.item_id.as_deref(), Some("b"));
    let pushed = take_pushed(&playout);
    assert!(
        pushed
            .iter()
            .any(|p| p.kind == Kind::Video && p.preroll && p.clip.mseconds() == 2000),
        "b's first frame shows"
    );

    // Stopped, what's next is the item after the last one.
    playout.stop().await;
    sleep(500).await;
    let status = playout.status();
    assert_eq!(status.next_id.as_deref(), Some("c"));
    assert!(status.next_ready);
    playout.next().await.unwrap();
    let status = playout.status();
    assert_eq!(status.state, TransportState::Playing);
    assert_eq!(status.item_id.as_deref(), Some("c"));

    mon.stop().unwrap();
    assert_eq!(
        *program.stalls.lock().unwrap(),
        Vec::<std::time::Duration>::new(),
        "program stalls"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A clip cued at a start shows and plays on from that frame; replayed after
/// it ends, it starts from its in point.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~4 s); run with --ignored"]
async fn cues_at_a_start() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join(format!("cr-start-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.mp4");
    make_clip(&file);

    let src = channel();
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(
        &src,
        &MonitorSettingsDto::default(),
        &gst::SystemClock::obtain(),
    )
    .unwrap();
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    sleep(500).await;

    let mut request = clip(&file, 0.5, Some(2.5), ClipEnd::Hold);
    request.start = Some(gst::ClockTime::from_seconds(2));
    playout.load(request, false).await.unwrap();
    sleep(200).await;
    let shown: Vec<_> = take_pushed(&playout)
        .into_iter()
        .filter(|p| p.kind == Kind::Video)
        .map(|p| p.clip.mseconds())
        .collect();
    assert!(!shown.is_empty(), "nothing shown");
    assert!(
        shown.iter().all(|&ms| ms >= 2000),
        "only the start frame shows: {shown:?}"
    );

    playout.play().await.unwrap();
    sleep(1000).await;
    assert_eq!(playout.status().state, TransportState::Ended);
    let video = played(&take_pushed(&playout), Kind::Video);
    assert!(video[0].clip.mseconds() >= 2000, "plays on from the start");
    assert!(
        (13..=17).contains(&video.len()),
        "{} frames to the out",
        video.len()
    );

    playout.play().await.unwrap();
    sleep(300).await;
    let video = played(&take_pushed(&playout), Kind::Video);
    assert!(
        (500..534).contains(&video[0].clip.mseconds()),
        "again from the in point: {}",
        video[0].clip.mseconds()
    );

    mon.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 5.1 program runs, laid out as 5.1, and a stereo clip plays into it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~3 s); run with --ignored"]
async fn surround_program_takes_stereo_clips() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join(format!("cr-surround-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.mp4");
    make_clip(&file);

    let src = channel_with_audio(6);
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(
        &src,
        &MonitorSettingsDto::default(),
        &gst::SystemClock::obtain(),
    )
    .unwrap();
    let caps = Arc::new(Mutex::new(None));
    let c = caps.clone();
    mon.audio.appsink().static_pad("sink").unwrap().add_probe(
        gst::PadProbeType::BUFFER,
        move |pad, _| {
            *c.lock().unwrap() = pad.current_caps();
            gst::PadProbeReturn::Ok
        },
    );
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    sleep(500).await;
    assert_eq!(mon.error(), None, "the program runs");

    playout
        .set_playlist(
            vec![item("a", clip(&file, 0.0, None, ClipEnd::Hold))],
            false,
        )
        .await;
    playout.play_item("a").await.unwrap();
    sleep(1000).await;
    assert_eq!(mon.error(), None);
    assert_eq!(playout.status().state, TransportState::Playing);
    assert!(
        !played(&take_pushed(&playout), Kind::Audio).is_empty(),
        "the clip's audio plays"
    );
    let caps = caps.lock().unwrap().clone().expect("program audio");
    let s = caps.structure(0).unwrap();
    assert_eq!(s.get::<i32>("channels").unwrap(), 6);
    assert_eq!(
        s.get::<gst::Bitmask>("channel-mask").unwrap().0,
        0x3f,
        "5.1"
    );

    mon.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A replay buffer through a running channel: forwards with sound, half speed
/// and backwards without, and stepped a frame each way.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real-time (~12 s); run with --ignored"]
async fn replay_plays_at_any_speed() {
    use crate::api::types::{AudioTestSignal, ReplayCodec, TestSourceConfig, VideoTestPattern};
    use crate::pipeline::replay::{Index, ReplayLeg};
    use crate::sources::test::TestSource;

    gst::init().unwrap();
    let clock = gst::SystemClock::obtain();
    let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
    let frame = gst::ClockTime::SECOND / 30;

    // A 30 fps source into a replay buffer.
    let camera = TestSource::new(
        "cam".into(),
        "cam".into(),
        TestSourceConfig {
            pattern: VideoTestPattern::Ball,
            width: 640,
            height: 360,
            fps_num: 30,
            fps_den: 1,
            audio_signal: AudioTestSignal::Tone,
            frequency: 440.0,
            channels: 2,
        },
    );
    let camera_mon = MonitorPipeline::new(&camera, &MonitorSettingsDto::default(), &clock).unwrap();
    let dir = std::env::temp_dir().join(format!("cr-replay-{}", uuid::Uuid::new_v4()));
    let index = Index::create(&dir, gst::ClockTime::from_seconds(60));
    while camera_mon.video_frames() == 0 {
        sleep(20).await;
    }
    let leg = ReplayLeg::start(&camera_mon, ReplayCodec::Mjpeg, &index, "cam").unwrap();
    for _ in 0..200 {
        if index
            .lock()
            .unwrap()
            .span()
            .is_some_and(|(a, b)| b - a >= gst::ClockTime::from_seconds(4))
        {
            break;
        }
        sleep(50).await;
    }
    let (start, _) = index.lock().unwrap().span().expect("nothing recorded");

    let src = ChannelSource::new(
        "c".into(),
        "c".into(),
        ChannelConfig {
            format: LiveVideoFormat {
                width: W,
                height: H,
                fps_num: 30,
                fps_den: 1,
            },
            audio_channels: 2,
            outputs: vec![],
        },
    );
    let playout = src.playout().unwrap().clone();
    let mon = MonitorPipeline::new(&src, &MonitorSettingsDto::default(), &clock).unwrap();

    let span = Index::clip(
        &index,
        start + gst::ClockTime::from_mseconds(500),
        Some(start + gst::ClockTime::from_seconds(4)),
    )
    .unwrap();
    let request = ClipRequest {
        media_id: "replay:cam".into(),
        name: "cam".into(),
        path: dir.to_string_lossy().into(),
        in_point: span.from,
        out_point: Some(span.to),
        start: None,
        end: ClipEnd::Hold,
        rate: 1.0,
        replay: Some(ReplayClip {
            source_id: "cam".into(),
            clip_id: None,
            span: span.clone(),
        }),
    };
    playout.load(request, true).await.unwrap();
    assert_eq!(playout.status().state, TransportState::Playing);

    // Played frames' positions, in the order pushed.
    let played = |pushed: &[Pushed]| -> Vec<gst::ClockTime> {
        pushed
            .iter()
            .filter(|p| p.kind == Kind::Video && !p.preroll)
            .map(|p| p.clip)
            .collect()
    };
    let steps = |v: &[gst::ClockTime]| -> Vec<i64> {
        v.windows(2)
            .map(|w| {
                ((w[1].nseconds() as f64 - w[0].nseconds() as f64) / frame.nseconds() as f64)
                    .round() as i64
            })
            .collect()
    };

    sleep(300).await;
    take_pushed(&playout);
    sleep(1000).await;
    let pushed = take_pushed(&playout);
    let forward = played(&pushed);
    assert!(
        (25..=35).contains(&forward.len()),
        "{} frames at 1×",
        forward.len()
    );
    assert!(
        steps(&forward).iter().all(|&s| s == 1),
        "1×: {:?}",
        steps(&forward)
    );
    assert!(
        pushed.iter().any(|p| p.kind == Kind::Audio),
        "no sound at 1×"
    );

    playout.set_rate(0.5).await.unwrap();
    sleep(300).await;
    take_pushed(&playout);
    sleep(1000).await;
    let pushed = take_pushed(&playout);
    let half = played(&pushed);
    assert!(
        (12..=18).contains(&half.len()),
        "{} frames at 0.5×",
        half.len()
    );
    assert!(
        steps(&half).iter().all(|&s| s == 1),
        "0.5×: {:?}",
        steps(&half)
    );
    assert!(
        !pushed.iter().any(|p| p.kind == Kind::Audio),
        "sound at 0.5×"
    );
    assert_eq!(playout.status().clip.unwrap().rate, 0.5);

    playout.set_rate(-1.0).await.unwrap();
    sleep(300).await;
    take_pushed(&playout);
    sleep(1000).await;
    let back = played(&take_pushed(&playout));
    assert!(
        (25..=35).contains(&back.len()),
        "{} frames at -1×",
        back.len()
    );
    assert!(
        steps(&back).iter().all(|&s| s == -1),
        "-1×: {:?}",
        steps(&back)
    );

    // Stepping pauses, then moves one frame each way.
    let shown = |pushed: Vec<Pushed>| {
        pushed
            .iter()
            .filter(|p| p.kind == Kind::Video && p.preroll)
            .map(|p| p.clip)
            .next_back()
            .expect("no frame shown")
    };
    playout.step(1).await.unwrap();
    assert_eq!(playout.status().state, TransportState::Paused);
    sleep(100).await;
    let a = shown(take_pushed(&playout));
    playout.step(1).await.unwrap();
    sleep(100).await;
    let b = shown(take_pushed(&playout));
    // Frame times are rounded to the ns.
    let same = |x: gst::ClockTime, y: gst::ClockTime| x.nseconds().abs_diff(y.nseconds()) < 1000;
    assert!(same(b, a + frame), "step forward: {a} → {b}");
    playout.step(-1).await.unwrap();
    sleep(100).await;
    let c = shown(take_pushed(&playout));
    assert!(same(c, a), "step back: {b} → {c}, not {a}");
    playout.step(-1).await.unwrap();
    sleep(100).await;
    let d = shown(take_pushed(&playout));
    assert!(same(d + frame, a), "step back again: {c} → {d}");

    // Play goes on at the clip's speed (-1) from there.
    playout.play().await.unwrap();
    sleep(500).await;
    let back = played(&take_pushed(&playout));
    assert!(back.first().is_some_and(|&t| t <= d), "{back:?} after {d}");
    assert!(steps(&back).iter().all(|&s| s == -1), "{:?}", steps(&back));

    playout.stop().await;
    // The lanes go in the background; the monitor waits for them.
    sleep(500).await;
    drop(span);
    drop(leg);
    let _ = camera_mon.stop();
    mon.stop().unwrap();
    drop(index);
}

use std::path::PathBuf;

use chrono::{DateTime, Local};
use gstreamer as gst;

use crate::api::types::{
    AudioChannels, AudioCodecChoice, ChromaSubsampling, Container, Deinterlace, EncoderChoice, OutputAdvanced,
    PresetOutputInput, RateControl, SpeedPreset, VideoCodec,
};

/// Values for the per-recording tokens of a path template.
pub struct PathVars {
    pub source: String,
    pub source_name: String,
    pub node: String,
    pub preset: String,
    /// When the recording starts.
    pub at: DateTime<Local>,
    /// `{take}`: the first number whose paths don't exist yet.
    pub take: u32,
    /// The source's current format, for `{resolution}` and `{fps}` on an
    /// output that matches the source. `None` until it's negotiated.
    pub source_resolution: Option<(u32, u32)>,
    pub source_framerate: Option<(u32, u32)>,
    /// Channel count and whether they're positioned; see
    /// [`RecordingProfile::source_audio`].
    pub source_audio: Option<(u32, bool)>,
}

impl PathVars {
    fn expand(&self, template: &str, profile: &RecordingProfile) -> String {
        let at = &self.at;
        let resolution = profile.resolution.or(self.source_resolution).map_or("source".into(), format_resolution);
        let fps = profile.framerate.or(self.source_framerate).map_or("source".into(), format_framerate);
        [
            ("{source}", sanitize(&self.source)),
            ("{source_name}", sanitize(&self.source_name)),
            ("{node}", sanitize(&self.node)),
            ("{preset}", sanitize(&self.preset)),
            ("{date}", at.format("%Y-%m-%d").to_string()),
            ("{time}", at.format("%H%M%S").to_string()),
            ("{datetime}", at.format("%Y%m%d_%H%M%S").to_string()),
            ("{year}", at.format("%Y").to_string()),
            ("{month}", at.format("%m").to_string()),
            ("{day}", at.format("%d").to_string()),
            ("{take}", format!("{:02}", self.take)),
            ("{resolution}", resolution),
            ("{fps}", fps),
        ]
        .iter()
        .fold(template.to_string(), |path, (token, value)| path.replace(token, value))
    }
}

/// Build every leg's profile and output path, rejecting a format that doesn't
/// parse or two legs that would write the same file.
///
/// With `vars` as `None` the per-recording tokens are left unexpanded, so the
/// check covers every future recording — what a preset save wants. Tokens the
/// leg settles itself (`{output}`, `{ext}`, `{codec}`, and `{resolution}` /
/// `{fps}` when it sets them) are always expanded; a leading `~` is this
/// node's home.
pub fn plan_legs(
    outputs: &[PresetOutputInput],
    vars: Option<&PathVars>,
) -> Result<Vec<(PathBuf, RecordingProfile)>, &'static str> {
    let mut legs: Vec<(PathBuf, RecordingProfile)> = Vec::with_capacity(outputs.len());
    for o in outputs {
        let mut profile = RecordingProfile::from_output(o)?;
        if let Some(v) = vars {
            profile.source_size = v.source_resolution;
            profile.source_rate = v.source_framerate;
            profile.source_audio = v.source_audio;
        }
        let mut path = o
            .path_template
            .replace("{output}", &sanitize(&o.name))
            .replace("{ext}", profile.file_extension())
            .replace("{codec}", profile.codec_slug());
        if let Some((w, h)) = profile.resolution {
            path = path.replace("{resolution}", &format_resolution((w, h)));
        }
        if let Some(fps) = profile.framerate {
            path = path.replace("{fps}", &format_framerate(fps));
        }
        if let Some(v) = vars {
            path = v.expand(&path, &profile);
        }
        // `{segment}` is the file's number within a split recording; the
        // first file's path is what the leg is known by. A leg that only
        // rolls over to a new file at `PCM_MAX_FILE` keeps a plain first name
        // (most recordings never reach it) and numbers the files after it.
        if profile.splits() {
            let template = with_segment(&path);
            profile.segment_template = Some(expand_home(&template).to_string_lossy().into_owned());
            path = template;
        } else if profile.reserves_index() {
            profile.segment_template = Some(expand_home(&with_segment(&path)).to_string_lossy().into_owned());
        }
        let path = expand_home(&path.replace("{segment}", &segment_number(0)));
        if legs.iter().any(|(p, _)| *p == path) {
            return Err("two outputs would write the same file; give each its own path template, \
                        container, or {output} name");
        }
        legs.push((path, profile));
    }
    Ok(legs)
}

/// A name made safe to use as one path component: separators and characters
/// Windows or macOS reject become `-`.
fn sanitize(value: &str) -> String {
    let s: String = value
        .trim()
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect();
    match s.as_str() {
        "" | "." | ".." => "_".into(),
        _ => s,
    }
}

fn format_resolution((w, h): (u32, u32)) -> String {
    format!("{w}x{h}")
}

/// (30, 1) → "30"; (30000, 1001) → "29.97"; (24000, 1001) → "23.976"
fn format_framerate((n, d): (u32, u32)) -> String {
    let s = format!("{:.3}", n as f64 / d as f64);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A split leg's template with `{segment}` in it: added before the file
/// name's extension (or at its end) when the template doesn't place it, so
/// every file gets its own name.
fn with_segment(path: &str) -> String {
    if path.contains("{segment}") {
        return path.to_string();
    }
    let name_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[name_start..].rfind('.') {
        Some(dot) if dot > 0 => {
            let at = name_start + dot;
            format!("{}_{{segment}}{}", &path[..at], &path[at..])
        }
        _ => format!("{path}_{{segment}}"),
    }
}

/// `{segment}` for the file at `index` (0-based): 001, 002, …
pub fn segment_number(index: u32) -> String {
    format!("{:03}", index + 1)
}

/// `~` or `~/…` → this node's home directory. Paths are resolved on the node,
/// so the UI can't know it.
fn expand_home(path: &str) -> PathBuf {
    let rest = match path.strip_prefix('~') {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
        _ => return PathBuf::from(path),
    };
    match std::env::home_dir() {
        Some(home) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// Configures a single recording output leg.
#[derive(Debug, Clone)]
pub struct RecordingProfile {
    pub video_codec: VideoCodec,
    pub container: Container,
    /// `None` = match source resolution
    pub resolution: Option<(u32, u32)>,
    /// `None` = match source framerate (num, den)
    pub framerate: Option<(u32, u32)>,
    /// `None` = Auto: sized from the frame size and rate, see [`Self::bitrate`].
    pub bitrate_kbps: Option<u32>,
    pub chroma: ChromaSubsampling,
    /// The source's format when recording starts (`None` when checking a
    /// preset, or before the source has negotiated). Sizes Auto bitrate and
    /// the keyframe interval for outputs that match the source.
    pub source_size: Option<(u32, u32)>,
    pub source_rate: Option<(u32, u32)>,
    /// The source's audio channel count and whether they have positions
    /// (stereo, 5.1…) rather than being plain numbered channels. Needed to
    /// mix down or pick channels; `None` until known.
    pub source_audio: Option<(u32, bool)>,
    pub advanced: OutputAdvanced,
    /// For a leg that splits: its path with `{segment}` still in it, to name
    /// each file. Set by [`plan_legs`].
    pub segment_template: Option<String>,
}

/// A video encoder element and what it needs around it. Each codec lists its
/// candidates best first (see [`RecordingProfile::encoders`]); a leg uses the
/// first one that builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    X264,
    X265,
    Vp9,
    ProResKs,
    /// VideoToolbox (macOS). `vtenc_h264`/`vtenc_h265` fall back to
    /// VideoToolbox's software encoder themselves when the hardware is busy;
    /// the `_hw` variants would fail instead.
    VtH264,
    VtH265,
    VtProRes,
    Uncompressed,
}

impl VideoEncoder {
    pub fn element(self) -> &'static str {
        match self {
            Self::X264 => "x264enc",
            Self::X265 => "x265enc",
            Self::Vp9 => "vp9enc",
            Self::ProResKs => "avenc_prores_ks",
            Self::VtH264 => "vtenc_h264",
            Self::VtH265 => "vtenc_h265",
            Self::VtProRes => "vtenc_prores",
            Self::Uncompressed => "identity",
        }
    }

    pub fn is_hardware(self) -> bool {
        matches!(self, Self::VtH264 | Self::VtH265 | Self::VtProRes)
    }
}

/// How a leg's audio is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// 24-bit PCM: what editors expect beside ProRes and uncompressed video.
    Pcm24,
    Aac { bitrate: u32 },
    Opus { bitrate: u32 },
}

/// A change to the source's audio channels.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioMix {
    /// `audioconvert`'s `mix-matrix`: a row of input gains per output
    /// channel. `None` leaves the mapping to `audioconvert` (a positioned
    /// downmix), or isn't known yet (a preset check, before the source's
    /// channel count is).
    pub matrix: Option<Vec<Vec<f32>>>,
    pub channels: u32,
}

/// Longest file a leg with PCM audio in .mov writes before starting the
/// next: the index space it reserves (see `CRASH_SAFE_*` in `recording.rs`)
/// is sized for this, and qtmux stops the leg with an error if it fills up.
pub const PCM_MAX_FILE: std::time::Duration = std::time::Duration::from_secs(4 * 3600);

/// Keyframe spacing: short enough to scrub, cut and split files cleanly.
pub const KEYFRAME_INTERVAL_SECS: f64 = 2.0;
/// Constant-quality level when an output doesn't set one (1–100).
pub const DEFAULT_QUALITY: u8 = 70;

/// Every encoder recording can use that this node has, by element name.
pub fn available_encoders() -> Vec<String> {
    use VideoEncoder::*;
    [VtH264, VtH265, VtProRes, X264, X265, Vp9, ProResKs]
        .into_iter()
        .map(VideoEncoder::element)
        .filter(|name| gst::ElementFactory::find(name).is_some())
        .map(String::from)
        .collect()
}

impl RecordingProfile {
    /// Build a profile from an output leg. Resolution and framerate are free
    /// text: blank means "match the source", anything else must parse — a
    /// typo is rejected rather than silently recording at the source format.
    pub fn from_output(o: &PresetOutputInput) -> Result<Self, &'static str> {
        if let Some(reason) = incompatible(o.codec, o.container) {
            return Err(reason);
        }
        Ok(Self {
            video_codec: o.codec,
            container: o.container,
            resolution: parse_optional(&o.resolution, parse_resolution)
                .ok_or("resolution must look like 1920x1080")?,
            framerate: parse_optional(&o.framerate, parse_framerate)
                .ok_or("framerate must look like 30, 29.97 or 30000/1001")?,
            bitrate_kbps: o.bitrate_kbps,
            chroma: o.chroma,
            source_size: None,
            source_rate: None,
            source_audio: None,
            advanced: o.advanced.clone(),
            segment_template: None,
        })
        .and_then(|p| p.check_advanced().map(|()| p))
    }

    /// Reject Advanced settings no recording could use.
    fn check_advanced(&self) -> Result<(), &'static str> {
        let a = &self.advanced;
        if self.encoders().is_empty() {
            return Err(match a.encoder {
                EncoderChoice::Hardware if self.prores_profile().is_none() && self.video_codec != VideoCodec::H264
                    && self.video_codec != VideoCodec::H265 => "this codec has no hardware encoder",
                EncoderChoice::Hardware => "hardware H.264/H.265 encoders take 4:2:0 only",
                _ => "this codec has no software encoder",
            });
        }
        if a.quality.is_some_and(|q| !(1..=100).contains(&q)) {
            return Err("quality must be 1–100");
        }
        if a.keyframe_secs.is_some_and(|k| !(0.1..=60.0).contains(&k)) {
            return Err("keyframe interval must be 0.1–60 seconds");
        }
        if a.split_minutes.is_some_and(|m| !(1..=1440).contains(&m)) {
            return Err("split every 1–1440 minutes");
        }
        if a.split_gb.is_some_and(|g| !(0.1..=10_000.0).contains(&g)) {
            return Err("split at 0.1–10,000 GB");
        }
        if a.audio_bitrate_kbps.is_some_and(|b| !(32..=512).contains(&b)) {
            return Err("audio bitrate must be 32–512 kbps");
        }
        match (a.audio_codec, self.container) {
            (AudioCodecChoice::Pcm, Container::Mp4) => return Err("PCM audio can only be recorded to .mov or .mkv"),
            (AudioCodecChoice::Opus, Container::Mov | Container::Mp4) => {
                return Err("Opus audio can only be recorded to .mkv")
            }
            _ => {}
        }
        if a.audio_channels == AudioChannels::Pick {
            if a.channel_pick.is_empty() || a.channel_pick.iter().any(|c| !(1..=64).contains(c)) {
                return Err("pick at least one channel, numbered 1–64");
            }
            if a.channel_pick.len() > 2 && self.audio_format() != AudioFormat::Pcm24 {
                return Err("AAC and Opus take 1 or 2 picked channels; use PCM for more");
            }
        }
        Ok(())
    }

    /// The encoders this leg can use, best first: hardware where the platform
    /// has it, then software. VideoToolbox H.264/H.265 only take 4:2:0, so
    /// 4:2:2 and 4:4:4 go straight to software.
    pub fn encoders(&self) -> Vec<VideoEncoder> {
        let all = self.encoders_for_codec();
        all.iter()
            .copied()
            .filter(|e| match self.advanced.encoder {
                EncoderChoice::Auto => true,
                EncoderChoice::Hardware => e.is_hardware(),
                // Uncompressed isn't encoded at all, so it fits either.
                EncoderChoice::Software => !e.is_hardware(),
            })
            .filter(|e| self.advanced.encoder != EncoderChoice::Hardware || *e != VideoEncoder::Uncompressed)
            .collect()
    }

    fn encoders_for_codec(&self) -> &'static [VideoEncoder] {
        use VideoEncoder::*;
        let yuv420 = self.chroma == ChromaSubsampling::Yuv420;
        match self.video_codec {
            VideoCodec::H264 if yuv420 => &[VtH264, X264],
            VideoCodec::H264 => &[X264],
            VideoCodec::H265 if yuv420 => &[VtH265, X265],
            VideoCodec::H265 => &[X265],
            VideoCodec::Vp9 => &[Vp9],
            VideoCodec::ProRes4444
            | VideoCodec::ProRes422Hq
            | VideoCodec::ProRes422
            | VideoCodec::ProRes422Lt
            | VideoCodec::ProRes422Proxy => &[VtProRes, ProResKs],
            VideoCodec::Uncompressed => &[Uncompressed],
        }
    }

    /// Raw format `encoder` is fed. Without this the encoder follows the
    /// source's format, which is how H.264 ended up as High 4:4:4 (unplayable
    /// in most players), and how VideoToolbox ProRes would get 4:2:0 input.
    pub fn encoder_input_format(&self, encoder: VideoEncoder) -> Option<&'static str> {
        match encoder {
            VideoEncoder::X264 | VideoEncoder::X265 | VideoEncoder::VtH264 | VideoEncoder::VtH265 => {
                Some(match self.chroma {
                    ChromaSubsampling::Yuv420 => "I420",
                    ChromaSubsampling::Yuv422 => "Y42B",
                    ChromaSubsampling::Yuv444 => "Y444",
                })
            }
            // Profile 0 (4:2:0) is the VP9 everything plays; left to follow
            // the source, an RGB source gave 4:4:4 Profile 1.
            VideoEncoder::Vp9 => Some("I420"),
            // 4:2:2 is what NDI sends, so this is usually no conversion at all.
            VideoEncoder::VtProRes if self.video_codec == VideoCodec::ProRes4444 => Some("AYUV64"),
            VideoEncoder::VtProRes => Some("UYVY"),
            _ => None,
        }
    }

    /// Parser between encoder and muxer. x265enc only emits Annex-B
    /// byte-stream, which no container muxer accepts (they want hvc1/hev1);
    /// the parser converts it, and fills in codec headers for H.264 too.
    pub fn video_parser_element(&self) -> Option<&'static str> {
        match self.video_codec {
            VideoCodec::H264 => Some("h264parse"),
            VideoCodec::H265 => Some("h265parse"),
            _ => None,
        }
    }

    /// GStreamer element name for the container muxer.
    pub fn muxer_element(&self) -> &'static str {
        match self.container {
            Container::Mov => "qtmux",
            Container::Mp4 => "mp4mux",
            Container::Mkv => "matroskamux",
        }
    }

    /// Auto audio: PCM beside ProRes and uncompressed video (both only go in
    /// .mov/.mkv, which take it), otherwise the container's usual codec at a
    /// rate fit for program audio.
    pub fn audio_format(&self) -> AudioFormat {
        let bitrate = |default: u32| self.advanced.audio_bitrate_kbps.unwrap_or(default) * 1000;
        let codec = match self.advanced.audio_codec {
            AudioCodecChoice::Auto => match (self.video_codec, self.container) {
                (VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Vp9, Container::Mov | Container::Mp4) => {
                    AudioCodecChoice::Aac
                }
                (VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Vp9, Container::Mkv) => AudioCodecChoice::Opus,
                _ => AudioCodecChoice::Pcm,
            },
            chosen => chosen,
        };
        match codec {
            AudioCodecChoice::Aac => AudioFormat::Aac { bitrate: bitrate(256) },
            AudioCodecChoice::Opus => AudioFormat::Opus { bitrate: bitrate(160) },
            _ => AudioFormat::Pcm24,
        }
    }

    /// Whether PCM bound for .mov has to be relabelled as plain numbered
    /// channels: qtmux takes PCM beyond stereo only that way, and otherwise
    /// audioconvert quietly downmixes it to stereo (it won't relabel a
    /// layout like 7.1 itself). Picked channels are numbered already.
    pub fn relabel_pcm_channels(&self) -> bool {
        self.container == Container::Mov
            && self.audio_format() == AudioFormat::Pcm24
            && self.advanced.audio_channels == AudioChannels::All
            && self.source_audio.is_some_and(|(n, _)| n > 2)
    }

    /// Whether interlaced video is deinterlaced. ProRes keeps interlacing: it
    /// stores it natively, and archives may want it.
    pub fn deinterlace(&self) -> bool {
        self.advanced.deinterlace == Deinterlace::Auto && self.prores_profile().is_none()
    }

    /// The x264/x265 `speed-preset` value.
    pub fn speed_preset(&self) -> &'static str {
        match self.advanced.speed_preset.unwrap_or(SpeedPreset::Veryfast) {
            SpeedPreset::Ultrafast => "ultrafast",
            SpeedPreset::Superfast => "superfast",
            SpeedPreset::Veryfast => "veryfast",
            SpeedPreset::Faster => "faster",
            SpeedPreset::Fast => "fast",
            SpeedPreset::Medium => "medium",
            SpeedPreset::Slow => "slow",
        }
    }

    pub fn rate_control(&self) -> RateControl {
        self.advanced.rate_control
    }

    /// Constant-quality level, 1–100.
    pub fn quality(&self) -> u8 {
        self.advanced.quality.unwrap_or(DEFAULT_QUALITY)
    }

    /// Whether the leg starts new files as it goes.
    pub fn splits(&self) -> bool {
        self.advanced.split_minutes.is_some() || self.advanced.split_gb.is_some()
    }

    /// Longest a file runs, when split by time.
    pub fn split_duration(&self) -> Option<std::time::Duration> {
        self.advanced.split_minutes.map(|m| std::time::Duration::from_secs(u64::from(m) * 60))
    }

    /// Whether the leg reserves its index at the front of the file for crash
    /// safety: PCM in .mov (the only container besides .mkv that takes it).
    /// Other .mov/.mp4 legs write fragments instead.
    pub fn reserves_index(&self) -> bool {
        self.container == Container::Mov && self.audio_format() == AudioFormat::Pcm24
    }

    /// Longest a file runs before the next starts: the output's split time,
    /// capped at [`PCM_MAX_FILE`] for legs that reserve their index.
    pub fn max_file_duration(&self) -> Option<std::time::Duration> {
        let split = self.split_duration();
        if self.reserves_index() {
            Some(split.map_or(PCM_MAX_FILE, |d| d.min(PCM_MAX_FILE)))
        } else {
            split
        }
    }

    /// Largest a file grows, when split by size.
    pub fn split_bytes(&self) -> Option<u64> {
        self.advanced.split_gb.map(|g| (g * 1e9) as u64)
    }

    pub fn keyframe_secs(&self) -> f64 {
        self.advanced.keyframe_secs.unwrap_or(KEYFRAME_INTERVAL_SECS)
    }

    /// How the output changes the source's audio channels; `None` records
    /// them as they are.
    ///
    /// Stereo lets `audioconvert` downmix positioned layouts (5.1…) itself;
    /// plain numbered channels have no layout to follow, so odd channels go
    /// left and even ones right. Picked channels the source doesn't have are
    /// silent.
    pub fn audio_mix(&self) -> Option<AudioMix> {
        let a = &self.advanced;
        let row = |inputs: u32, gains: &dyn Fn(u32) -> f32| (0..inputs).map(gains).collect::<Vec<f32>>();
        match a.audio_channels {
            AudioChannels::All => None,
            AudioChannels::Stereo => match self.source_audio {
                None | Some((_, true)) | Some((2, _)) => Some(AudioMix { matrix: None, channels: 2 }),
                Some((1, _)) => Some(AudioMix { matrix: Some(vec![vec![1.0], vec![1.0]]), channels: 2 }),
                Some((n, false)) => {
                    let gain = 1.0 / n.div_ceil(2) as f32;
                    let left = row(n, &|c| if c % 2 == 0 { gain } else { 0.0 });
                    let right = row(n, &|c| if c % 2 == 1 { gain } else { 0.0 });
                    Some(AudioMix { matrix: Some(vec![left, right]), channels: 2 })
                }
            },
            AudioChannels::Pick => {
                let out = a.channel_pick.len() as u32;
                match self.source_audio {
                    // A preset check: the matrix needs the source's channel count.
                    None => Some(AudioMix { matrix: None, channels: out }),
                    Some((n, _)) => {
                        let rows =
                            a.channel_pick.iter().map(|&p| row(n, &|c| if c + 1 == p { 1.0 } else { 0.0 })).collect();
                        Some(AudioMix { matrix: Some(rows), channels: out })
                    }
                }
            }
        }
    }

    /// File extension for the output path template.
    pub fn file_extension(&self) -> &'static str {
        match self.container {
            Container::Mov => "mov",
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }

    /// `{codec}` in a path template: the codec's API name.
    pub fn codec_slug(&self) -> &'static str {
        match self.video_codec {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "h265",
            VideoCodec::Vp9 => "vp9",
            VideoCodec::ProRes4444 => "prores_4444",
            VideoCodec::ProRes422Hq => "prores_422hq",
            VideoCodec::ProRes422 => "prores_422",
            VideoCodec::ProRes422Lt => "prores_422lt",
            VideoCodec::ProRes422Proxy => "prores_422proxy",
            VideoCodec::Uncompressed => "uncompressed",
        }
    }

    /// The ProRes profile: `avenc_prores_ks`'s `profile` and
    /// `vtenc_prores`'s output `variant` share these names.
    pub fn prores_profile(&self) -> Option<&'static str> {
        match self.video_codec {
            VideoCodec::ProRes4444 => Some("4444"),
            VideoCodec::ProRes422Hq => Some("hq"),
            VideoCodec::ProRes422 => Some("standard"),
            VideoCodec::ProRes422Lt => Some("lt"),
            VideoCodec::ProRes422Proxy => Some("proxy"),
            _ => None,
        }
    }

    /// Frame size written: the output's, else the source's, else 1080p (a
    /// preset check, or a source that hasn't negotiated yet).
    pub fn frame_size(&self) -> (u32, u32) {
        self.resolution.or(self.source_size).unwrap_or((1920, 1080))
    }

    /// Frame rate written, with the same fallbacks as [`Self::frame_size`].
    pub fn frame_rate(&self) -> (u32, u32) {
        self.framerate.or(self.source_rate).unwrap_or((30, 1))
    }

    /// Frames between keyframes.
    pub fn keyframe_interval(&self) -> u32 {
        let (n, d) = self.frame_rate();
        (self.keyframe_secs() * n as f64 / d as f64).round().max(1.0) as u32
    }

    /// Video bitrate in kbps, for codecs that take one: the output's, or Auto
    /// — scaled by pixels per second, about 12 Mbps for H.264 at 1080p30 and
    /// 60% of that for H.265 and VP9, which compress better.
    pub fn bitrate(&self) -> Option<u32> {
        let bits_per_pixel = match self.video_codec {
            VideoCodec::H264 => 0.19,
            VideoCodec::H265 | VideoCodec::Vp9 => 0.115,
            _ => return None,
        };
        Some(self.bitrate_kbps.unwrap_or_else(|| {
            let (w, h) = self.frame_size();
            let (n, d) = self.frame_rate();
            let kbps = w as f64 * h as f64 * n as f64 / d as f64 * bits_per_pixel / 1000.0;
            // Round to 500 kbps so the number reads like a choice.
            ((kbps / 500.0).round() * 500.0).max(500.0) as u32
        }))
    }
}

/// Why `codec` can't be recorded in `container`, if it can't. Found by
/// recording every combination: these fail when the muxer is linked, or (VP9
/// in MOV/MP4) at caps negotiation once frames flow. The preset editor offers
/// the same choices (`CONTAINERS_FOR` in `PresetsView.vue`).
fn incompatible(codec: VideoCodec, container: Container) -> Option<&'static str> {
    use VideoCodec::*;
    match (codec, container) {
        (Vp9, Container::Mov | Container::Mp4) => Some("VP9 can only be recorded to .mkv"),
        (ProRes4444 | ProRes422Hq | ProRes422 | ProRes422Lt | ProRes422Proxy, Container::Mp4) => {
            Some("ProRes can only be recorded to .mov or .mkv")
        }
        (Uncompressed, Container::Mp4) => Some("uncompressed video can only be recorded to .mov or .mkv"),
        _ => None,
    }
}

/// `None` or blank → `Some(None)`; otherwise `Some(parsed)`, or `None` if it
/// doesn't parse.
fn parse_optional(
    value: &Option<String>,
    parse: fn(&str) -> Option<(u32, u32)>,
) -> Option<Option<(u32, u32)>> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Some(None),
        Some(s) => parse(s).map(Some),
    }
}

/// "1920x1080" → (1920, 1080)
fn parse_resolution(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    nonzero_pair(w, h)
}

/// "30" → (30, 1); "30000/1001" → (30000, 1001); "29.97" → (30000, 1001).
/// A decimal within rounding of an NTSC rate (N×1000/1001) means that rate;
/// any other decimal is taken exactly ("12.5" → (25, 2)).
fn parse_framerate(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if s.contains('.') && !s.contains('/') {
        let fps: f64 = s.parse().ok().filter(|f: &f64| f.is_finite() && *f > 0.0 && *f <= 1000.0)?;
        let whole = fps.round();
        if (fps - whole).abs() < 1e-9 {
            return Some((whole as u32, 1));
        }
        let ntsc = (fps * 1.001).round();
        if (fps - ntsc * 1000.0 / 1001.0).abs() < 0.006 {
            return Some((ntsc as u32 * 1000, 1001));
        }
        let milli = (fps * 1000.0).round() as u32;
        let g = gcd(milli, 1000);
        return (milli > 0).then_some((milli / g, 1000 / g));
    }
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    nonzero_pair(n, d)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn nonzero_pair(a: &str, b: &str) -> Option<(u32, u32)> {
    let a: u32 = a.trim().parse().ok()?;
    let b: u32 = b.trim().parse().ok()?;
    (a > 0 && b > 0).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolution() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution(" 1280 X 720 "), Some((1280, 720)));
        assert_eq!(parse_resolution("1920*1080"), None);
        assert_eq!(parse_resolution("0x1080"), None);
    }

    #[test]
    fn parses_framerate() {
        assert_eq!(parse_framerate("30"), Some((30, 1)));
        assert_eq!(parse_framerate("30000/1001"), Some((30000, 1001)));
        assert_eq!(parse_framerate("29.97"), Some((30000, 1001)));
        assert_eq!(parse_framerate("23.976"), Some((24000, 1001)));
        assert_eq!(parse_framerate("59.94"), Some((60000, 1001)));
        assert_eq!(parse_framerate("25.0"), Some((25, 1)));
        assert_eq!(parse_framerate("12.5"), Some((25, 2)));
        assert_eq!(parse_framerate("abc"), None);
        assert_eq!(parse_framerate("-29.97"), None);
        assert_eq!(parse_framerate("30/0"), None);
    }

    fn leg(name: &str, container: Container, template: &str) -> PresetOutputInput {
        PresetOutputInput {
            name: name.into(),
            codec: VideoCodec::H264,
            container,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: template.into(),
            advanced: OutputAdvanced::default(),
        }
    }

    #[test]
    fn rejects_unrecordable_combinations() {
        assert!(RecordingProfile::from_output(&leg("a", Container::Mov, "x")).is_ok());
        let mut vp9 = leg("a", Container::Mp4, "x");
        vp9.codec = VideoCodec::Vp9;
        assert!(RecordingProfile::from_output(&vp9).is_err());
        vp9.container = Container::Mkv;
        assert!(RecordingProfile::from_output(&vp9).is_ok());
        let mut prores = leg("a", Container::Mp4, "x");
        prores.codec = VideoCodec::ProRes422Hq;
        assert!(RecordingProfile::from_output(&prores).is_err());
    }

    #[test]
    fn rejects_colliding_paths() {
        let t = "/rec/{source}_{datetime}.{ext}";
        assert!(plan_legs(&[leg("a", Container::Mov, t), leg("b", Container::Mov, t)], None).is_err());
        // Different extensions or output names keep them apart.
        assert!(plan_legs(&[leg("a", Container::Mov, t), leg("b", Container::Mkv, t)], None).is_ok());
        let named = "/rec/{source}_{output}.{ext}";
        assert!(plan_legs(&[leg("a", Container::Mov, named), leg("b", Container::Mov, named)], None).is_ok());
    }

    #[test]
    fn expands_tokens_and_home() {
        use chrono::TimeZone;
        let vars = PathVars {
            source: "cam1".into(),
            source_name: "HOST (Cam 1/A)".into(),
            node: "n".into(),
            preset: "Edit".into(),
            at: Local.with_ymd_and_hms(2026, 10, 3, 9, 5, 7).unwrap(),
            take: 3,
            source_resolution: Some((1920, 1080)),
            source_framerate: Some((30000, 1001)),
            source_audio: None,
        };
        let template = "~/rec/{year}/{month}/{day}/{source}_{source_name}_{preset}_{time}_{take}_{resolution}@{fps}_{codec}.{ext}";
        let legs = plan_legs(&[leg("a", Container::Mp4, template)], Some(&vars)).unwrap();
        assert_eq!(
            legs[0].0,
            std::env::home_dir()
                .unwrap()
                .join("rec/2026/10/03/cam1_HOST (Cam 1-A)_Edit_090507_03_1920x1080@29.97_h264.mp4")
        );
        // The output's own format wins over the source's.
        let mut scaled = leg("a", Container::Mp4, "/r/{resolution}_{fps}");
        scaled.resolution = Some("1280x720".into());
        scaled.framerate = Some("25".into());
        assert_eq!(plan_legs(&[scaled], Some(&vars)).unwrap()[0].0, PathBuf::from("/r/1280x720_25"));
        assert_eq!(expand_home("/abs/~x"), PathBuf::from("/abs/~x"));
        assert_eq!(expand_home("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn sizes_auto_bitrate_and_keyframes() {
        let mut p = RecordingProfile::from_output(&leg("a", Container::Mov, "x")).unwrap();
        // Unknown source: sized for 1080p30.
        assert_eq!(p.bitrate(), Some(12000));
        assert_eq!(p.keyframe_interval(), 60);
        p.source_size = Some((3840, 2160));
        p.source_rate = Some((60000, 1001));
        assert_eq!(p.bitrate(), Some(94500));
        assert_eq!(p.keyframe_interval(), 120);
        p.bitrate_kbps = Some(8000);
        assert_eq!(p.bitrate(), Some(8000));
        p.video_codec = VideoCodec::ProRes422Hq;
        assert_eq!(p.bitrate(), None);
    }

    #[test]
    fn picks_encoders_and_audio() {
        let mut p = RecordingProfile::from_output(&leg("a", Container::Mov, "x")).unwrap();
        assert_eq!(p.encoders(), [VideoEncoder::VtH264, VideoEncoder::X264]);
        p.advanced.encoder = EncoderChoice::Software;
        assert_eq!(p.encoders(), [VideoEncoder::X264]);
        p.advanced.encoder = EncoderChoice::Auto;
        assert_eq!(p.audio_format(), AudioFormat::Aac { bitrate: 256_000 });
        p.chroma = ChromaSubsampling::Yuv422;
        assert_eq!(p.encoders(), [VideoEncoder::X264]);
        p.video_codec = VideoCodec::ProRes422;
        assert_eq!(p.audio_format(), AudioFormat::Pcm24);
        p.video_codec = VideoCodec::Vp9;
        p.container = Container::Mkv;
        assert_eq!(p.audio_format(), AudioFormat::Opus { bitrate: 160_000 });
    }

    #[test]
    fn checks_advanced_settings() {
        let check = |f: &dyn Fn(&mut PresetOutputInput)| {
            let mut o = leg("a", Container::Mov, "x");
            f(&mut o);
            RecordingProfile::from_output(&o).map(|_| ())
        };
        assert!(check(&|_| {}).is_ok());
        assert!(check(&|o| o.chroma = ChromaSubsampling::Yuv422).is_ok());
        assert!(check(&|o| {
            o.chroma = ChromaSubsampling::Yuv422;
            o.advanced.encoder = EncoderChoice::Hardware;
        })
        .is_err());
        assert!(check(&|o| {
            o.codec = VideoCodec::Vp9;
            o.container = Container::Mkv;
            o.advanced.encoder = EncoderChoice::Hardware;
        })
        .is_err());
        assert!(check(&|o| o.advanced.audio_codec = AudioCodecChoice::Opus).is_err());
        assert!(check(&|o| o.advanced.keyframe_secs = Some(0.0)).is_err());
        assert!(check(&|o| {
            o.advanced.audio_channels = AudioChannels::Pick;
            o.advanced.channel_pick = vec![1, 2, 3];
        })
        .is_err());
        assert!(check(&|o| {
            o.advanced.audio_codec = AudioCodecChoice::Pcm;
            o.advanced.audio_channels = AudioChannels::Pick;
            o.advanced.channel_pick = vec![1, 2, 3];
        })
        .is_ok());
    }

    #[test]
    fn mixes_audio_channels() {
        let mut p = RecordingProfile::from_output(&leg("a", Container::Mov, "x")).unwrap();
        assert_eq!(p.audio_mix(), None);
        p.source_audio = Some((3, true));
        assert!(!p.relabel_pcm_channels());
        p.video_codec = VideoCodec::ProRes422;
        assert!(p.relabel_pcm_channels());
        p.video_codec = VideoCodec::H264;
        p.advanced.audio_channels = AudioChannels::Pick;
        p.advanced.channel_pick = vec![3, 9];
        p.source_audio = Some((4, false));
        let mix = p.audio_mix().unwrap();
        assert_eq!(mix.channels, 2);
        assert_eq!(mix.matrix.unwrap(), vec![vec![0.0, 0.0, 1.0, 0.0], vec![0.0; 4]]);
        p.advanced.audio_channels = AudioChannels::Stereo;
        let mix = p.audio_mix().unwrap();
        assert_eq!(mix.matrix.unwrap(), vec![vec![0.5, 0.0, 0.5, 0.0], vec![0.0, 0.5, 0.0, 0.5]]);
        p.source_audio = Some((6, true));
        assert_eq!(p.audio_mix(), Some(AudioMix { matrix: None, channels: 2 }));
    }

    #[test]
    fn names_split_files() {
        assert_eq!(with_segment("/r/{source}.{ext}"), "/r/{source}_{segment}.{ext}");
        assert_eq!(with_segment("/r/a.b/name"), "/r/a.b/name_{segment}");
        assert_eq!(with_segment("/r/{segment}/x.mov"), "/r/{segment}/x.mov");
        let mut o = leg("a", Container::Mov, "/r/x.{ext}");
        assert_eq!(plan_legs(std::slice::from_ref(&o), None).unwrap()[0].0, PathBuf::from("/r/x.mov"));
        o.advanced.split_minutes = Some(30);
        let legs = plan_legs(&[o], None).unwrap();
        assert_eq!(legs[0].0, PathBuf::from("/r/x_001.mov"));
        assert_eq!(legs[0].1.segment_template.as_deref(), Some("/r/x_{segment}.mov"));
    }

    #[test]
    fn rolls_pcm_over_to_new_files() {
        let mut o = leg("a", Container::Mov, "/r/x.{ext}");
        o.codec = VideoCodec::ProRes422;
        let legs = plan_legs(std::slice::from_ref(&o), None).unwrap();
        // A plain first file; later ones numbered.
        assert_eq!(legs[0].0, PathBuf::from("/r/x.mov"));
        assert_eq!(legs[0].1.segment_template.as_deref(), Some("/r/x_{segment}.mov"));
        assert_eq!(legs[0].1.max_file_duration(), Some(PCM_MAX_FILE));
        o.advanced.split_minutes = Some(30);
        let p = &plan_legs(std::slice::from_ref(&o), None).unwrap()[0].1;
        assert_eq!(p.max_file_duration(), Some(std::time::Duration::from_secs(1800)));
        o.advanced.split_minutes = Some(600);
        let p = &plan_legs(std::slice::from_ref(&o), None).unwrap()[0].1;
        assert_eq!(p.max_file_duration(), Some(PCM_MAX_FILE));
        // AAC legs write fragments: no reserve, no rollover.
        o.codec = VideoCodec::H264;
        o.advanced.split_minutes = None;
        let p = &plan_legs(&[o], None).unwrap()[0].1;
        assert!(p.segment_template.is_none() && p.max_file_duration().is_none());
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize(" a/b:c "), "a-b-c");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize(""), "_");
    }

    #[test]
    fn formats_framerates() {
        assert_eq!(format_framerate((30, 1)), "30");
        assert_eq!(format_framerate((30000, 1001)), "29.97");
        assert_eq!(format_framerate((24000, 1001)), "23.976");
        assert_eq!(format_framerate((25, 2)), "12.5");
    }

    #[test]
    fn blank_means_match_source() {
        assert_eq!(parse_optional(&None, parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("  ".into()), parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("1920*1080".into()), parse_resolution), None);
    }
}

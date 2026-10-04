use std::path::PathBuf;

use chrono::{DateTime, Local};

use crate::api::types::{ChromaSubsampling, Container, PresetOutputInput, VideoCodec};

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
        let profile = RecordingProfile::from_output(o)?;
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
        let path = expand_home(&path);
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
    /// `None` = the encoder's default rate control
    pub bitrate_kbps: Option<u32>,
    pub chroma: ChromaSubsampling,
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
        })
    }

    /// GStreamer element name for the video encoder.
    pub fn video_encoder_element(&self) -> &'static str {
        match self.video_codec {
            VideoCodec::H264 => "x264enc",
            VideoCodec::H265 => "x265enc",
            VideoCodec::Vp9 => "vp9enc",
            VideoCodec::ProRes4444
            | VideoCodec::ProRes422Hq
            | VideoCodec::ProRes422
            | VideoCodec::ProRes422Lt
            | VideoCodec::ProRes422Proxy => "avenc_prores_ks",
            VideoCodec::Uncompressed => "identity",
        }
    }

    /// Raw format the encoder is fed, where the codec lets the user choose.
    /// Without this the encoder follows the source's format, which is how
    /// H.264 ended up as High 4:4:4 (unplayable in most players).
    pub fn encoder_input_format(&self) -> Option<&'static str> {
        match self.video_codec {
            VideoCodec::H264 | VideoCodec::H265 => Some(match self.chroma {
                ChromaSubsampling::Yuv420 => "I420",
                ChromaSubsampling::Yuv422 => "Y42B",
                ChromaSubsampling::Yuv444 => "Y444",
            }),
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

    /// GStreamer audio encoder element appropriate for the container.
    pub fn audio_encoder_element(&self) -> &'static str {
        match self.container {
            Container::Mov | Container::Mp4 => "avenc_aac",
            Container::Mkv => "opusenc",
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

    /// Value of avenc_prores_ks's `profile` enum.
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

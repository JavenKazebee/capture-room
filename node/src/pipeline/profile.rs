use std::path::PathBuf;

use crate::api::types::{ChromaSubsampling, Container, PresetOutputInput, VideoCodec};

/// Values for the per-recording tokens of a path template.
pub struct PathVars {
    pub source: String,
    pub node: String,
    pub date: String,
    pub datetime: String,
}

/// Build every leg's profile and output path, rejecting a format that doesn't
/// parse or two legs that would write the same file.
///
/// With `vars` as `None` the per-recording tokens are left unexpanded, so the
/// check covers every future recording — what a preset save wants. `{output}`
/// and `{ext}` come from the leg itself; a leading `~` is this node's home.
pub fn plan_legs(
    outputs: &[PresetOutputInput],
    vars: Option<&PathVars>,
) -> Result<Vec<(PathBuf, RecordingProfile)>, &'static str> {
    let mut legs: Vec<(PathBuf, RecordingProfile)> = Vec::with_capacity(outputs.len());
    for o in outputs {
        let profile = RecordingProfile::from_output(o)?;
        let mut path = o.path_template.replace("{output}", &o.name).replace("{ext}", profile.file_extension());
        if let Some(v) = vars {
            path = path
                .replace("{source}", &v.source)
                .replace("{node}", &v.node)
                .replace("{date}", &v.date)
                .replace("{datetime}", &v.datetime);
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
                .ok_or("framerate must look like 30 or 30000/1001")?,
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

/// "30" → (30, 1); "30000/1001" → (30000, 1001)
fn parse_framerate(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    nonzero_pair(n, d)
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
        assert_eq!(parse_framerate("29.97"), None);
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
        let vars = PathVars { source: "cam1".into(), node: "n".into(), date: "d".into(), datetime: "dt".into() };
        let legs = plan_legs(&[leg("a", Container::Mp4, "~/rec/{source}_{datetime}.{ext}")], Some(&vars)).unwrap();
        assert_eq!(legs[0].0, std::env::home_dir().unwrap().join("rec/cam1_dt.mp4"));
        assert_eq!(expand_home("/abs/~x"), PathBuf::from("/abs/~x"));
        assert_eq!(expand_home("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn blank_means_match_source() {
        assert_eq!(parse_optional(&None, parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("  ".into()), parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("1920*1080".into()), parse_resolution), None);
    }
}

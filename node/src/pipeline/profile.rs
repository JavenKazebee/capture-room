use crate::api::types::{ChromaSubsampling, Container, PresetOutputInput, VideoCodec};

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
            Container::Mxf => "mxfmux",
        }
    }

    /// GStreamer audio encoder element appropriate for the container.
    pub fn audio_encoder_element(&self) -> &'static str {
        match self.container {
            Container::Mov | Container::Mp4 => "avenc_aac",
            Container::Mkv => "opusenc",
            Container::Mxf => "identity", // PCM passthrough; mxfmux accepts raw audio
        }
    }

    /// File extension for the output path template.
    pub fn file_extension(&self) -> &'static str {
        match self.container {
            Container::Mov => "mov",
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Mxf => "mxf",
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

    #[test]
    fn blank_means_match_source() {
        assert_eq!(parse_optional(&None, parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("  ".into()), parse_resolution), Some(None));
        assert_eq!(parse_optional(&Some("1920*1080".into()), parse_resolution), None);
    }
}

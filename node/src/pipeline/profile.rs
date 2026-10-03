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
    /// text; unparsable values mean "match the source".
    pub fn from_output(o: &PresetOutputInput) -> Self {
        Self {
            video_codec: o.codec,
            container: o.container,
            resolution: o.resolution.as_deref().and_then(parse_resolution),
            framerate: o.framerate.as_deref().and_then(parse_framerate),
            bitrate_kbps: o.bitrate_kbps,
            chroma: o.chroma,
        }
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

/// "1920x1080" → (1920, 1080)
fn parse_resolution(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// "30" → (30, 1); "30000/1001" → (30000, 1001)
fn parse_framerate(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if let Some((n, d)) = s.split_once('/') {
        Some((n.trim().parse().ok()?, d.trim().parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

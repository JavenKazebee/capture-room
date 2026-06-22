use anyhow::Result;

#[derive(Debug, Clone)]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    ProRes(ProResVariant),
    DnxHd,
    Uncompressed,
}

#[derive(Debug, Clone)]
pub enum ProResVariant {
    /// ProRes 4444
    P4444,
    /// ProRes 422 HQ
    P422Hq,
    /// ProRes 422
    P422,
    /// ProRes 422 LT
    P422Lt,
    /// ProRes 422 Proxy
    P422Proxy,
}

#[derive(Debug, Clone)]
pub enum Container {
    /// QuickTime MOV  (avmux_mov; qtmux when gst-plugins-good available)
    Mov,
    /// MPEG-4         (avmux_mp4; mp4mux when gst-plugins-good available)
    Mp4,
    /// Matroska MKV   (avmux_matroska; matroskamux when gst-plugins-good available)
    Mkv,
    /// Material eXchange Format
    Mxf,
}

/// Configures a single recording output leg (primary, secondary, or redundant).
#[derive(Debug, Clone)]
pub struct RecordingProfile {
    pub id: String,
    pub name: String,
    pub video_codec: VideoCodec,
    pub container: Container,
    /// `None` = match source resolution
    pub resolution: Option<(u32, u32)>,
    /// `None` = match source framerate (num, den)
    pub framerate: Option<(u32, u32)>,
    /// `None` = let the encoder pick based on `quality`
    pub bitrate_kbps: Option<u32>,
    pub quality: Option<String>,
    pub output_template: String,
}

impl RecordingProfile {
    /// GStreamer element name for the video encoder.
    pub fn video_encoder_element(&self) -> Result<&'static str> {
        Ok(match &self.video_codec {
            VideoCodec::H264 => "x264enc",
            VideoCodec::H265 => "x265enc",
            VideoCodec::Vp9 => "vp9enc",
            VideoCodec::ProRes(_) => "avenc_prores",
            VideoCodec::DnxHd => "avenc_dnxhd",
            VideoCodec::Uncompressed => "identity",
        })
    }

    /// GStreamer element name for the container muxer.
    pub fn muxer_element(&self) -> &'static str {
        match &self.container {
            Container::Mov => "qtmux",
            Container::Mp4 => "mp4mux",
            Container::Mkv => "matroskamux",
            Container::Mxf => "mxfmux",
        }
    }

    /// GStreamer audio encoder element appropriate for the container.
    pub fn audio_encoder_element(&self) -> Result<&'static str> {
        Ok(match &self.container {
            Container::Mov | Container::Mp4 => "avenc_aac",
            Container::Mkv => "opusenc",
            Container::Mxf => "identity", // PCM passthrough; mxfmux accepts raw audio
        })
    }

    /// File extension for the output path template.
    pub fn file_extension(&self) -> &'static str {
        match &self.container {
            Container::Mov => "mov",
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Mxf => "mxf",
        }
    }

    /// ProRes profile integer passed to avenc_prores.
    pub fn prores_profile_index(&self) -> Option<i32> {
        match &self.video_codec {
            VideoCodec::ProRes(v) => Some(match v {
                ProResVariant::P4444 => 4,
                ProResVariant::P422Hq => 0,
                ProResVariant::P422 => 2,
                ProResVariant::P422Lt => 1,
                ProResVariant::P422Proxy => 3,
            }),
            _ => None,
        }
    }
}

impl RecordingProfile {
    /// A sensible default for development: H.264 in a MOV container.
    pub fn h264_mov(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: "H.264 MOV".into(),
            video_codec: VideoCodec::H264,
            container: Container::Mov,
            resolution: None,
            framerate: None,
            bitrate_kbps: Some(8_000),
            quality: None,
            output_template: "/tmp/{source}_{datetime}.{ext}".into(),
        }
    }
}

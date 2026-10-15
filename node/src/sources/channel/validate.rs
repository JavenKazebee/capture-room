//! Checking a channel's config.

use crate::api::types::{ChannelConfig, OutputConfig};
use crate::pipeline::{output, rtsp, whep};
use crate::sources::stream;

/// Check a config before it's saved. `others` are the node's other
/// channels, whose NDI names this one mustn't reuse (listening ports are
/// checked against every source by the caller). `available` says whether
/// an element is installed on this node.
pub fn validate(
    cfg: &ChannelConfig,
    name: &str,
    others: &[(&str, &ChannelConfig)],
    available: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let f = &cfg.format;
    if f.width == 0 || f.height == 0 || f.width > 7680 || f.height > 4320 {
        return Err("the size must be from 1x1 up to 7680x4320".into());
    }
    if !f.width.is_multiple_of(2) || !f.height.is_multiple_of(2) {
        return Err("the width and height must be even".into());
    }
    if f.fps_num == 0 || f.fps_den == 0 || f.fps_num / f.fps_den > 240 {
        return Err("the frame rate must be above 0 and at most 240".into());
    }
    // Up to 7.1: clips are spread onto (and outputs mix down from) a
    // channel layout, and GStreamer has none beyond 8.
    if !(1..=8).contains(&cfg.audio_channels) {
        return Err("audio must have 1 to 8 channels".into());
    }
    for o in &cfg.outputs {
        match o {
            OutputConfig::Ndi { .. } if !available("ndisink") => {
                return Err("NDI outputs need ndisink, which isn't on this node".into());
            }
            OutputConfig::Ndi { .. } => {}
            OutputConfig::Srt { .. } => validate_srt(o, cfg.audio_channels, &available)?,
            OutputConfig::Rtsp { bitrate_kbps, .. } => {
                validate_rtsp(*bitrate_kbps, cfg.audio_channels, &available)?
            }
            OutputConfig::Whep { bitrate_kbps, .. } => validate_whep(*bitrate_kbps, &available)?,
            OutputConfig::Display { display, .. } => {
                if !available(output::DISPLAY_SINK) {
                    return Err(format!(
                        "display outputs need {}, which isn't installed on this node",
                        output::DISPLAY_SINK
                    ));
                }
                if display.trim().is_empty() {
                    return Err("pick a display".into());
                }
            }
        }
    }
    unique(
        cfg,
        name,
        others,
        |cfg, _| screens(cfg),
        |d| format!("two outputs both show on {d}"),
        |d, other| format!("{d} already shows {other}"),
    )?;
    let ports = listen_ports(cfg);
    for (i, port) in ports.iter().enumerate() {
        if ports[..i].contains(port) {
            return Err(format!("two outputs both listen on port {port}"));
        }
    }
    let ndi_names = |cfg: &ChannelConfig, name: &str| -> Vec<String> {
        cfg.outputs
            .iter()
            .filter_map(|o| match o {
                OutputConfig::Ndi { ndi_name } => Some(
                    ndi_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(name)
                        .to_lowercase(),
                ),
                _ => None,
            })
            .collect()
    };
    unique(
        cfg,
        name,
        others,
        mount_paths,
        |p| format!("two outputs are both served at {p}"),
        |p, other| format!("RTSP path {p} is already used by {other}"),
    )?;
    unique(
        cfg,
        name,
        others,
        whep_paths,
        |p| format!("two WHEP outputs are both served at {p}"),
        |p, other| format!("WHEP path {p} is already used by {other}"),
    )?;
    unique(
        cfg,
        name,
        others,
        ndi_names,
        |n| format!("two outputs are both called {n}"),
        |n, other| format!("NDI name {n} is already used by {other}"),
    )?;
    Ok(())
}

/// Check that none of the `keys` of the channel `name` (screens, paths,
/// names) appears twice in it, or in one of the `others`: `twice` and
/// `taken` say which went wrong.
fn unique(
    cfg: &ChannelConfig,
    name: &str,
    others: &[(&str, &ChannelConfig)],
    keys: impl Fn(&ChannelConfig, &str) -> Vec<String>,
    twice: impl Fn(&str) -> String,
    taken: impl Fn(&str, &str) -> String,
) -> Result<(), String> {
    let mine = keys(cfg, name);
    for (i, key) in mine.iter().enumerate() {
        if mine[..i].contains(key) {
            return Err(twice(key));
        }
        if let Some((other, _)) = others
            .iter()
            .find(|(other, ocfg)| keys(ocfg, other).contains(key))
        {
            return Err(taken(key, other));
        }
    }
    Ok(())
}

fn validate_srt(
    output: &OutputConfig,
    audio_channels: u32,
    available: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let OutputConfig::Srt {
        url,
        latency_ms,
        bitrate_kbps,
        passphrase,
    } = output
    else {
        return Ok(());
    };
    if let Some(missing) = output::SRT_ELEMENTS.iter().find(|e| !available(e)) {
        return Err(format!(
            "SRT outputs need {missing}, which isn't installed on this node"
        ));
    }
    if !output::LIVE_H264.iter().any(|e| available(e)) {
        return Err(
            "SRT outputs need an H.264 encoder (x264enc), which isn't installed on this node"
                .into(),
        );
    }
    let url = url.trim();
    if stream::scheme(url).as_deref() != Some("srt") {
        return Err("an SRT output needs an srt:// URL, like srt://:9000".into());
    }
    let (host, port) = stream::host_port(url);
    if port.is_none() {
        return Err(format!("{url} needs a port, like srt://:9000"));
    }
    if host.is_empty() && stream::url_listen_port(url).is_none() {
        return Err(format!("{url} needs a host"));
    }
    if !(20..=8000).contains(latency_ms) {
        return Err("SRT latency must be 20–8000 ms".into());
    }
    if !(100..=100_000).contains(bitrate_kbps) {
        return Err("SRT bitrate must be 100–100,000 kbps".into());
    }
    if let Some(p) = passphrase.as_deref().filter(|p| !p.is_empty()) {
        if !(10..=79).contains(&p.chars().count()) {
            return Err("an SRT passphrase must be 10–79 characters".into());
        }
    }
    if audio_channels > output::SRT_MAX_AUDIO_CHANNELS {
        return Err(format!(
            "SRT outputs carry up to {} audio channels; this channel has {audio_channels}",
            output::SRT_MAX_AUDIO_CHANNELS
        ));
    }
    Ok(())
}

fn validate_rtsp(
    bitrate_kbps: u32,
    audio_channels: u32,
    available: impl Fn(&str) -> bool,
) -> Result<(), String> {
    if let Some(missing) = rtsp::RTSP_ELEMENTS.iter().find(|e| !available(e)) {
        return Err(format!(
            "RTSP outputs need {missing}, which isn't installed on this node"
        ));
    }
    if !output::LIVE_H264.iter().any(|e| available(e)) {
        return Err(
            "RTSP outputs need an H.264 encoder (x264enc), which isn't installed on this node"
                .into(),
        );
    }
    if !(100..=100_000).contains(&bitrate_kbps) {
        return Err("RTSP bitrate must be 100–100,000 kbps".into());
    }
    if audio_channels > output::SRT_MAX_AUDIO_CHANNELS {
        return Err(format!(
            "RTSP outputs carry up to {} audio channels; this channel has {audio_channels}",
            output::SRT_MAX_AUDIO_CHANNELS
        ));
    }
    Ok(())
}

fn validate_whep(bitrate_kbps: u32, available: impl Fn(&str) -> bool) -> Result<(), String> {
    if let Some(missing) = whep::WHEP_ELEMENTS.iter().find(|e| !available(e)) {
        return Err(format!(
            "WHEP outputs need {missing}, which isn't installed on this node"
        ));
    }
    if !(100..=100_000).contains(&bitrate_kbps) {
        return Err("WHEP bitrate must be 100–100,000 kbps".into());
    }
    Ok(())
}

/// The screens a channel's display outputs show on.
pub fn screens(cfg: &ChannelConfig) -> Vec<String> {
    cfg.outputs
        .iter()
        .filter_map(|o| match o {
            OutputConfig::Display { display, .. } => Some(display.trim().to_string()),
            _ => None,
        })
        .collect()
}

/// Where a channel's RTSP outputs are served (`name` is the channel's).
pub fn mount_paths(cfg: &ChannelConfig, name: &str) -> Vec<String> {
    cfg.outputs
        .iter()
        .filter_map(|o| match o {
            OutputConfig::Rtsp { path, .. } => Some(rtsp::mount_path(path.as_deref(), name)),
            _ => None,
        })
        .collect()
}

/// Where a channel's WHEP outputs are served (`name` is the channel's).
pub fn whep_paths(cfg: &ChannelConfig, name: &str) -> Vec<String> {
    cfg.outputs
        .iter()
        .filter_map(|o| match o {
            OutputConfig::Whep { path, .. } => Some(rtsp::mount_path(path.as_deref(), name)),
            _ => None,
        })
        .collect()
}

/// The local ports a channel's outputs listen on (SRT listeners; RTSP
/// outputs share the node's RTSP server).
pub fn listen_ports(cfg: &ChannelConfig) -> Vec<u16> {
    cfg.outputs
        .iter()
        .filter_map(|o| match o {
            OutputConfig::Srt { url, .. } => stream::url_listen_port(url),
            _ => None,
        })
        .collect()
}

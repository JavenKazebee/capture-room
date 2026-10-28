use crate::api::types::{ChannelConfig, ConfiguredSourceDto, OutputConfig};
use crate::pipeline::{output, rtsp, whep};
use crate::sources::device::LocalDevices;
use crate::sources::{decklink, stream};

/// Check a config before it's saved. `others` are the node's other channels,
/// whose NDI names this one mustn't reuse (the caller checks listening ports).
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
    // A channel layout is needed (clips spread onto it, outputs mix down), and
    // GStreamer has none beyond 8.
    if !(1..=8).contains(&cfg.audio_channels) {
        return Err("audio must have 1 to 8 channels".into());
    }
    let available: &dyn Fn(&str) -> bool = &available;
    let channels = cfg.audio_channels;
    for o in &cfg.outputs {
        match o {
            OutputConfig::Ndi { .. } => needs("NDI", &["ndisink"], available)?,
            OutputConfig::Srt { .. } => validate_srt(o, channels, available)?,
            OutputConfig::Rtsp { bitrate_kbps, .. } => {
                needs("RTSP", rtsp::RTSP_ELEMENTS, available)?;
                needs_h264("RTSP", available)?;
                bitrate("RTSP", *bitrate_kbps)?;
                max_channels("RTSP", channels)?;
            }
            OutputConfig::Whep { bitrate_kbps, .. } => {
                needs("WHEP", whep::WHEP_ELEMENTS, available)?;
                bitrate("WHEP", *bitrate_kbps)?;
            }
            OutputConfig::Decklink { device, mode } => {
                needs("SDI", decklink::OUTPUT_ELEMENTS, available)?;
                if device.trim().is_empty() {
                    return Err("pick a Decklink sub-device".into());
                }
                if decklink::output_mode(*f, mode.as_deref())?.interlaced {
                    needs("interlaced SDI", decklink::INTERLACE_ELEMENTS, available)?;
                }
                if channels > 16 {
                    return Err("SDI carries at most 16 audio channels".into());
                }
            }
            OutputConfig::Display { display, .. } => {
                needs("display", &[output::DISPLAY_SINK], available)?;
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
    let ndi_names = |cfg: &ChannelConfig, name: &str| {
        each(cfg, |o| match o {
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

/// Check channel `name`'s outputs against the node's devices: its Decklink
/// sub-devices (free, or used by this channel `id`, as `configs` say) and
/// its displays' audio outputs.
pub fn validate_devices(
    cfg: &ChannelConfig,
    name: &str,
    id: Option<&str>,
    devices: &LocalDevices,
    configs: &[ConfiguredSourceDto],
) -> Result<(), String> {
    decklink::validate_outputs(&cfg.outputs, devices, id, name, configs)?;
    for o in &cfg.outputs {
        if let OutputConfig::Display {
            audio_device: Some(key),
            ..
        } = o
        {
            if !key.is_empty() && !devices.has_output(key) {
                return Err(format!("audio output {key} isn't connected"));
            }
        }
    }
    Ok(())
}

/// Check none of channel `name`'s `keys` (screens, paths, names) appears twice
/// in it or in `others`; `twice` and `taken` are the errors.
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

/// `kind` outputs need every one of `elements`.
fn needs(kind: &str, elements: &[&str], available: &dyn Fn(&str) -> bool) -> Result<(), String> {
    match elements.iter().find(|e| !available(e)) {
        Some(missing) => Err(format!("{kind} outputs need {missing}: see Setup › Checks")),
        None => Ok(()),
    }
}

/// `kind` outputs need one of the live H.264 encoders.
fn needs_h264(kind: &str, available: &dyn Fn(&str) -> bool) -> Result<(), String> {
    if output::LIVE_H264.iter().any(|e| available(e)) {
        return Ok(());
    }
    Err(format!(
        "{kind} outputs need an H.264 encoder (x264enc): see Setup › Checks"
    ))
}

fn bitrate(kind: &str, kbps: u32) -> Result<(), String> {
    if !(100..=100_000).contains(&kbps) {
        return Err(format!("{kind} bitrate must be 100–100,000 kbps"));
    }
    Ok(())
}

/// MPEG-TS outputs (SRT, RTSP) carry at most `SRT_MAX_AUDIO_CHANNELS`.
fn max_channels(kind: &str, audio_channels: u32) -> Result<(), String> {
    if audio_channels > output::SRT_MAX_AUDIO_CHANNELS {
        return Err(format!(
            "{kind} outputs carry up to {} audio channels; this channel has {audio_channels}",
            output::SRT_MAX_AUDIO_CHANNELS
        ));
    }
    Ok(())
}

fn validate_srt(
    output: &OutputConfig,
    audio_channels: u32,
    available: &dyn Fn(&str) -> bool,
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
    needs("SRT", output::SRT_ELEMENTS, available)?;
    needs_h264("SRT", available)?;
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
    bitrate("SRT", *bitrate_kbps)?;
    if let Some(p) = passphrase.as_deref().filter(|p| !p.is_empty()) {
        if !(10..=79).contains(&p.chars().count()) {
            return Err("an SRT passphrase must be 10–79 characters".into());
        }
    }
    max_channels("SRT", audio_channels)
}

fn each<T>(cfg: &ChannelConfig, f: impl FnMut(&OutputConfig) -> Option<T>) -> Vec<T> {
    cfg.outputs.iter().filter_map(f).collect()
}

/// The screens a channel's display outputs show on.
pub fn screens(cfg: &ChannelConfig) -> Vec<String> {
    each(cfg, |o| match o {
        OutputConfig::Display { display, .. } => Some(display.trim().to_string()),
        _ => None,
    })
}

/// Where a channel's RTSP outputs are served (`name` is the channel's).
pub fn mount_paths(cfg: &ChannelConfig, name: &str) -> Vec<String> {
    each(cfg, |o| match o {
        OutputConfig::Rtsp { path, .. } => Some(rtsp::mount_path(path.as_deref(), name)),
        _ => None,
    })
}

/// Where a channel's WHEP outputs are served (`name` is the channel's).
pub fn whep_paths(cfg: &ChannelConfig, name: &str) -> Vec<String> {
    each(cfg, |o| match o {
        OutputConfig::Whep { path, .. } => Some(rtsp::mount_path(path.as_deref(), name)),
        _ => None,
    })
}

/// The local ports a channel's outputs listen on (SRT listeners; RTSP shares
/// the node's server).
pub fn listen_ports(cfg: &ChannelConfig) -> Vec<u16> {
    each(cfg, |o| match o {
        OutputConfig::Srt { url, .. } => stream::url_listen_port(url),
        _ => None,
    })
}

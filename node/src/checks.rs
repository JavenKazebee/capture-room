//! Setup checks: what each feature needs from the machine, and whether it
//! has it. Everything is read live (permissions get revoked, machines get
//! reinstalled), so nothing here is stored. Network reachability can't be
//! seen from inside, so it's probed by a controller (`probe_ports`).

use std::path::PathBuf;
use std::time::Duration;

use crate::api::types::{
    AudioPlan, CheckDto, CheckFix, CheckGroup, CheckLink, CheckStatus, ClockSourceDto,
    ClockStatusDto, ConfiguredSourceDto, DeviceDto, DeviceKind, NodeChecksDto, NodePortDto,
    OutputConfig, PortProtocol, SourceConfig,
};
use crate::sources::privacy::{self, Access, Permission};
use crate::state::AppState;
use crate::storage;

/// The folder the default output template records into.
const DEFAULT_FOLDER: &str = "capture-room";

pub async fn run(state: &AppState) -> anyhow::Result<NodeChecksDto> {
    let configured = crate::db::configured_sources_list(&state.db).await?;
    let (devices, ndi_loaded) = {
        let manager = state.source_manager.read().await;
        (manager.devices().list(), manager.ndi_runtime_loaded())
    };
    let clock = state.clock_status().await;

    let holder = permission_holder();
    let mut checks = Vec::new();
    for permission in [
        Permission::ScreenRecording,
        Permission::Camera,
        Permission::Microphone,
    ] {
        if let Some(access) = privacy::access(permission) {
            let used_by = needing(&configured, &devices, permission);
            checks.push(permission_check(
                permission,
                access,
                used_by,
                holder.as_deref(),
            ));
        }
    }
    checks.push(ndi_check(ndi_loaded, &configured));
    checks.push(tokio::task::spawn_blocking(storage_check).await?);
    checks.extend(clock_check(&clock));

    Ok(NodeChecksDto {
        os: std::env::consts::OS.to_string(),
        permission_holder: holder,
        checks,
        ports: ports(state.port, &configured),
    })
}

fn check(id: &str, group: CheckGroup, label: &str, needed_for: &str) -> CheckDto {
    CheckDto {
        id: id.to_string(),
        group,
        label: label.to_string(),
        status: CheckStatus::Ok,
        summary: String::new(),
        needed_for: needed_for.to_string(),
        used_by: Vec::new(),
        required: true,
        fix: None,
    }
}

// ── macOS privacy ─────────────────────────────────────────────────────────────

/// The `id` of a permission's check.
pub fn permission_id(permission: Permission) -> &'static str {
    match permission {
        Permission::Camera => "camera",
        Permission::Microphone => "microphone",
        Permission::ScreenRecording => "screen_recording",
    }
}

pub fn permission_of(id: &str) -> Option<Permission> {
    [
        Permission::Camera,
        Permission::Microphone,
        Permission::ScreenRecording,
    ]
    .into_iter()
    .find(|p| permission_id(*p) == id)
}

fn permission_check(
    permission: Permission,
    access: Access,
    used_by: Vec<String>,
    holder: Option<&str>,
) -> CheckDto {
    let (label, needed_for) = match permission {
        Permission::Camera => ("Camera", "Capture cards, USB HDMI dongles and webcams"),
        Permission::Microphone => (
            "Microphone",
            "Audio from audio devices: interfaces, a dongle's own audio, Dante Virtual Soundcard",
        ),
        Permission::ScreenRecording => ("Screen Recording", "Capturing this machine's screens"),
    };
    let mut c = check(
        permission_id(permission),
        CheckGroup::Permissions,
        label,
        needed_for,
    );
    c.required = !used_by.is_empty();
    c.used_by = used_by;
    let pane = permission.pane();
    let who = match holder {
        Some(app) => format!("{app} (the app the node runs under)"),
        None => "the app the node runs under".to_string(),
    };
    let settings = |steps: String, can_request: bool| CheckFix {
        steps,
        settings_url: Some(permission.settings_url().to_string()),
        can_request,
        ..Default::default()
    };
    match access {
        Access::Granted => c.summary = "Allowed.".into(),
        Access::NotAsked => {
            c.status = CheckStatus::NotAsked;
            c.summary = "macOS hasn't asked yet; it will the first time a source needs it.".into();
            c.fix = Some(settings(
                "Ask macOS now, and allow it in the prompt on this machine.".into(),
                true,
            ));
        }
        Access::Denied => {
            c.status = CheckStatus::Missing;
            // macOS doesn't say whether it has asked about screen recording,
            // and only ever asks once, so offering its prompt would mostly do
            // nothing.
            if permission == Permission::ScreenRecording {
                c.summary = "Not allowed (or not asked yet).".into();
                c.fix = Some(settings(
                    format!(
                        "In System Settings › Privacy & Security › {pane}, turn it on for {}; \
                         if it isn't listed, add it with +. Then restart the node: macOS \
                         applies it on the next launch.",
                        who
                    ),
                    false,
                ));
            } else {
                c.summary = "Denied.".into();
                c.fix = Some(settings(
                    format!(
                        "Allow it in System Settings › Privacy & Security › {pane} for {}.",
                        who
                    ),
                    false,
                ));
            }
        }
    }
    c
}

/// Names of the configured sources that need `permission`.
fn needing(
    configured: &[ConfiguredSourceDto],
    devices: &[DeviceDto],
    permission: Permission,
) -> Vec<String> {
    let kind_of = |key: &str, name: &str| {
        devices
            .iter()
            .find(|d| d.key == key)
            .map(|d| d.kind)
            // Unplugged: go by its name, as the device list does.
            .unwrap_or(if name.to_lowercase().contains("screen") {
                DeviceKind::Screen
            } else {
                DeviceKind::Video
            })
    };
    let audio_device = |plan: &AudioPlan| matches!(plan, AudioPlan::Device { .. });
    configured
        .iter()
        .filter(|s| match (&s.config, permission) {
            (SourceConfig::Device(cfg), Permission::Camera) => {
                kind_of(&cfg.video_device, &cfg.video_device_name) == DeviceKind::Video
            }
            (SourceConfig::Device(cfg), Permission::ScreenRecording) => {
                kind_of(&cfg.video_device, &cfg.video_device_name) == DeviceKind::Screen
            }
            (SourceConfig::Device(cfg), Permission::Microphone) => audio_device(&cfg.audio),
            (SourceConfig::Stream(cfg), Permission::Microphone) => audio_device(&cfg.audio),
            (SourceConfig::Whip(cfg), Permission::Microphone) => audio_device(&cfg.audio),
            _ => false,
        })
        .map(|s| s.name.clone())
        .collect()
}

/// The app macOS grants privacy permissions to: the nearest app bundle
/// among the node's ancestors (outermost, for helpers nested in an app), or
/// the node itself when launchd started it.
#[cfg(target_os = "macos")]
fn permission_holder() -> Option<String> {
    use std::sync::OnceLock;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    fn app_of(exe: &std::path::Path) -> Option<String> {
        exe.components()
            .filter_map(|c| c.as_os_str().to_str())
            .find_map(|c| c.strip_suffix(".app"))
            .map(str::to_string)
    }

    static HOLDER: OnceLock<String> = OnceLock::new();
    let holder = HOLDER.get_or_init(|| {
        let mut sys = System::new();
        let kind = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
        let mut pid = Pid::from_u32(std::process::id());
        // Bounded, in case of a cycle.
        for _ in 0..32 {
            sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
            let Some(process) = sys.process(pid) else {
                break;
            };
            if let Some(app) = process.exe().and_then(app_of) {
                return app;
            }
            match process.parent() {
                Some(parent) if parent.as_u32() > 1 => pid = parent,
                _ => break,
            }
        }
        "capture-room".to_string()
    });
    Some(holder.clone())
}

#[cfg(not(target_os = "macos"))]
fn permission_holder() -> Option<String> {
    None
}

// ── NDI ───────────────────────────────────────────────────────────────────────

fn ndi_check(loaded: bool, configured: &[ConfiguredSourceDto]) -> CheckDto {
    let mut c = check(
        "ndi_runtime",
        CheckGroup::Software,
        "NDI Runtime",
        "Finding and capturing NDI sources, and NDI outputs on playout channels",
    );
    c.used_by = configured
        .iter()
        .filter(|s| {
            matches!(&s.config, SourceConfig::Channel(cfg)
                if cfg.outputs.iter().any(|o| matches!(o, OutputConfig::Ndi { .. })))
        })
        .map(|s| s.name.clone())
        .collect();
    if loaded {
        c.summary = match ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"]
            .iter()
            .find_map(|v| std::env::var(v).ok())
        {
            Some(dir) => format!("Loaded from {dir}."),
            None => "Loaded.".into(),
        };
    } else {
        c.status = CheckStatus::Missing;
        c.summary = "The NDI library couldn't be loaded.".into();
        c.fix = Some(CheckFix {
            steps: "Install NDI Tools (or the NDI Runtime) on this machine, then restart the \
                    node: it only looks for the library when it starts."
                .into(),
            link: Some(CheckLink {
                url: "https://ndi.video/tools/".into(),
                label: "Download NDI Tools".into(),
            }),
            ..Default::default()
        });
    }
    c
}

// ── Storage ───────────────────────────────────────────────────────────────────

/// The default recording folder can be written, with room to record.
/// Blocking.
fn storage_check() -> CheckDto {
    let mut c = check(
        "storage",
        CheckGroup::Storage,
        "Recording folder",
        "Recording with the default output path",
    );
    let Some(dir) = std::env::home_dir().map(|h| h.join(DEFAULT_FOLDER)) else {
        c.status = CheckStatus::Unknown;
        c.summary = "This node has no home folder.".into();
        return c;
    };
    let shown = format!("~/{DEFAULT_FOLDER}");
    if let Err(e) = write_test(&dir) {
        c.status = CheckStatus::Missing;
        c.summary = format!("Can't write to {shown}: {e}.");
        c.fix = Some(CheckFix {
            steps: format!(
                "Make {} writable by the user the node runs as, or record somewhere else by \
                 changing the presets' output paths.",
                dir.display()
            ),
            ..Default::default()
        });
        return c;
    }
    let volumes = storage::list_volumes();
    let Some(volume) = storage::volume_of(&dir, &volumes).map(|i| &volumes[i]) else {
        c.summary = format!("{shown} is writable.");
        return c;
    };
    let free = format_bytes(volume.available_bytes);
    if volume.available_bytes < storage::MIN_FREE_TO_RECORD {
        c.status = CheckStatus::Missing;
        c.summary = format!("{shown} is writable, but only {free} is free on its volume.");
        c.fix = Some(CheckFix {
            steps: format!(
                "Free up space on {}: recordings onto a volume with under {} free are refused.",
                volume.mount_point,
                format_bytes(storage::MIN_FREE_TO_RECORD)
            ),
            ..Default::default()
        });
    } else {
        c.summary = format!("{shown} is writable, {free} free.");
    }
    c
}

fn write_test(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let file: PathBuf = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&file, b"")?;
    std::fs::remove_file(&file)
}

fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let gb = bytes as f64 / GB;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

// ── Clock ─────────────────────────────────────────────────────────────────────

/// Whether the shared clock this node is meant to follow reaches it. None
/// on a node running on its own clock.
fn clock_check(clock: &ClockStatusDto) -> Option<CheckDto> {
    let wanted = clock.pending.as_ref().unwrap_or(&clock.source);
    let switching = clock.pending.is_some();
    match wanted {
        ClockSourceDto::Local => None,
        ClockSourceDto::Controller {
            controller_name,
            address,
            port,
            ..
        } => {
            let mut c = check(
                "controller_clock",
                CheckGroup::Clock,
                "Controller's clock",
                "Synchronized starts and frame times that line up across nodes",
            );
            if let Some(error) = &clock.error {
                c.status = CheckStatus::Missing;
                c.summary = error.clone();
            } else if switching {
                c.status = CheckStatus::Unknown;
                c.summary = format!("Syncing to {controller_name}'s clock…");
            } else if clock.lost {
                c.status = CheckStatus::Missing;
                c.summary = format!(
                    "No word from {controller_name}'s clock at {address}:{port} lately; \
                     running on its last known rate."
                );
            } else {
                c.summary = format!("Following {controller_name}'s clock.");
                return Some(c);
            }
            if c.status == CheckStatus::Missing {
                c.fix = Some(CheckFix {
                    steps: format!(
                        "Allow incoming UDP {port} on {controller_name}'s firewall, and check \
                         that this node can reach {address}."
                    ),
                    ..Default::default()
                });
            }
            Some(c)
        }
        ClockSourceDto::Ptp { domain } => {
            let mut c = check(
                "ptp",
                CheckGroup::Clock,
                "PTP clock",
                "Following a PTP grandmaster (UDP 319 and 320)",
            );
            if let Some(error) = &clock.error {
                c.status = CheckStatus::Missing;
                c.summary = error.clone();
                c.fix = Some(ptp_fix());
            } else if switching || !clock.synced {
                c.status = CheckStatus::Unknown;
                c.summary = format!("Waiting for a grandmaster on PTP domain {domain}…");
            } else if clock.lost {
                c.status = CheckStatus::Missing;
                c.summary = format!("No word from the grandmaster on domain {domain} lately.");
            } else {
                c.summary = format!("Following the grandmaster on PTP domain {domain}.");
            }
            Some(c)
        }
    }
}

fn ptp_fix() -> CheckFix {
    if cfg!(target_os = "linux") {
        CheckFix {
            steps: "GStreamer's PTP helper needs permission to bind UDP 319 and 320.".into(),
            command: Some(
                "sudo setcap cap_net_bind_service,cap_net_admin+ep \
                 \"$(find /usr/lib* /usr/libexec -name gst-ptp-helper 2>/dev/null | head -1)\""
                    .into(),
            ),
            ..Default::default()
        }
    } else {
        CheckFix {
            steps: "Check that GStreamer's PTP helper (gst-ptp-helper) is installed and allowed \
                    to bind UDP 319 and 320, and that nothing else on this machine uses them."
                .into(),
            ..Default::default()
        }
    }
}

// ── Network ───────────────────────────────────────────────────────────────────

/// Ports others connect to on this node.
fn ports(http_port: u16, configured: &[ConfiguredSourceDto]) -> Vec<NodePortDto> {
    let port = |port, protocol, label: &str| NodePortDto {
        port,
        protocol,
        label: label.to_string(),
    };
    let mut ports = vec![port(http_port, PortProtocol::Tcp, "API and web UI")];
    let mut rtsp = false;
    for s in configured {
        match &s.config {
            SourceConfig::Whip(cfg) => ports.push(port(cfg.port, PortProtocol::Tcp, &s.name)),
            SourceConfig::Stream(cfg) => {
                if let Some(p) = crate::sources::stream::listen_port(cfg) {
                    ports.push(port(p, PortProtocol::Udp, &s.name));
                }
            }
            SourceConfig::Channel(cfg) => {
                for p in crate::sources::channel::listen_ports(cfg) {
                    ports.push(port(p, PortProtocol::Udp, &s.name));
                }
                rtsp |= cfg
                    .outputs
                    .iter()
                    .any(|o| matches!(o, OutputConfig::Rtsp { .. }));
            }
            _ => {}
        }
    }
    if rtsp {
        ports.push(port(
            crate::pipeline::rtsp::RTSP_PORT,
            PortProtocol::Tcp,
            "RTSP outputs",
        ));
    }
    ports
}

/// The controller's own ports: it can't test its firewall from inside.
pub fn own_firewall() -> CheckDto {
    let mut c = check(
        "port:self",
        CheckGroup::Network,
        "Incoming connections",
        "Other machines reaching this one",
    );
    c.status = CheckStatus::Unknown;
    c.summary = "A machine can't test its own firewall: another controller can, or check from \
                 a node that follows this one's clock."
        .into();
    c.required = false;
    c
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Probe a node's ports from this machine (a controller): `host` is how
/// the controller reaches it, `os` the node's (for the fix).
pub async fn probe_ports(host: &str, os: &str, ports: &[NodePortDto]) -> Vec<CheckDto> {
    let probes = ports.iter().map(|p| async move {
        let what = match p.protocol {
            PortProtocol::Tcp => "TCP",
            PortProtocol::Udp => "UDP",
        };
        let mut c = check(
            &format!("port:{}:{}", what.to_lowercase(), p.port),
            CheckGroup::Network,
            &format!("{what} {}", p.port),
            &p.label,
        );
        if p.protocol == PortProtocol::Udp {
            // Nothing answers a stray UDP packet, open or not.
            c.status = CheckStatus::Unknown;
            c.summary = "UDP can't be tested from outside; make sure it's allowed in.".into();
            c.fix = Some(firewall_fix(os, p));
            return c;
        }
        let connect = tokio::net::TcpStream::connect((host, p.port));
        match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
            Ok(Ok(_)) => c.summary = "Reachable from the controller.".into(),
            Ok(Err(e)) => {
                c.status = CheckStatus::Missing;
                c.summary = if e.kind() == std::io::ErrorKind::ConnectionRefused {
                    "Refused: nothing listening, or a firewall rejecting it.".into()
                } else {
                    format!("Couldn't connect from the controller: {e}.")
                };
                c.fix = Some(firewall_fix(os, p));
            }
            Err(_) => {
                c.status = CheckStatus::Missing;
                c.summary = "No answer from the controller: likely blocked by a firewall.".into();
                c.fix = Some(firewall_fix(os, p));
            }
        }
        c
    });
    futures_util::future::join_all(probes).await
}

fn firewall_fix(os: &str, p: &NodePortDto) -> CheckFix {
    let proto = match p.protocol {
        PortProtocol::Tcp => "tcp",
        PortProtocol::Udp => "udp",
    };
    let port = p.port;
    match os {
        "macos" => CheckFix {
            steps: "If the macOS firewall is on, allow incoming connections for the node's app in \
                    System Settings › Network › Firewall › Options."
                .into(),
            settings_url: Some(
                "x-apple.systempreferences:com.apple.preference.security?Firewall".into(),
            ),
            ..Default::default()
        },
        "windows" => CheckFix {
            steps: "Allow it in through Windows Firewall (in an administrator PowerShell):".into(),
            command: Some(format!(
                "New-NetFirewallRule -DisplayName \"Capture Room {proto} {port}\" -Direction \
                 Inbound -Protocol {} -LocalPort {port} -Action Allow",
                proto.to_uppercase()
            )),
            ..Default::default()
        },
        _ => CheckFix {
            steps: "Allow it in through the firewall, e.g. with ufw:".into(),
            command: Some(format!("sudo ufw allow {port}/{proto}")),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(port: u16, protocol: PortProtocol) -> NodePortDto {
        NodePortDto {
            port,
            protocol,
            label: "test".into(),
        }
    }

    #[tokio::test]
    async fn probes_tcp_ports_and_leaves_udp_unknown() {
        let open = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_port = open.local_addr().unwrap().port();
        // Bound and dropped: nothing listens there now.
        let closed_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let ports = [
            port(open_port, PortProtocol::Tcp),
            port(closed_port, PortProtocol::Tcp),
            port(9000, PortProtocol::Udp),
        ];
        let checks = probe_ports("127.0.0.1", "windows", &ports).await;
        let status: Vec<_> = checks.iter().map(|c| c.status).collect();
        assert_eq!(
            status,
            [CheckStatus::Ok, CheckStatus::Missing, CheckStatus::Unknown]
        );
        assert_eq!(checks[1].id, format!("port:tcp:{closed_port}"));
        let command = checks[1].fix.as_ref().unwrap().command.as_deref().unwrap();
        assert!(command.contains(&format!("-Protocol TCP -LocalPort {closed_port}")));
    }
}

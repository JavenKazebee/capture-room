//! The menu bar icon (system tray on Windows) of a node running as a service.
//! It lives on the window loop's thread (see [`crate::display`]), which gets
//! the node's [`Status`] and hands the node the [`Action`]s that need it.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::api::types::{UpdatePhase, UpdateStatusDto};
use crate::display::Displays;
use crate::state::AppState;

/// What the menu shows, sent by the node whenever it changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub recordings: usize,
    pub playing: usize,
    pub update: UpdateState,
    /// What a controller adds this node with (`ABCD-EFGH`).
    pub pairing_code: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum UpdateState {
    #[default]
    None,
    Available(String),
    /// Updating: what it's doing (`Downloading 0.2.0 (40%)`).
    Running(String),
    Failed(String),
}

/// What the menu asks the node to do. There's no menu on Linux yet.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// Open the UI in the browser, signed in.
    Open,
    Quit,
    Update,
    CheckForUpdates,
    /// Someone at this machine lost the token: whoever can use the menu bar
    /// is trusted with it.
    CopyAccessToken,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub struct Config {
    pub actions: UnboundedSender<Action>,
}

impl From<&UpdateStatusDto> for UpdateState {
    fn from(status: &UpdateStatusDto) -> Self {
        let latest = status.latest.clone().unwrap_or_default();
        match status.phase {
            UpdatePhase::Checking => Self::Running("Checking for updates…".into()),
            UpdatePhase::Downloading => Self::Running(match status.percent {
                Some(p) => format!("Downloading {latest} ({p}%)"),
                None => format!("Downloading {latest}"),
            }),
            UpdatePhase::Installing => Self::Running(format!("Installing {latest}…")),
            UpdatePhase::Restarting => Self::Running(format!("Restarting into {latest}…")),
            UpdatePhase::Idle => match &status.error {
                Some(e) => Self::Failed(e.clone()),
                None if status.available => Self::Available(latest),
                None => Self::None,
            },
        }
    }
}

/// Keep the icon's menu up to date, and do what's picked from it.
pub fn spawn(state: Arc<AppState>, displays: Displays, mut actions: UnboundedReceiver<Action>) {
    let watched = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            let activity = crate::session::activity(&watched).await;
            displays.show_status(Status {
                recordings: activity.recordings,
                playing: activity.playing,
                update: (&watched.updater.status()).into(),
                pairing_code: watched.access_token.pairing_code(),
            });
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    tokio::spawn(async move {
        while let Some(action) = actions.recv().await {
            match action {
                Action::Open => {
                    let url = crate::auth::sign_in_link(&state.db, state.port)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "couldn't make a sign-in link");
                            format!("http://localhost:{}", state.port)
                        });
                    crate::service::open_url(&url);
                }
                Action::Quit => crate::session::quit(&state, 0).await,
                Action::Update => {
                    if let Err(e) = crate::update::start(&state).await {
                        state.updater.fail(e.to_string());
                    }
                }
                Action::CheckForUpdates => crate::update::check(&state).await,
                Action::CopyAccessToken => {
                    if let Some(token) = state.access_token.get() {
                        match crate::service::copy_to_clipboard(&token) {
                            Ok(()) => tracing::info!("access token copied from the menu bar"),
                            Err(e) => tracing::warn!(error = %e, "couldn't copy the access token"),
                        }
                    }
                }
            }
        }
    });
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use imp::Tray;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use anyhow::Result;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::{Action, Config, Status, UpdateState};
    use crate::service;

    pub struct Tray {
        config: Config,
        icon: TrayIcon,
        status_item: MenuItem,
        open: MenuItem,
        update: MenuItem,
        pairing: MenuItem,
        copy_token: MenuItem,
        at_login: CheckMenuItem,
        quit: MenuItem,
        status: Status,
    }

    impl Tray {
        /// Show the icon. `on_menu` gets the id of each item picked, to
        /// pass back to [`Tray::picked`] on this thread.
        pub fn new(
            config: Config,
            on_menu: impl Fn(MenuId) + Send + Sync + 'static,
        ) -> Result<Self> {
            let version = MenuItem::new(
                format!("Capture Room {}", env!("CARGO_PKG_VERSION")),
                false,
                None,
            );
            let status_item = MenuItem::new("Idle", false, None);
            let open = MenuItem::new("Open Capture Room", true, None);
            let update = MenuItem::new("Check for Updates", true, None);
            // Shown, not clickable: read it off here into the controller.
            let pairing = MenuItem::new("Pairing Code", false, None);
            let copy_token = MenuItem::new("Copy Access Token", true, None);
            let at_login =
                CheckMenuItem::new("Start at Login", true, service::start_at_login(), None);
            let quit = MenuItem::new("Quit Capture Room", true, None);
            let menu = Menu::with_items(&[
                &version,
                &status_item,
                &PredefinedMenuItem::separator(),
                &open,
                &update,
                &PredefinedMenuItem::separator(),
                &pairing,
                &copy_token,
                &at_login,
                &PredefinedMenuItem::separator(),
                &quit,
            ])?;
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| on_menu(event.id)));
            let builder = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Capture Room");
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_templated(icon(false));
            #[cfg(not(target_os = "macos"))]
            let builder = builder.with_icon(icon(false));
            let icon = builder.build()?;
            Ok(Self {
                config,
                icon,
                status_item,
                open,
                update,
                pairing,
                copy_token,
                at_login,
                quit,
                status: Status::default(),
            })
        }

        pub fn picked(&mut self, id: &MenuId) {
            let send = |action| {
                let _ = self.config.actions.send(action);
            };
            if id == self.open.id() {
                send(Action::Open);
            } else if id == self.update.id() {
                match self.status.update {
                    UpdateState::Available(_) => send(Action::Update),
                    _ => send(Action::CheckForUpdates),
                }
            } else if id == self.copy_token.id() {
                send(Action::CopyAccessToken);
            } else if id == self.at_login.id() {
                // The item has ticked itself already.
                let on = self.at_login.is_checked();
                if let Err(e) = service::set_start_at_login(on) {
                    tracing::warn!(error = %e, "couldn't change Start at Login");
                    self.at_login.set_checked(service::start_at_login());
                }
            } else if id == self.quit.id() {
                send(Action::Quit);
            }
        }

        pub fn show(&mut self, status: Status) {
            if status == self.status {
                return;
            }
            let recording = status.recordings > 0;
            let mut doing = Vec::new();
            if recording {
                doing.push(format!("Recording {}", plural(status.recordings, "feed")));
            }
            if status.playing > 0 {
                doing.push(format!("Playing {}", plural(status.playing, "channel")));
            }
            let busy = !doing.is_empty();
            self.status_item.set_text(if busy {
                doing.join(" · ")
            } else {
                "Idle".into()
            });

            let (text, enabled) = match &status.update {
                UpdateState::Available(v) if busy => (format!("Update to {v} when idle"), false),
                UpdateState::Available(v) => (format!("Update to {v}"), true),
                UpdateState::Running(what) => (what.clone(), false),
                UpdateState::Failed(e) => (format!("Update failed: {e}"), true),
                UpdateState::None => ("Check for Updates".into(), true),
            };
            self.update.set_text(text);
            self.update.set_enabled(enabled);
            self.pairing
                .set_text(format!("Pairing Code: {}", status.pairing_code));

            self.quit.set_text(if recording {
                format!("Quit (stops {})", plural(status.recordings, "recording"))
            } else {
                "Quit Capture Room".into()
            });

            if recording != (self.status.recordings > 0) {
                // The idle icon follows the menu bar's colour; recording is red.
                let _ = if recording {
                    self.icon.set_icon(Some(icon(true)))
                } else {
                    set_idle_icon(&self.icon)
                };
            }
            self.status = status;
        }
    }

    /// On macOS a template image, which the menu bar draws in its own colour.
    fn set_idle_icon(tray: &TrayIcon) -> tray_icon::Result<()> {
        #[cfg(target_os = "macos")]
        return tray.set_icon_templated(Some(icon(false)));
        #[cfg(not(target_os = "macos"))]
        tray.set_icon(Some(icon(false)))
    }

    fn plural(n: usize, what: &str) -> String {
        if n == 1 {
            format!("1 {what}")
        } else {
            format!("{n} {what}s")
        }
    }

    /// A record symbol (a ring round a dot), drawn at 2x for an 18 pt menu
    /// bar: black for a template image when idle, red when recording.
    fn icon(recording: bool) -> Icon {
        const SIZE: u32 = 36;
        let colour: [u8; 3] = if recording { [229, 72, 77] } else { [0, 0, 0] };
        let centre = (SIZE as f32 - 1.0) / 2.0;
        let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let d = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
                // Antialiased: coverage of the ring (radius 13–16) and the dot (radius 8).
                let ring = (16.0 - d).clamp(0.0, 1.0) * (d - 13.0).clamp(0.0, 1.0);
                let dot = (8.0 - d).clamp(0.0, 1.0);
                let alpha = ring.max(dot);
                rgba.extend_from_slice(&colour);
                rgba.push((alpha * 255.0) as u8);
            }
        }
        Icon::from_rgba(rgba, SIZE, SIZE).expect("valid icon")
    }
}

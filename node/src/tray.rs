//! The menu bar icon (system tray on Windows, a StatusNotifierItem on Linux)
//! of a node running as a service. It lives on the window loop's thread (see
//! [`crate::display`]), which gets the node's [`Status`] and hands the node
//! the [`Action`]s that need it.
//!
//! What an item says is worked out here once ([`Labels`]); each platform's
//! `imp` only puts it in its menu. On Linux the item is served over D-Bus
//! (`ksni`), so it shows wherever a StatusNotifierItem host runs: KDE, most
//! other desktops, and GNOME with the AppIndicator extension.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;

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
    /// What just came of an item picked, shown in its place for [`NOTE_FOR`].
    pub note: Option<Note>,
}

/// Picking an item closes the menu, so the note outlasts it: long enough to
/// open the menu again and read it.
const NOTE_FOR: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Note {
    UpToDate,
    Copied,
    CopyFailed,
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

/// What the menu asks the node to do.
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
    let (note_tx, mut note_rx) = watch::channel(None::<(Note, Instant)>);
    let watched = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            let activity = crate::session::activity(&watched).await;
            let note = *note_rx.borrow_and_update();
            displays.show_status(Status {
                recordings: activity.recordings,
                playing: activity.playing,
                update: (&watched.updater.status()).into(),
                pairing_code: watched.access_token.pairing_code(),
                note: note
                    .filter(|(_, at)| at.elapsed() < NOTE_FOR)
                    .map(|(note, _)| note),
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                Ok(()) = note_rx.changed() => {}
            }
        }
    });
    tokio::spawn(async move {
        let note = |note| {
            note_tx.send_replace(Some((note, Instant::now())));
        };
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
                    if let Err(e) = crate::update::start(&state, None).await {
                        state.updater.fail(e.to_string());
                    }
                }
                Action::CheckForUpdates => {
                    crate::update::check(&state).await;
                    // A version found shows as Update to it; a failure as itself.
                    if UpdateState::from(&state.updater.status()) == UpdateState::None {
                        note(Note::UpToDate);
                    }
                }
                Action::CopyAccessToken => {
                    let copied = state
                        .access_token
                        .get()
                        .ok_or_else(|| anyhow::anyhow!("no access token yet"))
                        .and_then(|token| crate::service::copy_to_clipboard(&token));
                    match copied {
                        Ok(()) => {
                            tracing::info!("access token copied from the menu bar");
                            note(Note::Copied);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "couldn't copy the access token");
                            note(Note::CopyFailed);
                        }
                    }
                }
            }
        }
    });
}

/// The text of each item that changes, for a [`Status`].
#[derive(Debug, Clone, PartialEq)]
struct Labels {
    status: String,
    update: String,
    update_enabled: bool,
    pairing: String,
    copy_token: String,
    quit: String,
    recording: bool,
}

impl Labels {
    fn new(status: &Status) -> Self {
        let recording = status.recordings > 0;
        let mut doing = Vec::new();
        if recording {
            doing.push(format!("Recording {}", plural(status.recordings, "feed")));
        }
        if status.playing > 0 {
            doing.push(format!("Playing {}", plural(status.playing, "channel")));
        }
        let busy = !doing.is_empty();
        let (update, update_enabled) = match &status.update {
            UpdateState::Available(v) if busy => (format!("Update to {v} when idle"), false),
            UpdateState::Available(v) => (format!("Update to {v}"), true),
            UpdateState::Running(what) => (what.clone(), false),
            UpdateState::Failed(e) => (format!("Update failed: {e}"), true),
            UpdateState::None if status.note == Some(Note::UpToDate) => ("Up to date".into(), true),
            UpdateState::None => ("Check for Updates".into(), true),
        };
        Self {
            status: if busy {
                doing.join(" · ")
            } else {
                "Idle".into()
            },
            update,
            update_enabled,
            pairing: format!("Pairing Code: {}", status.pairing_code),
            copy_token: match status.note {
                Some(Note::Copied) => "Copied",
                Some(Note::CopyFailed) => "Couldn't Copy Access Token",
                _ => "Copy Access Token",
            }
            .into(),
            quit: if recording {
                format!("Quit (stops {})", plural(status.recordings, "recording"))
            } else {
                "Quit Capture Room".into()
            },
            recording,
        }
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The icon's size in pixels: 2x for an 18 pt menu bar.
const ICON_SIZE: u32 = 36;

/// A record symbol (a ring round a dot) as RGBA: red when recording, else
/// `idle`, which each platform picks to stand out on its bar.
fn icon_rgba(recording: bool, idle: [u8; 3]) -> Vec<u8> {
    let colour: [u8; 3] = if recording { [229, 72, 77] } else { idle };
    let centre = (ICON_SIZE as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let d = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // Antialiased: coverage of the ring (radius 13–16) and the dot (radius 8).
            let ring = (16.0 - d).clamp(0.0, 1.0) * (d - 13.0).clamp(0.0, 1.0);
            let dot = (8.0 - d).clamp(0.0, 1.0);
            let alpha = ring.max(dot);
            rgba.extend_from_slice(&colour);
            rgba.push((alpha * 255.0) as u8);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_take_their_items_place() {
        let up_to_date = Status {
            note: Some(Note::UpToDate),
            ..Status::default()
        };
        assert_eq!(Labels::new(&up_to_date).update, "Up to date");
        // A version found meanwhile shows instead.
        let found = Status {
            update: UpdateState::Available("0.2.1".into()),
            ..up_to_date
        };
        assert_eq!(Labels::new(&found).update, "Update to 0.2.1");

        let copied = Status {
            note: Some(Note::Copied),
            ..Status::default()
        };
        let labels = Labels::new(&copied);
        assert_eq!(labels.copy_token, "Copied");
        assert_eq!(labels.update, "Check for Updates");
        assert_eq!(
            Labels::new(&Status::default()).copy_token,
            "Copy Access Token"
        );
    }
}

pub use imp::{MenuId, Tray};

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use anyhow::Result;
    pub use tray_icon::menu::MenuId;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::{Action, Config, Labels, Status, UpdateState};
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
            let labels = Labels::new(&status);
            let was_recording = self.status.recordings > 0;
            self.status_item.set_text(labels.status);
            self.update.set_text(labels.update);
            self.update.set_enabled(labels.update_enabled);
            self.pairing.set_text(labels.pairing);
            self.copy_token.set_text(labels.copy_token);
            self.quit.set_text(labels.quit);
            if labels.recording != was_recording {
                // The idle icon follows the menu bar's colour; recording is red.
                let _ = if labels.recording {
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

    #[cfg(target_os = "macos")]
    fn idle_colour() -> [u8; 3] {
        [0, 0, 0]
    }

    /// Black on a light taskbar, white on a dark one (the default), as the
    /// theme was when the icon was last drawn.
    #[cfg(target_os = "windows")]
    fn idle_colour() -> [u8; 3] {
        use crate::registry::{self, Root};
        let light = registry::dword(
            Root::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "SystemUsesLightTheme",
        ) == Some(1);
        if light {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        }
    }

    /// Red when recording; when idle, black for a template image on macOS,
    /// and on Windows whatever stands out on the taskbar.
    fn icon(recording: bool) -> Icon {
        let rgba = super::icon_rgba(recording, idle_colour());
        Icon::from_rgba(rgba, super::ICON_SIZE, super::ICON_SIZE).expect("valid icon")
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{Context, Result};
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{CheckmarkItem, StandardItem};
    use ksni::MenuItem;

    use super::{Action, Config, Labels, Status, UpdateState};
    use crate::service;

    /// An item picked, passed back to [`Tray::picked`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MenuId {
        Open,
        Update,
        CopyToken,
        AtLogin,
        Quit,
    }

    pub struct Tray {
        config: Config,
        item: Handle<Item>,
        at_login: bool,
        status: Status,
    }

    /// What ksni serves; it builds the menu from this, on its own thread.
    struct Item {
        on_menu: Box<dyn Fn(MenuId) + Send + Sync>,
        labels: Labels,
        at_login: bool,
    }

    impl Tray {
        /// Show the icon. `on_menu` gets the id of each item picked, to
        /// pass back to [`Tray::picked`] on this thread.
        pub fn new(
            config: Config,
            on_menu: impl Fn(MenuId) + Send + Sync + 'static,
        ) -> Result<Self> {
            let at_login = service::start_at_login();
            let item = Item {
                on_menu: Box::new(on_menu),
                labels: Labels::new(&Status::default()),
                at_login,
            }
            // The service starts at login, perhaps before the panel that hosts
            // the icon: wait for it rather than give up.
            .assume_sni_available(true)
            .spawn()
            .context("couldn't reach the session bus")?;
            Ok(Self {
                config,
                item,
                at_login,
                status: Status::default(),
            })
        }

        pub fn picked(&mut self, id: &MenuId) {
            let send = |action| {
                let _ = self.config.actions.send(action);
            };
            match id {
                MenuId::Open => send(Action::Open),
                MenuId::Update => match self.status.update {
                    UpdateState::Available(_) => send(Action::Update),
                    _ => send(Action::CheckForUpdates),
                },
                MenuId::CopyToken => send(Action::CopyAccessToken),
                MenuId::AtLogin => {
                    let on = !self.at_login;
                    self.at_login = match service::set_start_at_login(on) {
                        Ok(()) => on,
                        Err(e) => {
                            tracing::warn!(error = %e, "couldn't change Start at Login");
                            service::start_at_login()
                        }
                    };
                    let at_login = self.at_login;
                    self.item.update(|item| item.at_login = at_login);
                }
                MenuId::Quit => send(Action::Quit),
            }
        }

        pub fn show(&mut self, status: Status) {
            if status == self.status {
                return;
            }
            let labels = Labels::new(&status);
            self.item.update(|item| item.labels = labels);
            self.status = status;
        }
    }

    impl ksni::Tray for Item {
        // Left click opens the menu too, as on macOS.
        const MENU_ON_ACTIVATE: bool = true;

        fn id(&self) -> String {
            "capture-room".into()
        }

        fn title(&self) -> String {
            "Capture Room".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Capture Room".into(),
                ..Default::default()
            }
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            // Mid grey: there's no telling whether the panel is light or dark.
            let rgba = super::icon_rgba(self.labels.recording, [128, 128, 128]);
            // ksni wants ARGB.
            let data = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, a]| [a, r, g, b])
                .collect();
            vec![ksni::Icon {
                width: super::ICON_SIZE as i32,
                height: super::ICON_SIZE as i32,
                data,
            }]
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let labels = &self.labels;
            let shown = |label: String| -> MenuItem<Self> {
                StandardItem {
                    label,
                    enabled: false,
                    ..Default::default()
                }
                .into()
            };
            let item = |label: String, enabled: bool, id: MenuId| -> MenuItem<Self> {
                StandardItem {
                    label,
                    enabled,
                    activate: Box::new(move |item: &mut Self| (item.on_menu)(id)),
                    ..Default::default()
                }
                .into()
            };
            vec![
                shown(format!("Capture Room {}", env!("CARGO_PKG_VERSION"))),
                shown(labels.status.clone()),
                MenuItem::Separator,
                item("Open Capture Room".into(), true, MenuId::Open),
                item(labels.update.clone(), labels.update_enabled, MenuId::Update),
                MenuItem::Separator,
                // Shown, not clickable: read it off here into the controller.
                shown(labels.pairing.clone()),
                item(labels.copy_token.clone(), true, MenuId::CopyToken),
                CheckmarkItem {
                    label: "Start at Login".into(),
                    checked: self.at_login,
                    activate: Box::new(|item: &mut Self| (item.on_menu)(MenuId::AtLogin)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                item(labels.quit.clone(), true, MenuId::Quit),
            ]
        }
    }
}

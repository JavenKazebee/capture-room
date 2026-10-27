//! Screens under Wayland, through the xdg-desktop-portal ScreenCast portal.
//!
//! Wayland lets no program read the screen; the compositor shows its own
//! dialog to pick one and hands back a PipeWire stream, which `pipewiresrc`
//! plays. The device list offers one entry for it ([`KEY`]) whenever the
//! portal is there; which screen a source shows is picked per source:
//!
//! - [`choose`] opens the dialog, on the node's own display (not the
//!   browser's), and keeps the session it starts for the source's next build
//!   ([`Screencasts::take_pending`]), so choosing doesn't ask twice. Someone
//!   may take minutes to get to that display, longer than a controller waits
//!   for a forwarded request, so the API runs it in the background and the
//!   outcome shows as the source's state.
//! - Every later build restores the session from the portal's restore token,
//!   without a dialog. Tokens are single use: each start returns the next
//!   one, which is saved ([`Screencasts::watch`]). They're kept apart from
//!   the source config so a new token doesn't change the source's
//!   fingerprint; choosing again bumps a generation that does.
//! - A desktop that no longer honours the token shows its dialog again. A
//!   build can't wait for someone to answer it (the other sources start in
//!   the same batch), so it gives up after [`RESTORE_TIMEOUT`] and the source
//!   fails asking for the screen to be chosen again.
//! - Before anyone signs in to the desktop there's no portal: the source
//!   fails saying it's waiting for a graphical session, and the failed-source
//!   rescan retries it.
//!
//! The session lives as long as the source that holds its [`Stream`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

/// The device key of "a screen picked through the portal".
pub const KEY: &str = "screen:portal";

/// A restored session starts at once unless the desktop shows its dialog.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const RESTORE_TIMEOUT: Duration = Duration::from_secs(15);
/// Someone has to get to the node's display to answer the dialog.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const CHOOSE_TIMEOUT: Duration = Duration::from_secs(300);

const NO_SESSION: &str = "waiting for someone to sign in to the node's desktop: screen \
    capture under Wayland needs a graphical session";
const NOT_CHOSEN: &str = "no screen chosen yet: press Choose screen in the source's settings, \
    then pick one in the dialog on the node's display";
const ASKING: &str = "pick a screen in the dialog on the node's display";

/// What's kept per source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// The portal's restore token for the next start.
    pub token: Option<String>,
    /// Bumped each time a screen is chosen, so the source restarts on it.
    pub generation: u32,
}

/// Every source's [`Choice`], and sessions just chosen that no build has
/// taken yet. Shared like the Decklink profiles: a change reaches what's
/// built next.
#[derive(Default)]
pub struct Screencasts {
    choices: Mutex<HashMap<String, Choice>>,
    pending: Mutex<HashMap<String, Stream>>,
    /// Sources whose dialog is open.
    asking: Mutex<HashSet<String>>,
    /// Why the last choice failed, for a source that has none yet.
    failed: Mutex<HashMap<String, String>>,
    changed: Arc<Notify>,
}

impl Screencasts {
    pub fn replace(&self, all: HashMap<String, Choice>) {
        *self.choices.lock().unwrap() = all;
    }

    pub fn all(&self) -> HashMap<String, Choice> {
        self.choices.lock().unwrap().clone()
    }

    /// Notified after every change, for the caller to save [`Self::all`].
    pub fn watch(&self) -> Arc<Notify> {
        self.changed.clone()
    }

    pub fn generation(&self, source: &str) -> u32 {
        self.choices
            .lock()
            .unwrap()
            .get(source)
            .map_or(0, |c| c.generation)
    }

    fn token(&self, source: &str) -> Option<String> {
        self.choices
            .lock()
            .unwrap()
            .get(source)
            .and_then(|c| c.token.clone())
    }

    /// The next token, after a start used the last one.
    fn set_token(&self, source: &str, token: Option<String>) {
        let mut choices = self.choices.lock().unwrap();
        let choice = choices.entry(source.to_string()).or_default();
        if choice.token != token {
            choice.token = token;
            self.changed.notify_one();
        }
    }

    /// A screen was just chosen for `source`: its session is used by the
    /// next build, which the new generation makes happen.
    fn chosen(&self, source: &str, stream: Stream) {
        self.record_choice(source, stream.token.clone());
        self.pending
            .lock()
            .unwrap()
            .insert(source.to_string(), stream);
    }

    fn record_choice(&self, source: &str, token: Option<String>) {
        let mut choices = self.choices.lock().unwrap();
        let choice = choices.entry(source.to_string()).or_default();
        choice.token = token;
        choice.generation = choice.generation.wrapping_add(1);
        self.changed.notify_one();
    }

    /// Note that `source`'s dialog is about to open; an error if the portal
    /// isn't there or the dialog is open already.
    pub fn begin_choice(&self, source: &str) -> Result<(), &'static str> {
        if !imp::available() {
            return Err(NO_SESSION);
        }
        if !self.asking.lock().unwrap().insert(source.to_string()) {
            return Err("the dialog to pick a screen is open on the node's display already");
        }
        Ok(())
    }

    fn end_choice(&self, source: &str, result: Result<Stream>) -> Result<()> {
        self.asking.lock().unwrap().remove(source);
        let mut failed = self.failed.lock().unwrap();
        match result {
            Ok(stream) => {
                failed.remove(source);
                self.chosen(source, stream);
                Ok(())
            }
            Err(e) => {
                failed.insert(source.to_string(), format!("{e:#}"));
                Err(e)
            }
        }
    }

    fn take_pending(&self, source: &str) -> Option<Stream> {
        self.pending.lock().unwrap().remove(source)
    }

    /// Forget sources that no longer exist.
    pub fn retain(&self, sources: &[&str]) {
        let mut choices = self.choices.lock().unwrap();
        let before = choices.len();
        choices.retain(|id, _| sources.contains(&id.as_str()));
        self.pending
            .lock()
            .unwrap()
            .retain(|id, _| sources.contains(&id.as_str()));
        self.failed
            .lock()
            .unwrap()
            .retain(|id, _| sources.contains(&id.as_str()));
        if choices.len() != before {
            self.changed.notify_one();
        }
    }

    /// The session for `source`'s next build: the one just chosen, or one
    /// restored from its token.
    pub fn open(&self, source: &str) -> Result<Stream> {
        if let Some(stream) = self.take_pending(source) {
            return Ok(stream);
        }
        if !imp::available() {
            bail!(NO_SESSION);
        }
        let Some(token) = self.token(source) else {
            if self.asking.lock().unwrap().contains(source) {
                bail!(ASKING);
            }
            if let Some(e) = self.failed.lock().unwrap().get(source) {
                bail!("{e}: press Choose screen to try again");
            }
            bail!(NOT_CHOSEN);
        };
        let stream = imp::restore(&token)?;
        self.set_token(source, stream.token.clone());
        Ok(stream)
    }
}

/// Whether the portal can capture a screen now.
pub fn available() -> bool {
    imp::available()
}

/// Start checking for the portal, which comes and goes with the desktop
/// session.
pub fn start() {
    imp::start();
}

/// Ask on the node's display which screen `source` shows, after
/// [`Screencasts::begin_choice`]. Returns once someone answers the dialog.
pub async fn choose(screencasts: &Screencasts, source: &str) -> Result<()> {
    screencasts.end_choice(source, imp::choose().await)
}

pub use imp::Stream;

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::LazyLock;
    use std::time::Duration;

    use anyhow::{anyhow, bail, Context, Result};
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::{PersistMode, ResponseError, Session};
    use ashpd::enumflags2::BitFlags;
    use gstreamer::{self as gst, prelude::*};
    use tracing::{info, warn};

    use super::{CHOOSE_TIMEOUT, RESTORE_TIMEOUT};
    use crate::pipeline::{capsfilter, make_el, set_property};
    use crate::sources::add_ghost_pad;

    const POLL: Duration = Duration::from_secs(5);
    const PORTAL: &str = "org.freedesktop.portal.Desktop";

    static AVAILABLE: AtomicBool = AtomicBool::new(false);

    /// ashpd shares one D-Bus connection, which runs its tasks on the
    /// runtime that opened it; every portal call goes through this one.
    static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("screencast-portal")
            .enable_all()
            .build()
            .expect("start the screencast portal runtime")
    });

    /// Closes the portal session when dropped.
    struct Guard {
        proxy: Screencast,
        session: Option<Session<Screencast>>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                RT.spawn(async move {
                    if let Err(e) = session.close().await {
                        warn!(error = %e, "close a screencast session");
                    }
                });
            }
        }
    }

    /// A running screencast session and its PipeWire stream.
    pub struct Stream {
        _guard: Guard,
        fd: OwnedFd,
        node_id: u32,
        pub(super) token: Option<String>,
    }

    impl Stream {
        /// A bin with a raw-video `src` pad playing the stream. The stream
        /// must outlive it.
        pub fn element(&self, id: &str) -> Result<gst::Element> {
            let src = make_el("pipewiresrc", &format!("portal-src-{id}"))?;
            // pipewiresrc dups the fd; ours stays open with the session.
            set_property(&src, "fd", &self.fd.as_raw_fd().to_string())?;
            set_property(&src, "path", &self.node_id.to_string())?;
            set_property(&src, "do-timestamp", "true")?;
            let caps = capsfilter(
                &format!("portal-caps-{id}"),
                gst::Caps::builder("video/x-raw").build(),
            )?;
            let bin = gst::Bin::with_name(&format!("portal-capture-{id}"));
            bin.add_many([&src, &caps]).context("add screen capture")?;
            src.link(&caps).context("link screen capture")?;
            add_ghost_pad(&bin, &caps, "src")?;
            Ok(bin.upcast())
        }
    }

    pub fn available() -> bool {
        AVAILABLE.load(Ordering::Relaxed)
    }

    pub fn start() {
        RT.spawn(async {
            let mut conn = None;
            loop {
                if conn.is_none() {
                    conn = zbus::Connection::session().await.ok();
                }
                let now = match &conn {
                    Some(c) => check(c).await,
                    None => false,
                };
                if AVAILABLE.swap(now, Ordering::Relaxed) != now {
                    info!(available = now, "screencast portal");
                }
                tokio::time::sleep(POLL).await;
            }
        });
    }

    /// Whether the portal is running and offers monitors. Asking whether it
    /// has an owner doesn't start it: D-Bus activation before anyone signs in
    /// would only fail, slowly.
    async fn check(conn: &zbus::Connection) -> bool {
        let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
            return false;
        };
        let name = zbus::names::BusName::try_from(PORTAL).expect("a valid bus name");
        if !dbus.name_has_owner(name).await.unwrap_or(false) {
            return false;
        }
        match Screencast::new().await {
            Ok(proxy) => proxy
                .available_source_types()
                .await
                .is_ok_and(|t| t.contains(SourceType::Monitor)),
            Err(_) => false,
        }
    }

    /// Run `fut` on the portal runtime from any thread, async or not.
    fn block_on<T: Send + 'static>(
        timeout: Duration,
        fut: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        let task = RT.spawn(async move {
            let _ = tx.send(fut.await);
        });
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => {
                task.abort();
                Err(anyhow!("timed out"))
            }
        }
    }

    pub async fn choose() -> Result<Stream> {
        let task = RT.spawn(async {
            tokio::time::timeout(CHOOSE_TIMEOUT, open(None))
                .await
                .map_err(|_| anyhow!("nobody chose a screen on the node's display in time"))?
        });
        task.await.context("screencast portal task")?
    }

    pub fn restore(token: &str) -> Result<Stream> {
        let token = token.to_string();
        block_on(RESTORE_TIMEOUT, open(Some(token))).map_err(|e| {
            anyhow!(
                "the desktop didn't restore the chosen screen ({e}): choose it again in the \
                 source's settings"
            )
        })
    }

    async fn open(token: Option<String>) -> Result<Stream> {
        let proxy = Screencast::new().await?;
        let session = proxy.create_session(Default::default()).await?;
        // Closes the session if a step fails or a timeout drops this.
        let guard = Guard {
            proxy,
            session: Some(session),
        };
        let (proxy, session) = (&guard.proxy, guard.session.as_ref().expect("set above"));
        proxy
            .select_sources(
                session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::Embedded)
                    .set_sources(BitFlags::from(SourceType::Monitor))
                    .set_multiple(false)
                    .set_restore_token(token.as_deref())
                    .set_persist_mode(PersistMode::ExplicitlyRevoked),
            )
            .await?
            .response()?;
        let response = match proxy
            .start(session, None, Default::default())
            .await?
            .response()
        {
            Ok(r) => r,
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => {
                bail!("the screen choice was cancelled on the node's display")
            }
            Err(e) => return Err(e.into()),
        };
        let Some(first) = response.streams().first() else {
            bail!("the desktop shared no screen");
        };
        let node_id = first.pipe_wire_node_id();
        let token = response.restore_token().map(str::to_string);
        let fd = proxy
            .open_pipe_wire_remote(session, Default::default())
            .await?;
        Ok(Stream {
            _guard: guard,
            fd,
            node_id,
            token,
        })
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use anyhow::{bail, Result};
    use gstreamer as gst;

    pub struct Stream {
        pub(super) token: Option<String>,
    }

    impl Stream {
        pub fn element(&self, _: &str) -> Result<gst::Element> {
            bail!("screens through the portal are Linux only")
        }
    }

    pub fn available() -> bool {
        false
    }

    pub fn start() {}

    pub async fn choose() -> Result<Stream> {
        bail!("screens through the portal are Linux only")
    }

    pub fn restore(_: &str) -> Result<Stream> {
        bail!("screens through the portal are Linux only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_bumps_the_generation() {
        let s = Screencasts::default();
        assert_eq!(s.generation("a"), 0);
        s.record_choice("a", Some("t1".into()));
        assert_eq!(s.generation("a"), 1);
        assert_eq!(s.token("a").as_deref(), Some("t1"));
        s.record_choice("a", None);
        assert_eq!(s.generation("a"), 2);
        assert_eq!(s.token("a"), None);
    }

    #[test]
    fn a_new_token_keeps_the_generation() {
        let s = Screencasts::default();
        s.replace(HashMap::from([(
            "a".to_string(),
            Choice {
                token: Some("t1".into()),
                generation: 3,
            },
        )]));
        s.set_token("a", Some("t2".into()));
        assert_eq!(s.generation("a"), 3);
        assert_eq!(s.token("a").as_deref(), Some("t2"));
    }

    #[test]
    fn a_source_without_a_choice_asks_for_one() {
        let s = Screencasts::default();
        let err = s.open("a").err().unwrap().to_string();
        // Without a portal (as in CI) the session message comes first.
        assert!(err == NO_SESSION || err == NOT_CHOSEN, "{err}");
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn a_failed_choice_is_reported_until_the_next() {
        let s = Screencasts::default();
        s.asking.lock().unwrap().insert("a".into());
        assert!(s
            .end_choice("a", Err(anyhow::anyhow!("cancelled")))
            .is_err());
        assert!(s.asking.lock().unwrap().is_empty());
        assert_eq!(
            s.failed.lock().unwrap().get("a").map(String::as_str),
            Some("cancelled")
        );
        let ok = Stream {
            token: Some("t".into()),
        };
        s.end_choice("a", Ok(ok)).unwrap();
        assert!(s.failed.lock().unwrap().is_empty());
        assert_eq!(s.generation("a"), 1);
        assert!(s.open("a").is_ok(), "the chosen session is used first");
    }

    #[test]
    fn retain_forgets_deleted_sources() {
        let s = Screencasts::default();
        s.set_token("a", Some("t".into()));
        s.set_token("b", Some("t".into()));
        s.retain(&["b"]);
        assert_eq!(s.all().keys().collect::<Vec<_>>(), vec!["b"]);
    }
}

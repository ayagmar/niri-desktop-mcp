//! The desktop engine: what a niri instance's clients share, apart from any transport. It
//! owns the crash guardian, the event stream, the accessibility bus, the audit log and the
//! desk, and runs each tool's work through them: the readiness check, the action gate with
//! its evidence, captures and their refs, waits, and giving focus back. `tools.rs` turns
//! MCP calls into calls here and the results into MCP content.

pub(crate) mod hello;
pub(crate) mod host;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;
use tokio::time::Instant;

use crate::a11y::model::NameHash;
use crate::a11y::{self, A11y, Presence};
use crate::act::{self, Outcome};
use crate::audit::Audit;
use crate::control::desk::{Desk, Worked};
use crate::control::lease::Holder;
use crate::error::{CallError, ErrorName, ToolError};
use crate::input::Input;
use crate::niri::events::{EventStream, StreamState};
use crate::observe;
use crate::policy::{self, SaveTarget};
use crate::refs::Shot;
use crate::session::{Session, SessionId, Settings};
use crate::{Env, clipboard, elements, niri, noctalia, runner, settle, status, wait};

#[derive(Debug)]
pub(crate) struct Engine {
    env: Env,
    /// Without niri's socket, the socket's error.
    events: Result<EventStream, ToolError>,
    audit: Audit,
    desk: Desk,
    /// Decided once, because the tool list depends on it.
    noctalia_installed: bool,
    /// Whether there is an accessibility bus, decided once at startup for the same reason.
    accessibility: Presence,
    /// The accessibility bus, when there is one.
    a11y: Option<A11y>,
    /// The sessions being served, by id.
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    /// The last session id given out.
    last_session: AtomicU64,
    mode: status::Mode,
    /// Why a standalone engine's client isn't served by the shared engine it asked for.
    fallback: Option<String>,
    /// Lives as long as the engine; see `control::guard`.
    _guardian: Option<runner::Watcher>,
}

/// The action gate's checks for one session, which an element action asks again while
/// it runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Recheck<'a> {
    engine: &'a Engine,
    session: &'a Session,
}

impl elements::actions::Recheck for Recheck<'_> {
    async fn control(&self) -> Result<(), ToolError> {
        // Boxed, because the readiness report makes a large future.
        Box::pin(self.engine.refusal(self.session))
            .await
            .map_or(Ok(()), Err)
    }

    fn desk(&self) -> Result<(), ToolError> {
        self.engine.desk.checkpoint(self.session)
    }

    fn sending(&self) {
        self.engine.desk.sending();
    }
}

/// What `release_desktop` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Release {
    pub(crate) users_window: Option<u64>,
    /// How giving focus back went, when it was asked for and there was a window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) restored: Option<Value>,
    pub(crate) released: bool,
}

impl Engine {
    /// Starts the crash guardian before anything else, then niri's event stream, and looks
    /// for the accessibility bus.
    pub(crate) async fn start(
        env: Env,
        mode: status::Mode,
        fallback: Option<String>,
    ) -> Result<Self, String> {
        // `/proc/self/exe` still runs this binary when its file has been replaced since.
        let guardian = runner::watcher(
            "/proc/self/exe",
            &["guard".to_owned(), std::process::id().to_string()],
        )
        .map_err(|error| format!("start the crash guardian: {}", error.detail))?;
        let events = env
            .niri_socket
            .path()
            .map(|socket| EventStream::spawn(socket.to_path_buf()))
            .map_err(Clone::clone);
        let audit = Audit::new(env.state_dir.clone());
        let accessibility = a11y::detect(env.session_bus.as_deref()).await;
        Ok(Self {
            _guardian: Some(guardian),
            mode,
            fallback,
            ..Self::new(env, events, audit, accessibility)
        })
    }

    /// A standalone engine without a crash guardian, from resources already made.
    pub(crate) fn new(
        env: Env,
        events: Result<EventStream, ToolError>,
        audit: Audit,
        accessibility: Presence,
    ) -> Self {
        let a11y = accessibility.address.clone().map(A11y::new);
        Self {
            desk: Desk::start(&env),
            noctalia_installed: env.finds("noctalia"),
            env,
            events,
            audit,
            accessibility,
            a11y,
            sessions: Mutex::new(BTreeMap::new()),
            last_session: AtomicU64::new(0),
            mode: status::Mode::Standalone,
            fallback: None,
            _guardian: None,
        }
    }

    /// Starts serving a session for the client process `pid`, with `settings`.
    pub(crate) fn open_session(&self, pid: u32, settings: Settings) -> Session {
        let id = SessionId(self.last_session.fetch_add(1, Ordering::Relaxed) + 1);
        let session = Session::new(id, pid, settings);
        self.sessions().insert(id, session.clone());
        session
    }

    /// Ends `session`, if it hasn't ended yet, and stops counting it.
    pub(crate) async fn close_session(&self, session: &Session) {
        self.end_session(session).await;
        self.sessions().remove(&session.id());
    }

    /// Ends every session being served.
    pub(crate) async fn end_sessions(&self) {
        let sessions: Vec<Session> = self.sessions().values().cloned().collect();
        for session in &sessions {
            self.end_session(session).await;
        }
    }

    /// Whether the engine has nothing to finish: no session holds the lease and no input
    /// cleanup is pending.
    pub(crate) fn settled(&self) -> bool {
        !self.desk.held() && crate::control::cleanup::pending() == 0
    }

    /// Returns once the stop watcher has ended, for good: never, without a watcher.
    pub(crate) async fn watcher_ended(&self) {
        self.desk.watcher_ended().await;
    }

    /// No code panics while holding the lock, so a poisoned one still holds sound entries.
    fn sessions(&self) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, Session>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) const fn env(&self) -> &Env {
        &self.env
    }

    pub(crate) const fn audit(&self) -> &Audit {
        &self.audit
    }

    pub(crate) const fn noctalia_installed(&self) -> bool {
        self.noctalia_installed
    }

    pub(crate) const fn has_a11y(&self) -> bool {
        self.a11y.is_some()
    }

    pub(crate) const fn niri(&self) -> act::Niri<'_> {
        act::Niri {
            socket: &self.env.niri_socket,
            events: self.events.as_ref(),
        }
    }

    /// What the input tools work with, for `session`.
    pub(crate) fn input<'a>(&'a self, session: &'a Session) -> Result<Input<'a>, ToolError> {
        let settings = session.settings();
        Ok(Input {
            niri: self.niri(),
            display: &self.env.display,
            runtime: self.desk.runtime()?,
            policy: &settings.policy,
            keyboard: settings.keyboard.as_deref(),
            a11y: self.a11y.as_ref(),
        })
    }

    /// The action gate's checks for `session`, to ask again while its action runs.
    pub(crate) const fn recheck<'a>(&'a self, session: &'a Session) -> Recheck<'a> {
        Recheck {
            engine: self,
            session,
        }
    }

    /// The screenshot ref named `id` of the lease `session` holds.
    pub(crate) fn shot(&self, session: &Session, id: &str) -> Result<Shot, ToolError> {
        self.desk.shot(session, id)
    }

    /// The element ref named `id` of the lease `session` holds.
    pub(crate) fn element(
        &self,
        session: &Session,
        id: &str,
    ) -> Result<a11y::ElementRef, ToolError> {
        self.desk.element(session, id)
    }

    /// Keeps `name` as the name of the element ref `id` of the lease `session` holds, as
    /// an element action read it after acting.
    pub(crate) fn rename_element(&self, session: &Session, id: &str, name: NameHash) {
        self.desk.rename_element(session, id, name);
    }

    /// The readiness report, as `status` returns it to `session`.
    pub(crate) async fn status(&self, session: &Session) -> status::Status {
        let settings = session.settings();
        let event_stream = self
            .events
            .as_ref()
            .map_or(StreamState::Disconnected, EventStream::state);
        let engine = status::EngineStatus {
            pid: std::process::id(),
            mode: self.mode,
            sessions: self.sessions().len(),
            fallback: self.fallback.clone(),
        };
        let sources = status::Sources {
            engine: Some(engine),
            event_stream: Some(event_stream),
            audit: &self.audit,
            noctalia_installed: self.noctalia_installed,
            lease: self.desk.status(session),
            policy: &settings.policy,
            unrestricted: &settings.unrestricted,
            accessibility: &self.accessibility,
        };
        status::collect(&self.env, sources).await
    }

    /// Takes the lease for `session`, labelled `label`. Returns the holder and the window
    /// that had keyboard focus, which `release` can give focus back to.
    pub(crate) async fn acquire(
        &self,
        session: &Session,
        label: &str,
    ) -> Result<(Holder, Option<u64>), ToolError> {
        // Boxed, because the readiness report makes a large future.
        let refusal = Box::pin(self.refusal(session));
        let focused = async {
            niri::waiter(self.events.as_ref())
                .await
                .ok()
                .and_then(|waiter| waiter.view().focused_window())
        };
        let holder = self.desk.acquire(session, label, refusal, focused).await?;
        Ok((holder, self.desk.users_window(session)))
    }

    /// Gives the lease up, with `restore_focus` first giving focus back to the user's
    /// window through the action gate.
    pub(crate) async fn release(&self, session: &Session, restore_focus: bool) -> Release {
        let users_window = self.desk.users_window(session);
        let restored = match (restore_focus, users_window) {
            (true, Some(id)) => Some(self.restore(session, id).await),
            _ => None,
        };
        Release {
            users_window,
            restored,
            released: self.desk.release(session).await,
        }
    }

    /// Ends `session` once its client has gone: drops its running action and gives its
    /// lease up, without giving focus back.
    pub(crate) async fn end_session(&self, session: &Session) {
        self.desk.end_session(session).await;
    }

    /// Runs one action through the desk's gate, with a screenshot when `shoot` asks for one
    /// or its outcome is in doubt.
    pub(crate) async fn act(
        &self,
        session: &Session,
        shoot: bool,
        work: impl Future<Output = Result<Outcome, CallError>>,
    ) -> Result<act::Evidenced, CallError> {
        // Boxed, because the readiness report, the action's work and its wait make large
        // futures.
        let refusal = Box::pin(self.refusal(session));
        let evidence = |worked| Box::pin(self.evidence(session, worked, shoot));
        self.desk
            .act(session, refusal, Box::pin(work), evidence)
            .await
    }

    /// A screenshot, serialized with this engine's actions. With `save`, a full-resolution
    /// PNG of the target is written there first.
    pub(crate) async fn screenshot(
        &self,
        session: &Session,
        request: observe::Request,
        save: Option<SaveTarget>,
    ) -> Result<observe::Screenshot, CallError> {
        // Boxed, because a saved capture makes a large future.
        let capture = Box::pin(self.capture_saving(session, request, save));
        self.desk.observe(session, capture).await
    }

    /// The accessible elements `ask` names, each kept as an element ref while the lease is
    /// held.
    pub(crate) async fn elements(
        &self,
        session: &Session,
        ask: &elements::Ask,
    ) -> Result<elements::Listing, CallError> {
        let Some(a11y) = &self.a11y else {
            return Err(
                a11y::not_accessible("this session has no accessibility bus".to_owned()).into(),
            );
        };
        let lease = self.desk.ref_lease(session);
        let remember = |element| lease.and_then(|lease| self.desk.remember_element(lease, element));
        // Boxed, because the walk's calls make a large future.
        Box::pin(elements::list(
            &self.env.niri_socket,
            a11y,
            &session.settings().policy,
            ask,
            remember,
        ))
        .await
    }

    /// niri's outputs.
    pub(crate) async fn outputs(&self) -> Result<impl Serialize, ToolError> {
        niri::outputs(&self.env.niri_socket).await
    }

    /// One snapshot of the event stream.
    pub(crate) async fn desktop(&self) -> Result<impl Serialize, ToolError> {
        niri::desktop(self.events.as_ref()).await
    }

    /// Noctalia's status.
    pub(crate) async fn shell_status(&self) -> Result<impl Serialize, ToolError> {
        noctalia::status(&self.env).await
    }

    /// The clipboard's text.
    pub(crate) async fn clipboard(&self) -> Result<impl Serialize, ToolError> {
        clipboard::read_text(&self.env.niri_socket, &self.env.display).await
    }

    /// Waits for `until`, then with `screenshot` adds one taken once the screen stopped
    /// changing. Each capture serializes with actions, without holding the mutex between
    /// samples or while waiting for a window condition.
    pub(crate) async fn wait(
        &self,
        session: &Session,
        until: &wait::Until,
        limit: std::time::Duration,
        screenshot: bool,
    ) -> Result<(wait::Report, Option<observe::Screenshot>), CallError> {
        let capture = || {
            let capture = self.capture(session, observe::Request::focused());
            self.desk.observe(session, capture)
        };
        if *until == wait::Until::ScreenStable {
            let (report, last) = wait::screen(capture, limit).await?;
            return Ok((report, screenshot.then_some(last)));
        }
        let report = wait::window(self.events.as_ref(), until, limit).await?;
        if !screenshot {
            return Ok((report, None));
        }
        match settle::screenshot(capture, settle::LIMIT).await {
            Ok(shot) => Ok((report, Some(shot))),
            Err(CallError::Tool(error)) => Ok((
                wait::Report {
                    screenshot_error: Some(error),
                    ..report
                },
                None,
            )),
            Err(mistake @ CallError::InvalidArguments(_)) => Err(mistake),
        }
    }

    /// Focuses the user's window `id` through the action gate, as the outcome or the error.
    async fn restore(&self, session: &Session, id: u64) -> Value {
        let refusal = Box::pin(self.refusal(session));
        let work = Box::pin(act::refocus(self.niri(), id));
        let concluded = |worked| std::future::ready(Outcome::concluded(worked));
        let acted = self.desk.act(session, refusal, work, concluded);
        let restored = match acted.await {
            Ok(outcome) => serde_json::to_value(outcome),
            Err(CallError::Tool(error)) => serde_json::to_value(error),
            Err(CallError::InvalidArguments(message)) => {
                serde_json::to_value(ToolError::new(ErrorName::UpstreamError, message))
            }
        };
        restored.unwrap_or(Value::Null)
    }

    /// Takes a screenshot and, while this engine holds the lease, keeps it as a ref that
    /// the result names.
    async fn capture(
        &self,
        session: &Session,
        request: observe::Request,
    ) -> Result<observe::Screenshot, CallError> {
        let lease = self.desk.ref_lease(session);
        let connection = self.events.as_ref().ok().and_then(EventStream::connection);
        let taken = Instant::now();
        let mut shot =
            observe::screenshot(&self.env.niri_socket, &self.env.display, &request).await?;
        if let (Some(lease), Some(connection)) = (lease, connection) {
            let kept = Shot::of(&shot, taken, connection);
            shot.metadata.screenshot_ref = self.desk.remember(lease, kept);
        }
        Ok(shot)
    }

    /// With `save`, first writes a full-resolution PNG of the target there; then captures
    /// the screenshot to return, which says where the PNG went.
    async fn capture_saving(
        &self,
        session: &Session,
        request: observe::Request,
        save: Option<SaveTarget>,
    ) -> Result<observe::Screenshot, CallError> {
        let saved = match &save {
            Some(save) => {
                let (socket, display) = (&self.env.niri_socket, &self.env.display);
                Some(observe::save(socket, display, &request.target, save).await?)
            }
            None => None,
        };
        let mut shot = self.capture(session, request).await?;
        shot.metadata.saved = saved;
        Ok(shot)
    }

    /// Why `session` may not take the lease or act now, from the readiness report.
    async fn refusal(&self, session: &Session) -> Option<ToolError> {
        let report = self.status(session).await;
        policy::refuse_control(report.facts(&session.settings().policy))
    }

    /// The outcome with the screenshot `shoot` asks for, or the one an outcome in doubt gets.
    /// The outcome with its evidence, or, for work cut short after its call went out, the
    /// outcome alone: after a stop or with the lease gone, nothing more is asked.
    async fn evidence(
        &self,
        session: &Session,
        worked: Worked<Outcome>,
        shoot: bool,
    ) -> act::Evidenced {
        let Worked::Done(outcome) = worked else {
            return act::Evidenced {
                outcome: Outcome::concluded(worked),
                image: None,
            };
        };
        act::with_evidence(outcome, shoot, |request| self.capture(session, request)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn another_session_releasing_neither_refocuses_nor_learns_the_owners_window() {
        let dir = crate::test_support::fresh_dir("engine-release");
        let env = crate::test_support::niri_env(&dir);
        let events = Err(ToolError::new(ErrorName::NiriUnavailable, "no niri"));
        let absent = Presence {
            available: false,
            address: None,
            reason: Some("no session bus".to_owned()),
        };
        let engine = Engine::new(env, events, Audit::new(None), absent);
        let owner = crate::test_support::session(1);
        engine
            .desk
            .acquire(
                &owner,
                "owner/1",
                std::future::ready(None),
                std::future::ready(Some(5)),
            )
            .await
            .unwrap();
        let other = crate::test_support::session(2);
        let release = engine.release(&other, true).await;
        assert_eq!(release.users_window, None);
        assert!(release.restored.is_none(), "{:?}", release.restored);
        assert!(!release.released);
        assert!(engine.desk.status(&owner).held_by_me);
        assert_eq!(engine.desk.users_window(&owner), Some(5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

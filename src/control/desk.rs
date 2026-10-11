//! The desk: whether a session of this server controls the niri instance. It owns the
//! lease, takes it for one session at a time, only when neither the stop flag nor the
//! input-dirty marker is set, and gives it up as soon as the stop flag appears. The lease,
//! its refs and the window to give focus back to belong to the session that took it: no
//! other session can act, release, or use them. It also gates every action, one at a time,
//! and cancels the running one when the stop flag appears. A session that ends gives the
//! lease up at once: its running action is dropped in whatever phase it is.

use std::path::Path;
use std::sync::{Arc, PoisonError, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Mutex, watch};

use super::lease::{self, Holder, Lease, Refused};
use super::runtime::RuntimeDir;
use super::stop;
use super::{marker, procs};
use crate::Env;
use crate::a11y::ElementRef;
use crate::a11y::model::NameHash;
use crate::error::{CallError, ErrorName, ToolError};
use crate::refs::{Refs, Shot};
use crate::session::{Session, SessionId};

/// How often a held lease checks that its file is still the one at `lease`.
const CHECK: Duration = Duration::from_secs(1);
/// How often a running action checks the same.
const MOVED_CHECK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub(crate) struct Desk {
    /// Why there is no runtime directory, when there isn't one.
    runtime: Result<RuntimeDir, ToolError>,
    seat: Arc<Seat>,
    /// The stop watcher's view of the flag, or why the flag can't be watched.
    stopped: Result<watch::Receiver<bool>, String>,
}

/// The lease a session holds, if any.
#[derive(Debug)]
struct Seat {
    /// Also the action mutex: an action holds it while it runs.
    lease: Mutex<Option<Grant>>,
    /// The grant's owner while the lease is held, which `status` and the checks before an
    /// action read without waiting for a running action. Changed only together with
    /// `lease`.
    owner: watch::Sender<Option<Owner>>,
    /// The refs of the lease held, which a screenshot adds to without waiting for a running
    /// action. Started and ended together with `lease`.
    refs: std::sync::Mutex<Refs>,
}

/// The lease, held for one session.
#[derive(Debug)]
struct Grant {
    owner: SessionId,
    lease: Lease,
}

/// Who holds the lease, as read without the action mutex.
#[derive(Debug, Clone)]
struct Owner {
    session: SessionId,
    holder: Holder,
    /// The lease's file, to check it is still the one locked.
    locked: lease::Locked,
    /// The window that had keyboard focus when the lease was taken, to give it back to.
    users_window: Option<u64>,
}

impl Seat {
    fn put(&self, held: &mut Option<Grant>, grant: Grant, users_window: Option<u64>) {
        self.owner.send_replace(Some(Owner {
            session: grant.owner,
            holder: grant.lease.holder().clone(),
            locked: grant.lease.locked().clone(),
            users_window,
        }));
        self.refs().start();
        *held = Some(grant);
    }

    /// Gives the lease up, whoever holds it. Returns whether it was held.
    fn take(&self, held: &mut Option<Grant>) -> bool {
        let had = held.take().is_some();
        self.owner.send_replace(None);
        self.refs().end();
        had
    }

    /// Gives the lease up once any running action has ended, if `whose` holds it, or
    /// whoever does for `None`. Returns whether it was given up.
    async fn give_up(&self, whose: Option<SessionId>) -> bool {
        if whose.is_some_and(|session| self.held_by_other(session).is_some()) {
            return false;
        }
        let mut held = self.lease.lock().await;
        let owned = held
            .as_ref()
            .is_some_and(|grant| whose.is_none_or(|session| grant.owner == session));
        let released = owned && self.take(&mut held);
        drop(held);
        released
    }

    /// The holder, if a session other than `session` holds the lease. Read without the
    /// action mutex: while another session holds the lease, `session` can't be granted it,
    /// so the owner's running action has nothing to make `session` wait for. With the lease
    /// free, `session` may be taking it right now, so the caller waits for the mutex.
    fn held_by_other(&self, session: SessionId) -> Option<Holder> {
        self.owner
            .borrow()
            .as_ref()
            .filter(|owner| owner.session != session)
            .map(|owner| owner.holder.clone())
    }

    /// The owner, if it is `session`.
    fn owned_by(&self, session: SessionId) -> Option<Owner> {
        self.owner
            .borrow()
            .as_ref()
            .filter(|owner| owner.session == session)
            .cloned()
    }

    /// No code panics while holding the lock, so a poisoned one still holds sound refs.
    fn refs(&self) -> std::sync::MutexGuard<'_, Refs> {
        self.refs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What `status` reports about the lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LeaseStatus {
    pub(crate) held_by_me: bool,
    pub(crate) holder: Option<Holder>,
    /// Why no lease can be taken for this niri at all, as `acquire_desktop` says it.
    pub(crate) error: Option<String>,
}

impl Desk {
    /// Starts watching the stop flag on the current Tokio runtime, when `env` names a niri
    /// instance.
    pub(crate) fn start(env: &Env) -> Self {
        let seat = Arc::new(Seat {
            lease: Mutex::new(None),
            owner: watch::Sender::new(None),
            refs: std::sync::Mutex::new(Refs::default()),
        });
        let runtime = RuntimeDir::of(env).map_err(|detail| {
            let name = if env.instance.socket().is_err() {
                ErrorName::NiriUnavailable
            } else {
                ErrorName::UpstreamError
            };
            ToolError::new(name, detail)
        });
        let stopped = match &runtime {
            Ok(runtime) => stop::watch(runtime.clone())
                .inspect(|stopped| release_on_stop(stopped.clone(), Arc::downgrade(&seat)))
                .map_err(|error| {
                    format!(
                        "watch {} for the stop flag: {error}",
                        runtime.path().display()
                    )
                }),
            Err(error) => Err(error.detail.clone()),
        };
        Self {
            runtime,
            seat,
            stopped,
        }
    }

    /// Takes the lease for `session`, labelled `label`, or returns the holder if that
    /// session has it already. Another session's lease refuses at once, without waiting
    /// for its owner's running action, and another server's lock refuses before anything
    /// slow is asked. `refusal`, the policy's answer, is asked under the action mutex with
    /// the lock taken, after the stop flag and the input-dirty marker, so a wait for the
    /// mutex can't make it stale; a refusal gives the lock back. `users_window`, asked
    /// last, finds the window with keyboard focus, kept with the lease. Since both take
    /// time, the session's end, the stop flag and the marker are checked again before the
    /// lease is granted.
    pub(crate) async fn acquire(
        &self,
        session: &Session,
        label: &str,
        refusal: impl Future<Output = Option<ToolError>>,
        users_window: impl Future<Output = Option<u64>>,
    ) -> Result<Holder, ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        // The same checks, in the same order, as under the mutex.
        self.unblocked(runtime)?;
        if let Some(holder) = self.seat.held_by_other(session.id()) {
            return Err(refused(Refused::Held(Some(holder))));
        }
        let mut held = self.seat.lease.lock().await;
        // Checked under the mutex, so `end_session` either sees this grant or refuses it.
        if session.has_ended() {
            return Err(session_ended());
        }
        if let Some(grant) = held.as_ref().filter(|grant| grant.owner == session.id()) {
            return Ok(grant.lease.holder().clone());
        }
        self.unblocked(runtime)?;
        // Another session's grant refuses where another server's lock would.
        if let Some(grant) = held.as_ref() {
            return Err(refused(Refused::Held(Some(grant.lease.holder().clone()))));
        }
        let lease = Lease::acquire(runtime, label).map_err(refused)?;
        if let Some(refusal) = refusal.await {
            return Err(refusal);
        }
        let users_window = users_window.await;
        if session.has_ended() {
            return Err(session_ended());
        }
        self.unblocked(runtime)?;
        let holder = lease.holder().clone();
        let grant = Grant {
            owner: session.id(),
            lease,
        };
        self.seat.put(&mut held, grant, users_window);
        drop(held);
        Ok(holder)
    }

    /// Runs one action of `session` with the action mutex held. It runs only while neither
    /// the stop flag nor the input-dirty marker is set, `session` holds the lease, and
    /// `refusal`, the policy's answer asked once those checks pass, has none. A session
    /// without the lease is refused without waiting for another session's action. The lease
    /// file must still be the one locked right before `work` starts. A stop that arrives
    /// while `work` runs cancels it; the stop watcher then takes the lease back. A lease
    /// file removed or replaced meanwhile cancels it too, and the lease is given up at once.
    /// `finish` turns the work's result into the call's, still under the mutex but past the
    /// stop, so a stop can't discard what the work already found. The session ending drops
    /// the action in any phase: waiting for the mutex, `refusal`, `work` or `finish`.
    pub(crate) async fn act<T, U, F>(
        &self,
        session: &Session,
        refusal: impl Future<Output = Option<ToolError>>,
        work: impl Future<Output = Result<T, CallError>>,
        finish: impl FnOnce(T) -> F,
    ) -> Result<U, CallError>
    where
        F: Future<Output = U>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        // The same checks, in the same order, as under the mutex.
        self.unblocked(runtime)?;
        if self.seat.owned_by(session.id()).is_none() {
            return Err(lease_required().into());
        }
        tokio::select! {
            biased;
            () = session.ended() => Err(session_ended().into()),
            result = self.gated(session, refusal, work, finish) => result,
        }
    }

    /// `act` under the action mutex.
    async fn gated<T, U, F>(
        &self,
        session: &Session,
        refusal: impl Future<Output = Option<ToolError>>,
        work: impl Future<Output = Result<T, CallError>>,
        finish: impl FnOnce(T) -> F,
    ) -> Result<U, CallError>
    where
        F: Future<Output = U>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let mut held = self.seat.lease.lock().await;
        let mut stopped = self.unblocked(runtime)?;
        let Some(grant) = held.as_ref().filter(|grant| grant.owner == session.id()) else {
            return Err(lease_required().into());
        };
        if let Some(refusal) = refusal.await {
            return Err(refusal.into());
        }
        let ended = if grant.lease.intact() {
            tokio::select! {
                biased;
                _ = stopped.wait_for(|stopped| *stopped) => Ended::Stopped,
                () = moved(&grant.lease) => Ended::Moved("cancelled the action; anything niri had already accepted may have taken effect"),
                done = work => Ended::Done(done),
            }
        } else {
            Ended::Moved("sent nothing")
        };
        let done = match ended {
            Ended::Done(done) => done,
            Ended::Stopped => return Err(cancelled(&stopped).into()),
            Ended::Moved(what) => {
                self.seat.take(&mut held);
                // Without the directory, the stop watcher is about to end too.
                if !runtime.path().exists() {
                    return Err(watcher_ended().into());
                }
                return Err(lease_moved(what).into());
            }
        };
        let finished = finish(done?).await;
        drop(held);
        Ok(finished)
    }

    /// The desk's checks before an action, asked again while `session`'s action runs, as an
    /// element action does right before the one call that acts: neither the stop flag nor
    /// the input-dirty marker is set, `session` holds the lease, and the lease file is
    /// still the one locked. Doesn't wait for the action mutex, which the running action
    /// holds.
    pub(crate) fn checkpoint(&self, session: &Session) -> Result<(), ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        self.unblocked(runtime)?;
        let owner = self
            .seat
            .owned_by(session.id())
            .ok_or_else(lease_required)?;
        if !owner.locked.intact() {
            return Err(lease_moved("sent nothing"));
        }
        Ok(())
    }

    /// Checks that a stop can reach this server and that neither the stop flag nor the
    /// input-dirty marker is set. Returns the watcher's view of the flag.
    fn unblocked(&self, runtime: &RuntimeDir) -> Result<watch::Receiver<bool>, ToolError> {
        // Without a live watcher a stop couldn't take the lease back.
        let stopped = self
            .stopped
            .as_ref()
            .map_err(|detail| ToolError::new(ErrorName::UpstreamError, detail.clone()))?;
        if stopped.has_changed().is_err() {
            return Err(watcher_ended());
        }
        if *stopped.borrow() || runtime.stopped().map_err(|error| unreadable(&error))? {
            return Err(ToolError::new(
                ErrorName::Stopped,
                "the stop flag is set; the user clears it with `niri-computer-use resume`",
            ));
        }
        if runtime.input_dirty().map_err(|error| unreadable(&error))? {
            let found = marker::read(runtime);
            return Err(input_dirty(found.as_ref(), Path::new("/proc")));
        }
        Ok(stopped.clone())
    }

    /// Gives `session`'s lease up, once any running action has ended. Returns whether
    /// `session` held it; another session's lease stays.
    pub(crate) async fn release(&self, session: &Session) -> bool {
        self.seat.give_up(Some(session.id())).await
    }

    /// Ends `session` for good: drops its running action or observation, whatever phase it
    /// is in, and gives its lease up the way a stop does, without giving focus back, since
    /// no one is left to have asked for that. It can't take the lease again.
    pub(crate) async fn end_session(&self, session: &Session) {
        session.end();
        self.seat.give_up(Some(session.id())).await;
    }

    /// Serializes an observation of `session` with this server's actions and lease changes
    /// while `session` holds the lease, so a ref issued for it matches the desktop its
    /// actions left. It needs no lease and does not freeze external input or redraws.
    /// Action-return captures already hold the mutex and must not call this again. The
    /// session ending drops an observation that holds the mutex.
    pub(crate) async fn observe<T>(
        &self,
        session: &Session,
        capture: impl Future<Output = Result<T, CallError>>,
    ) -> Result<T, CallError> {
        if self.seat.owned_by(session.id()).is_none() {
            return capture.await;
        }
        let serialized = async {
            let held = self.seat.lease.lock().await;
            let result = capture.await;
            drop(held);
            result
        };
        tokio::select! {
            biased;
            () = session.ended() => Err(session_ended().into()),
            result = serialized => result,
        }
    }

    /// The window that had keyboard focus when `session` took the lease it holds.
    pub(crate) fn users_window(&self, session: &Session) -> Option<u64> {
        self.seat
            .owned_by(session.id())
            .and_then(|owner| owner.users_window)
    }

    /// The lease a screenshot of `session` starting now would issue its ref under, if
    /// `session` holds one. Doesn't wait for a running action.
    pub(crate) fn ref_lease(&self, session: &Session) -> Option<u64> {
        self.seat.owned_by(session.id())?;
        self.seat.refs().lease()
    }

    /// Keeps `shot` as a ref of `lease` and returns its id, unless that lease has ended
    /// since the capture started.
    pub(crate) fn remember(&self, lease: u64, shot: Shot) -> Option<String> {
        self.seat.refs().insert(lease, shot)
    }

    /// Keeps `element` as an element ref of `lease` and returns its id, unless that lease
    /// has ended since.
    pub(crate) fn remember_element(&self, lease: u64, element: ElementRef) -> Option<String> {
        self.seat.refs().insert_element(lease, element)
    }

    /// The element ref named `id` of the lease `session` holds.
    pub(crate) fn element(&self, session: &Session, id: &str) -> Result<ElementRef, ToolError> {
        self.owned_refs(session)
            .map_or_else(|| Refs::default().element(id), |refs| refs.element(id))
    }

    /// Keeps `name` as the name of the element the ref `id` names, if the lease `session`
    /// holds kept that ref.
    pub(crate) fn rename_element(&self, session: &Session, id: &str, name: NameHash) {
        if let Some(mut refs) = self.owned_refs(session) {
            refs.rename_element(id, name);
        }
    }

    /// The runtime directory, where input writes its marker.
    pub(crate) fn runtime(&self) -> Result<&RuntimeDir, ToolError> {
        self.runtime.as_ref().map_err(Clone::clone)
    }

    /// The ref named `id` of the lease `session` holds.
    pub(crate) fn shot(&self, session: &Session, id: &str) -> Result<Shot, ToolError> {
        self.owned_refs(session)
            .map_or_else(|| Refs::default().get(id), |refs| refs.get(id))
    }

    /// The refs of the lease `session` holds, if it holds the lease. Taken before the
    /// owner is read, so a lease change can't come in between.
    fn owned_refs(&self, session: &Session) -> Option<std::sync::MutexGuard<'_, Refs>> {
        let refs = self.seat.refs();
        self.seat.owned_by(session.id()).map(|_| refs)
    }

    /// Whether a session holds the lease. Doesn't wait for a running action.
    pub(crate) fn held(&self) -> bool {
        self.seat.owner.borrow().is_some()
    }

    /// Returns once the stop watcher has ended, for good: never, without a watcher.
    pub(crate) async fn watcher_ended(&self) {
        let Ok(stopped) = &self.stopped else {
            return std::future::pending().await;
        };
        let mut stopped = stopped.clone();
        while stopped.changed().await.is_ok() {}
    }

    /// The lease as `session` sees it. Doesn't wait for a running action.
    pub(crate) fn status(&self, session: &Session) -> LeaseStatus {
        let owner = self.seat.owner.borrow().clone();
        LeaseStatus {
            held_by_me: owner
                .as_ref()
                .is_some_and(|owner| owner.session == session.id()),
            holder: owner
                .map(|owner| owner.holder)
                .or_else(|| self.runtime.as_ref().ok().and_then(lease::holder)),
            error: self
                .runtime
                .as_ref()
                .err()
                .map(|error| error.detail.clone()),
        }
    }
}

/// What `status` reports when there is no desk, as in the `status` subcommand.
pub(crate) fn status_without_desk(env: &Env) -> LeaseStatus {
    let runtime = RuntimeDir::of(env);
    LeaseStatus {
        held_by_me: false,
        holder: runtime.as_ref().ok().and_then(lease::holder),
        error: runtime.err(),
    }
}

/// Drops the lease whenever the flag is seen set, and when the watcher ends, until the desk
/// is gone. It reads the latest value on every change, so a stop is never lost between a
/// resume and the next stop.
fn release_on_stop(stopped: watch::Receiver<bool>, seat: Weak<Seat>) {
    tokio::spawn(release_loop(stopped, seat));
}

async fn release_loop(mut stopped: watch::Receiver<bool>, seat: Weak<Seat>) {
    let mut check = tokio::time::interval(CHECK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stopped.borrow_and_update() && !give_up(&seat).await {
            return;
        }
        tokio::select! {
            changed = stopped.changed() => if changed.is_err() {
                give_up(&seat).await;
                return;
            },
            _ = check.tick() => if !give_up_if_moved(&seat).await {
                return;
            },
        }
    }
}

/// How an action's work ended.
enum Ended<T> {
    Done(T),
    /// The stop flag cancelled it.
    Stopped,
    /// The lease file was removed or replaced, before the work or during it, as said.
    Moved(&'static str),
}

/// Returns once `lease` no longer names the file it locked. A running action checks this
/// itself, because the periodic check waits for the action mutex the action holds.
async fn moved(lease: &Lease) {
    let mut check = tokio::time::interval(MOVED_CHECK);
    loop {
        check.tick().await;
        if !lease.intact() {
            return;
        }
    }
}

fn lease_moved(what: &str) -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        format!(
            "the lease file was removed or replaced, so another server may hold the lease; this server gave it up and {what}"
        ),
    )
}

/// Drops the lease if its file was removed or replaced. Returns false once the desk is
/// gone.
async fn give_up_if_moved(seat: &Weak<Seat>) -> bool {
    let Some(seat) = seat.upgrade() else {
        return false;
    };
    let mut held = seat.lease.lock().await;
    if held.as_ref().is_some_and(|grant| !grant.lease.intact()) {
        seat.take(&mut held);
    }
    drop(held);
    true
}

/// Drops the lease if it is held. Returns false once the desk is gone.
async fn give_up(seat: &Weak<Seat>) -> bool {
    let Some(seat) = seat.upgrade() else {
        return false;
    };
    seat.give_up(None).await;
    true
}

fn session_ended() -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        "this session ended, so its action was dropped and its lease given up",
    )
}

fn lease_required() -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        "this session doesn't hold the lease; call acquire_desktop first",
    )
}

/// Why a running action was cancelled. A watcher that loses the runtime directory reports
/// a stop and then ends, which on this single-threaded runtime happens before the action's
/// task runs again.
fn cancelled(stopped: &watch::Receiver<bool>) -> ToolError {
    if stopped.has_changed().is_err() {
        return watcher_ended();
    }
    ToolError::new(
        ErrorName::Stopped,
        "the user's stop flag cancelled this action; anything niri had already accepted may have taken effect",
    )
}

/// The refusal while the input-dirty marker is set. A marker whose input this server still
/// holds belongs to a dropped call's input that is still finishing, which `recover` would
/// cut short. One another live server wrote may be too, which only that server knows, so
/// the agent tries again once before `recover`. Any other stays until `recover`, which
/// needs the lease.
fn input_dirty(found: Option<&marker::Found>, proc_root: &Path) -> ToolError {
    let summary = found.map_or_else(String::new, marker::Found::summary);
    let marker = match found {
        Some(marker::Found::Marker(marker)) => Some(marker),
        _ => None,
    };
    let detail = match marker {
        Some(marker) if marker.finishing() => format!(
            "a cancelled call's input is still finishing ({summary}); try again once it has, within seconds"
        ),
        Some(marker) if another_server_may_finish(marker, proc_root) => format!(
            "input may be stuck ({summary}), or another server's input may still be finishing; try again in a few seconds, and if it still refuses, call release_desktop, then the user runs `niri-computer-use recover`, which needs the lease"
        ),
        _ => format!(
            "input may be stuck ({summary}); call release_desktop, then the user runs `niri-computer-use recover`, which needs the lease"
        ),
    };
    ToolError::new(ErrorName::RecoveryRequired, detail)
}

/// Whether another server wrote `marker` and may still clear it: it runs, as far as
/// `proc_root` tells, and its crash guardian hasn't sent the releases, which it does once
/// the server has died. The marker records no start time, so a PID reused since the
/// server died reads as running.
fn another_server_may_finish(marker: &marker::Marker, proc_root: &Path) -> bool {
    marker.server_pid != std::process::id()
        && marker.released.is_none()
        && procs::stat(proc_root, marker.server_pid).is_some_and(|stat| !stat.exited())
}

/// A stop can't reach this server any more.
fn watcher_ended() -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        "the stop watcher ended, because the runtime directory was removed or moved; restart the server",
    )
}

/// The runtime directory can't be read, so neither flag can be ruled out.
fn unreadable(error: &std::io::Error) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("read the runtime directory: {error}"),
    )
}

fn refused(refused: Refused) -> ToolError {
    match refused {
        Refused::Held(Some(holder)) => ToolError::new(
            ErrorName::LeaseHeld,
            format!(
                "held by PID {} ({}) since {}",
                holder.pid, holder.label, holder.since
            ),
        ),
        Refused::Held(None) => ToolError::new(
            ErrorName::LeaseHeld,
            "held by another process that left no record",
        ),
        Refused::Io(detail) => ToolError::new(ErrorName::UpstreamError, detail),
    }
}

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::time::Duration;

    use super::*;

    use crate::test_support::session;

    fn env(dir: &Path) -> Env {
        crate::test_support::niri_env(dir)
    }

    /// Whether the desk gives the lease up within five seconds.
    async fn released(desk: &Desk) -> bool {
        let me = session(1);
        for _ in 0..500 {
            if !desk.status(&me).held_by_me {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn takes_keeps_and_gives_back_the_lease() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap(),
            holder
        );
        let status = desk.status(&me);
        assert!(status.held_by_me);
        assert_eq!(other.status(&me).holder, Some(holder.clone()));
        let refused = other
            .acquire(&me, "other/2", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(desk.release(&me).await);
        assert!(!desk.release(&me).await);
        assert_eq!(
            other
                .acquire(&me, "other/2", ready(None), ready(None))
                .await
                .unwrap()
                .label,
            "other/2"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn status_reports_the_lease_while_an_action_holds_the_mutex() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-busy");
        let desk = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let action = desk.seat.lease.lock().await;
        assert_eq!(
            desk.status(&me),
            LeaseStatus {
                held_by_me: true,
                holder: Some(holder),
                error: None
            }
        );
        drop(action);
        assert!(desk.release(&me).await);
        assert!(!desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn release_waits_for_capture_and_a_cancelled_capture_unlocks_the_desk() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel();
        let capture_desk = desk.clone();
        let work = async move {
            started.send(()).unwrap();
            std::future::pending::<Result<(), CallError>>().await
        };
        let capture_me = me.clone();
        let capture = tokio::spawn(async move { capture_desk.observe(&capture_me, work).await });
        running.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), desk.release(&me))
                .await
                .is_err()
        );
        assert!(desk.status(&me).held_by_me);
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());
        assert!(desk.release(&me).await);
        // Observation is still available without a lease.
        assert_eq!(desk.observe(&me, async { Ok(7) }).await.unwrap(), 7);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An action whose work records that it ran and returns `value`.
    async fn act(desk: &Desk, refusal: Option<ToolError>) -> Result<u8, ErrorName> {
        let me = session(1);
        desk.act(&me, async { refusal }, async { Ok(7) }, ready)
            .await
            .map_err(|error| match error {
                CallError::Tool(error) => error.name,
                CallError::InvalidArguments(message) => panic!("{message}"),
            })
    }

    #[tokio::test]
    async fn an_action_needs_the_lease_and_a_clear_gate() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert_eq!(act(&desk, None).await, Err(ErrorName::LeaseRequired));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(act(&desk, None).await, Ok(7));
        let locked = ToolError::new(ErrorName::ScreenLocked, "locked");
        assert_eq!(act(&desk, Some(locked)).await, Err(ErrorName::ScreenLocked));
        // The marker comes before the lease and the policy, so an agent is told to stop.
        std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
        desk.release(&me).await;
        assert_eq!(act(&desk, None).await, Err(ErrorName::RecoveryRequired));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn only_a_marker_whose_input_is_still_finishing_spares_the_user_recover() {
        let dir = crate::test_support::fresh_dir("desk-finishing");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.create().unwrap();
        // No process runs in this `/proc` yet.
        let proc_root = dir.join("proc");
        let detail = |found: Option<marker::Found>| input_dirty(found.as_ref(), &proc_root).detail;
        // Input this server still holds clears its own marker, which `recover` would cut
        // short.
        let written =
            marker::Written::write(&runtime, marker::Marker::pending("paste", Vec::new()))
                .await
                .unwrap();
        let held = detail(marker::read(&runtime));
        assert!(
            held.contains("still finishing") && !held.contains("recover"),
            "{held}"
        );
        // Input that gave up on its marker, such as a wtype killed by a signal, leaves it
        // for the user, though this server wrote it.
        drop(written);
        let given_up = detail(marker::read(&runtime));
        assert!(
            given_up
                .contains("call release_desktop, then the user runs `niri-computer-use recover`")
                && !given_up.contains("try again"),
            "{given_up}"
        );
        assert!(detail(None).contains("`niri-computer-use recover`"));
        // Another server's input, such as a shared engine's beside this standalone server,
        // may still be finishing while that server runs; only it knows.
        let mut other = marker::Marker::pending("key", Vec::new());
        other.server_pid = std::process::id() + 1;
        let others = |marker: &marker::Marker| {
            let path = runtime.path().join(crate::control::runtime::INPUT_DIRTY);
            std::fs::write(path, serde_json::to_vec(marker).unwrap()).unwrap();
            detail(marker::read(&runtime))
        };
        let dead = others(&other);
        assert!(
            dead.contains("call release_desktop, then the user runs `niri-computer-use recover`")
                && !dead.contains("try again"),
            "{dead}"
        );
        let pid = other.server_pid;
        let stat = |state: char| {
            let process = proc_root.join(pid.to_string());
            std::fs::create_dir_all(&process).unwrap();
            let stat = format!(
                "{pid} (niri-computer-u) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0"
            );
            std::fs::write(process.join("stat"), stat).unwrap();
        };
        stat('S');
        let running = others(&other);
        assert!(
            running.contains(
                "another server's input may still be finishing; try again in a few seconds"
            ) && running.contains("then the user runs `niri-computer-use recover`"),
            "{running}"
        );
        // A server that has exited, or whose crash guardian has sent the releases, which
        // it does once the server died, finishes nothing, whatever runs under its PID now.
        stat('Z');
        assert!(!others(&other).contains("try again"));
        stat('S');
        other.released = Some(marker::now());
        let released = others(&other);
        assert!(
            released.contains("the crash guardian sent its releases")
                && !released.contains("try again"),
            "{released}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_stop_cancels_the_running_action_then_takes_the_lease_back() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let (held, dropped) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                let _held = held;
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let stop = async {
            running.await.unwrap();
            runtime.stop().unwrap();
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(action, stop) })
                .await
                .unwrap();
        assert!(matches!(
            result,
            Err(CallError::Tool(ToolError {
                name: ErrorName::Stopped,
                ..
            }))
        ));
        // The work was dropped, and with it its sender.
        assert!(dropped.await.is_err());
        assert!(released(&desk).await);
        assert_eq!(act(&desk, None).await, Err(ErrorName::Stopped));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Removes the `lease` file and lets `other` lock the new one, as a second server would.
    async fn replace_lease(runtime: &RuntimeDir, other: &Desk) {
        let me = session(1);
        std::fs::remove_file(runtime.path().join("lease")).unwrap();
        other
            .acquire(&me, "other/2", ready(None), ready(None))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_lease_replaced_during_the_readiness_check_refuses_the_work() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-replaced");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let readiness = async {
            replace_lease(&runtime, &other).await;
            None
        };
        let mut ran = false;
        let result = desk
            .act(
                &me,
                readiness,
                async {
                    ran = true;
                    Ok(())
                },
                ready,
            )
            .await;
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::LeaseRequired);
        assert!(!ran);
        assert!(!desk.status(&me).held_by_me);
        assert!(other.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_lease_replaced_mid_action_cancels_it_and_gives_the_lease_up() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-moved");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let (held, dropped) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                let _held = held;
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let replace = async {
            running.await.unwrap();
            replace_lease(&runtime, &other).await;
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(action, replace)
        })
        .await
        .unwrap();
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::LeaseRequired);
        assert!(dropped.await.is_err());
        assert!(!desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn losing_the_stop_watcher_mid_action_says_to_restart() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-gone");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let remove = async {
            running.await.unwrap();
            std::fs::remove_dir_all(runtime.path()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(action, remove)
        })
        .await
        .unwrap();
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("restart the server"), "{error:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_while_stopped_or_dirty_and_a_stop_takes_the_lease_back() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        runtime.stop().unwrap();
        assert!(released(&desk).await);
        assert_eq!(lease::holder(&runtime), None);
        assert_eq!(
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err()
                .name,
            ErrorName::Stopped
        );
        runtime.resume().unwrap();
        // Until the watcher has seen the resume, its view still refuses the lease.
        let mut seen = desk.stopped.clone().unwrap();
        tokio::time::timeout(Duration::from_secs(5), seen.wait_for(|stopped| !*stopped))
            .await
            .unwrap()
            .unwrap();
        std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
        assert_eq!(
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err()
                .name,
            ErrorName::RecoveryRequired
        );
        std::fs::remove_file(runtime.path().join("input-dirty")).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removing_the_runtime_directory_gives_the_lease_up_for_good() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-removed");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        assert!(released(&desk).await);
        // A new directory has a new lock file, which no stop could reach for this server.
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(
            error.detail.contains("restart the server"),
            "{}",
            error.detail
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_runtime_directory_refuses_the_lease() {
        use std::os::unix::fs::PermissionsExt as _;
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-unreadable");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.create().unwrap();
        let desk = Desk::start(&env(&dir));
        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(runtime.path(), mode(0o000)).unwrap();
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
        std::fs::set_permissions(runtime.path(), mode(0o700)).unwrap();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(
            error.detail.starts_with("read the runtime directory"),
            "{}",
            error.detail
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_a_niri_instance_there_is_nothing_to_take() {
        let me = session(1);
        let desk = Desk::start(&Env::default());
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert_eq!(error.detail, "NIRI_SOCKET is not set");
        assert_eq!(
            desk.status(&me),
            LeaseStatus {
                held_by_me: false,
                holder: None,
                error: Some("NIRI_SOCKET is not set".to_owned())
            }
        );
    }

    #[tokio::test]
    async fn one_session_holds_the_lease_and_another_can_neither_take_nor_release_it() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions");
        let desk = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(Some(5)))
            .await
            .unwrap();
        let refused = desk
            .acquire(&other, "other/2", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(!desk.release(&other).await);
        assert_eq!(
            desk.status(&other),
            LeaseStatus {
                held_by_me: false,
                holder: Some(holder.clone()),
                error: None
            }
        );
        assert!(desk.status(&me).held_by_me);
        assert_eq!(desk.users_window(&other), None);
        assert_eq!(desk.users_window(&me), Some(5));
        assert_eq!(act_as(&desk, &other).await, Err(ErrorName::LeaseRequired));
        assert_eq!(act_as(&desk, &me).await, Ok(7));
        // Released by its owner, the lease goes to whoever takes it next, with nothing of
        // the last owner's.
        assert!(desk.release(&me).await);
        assert_eq!(
            desk.acquire(&other, "other/2", ready(None), ready(None))
                .await
                .unwrap()
                .label,
            "other/2"
        );
        assert_eq!(desk.users_window(&other), None);
        assert_eq!(act_as(&desk, &me).await, Err(ErrorName::LeaseRequired));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An action of `session` whose work returns 7.
    async fn act_as(desk: &Desk, session: &Session) -> Result<u8, ErrorName> {
        desk.act(session, async { None }, async { Ok(7) }, ready)
            .await
            .map_err(|error| match error {
                CallError::Tool(error) => error.name,
                CallError::InvalidArguments(message) => panic!("{message}"),
            })
    }

    #[tokio::test]
    async fn a_session_without_the_lease_is_refused_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-busy");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let others = async {
            running.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), act_as(&desk, &other)).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            refused = others => assert_eq!(refused.unwrap(), Err(ErrorName::LeaseRequired)),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn another_sessions_acquire_is_refused_at_once_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-acquire");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let others = async {
            running.await.unwrap();
            let acquire = desk.acquire(&other, "other/2", ready(None), ready(None));
            tokio::time::timeout(Duration::from_millis(100), acquire).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            refused = others => {
                let error = refused.expect("waited for the owner's action").unwrap_err();
                assert_eq!(error.name, ErrorName::LeaseHeld);
                assert!(error.detail.contains("(me/1)"), "{}", error.detail);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn acquire_asks_for_readiness_and_focus_once_the_running_action_has_ended() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-fresh");
        let desk = Desk::start(&env(&dir));
        let action = desk.seat.lease.lock().await;
        let asked = std::cell::Cell::new(false);
        let refusal = async {
            asked.set(true);
            Some(ToolError::new(ErrorName::ScreenLocked, "locked meanwhile"))
        };
        let acquire = desk.acquire(&me, "me/1", refusal, ready(None));
        let release = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!asked.get(), "asked before the running action ended");
            drop(action);
        };
        let (result, ()) = tokio::join!(acquire, release);
        assert_eq!(result.unwrap_err().name, ErrorName::ScreenLocked);
        // The refusal gave the lock it had taken back.
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert_eq!(lease::holder(&runtime), None);
        drop(Lease::acquire(&runtime, "another server").unwrap());
        let focused = ready(Some(9));
        desk.acquire(&me, "me/1", async { None }, focused)
            .await
            .unwrap();
        assert_eq!(desk.users_window(&me), Some(9));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn another_servers_lock_refuses_before_readiness_is_asked() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-locked");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let theirs = Lease::acquire(&runtime, "them/2").unwrap();
        let refusal = async { panic!("readiness asked while another server holds the lock") };
        let error = desk
            .acquire(&me, "me/1", refusal, ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::LeaseHeld);
        assert!(error.detail.contains("(them/2)"), "{}", error.detail);
        drop(theirs);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_stop_or_the_sessions_end_during_readiness_grants_nothing() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-late");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let stopping = async {
            runtime.stop().unwrap();
            None
        };
        let error = desk
            .acquire(&me, "me/1", stopping, ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::Stopped);
        assert_eq!(lease::holder(&runtime), None);
        runtime.resume().unwrap();
        let ending = async {
            me.end();
            None
        };
        let ended = desk
            .acquire(&me, "me/1", ready(None), ending)
            .await
            .unwrap_err();
        assert_eq!(ended.detail, session_ended().detail);
        assert!(desk.seat.owned_by(me.id()).is_none());
        assert_eq!(lease::holder(&runtime), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Releases `session`'s lease and ends it. Returns whether it held the lease.
    async fn release_and_end(desk: &Desk, session: &Session) -> bool {
        let released = desk.release(session).await;
        desk.end_session(session).await;
        released
    }

    #[tokio::test]
    async fn another_session_releases_and_ends_at_once_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-release");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let others = async {
            running.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), release_and_end(&desk, &other)).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            released = others => assert_eq!(released, Ok(false)),
        }
        assert!(desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refs_belong_to_the_session_that_holds_the_lease() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-refs");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(desk.ref_lease(&other), None);
        let lease = desk.ref_lease(&me).unwrap();
        let id = desk.remember(lease, crate::test_support::shot()).unwrap();
        assert!(desk.shot(&me, &id).is_ok());
        assert_eq!(
            desk.shot(&other, &id).unwrap_err().name,
            ErrorName::RefInvalid
        );
        // The lease changing hands drops them, even for the next owner.
        desk.release(&me).await;
        desk.acquire(&other, "other/2", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(
            desk.shot(&other, &id).unwrap_err().name,
            ErrorName::RefInvalid
        );
        assert_eq!(desk.shot(&me, &id).unwrap_err().name, ErrorName::RefInvalid);
        assert_eq!(
            desk.element(&me, "elem-1").unwrap_err().name,
            ErrorName::ElementStale
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A phase of an action.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Refusal,
        Work,
        Finish,
    }

    #[tokio::test]
    async fn ending_a_session_drops_its_action_in_any_phase_and_frees_the_lease() {
        for stalled in [Phase::Refusal, Phase::Work, Phase::Finish] {
            let (me, other) = (session(1), session(2));
            let dir = crate::test_support::fresh_dir("desk-ended");
            let desk = Desk::start(&env(&dir));
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap();
            // Another session ending leaves the lease alone.
            desk.end_session(&session(3)).await;
            assert!(desk.status(&me).held_by_me);
            let error = end_while_stalled(&desk, &me, stalled).await;
            assert_eq!(error.name, ErrorName::LeaseRequired);
            assert!(error.detail.contains("session ended"), "{error:?}");
            assert_eq!(desk.status(&other).holder, None);
            // The ended session can't take it again; another can.
            let refused = desk
                .acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err();
            assert_eq!(refused.name, ErrorName::LeaseRequired);
            desk.acquire(&other, "other/2", ready(None), ready(None))
                .await
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// Waits forever, once `started` is told, if it `stalls`.
    async fn stall(stalls: bool, started: &tokio::sync::Notify) {
        if stalls {
            started.notify_one();
            std::future::pending::<()>().await;
        }
    }

    /// Ends `me` while its action waits forever in `stalled`, and returns the action's
    /// error, failing unless both end within a second.
    async fn end_while_stalled(desk: &Desk, me: &Session, stalled: Phase) -> ToolError {
        let started = tokio::sync::Notify::new();
        let stall = |phase| stall(phase == stalled, &started);
        let refusal = async {
            stall(Phase::Refusal).await;
            None
        };
        let work = async {
            stall(Phase::Work).await;
            Ok(())
        };
        let action = desk.act(me, refusal, work, |()| stall(Phase::Finish));
        let end = async {
            started.notified().await;
            desk.end_session(me).await;
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(1), async { tokio::join!(action, end) })
                .await
                .unwrap_or_else(|_| panic!("stalled in {stalled:?}"));
        match result {
            Err(CallError::Tool(error)) => error,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn ending_a_session_drops_its_observation_and_frees_the_lease() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-ended-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let started = tokio::sync::Notify::new();
        let capture = async {
            started.notify_one();
            std::future::pending::<Result<(), CallError>>().await
        };
        let end = async {
            started.notified().await;
            desk.end_session(&me).await;
        };
        let observation = desk.observe(&me, capture);
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(observation, end)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(desk.status(&other).holder, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An observation of `session` that returns 7, if it ran within 50 ms.
    async fn observed(desk: &Desk, session: &Session) -> Option<u8> {
        let observation = desk.observe(session, async { Ok(7) });
        tokio::time::timeout(Duration::from_millis(50), observation)
            .await
            .ok()
            .map(Result::unwrap)
    }

    #[tokio::test]
    async fn only_the_owners_observations_wait_for_its_action() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let action = desk.seat.lease.lock().await;
        assert_eq!(observed(&desk, &other).await, Some(7));
        assert_eq!(observed(&desk, &me).await, None);
        drop(action);
        assert_eq!(observed(&desk, &me).await, Some(7));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

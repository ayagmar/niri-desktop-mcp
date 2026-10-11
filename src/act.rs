//! The action tools' work: each checks its arguments against niri's state, dispatches one
//! niri action, and watches the event stream for the effect (plan §6, §7). A result
//! separates whether niri accepted the action from what was observed. Nothing is retried.
//!
//! The waiter is registered before the action is sent, so no event is missed between the
//! two. While waiting, focus moving to a window that was neither focused before nor the
//! expected target ends the wait as `interrupted`: someone else is using the desktop.

use std::collections::BTreeSet;
use std::time::Duration;

use niri_ipc::{Action, Window, WorkspaceReferenceArg};
use serde::Serialize;

use crate::control::desk::Worked;
use crate::elements::actions::Acted;
use crate::error::{CallError, ErrorName, ToolError, Unanswered};
use crate::input::keyboard::Focus;
use crate::input::paste::Pasted;
use crate::niri;
use crate::niri::Socket;
use crate::niri::waiter::{View, Waited, Waiter};
use crate::observe::{self, Metadata, Screenshot};
use crate::policy::{self, Loaded, Preset};
use crate::settle;

pub(crate) mod compositor;
pub(crate) mod shell;

/// How long an action waits for its effect.
const WAIT: Duration = Duration::from_secs(5);
/// How long `launch` keeps counting matching windows after the first one appears, since an
/// app may open several.
const SETTLE: Duration = Duration::from_millis(500);

/// What was observed after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Observed {
    /// The window or workspace has focus.
    Focused,
    /// The window closed, or the Noctalia panel is no longer the open one.
    Closed,
    /// The Noctalia panel is the open one.
    Opened,
    /// The window is still open when the wait ends, for example behind an unsaved-changes
    /// dialog.
    Pending,
    /// `launch`: no matching window appeared.
    None,
    /// `launch`: exactly one matching window appeared.
    One,
    /// `launch`: several matching windows appeared, or, with `reuse`, several already
    /// existed.
    Ambiguous,
    /// The effect wasn't seen in time.
    Timeout,
    /// Focus moved to a window that was neither focused before nor the target.
    Interrupted,
    /// The request's reply, or the event stream, was lost: the action may or may not have
    /// happened.
    Uncertain,
    /// Input tools and `niri_action`: niri handled it. What it did is for the next
    /// screenshot to show.
    Sent,
    /// `niri_action`: niri reported a change in the window the action is about.
    Changed,
    /// `niri_action`: niri reported no change in the window within a second.
    Unchanged,
    /// `activate_element`: the element is still there a moment later.
    Present,
    /// `activate_element`: the element or its window went away, as when a button closes
    /// its dialog.
    Gone,
    /// Element actions: the element couldn't be read afterwards; `detail` says why.
    Unknown,
    /// `set_element_text`: the element holds as many characters as were set.
    Matched,
    /// `set_element_text`: the element holds another number of characters, as when an app
    /// filters what it is given.
    Differs,
}

/// An action's result.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Outcome {
    /// True once niri acknowledged the request; false when nothing was sent; null when the
    /// request was sent but its reply was lost.
    pub(crate) accepted: Option<bool>,
    pub(crate) observed: Observed,
    /// The window with keyboard focus when the observation ended; null when focus isn't on
    /// a window, or when the reply was lost before observing.
    pub(crate) focused_window: Option<u64>,
    /// The windows the outcome is about: the one launched or reused, or the candidates.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) windows: Vec<u64>,
    /// Keyboard tools: whether `expect` was checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus: Option<Focus>,
    /// `type_text`: the characters sent before it stopped early; absent when it typed all
    /// of the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) typed: Option<usize>,
    /// `key`: the combinations pressed before it stopped early; absent when it pressed all
    /// of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pressed: Option<usize>,
    /// `type_text` with `submit`: whether `Return` was pressed after the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) submitted: Option<bool>,
    /// Shell tools: Noctalia's open panel when the observation ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shell: Option<shell::Shell>,
    /// `noctalia`: Noctalia's reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) noctalia: Option<crate::noctalia::Reply>,
    /// `niri_action`: the window the action is about, as niri reports it at the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) window: Option<compositor::WindowState>,
    /// `paste`: whether the text was read, and what became of the clipboard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) paste: Option<Pasted>,
    /// Element actions: the element's role and what the action did to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) element: Option<Acted>,
    /// Why the outcome is uncertain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// The metadata of the focused output's screenshot that comes with the result: one asked
    /// for with `screenshot`, or one for an outcome in doubt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) screenshot: Option<Metadata>,
    /// For an outcome in doubt, why there is no screenshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) screenshot_error: Option<ToolError>,
}

/// An outcome, with the image of its screenshot when it has one.
#[derive(Debug)]
pub(crate) struct Evidenced {
    pub(crate) outcome: Outcome,
    pub(crate) image: Option<Vec<u8>>,
}

impl Outcome {
    pub(crate) fn seen(observed: Observed, view: &View, windows: Vec<u64>) -> Self {
        Self {
            accepted: Some(true),
            observed,
            focused_window: view.focused_window(),
            windows,
            focus: None,
            typed: None,
            pressed: None,
            submitted: None,
            shell: None,
            noctalia: None,
            window: None,
            paste: None,
            element: None,
            detail: None,
            screenshot: None,
            screenshot_error: None,
        }
    }

    pub(crate) fn uncertain(accepted: Option<bool>, view: Option<&View>, detail: String) -> Self {
        Self {
            accepted,
            observed: Observed::Uncertain,
            focused_window: view.and_then(View::focused_window),
            windows: Vec::new(),
            focus: None,
            typed: None,
            pressed: None,
            submitted: None,
            shell: None,
            noctalia: None,
            window: None,
            paste: None,
            element: None,
            detail: Some(detail),
            screenshot: None,
            screenshot_error: None,
        }
    }

    /// The outcome of work the desk ran: its own, or `uncertain` with `accepted: null` and
    /// why, when a stop or a moved lease cut it short after its call went out.
    pub(crate) fn concluded(worked: Worked<Self>) -> Self {
        match worked {
            Worked::Done(outcome) => outcome,
            Worked::CutAfterSending(cut) => Self::uncertain(None, None, cut.detail),
        }
    }

    /// Outcomes after which the agent can't tell what the desktop looks like.
    const fn in_doubt(&self) -> bool {
        match self.observed {
            Observed::Timeout
            | Observed::Pending
            | Observed::None
            | Observed::Interrupted
            | Observed::Uncertain
            | Observed::Unknown => true,
            Observed::Focused
            | Observed::Closed
            | Observed::Opened
            | Observed::One
            | Observed::Ambiguous
            | Observed::Sent
            | Observed::Changed
            | Observed::Unchanged
            | Observed::Present
            | Observed::Gone
            | Observed::Matched
            | Observed::Differs => false,
        }
    }
}

/// Attaches a fresh screenshot of the focused output to an outcome when the agent `asked`
/// for one, taken once the screen stopped changing, or to an outcome in doubt (plan §6), at
/// once, so the agent sees the desktop without another call. `capture` takes it the way
/// the `screenshot` tool does. A failed capture leaves the outcome as it is and says why.
pub(crate) async fn with_evidence<F>(
    mut outcome: Outcome,
    asked: bool,
    capture: impl Fn(observe::Request) -> F + Sync,
) -> Evidenced
where
    F: Future<Output = Result<Screenshot, CallError>> + Send,
{
    if !asked && !outcome.in_doubt() {
        return Evidenced {
            outcome,
            image: None,
        };
    }
    let taken = if asked {
        settle::screenshot(|| capture(observe::Request::focused()), settle::LIMIT).await
    } else {
        capture(observe::Request::focused()).await
    };
    let image = match taken {
        Ok(shot) => {
            outcome.screenshot = Some(shot.metadata);
            Some(shot.image)
        }
        Err(CallError::Tool(error)) => {
            outcome.screenshot_error = Some(error);
            None
        }
        // No focused output, for example.
        Err(CallError::InvalidArguments(message)) => {
            outcome.screenshot_error = Some(ToolError::new(ErrorName::UpstreamError, message));
            None
        }
    };
    Evidenced { outcome, image }
}

/// Where actions go and where their effects are watched.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Niri<'a> {
    pub(crate) socket: &'a Socket,
    pub(crate) events: niri::Events<'a>,
}

pub(crate) async fn focus_window(niri: Niri<'_>, id: u64) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    if !waiter.view().windows().contains_key(&id) {
        return Err(no_window(id));
    }
    focus(niri.socket, &mut waiter, id, Vec::new()).await
}

/// Gives focus back to the user's window `id` before the lease is released: `closed`, with
/// nothing sent, if the window is gone.
pub(crate) async fn refocus(niri: Niri<'_>, id: u64) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    if !waiter.view().windows().contains_key(&id) {
        return Ok(unsent(Observed::Closed, waiter.view(), vec![id]));
    }
    focus(niri.socket, &mut waiter, id, vec![id]).await
}

/// `id` is a workspace id from `desktop_state`.
pub(crate) async fn focus_workspace(niri: Niri<'_>, id: u64) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    if !waiter.view().workspaces().contains_key(&id) {
        return Err(CallError::InvalidArguments(format!(
            "no workspace with id {id}; desktop_state lists them"
        )));
    }
    // With `workspace-auto-back-and-forth`, niri answers a focus on the focused workspace
    // by switching to the previous one, so nothing is sent.
    if waiter
        .view()
        .workspaces()
        .get(&id)
        .is_some_and(|ws| ws.is_focused)
    {
        return Ok(unsent(Observed::Focused, waiter.view(), Vec::new()));
    }
    let before = waiter.view().focused_window();
    let action = Action::FocusWorkspace {
        reference: WorkspaceReferenceArg::Id(id),
    };
    if let Some(lost) = send(niri.socket, action).await? {
        return Ok(lost);
    }
    let ended = waiter
        .until(WAIT, |view| workspace_focused(view, id, before))
        .await;
    Ok(conclude(
        ended,
        waiter.view(),
        Observed::Timeout,
        Vec::new(),
    ))
}

/// Asks the window to close, as its close button would. An app may ask first, so a window
/// still open at the end of the wait is `pending`; nothing escalates.
pub(crate) async fn close_window(
    niri: Niri<'_>,
    policy: &Loaded,
    id: u64,
) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    if !waiter.view().windows().contains_key(&id) {
        return Err(no_window(id));
    }
    refuse_close(policy, waiter.view(), id)?;
    if let Some(lost) = send(niri.socket, Action::CloseWindow { id: Some(id) }).await? {
        return Ok(lost);
    }
    let ended = waiter.until(WAIT, |view| closed(view, id)).await;
    Ok(conclude(ended, waiter.view(), Observed::Pending, vec![id]))
}

/// What `launch` does with `reuse` given the windows that already match the preset.
#[derive(Debug, PartialEq, Eq)]
enum Reuse {
    Spawn,
    Focus(u64),
    Ambiguous,
}

const fn reuse(existing: &[u64]) -> Reuse {
    match existing {
        [] => Reuse::Spawn,
        [one] => Reuse::Focus(*one),
        _ => Reuse::Ambiguous,
    }
}

/// Starts the preset's fixed command through niri, or with `reuse` focuses its one existing
/// window, and reports the windows that match its `app_id`.
pub(crate) async fn launch(
    niri: Niri<'_>,
    preset: &Preset,
    reuse_existing: bool,
) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    let app_id = preset.app_id.as_str();
    if reuse_existing {
        let existing = matching(waiter.view(), app_id, &BTreeSet::new());
        match reuse(&existing) {
            Reuse::Spawn => {}
            Reuse::Focus(id) => return focus(niri.socket, &mut waiter, id, vec![id]).await,
            Reuse::Ambiguous => {
                return Ok(unsent(Observed::Ambiguous, waiter.view(), existing));
            }
        }
    }
    let before: BTreeSet<u64> = waiter.view().windows().keys().copied().collect();
    let focused = waiter.view().focused_window();
    let action = Action::Spawn {
        command: preset.command(),
    };
    if let Some(lost) = send(niri.socket, action).await? {
        return Ok(lost);
    }
    let first = waiter
        .until(WAIT, |view| launched(view, app_id, &before, focused))
        .await;
    match first {
        Waited::Done(Observed::One) => {}
        // A single-instance app answered by focusing the window it already had.
        Waited::Done(Observed::Focused) => {
            let windows = waiter.view().focused_window().into_iter().collect();
            return Ok(Outcome::seen(Observed::Focused, waiter.view(), windows));
        }
        Waited::Done(_) | Waited::Timeout | Waited::Lost(_) => {
            return Ok(conclude(first, waiter.view(), Observed::None, Vec::new()));
        }
    }
    if let Waited::Lost(reason) = waiter.until(SETTLE, |_| None::<()>).await {
        return Ok(Outcome::uncertain(Some(true), Some(waiter.view()), reason));
    }
    let launched = matching(waiter.view(), app_id, &before);
    let observed = if launched.len() == 1 {
        Observed::One
    } else {
        Observed::Ambiguous
    };
    Ok(Outcome::seen(observed, waiter.view(), launched))
}

/// Focuses `id` and waits until it has focus.
async fn focus(
    socket: &Socket,
    waiter: &mut Waiter,
    id: u64,
    windows: Vec<u64>,
) -> Result<Outcome, CallError> {
    let before = waiter.view().focused_window();
    if before == Some(id) {
        return Ok(unsent(Observed::Focused, waiter.view(), windows));
    }
    if let Some(lost) = send(socket, Action::FocusWindow { id }).await? {
        return Ok(lost);
    }
    let ended = waiter
        .until(WAIT, |view| window_focused(view, id, before))
        .await;
    Ok(conclude(ended, waiter.view(), Observed::Timeout, windows))
}

/// An outcome reached without sending anything.
fn unsent(observed: Observed, view: &View, windows: Vec<u64>) -> Outcome {
    Outcome {
        accepted: Some(false),
        ..Outcome::seen(observed, view, windows)
    }
}

/// Sends the action. A refusal is an error; a lost reply is the `uncertain` outcome.
async fn send(socket: &Socket, action: Action) -> Result<Option<Outcome>, CallError> {
    match niri::act(socket, action).await {
        Ok(()) => Ok(None),
        Err(Unanswered::Refused(error)) => Err(error.into()),
        Err(Unanswered::Lost(error)) => Ok(Some(Outcome::uncertain(None, None, error.detail))),
    }
}

/// The outcome of a wait that ended `waited`, reporting `on_timeout` if it timed out.
fn conclude(
    waited: Waited<Observed>,
    view: &View,
    on_timeout: Observed,
    windows: Vec<u64>,
) -> Outcome {
    match waited {
        Waited::Done(observed) => Outcome::seen(observed, view, windows),
        Waited::Timeout => Outcome::seen(on_timeout, view, windows),
        Waited::Lost(reason) => Outcome::uncertain(Some(true), Some(view), reason),
    }
}

/// Window `id` has focus. `before` had focus when the wait began.
fn window_focused(view: &View, id: u64, before: Option<u64>) -> Option<Observed> {
    if view.focused_window() == Some(id) {
        return Some(Observed::Focused);
    }
    interrupted(view, before, |window| window.id == id)
}

/// Workspace `id` has focus, and so does its active window, or nothing when it has none.
/// niri reports the workspace and the window focus that follows it as separate events, in
/// either order, so the workspace alone could end the wait with focus still on the old
/// workspace's window, or on nothing. Focus moving to one of its windows is expected.
fn workspace_focused(view: &View, id: u64, before: Option<u64>) -> Option<Observed> {
    let on_it = |window: &Window| window.workspace_id == Some(id);
    let settled = view
        .workspaces()
        .get(&id)
        .is_some_and(|ws| ws.is_focused && view.focused_window() == ws.active_window_id);
    if settled {
        return Some(Observed::Focused);
    }
    interrupted(view, before, on_it)
}

fn closed(view: &View, id: u64) -> Option<Observed> {
    (!view.windows().contains_key(&id)).then_some(Observed::Closed)
}

/// A window that wasn't open `before` has `app_id`: `One`. Focus moving to an older window
/// with `app_id` is the app answering the launch with the window it had, as single-instance
/// apps do: `Focused`. A new window may take focus before it sets its `app_id`, so only
/// focus on an older window of another app interrupts.
fn launched(
    view: &View,
    app_id: &str,
    before: &BTreeSet<u64>,
    focused: Option<u64>,
) -> Option<Observed> {
    if !matching(view, app_id, before).is_empty() {
        return Some(Observed::One);
    }
    let now = view.windows().get(&view.focused_window()?)?;
    if Some(now.id) != focused && now.app_id.as_deref() == Some(app_id) {
        return Some(Observed::Focused);
    }
    interrupted(view, focused, |window| !before.contains(&window.id))
}

/// `Interrupted` when focus is on a window that wasn't focused before and isn't `expected`.
fn interrupted(
    view: &View,
    before: Option<u64>,
    expected: impl Fn(&Window) -> bool,
) -> Option<Observed> {
    let focused = view.windows().get(&view.focused_window()?)?;
    (Some(focused.id) != before && !expected(focused)).then_some(Observed::Interrupted)
}

/// The windows with `app_id` that aren't in `except`, by id.
fn matching(view: &View, app_id: &str, except: &BTreeSet<u64>) -> Vec<u64> {
    let mut ids: Vec<u64> = view
        .windows()
        .values()
        .filter(|window| window.app_id.as_deref() == Some(app_id))
        .map(|window| window.id)
        .filter(|id| !except.contains(id))
        .collect();
    ids.sort_unstable();
    ids
}

/// `app_denied` when window `id` belongs to an app on the policy's deny list. Closing is
/// the one window action the deny list covers besides input: focus and layout leave the
/// app as it was.
fn refuse_close(policy: &Loaded, view: &View, id: u64) -> Result<(), CallError> {
    let app_id = view
        .windows()
        .get(&id)
        .and_then(|window| window.app_id.as_deref());
    policy::refuse_window(policy, id, app_id).map_or(Ok(()), |refused| Err(refused.into()))
}

fn no_window(id: u64) -> CallError {
    CallError::InvalidArguments(format!("no window with id {id}; desktop_state lists them"))
}

#[cfg(test)]
mod tests {
    use niri_ipc::Event;

    use super::*;
    use crate::niri::waiter::Update;
    use crate::niri::waiter::tests::{view, view_on, waiter, window, workspace};

    #[test]
    fn reuse_spawns_for_none_focuses_one_and_refuses_to_pick_among_several() {
        assert_eq!(reuse(&[]), Reuse::Spawn);
        assert_eq!(reuse(&[4]), Reuse::Focus(4));
        assert_eq!(reuse(&[4, 9]), Reuse::Ambiguous);
    }

    #[test]
    fn focus_on_the_target_is_seen_and_on_another_window_interrupts() {
        let windows = |focused: u64| {
            view(vec![
                window(1, Some("a"), 1, focused == 1),
                window(2, Some("b"), 1, focused == 2),
                window(3, Some("c"), 2, focused == 3),
            ])
        };
        assert_eq!(
            window_focused(&windows(2), 2, Some(1)),
            Some(Observed::Focused)
        );
        assert_eq!(window_focused(&windows(1), 2, Some(1)), None);
        assert_eq!(
            window_focused(&windows(3), 2, Some(1)),
            Some(Observed::Interrupted)
        );
        // Focus off every window, as on a shell panel, isn't someone else's window.
        assert_eq!(window_focused(&windows(0), 2, Some(1)), None);
        // A workspace's own windows are where focus is expected to go.
        assert_eq!(workspace_focused(&windows(3), 2, Some(1)), None);
        assert_eq!(
            workspace_focused(&windows(2), 2, Some(1)),
            Some(Observed::Interrupted)
        );
        assert_eq!(closed(&windows(1), 2), None);
        assert_eq!(closed(&windows(1), 7), Some(Observed::Closed));
    }

    #[test]
    fn a_workspace_is_focused_once_its_active_window_has_focus() {
        // Workspace 1 holds window 1, its active window; workspace 2 is empty.
        let state = |focused_workspace: u64, focused_window: u64| {
            let mut one = workspace(1, focused_workspace == 1);
            one.active_window_id = Some(1);
            view_on(
                vec![one, workspace(2, focused_workspace == 2)],
                vec![window(1, Some("a"), 1, focused_window == 1)],
            )
        };
        assert_eq!(
            workspace_focused(&state(1, 1), 1, None),
            Some(Observed::Focused)
        );
        // Back on workspace 1 before niri has focused its window.
        assert_eq!(workspace_focused(&state(1, 0), 1, None), None);
        assert_eq!(
            workspace_focused(&state(2, 0), 2, Some(1)),
            Some(Observed::Focused)
        );
        // On workspace 2 before niri has moved focus off workspace 1's window.
        assert_eq!(workspace_focused(&state(2, 1), 2, Some(1)), None);
    }

    #[test]
    fn a_launch_counts_only_new_windows_with_the_app_id() {
        let before = BTreeSet::from([1, 2]);
        let current = view(vec![
            window(1, Some("foot"), 1, false),
            window(2, Some("x"), 1, false),
            window(9, Some("foot"), 1, true),
            window(5, Some("foot"), 1, false),
            window(6, None, 1, false),
        ]);
        assert_eq!(matching(&current, "foot", &before), [5, 9]);
        assert_eq!(
            launched(&current, "foot", &before, Some(2)),
            Some(Observed::One)
        );
        // A new window without its app_id yet may take focus; an old one interrupts.
        let pending = view(vec![
            window(1, Some("foot"), 1, false),
            window(2, Some("x"), 1, false),
            window(6, None, 1, true),
        ]);
        assert_eq!(launched(&pending, "foot", &before, Some(2)), None);
        let stolen = view(vec![
            window(1, Some("x"), 1, true),
            window(2, Some("y"), 1, false),
        ]);
        assert_eq!(
            launched(&stolen, "foot", &before, Some(2)),
            Some(Observed::Interrupted)
        );
        // A single-instance app focuses the window it already had.
        let answered = view(vec![
            window(1, Some("foot"), 1, true),
            window(2, Some("x"), 1, false),
        ]);
        assert_eq!(
            launched(&answered, "foot", &before, Some(2)),
            Some(Observed::Focused)
        );
        assert_eq!(launched(&answered, "foot", &before, Some(1)), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_app_id_and_focus_passing_through_an_intruder_are_seen() {
        let before = BTreeSet::from([1]);
        let (mut launch, opened) = waiter(view(vec![window(1, Some("x"), 1, true)]), 0);
        for (seq, window) in [
            (1, window(6, None, 1, true)),
            (2, window(6, Some("foot"), 1, true)),
        ] {
            opened
                .send(Update::Event(
                    seq,
                    Box::new(Event::WindowOpenedOrChanged { window }),
                ))
                .unwrap();
        }
        let mut checks = 0;
        let seen = launch
            .until(WAIT, |view| {
                checks += 1;
                launched(view, "foot", &before, Some(1))
            })
            .await;
        assert_eq!((seen, checks), (Waited::Done(Observed::One), 3));

        let windows = vec![
            window(1, Some("a"), 1, true),
            window(2, Some("b"), 1, false),
            window(3, Some("c"), 1, false),
        ];
        let (mut focus, focuses) = waiter(view(windows), 0);
        for (seq, id) in [(1, 3), (2, 2)] {
            focuses
                .send(Update::Event(
                    seq,
                    Box::new(Event::WindowFocusChanged { id: Some(id) }),
                ))
                .unwrap();
        }
        assert_eq!(
            focus
                .until(WAIT, |view| window_focused(view, 2, Some(1)))
                .await,
            Waited::Done(Observed::Interrupted)
        );
        assert_eq!(focus.view().focused_window(), Some(3));
    }

    #[test]
    fn outcomes_say_what_was_accepted_and_observed() {
        let current = view(vec![window(4, Some("a"), 1, true)]);
        assert_eq!(
            serde_json::to_value(conclude(
                Waited::Timeout,
                &current,
                Observed::Pending,
                vec![4]
            ))
            .unwrap(),
            serde_json::json!({
                "accepted": true, "observed": "pending", "focused_window": 4, "windows": [4]
            })
        );
        assert_eq!(
            serde_json::to_value(conclude(
                Waited::Lost("gone".to_owned()),
                &current,
                Observed::Timeout,
                Vec::new()
            ))
            .unwrap(),
            serde_json::json!({
                "accepted": true, "observed": "uncertain", "focused_window": 4, "detail": "gone"
            })
        );
    }
}

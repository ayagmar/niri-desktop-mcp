//! `activate_element` and `set_element_text`: an element's own action, or its text
//! replaced, through the accessibility bus instead of the pointer or keyboard. They reach
//! an app without any input event, so they get the keyboard's gates on the element's own
//! window: it must still be the ref's, not be on the deny list, have keyboard focus, and
//! match `expect`. A last gate right before the one call that acts asks all of that
//! again, with the action gate's own checks and the element as it is then, and nothing is
//! retried. That call only says the app took the request (research A4), so
//! the result is what was observed afterwards. Names and text are never returned or
//! logged.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::a11y::model::{self, State, States};
use crate::a11y::{self, A11y, Dispatched, ElementRef, Failed, Request};
use crate::act::{Observed, Outcome};
use crate::error::{CallError, ErrorName, ToolError};
use crate::input::Input;
use crate::input::keyboard::{self, Expect, Focus};
use crate::niri;
use crate::niri::waiter::{View, Waited, Waiter};
use crate::policy::{self, Loaded};
use niri_ipc::Window;

use super::{gone_is_stale, identify, stale};

/// How long an activation's effect has to show before the element is looked at again.
/// GTK 3 and GTK 4 buttons answer the action as they answer Enter: shown pressed for
/// 250 ms (`ACTIVATE_TIMEOUT` in `gtkbutton.c`), then clicked. Qt acts at once. The
/// window closing ends the wait sooner.
const SETTLE: Duration = Duration::from_millis(300);
/// `set_element_text`'s most text, in bytes of UTF-8 (see docs/decisions.md).
pub(crate) const MAX_TEXT: usize = 64 * 1024;

/// What kind an action is, by its name. An app names its actions, and may put anything
/// in a name, so only these are ever logged or said about one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionKind {
    Click,
    Press,
    Activate,
    Toggle,
    /// Any other name.
    Other,
}

impl ActionKind {
    const DEFAULTS: [Self; 4] = [Self::Click, Self::Press, Self::Activate, Self::Toggle];

    /// The kind of the action named `name`, in any case, as Qt names its `Press`.
    fn of(name: &str) -> Self {
        Self::DEFAULTS
            .into_iter()
            .find(|kind| name.eq_ignore_ascii_case(kind.name()))
            .unwrap_or(Self::Other)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Press => "press",
            Self::Activate => "activate",
            Self::Toggle => "toggle",
            Self::Other => "other",
        }
    }
}

/// An action of an element: its kind and its index among the element's actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Chosen {
    pub(crate) kind: ActionKind,
    pub(crate) index: usize,
}

/// What an element action did, without the element's name or text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Acted {
    pub(crate) role: &'static str,
    /// `activate_element`: the action taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<Chosen>,
    /// `activate_element`, observed `present`: the states set since.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_set: Vec<&'static str>,
    /// `activate_element`, observed `present`: the states cleared since.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_cleared: Vec<&'static str>,
    /// `set_element_text`: the characters the element says it holds now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) characters: Option<i32>,
}

/// The index of the action `activate_element` takes among the element's `advertised`
/// ones: `requested`, which must be one of them, or by default the first of them whose
/// kind is click, press, activate or toggle. A mistake says how many actions there are
/// and of which kinds, never their names.
pub(crate) fn choose_action(
    advertised: &[String],
    requested: Option<&str>,
) -> Result<usize, String> {
    let listed = || {
        let mut kinds: Vec<&str> = Vec::new();
        for action in advertised {
            let kind = ActionKind::of(action).name();
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        format!(
            "the element has {} actions, of the kinds {}; elements lists them",
            advertised.len(),
            kinds.join(", ")
        )
    };
    if let Some(requested) = requested {
        return advertised
            .iter()
            .position(|action| action == requested)
            .ok_or_else(|| format!("`action` isn't one of the element's actions: {}", listed()));
    }
    advertised
        .iter()
        .position(|action| ActionKind::of(action) != ActionKind::Other)
        .ok_or_else(|| {
            format!(
                "none of the element's actions is click, press, activate or toggle, so name one in `action`: {}",
                listed()
            )
        })
}

/// What the audit log keeps of an element action's arguments: the ref, the role it was
/// listed with, the kind and index of the action it names, the text's length, `expect` and
/// `screenshot`. Never a name, text, or an action's own name.
pub(crate) fn logged(
    id: &str,
    kept: Option<&ElementRef>,
    action: Option<&str>,
    text_len: Option<usize>,
    expect: &Value,
) -> Value {
    let role = kept.map(|element| model::role_name(element.kept.role));
    if let Some(text_len) = text_len {
        return json!({"element": id, "role": role, "text_len": text_len, "expect": expect});
    }
    let index = kept.and_then(|element| choose_action(&element.kept.actions, action).ok());
    let kind = match (kept, index) {
        (Some(element), Some(index)) => element
            .kept
            .actions
            .get(index)
            .map(|name| ActionKind::of(name)),
        _ => action.map(ActionKind::of),
    };
    json!({"element": id, "role": role, "action": kind, "action_index": index, "expect": expect})
}

/// The action gate's checks, which an element action asks again right before its call,
/// since reading the element takes time.
pub(crate) trait Recheck: Sync {
    /// The policy's answer from a fresh readiness report: niri, the policy file and the
    /// lock screen.
    fn control(&self) -> impl Future<Output = Result<(), ToolError>> + Send;
    /// The desk's: the stop flag, the input-dirty marker and the lease.
    fn desk(&self) -> Result<(), ToolError>;
}

/// What an element action's last gate checks: `expect`, and the action gate's checks.
pub(crate) struct Gate<'a, R> {
    pub(crate) expect: Expect,
    pub(crate) recheck: &'a R,
}

/// What the last gate saw.
struct Checked {
    role: u32,
    states: States,
    focus: Focus,
}

impl<R: Recheck> Gate<'_, R> {
    /// The last checks before the call that acts, slowest first so the ones that change
    /// fastest are read nearest the call: the action gate's own checks again; the
    /// element's role and states; then niri's events received so far, failing if the
    /// stream was lost, and the window's focus, deny list and `expect` on them; and the
    /// desk last. A change after them, before the app takes the call, isn't seen: niri,
    /// the desk and the app are asked apart, and nothing makes those reads one step.
    async fn last(
        &self,
        policy: &Loaded,
        element: &ElementRef,
        request: &Request,
        waiter: &mut Waiter,
    ) -> Result<Checked, CallError> {
        self.recheck.control().await?;
        let (role, states) = current(request, element).await?;
        if let Waited::Lost(why) = waiter.until(Duration::ZERO, |_| None::<()>).await {
            return Err(ToolError::new(
                ErrorName::NiriUnavailable,
                format!("{why}; nothing was sent"),
            )
            .into());
        }
        let focus = owner_focused(policy, waiter.view(), element, &self.expect)?;
        self.recheck.desk()?;
        Ok(Checked {
            role,
            states,
            focus,
        })
    }
}

/// Takes the element's action `requested`, or its default one, and looks at the element
/// a moment later.
pub(crate) async fn activate(
    input: Input<'_>,
    element: &ElementRef,
    requested: Option<&str>,
    gate: Gate<'_, impl Recheck>,
) -> Result<Outcome, CallError> {
    let a11y = accessible(input)?;
    let chosen =
        choose_action(&element.kept.actions, requested).map_err(CallError::InvalidArguments)?;
    let name = element
        .kept
        .actions
        .get(chosen)
        .cloned()
        .unwrap_or_default();
    let action = Chosen {
        kind: ActionKind::of(&name),
        index: chosen,
    };
    let mut waiter = niri::waiter(input.niri.events).await?;
    owner_focused(input.policy, waiter.view(), element, &gate.expect)?;
    let request = a11y.request(a11y::BUDGET).await?.names_only();
    identify(&request, element, owner(waiter.view(), element)?).await?;
    let actions = request.actions(element).await.map_err(gone_is_stale)?;
    let index = actions
        .iter()
        .position(|offered| *offered == name)
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| {
            stale(&format!(
                "the element no longer offers its {} action at index {}",
                action.kind.name(),
                action.index
            ))
        })?;
    let checked = gate
        .last(input.policy, element, &request, &mut waiter)
        .await?;
    let dispatched = request
        .do_action(element, index)
        .await
        .map_err(gone_is_stale)?;
    let lost = taken(dispatched, &format!("its {} action", action.kind.name()))?;
    let window = element.kept.window;
    let after = match waiter
        .until(SETTLE, |view| {
            (!view.windows().contains_key(&window)).then_some(())
        })
        .await
    {
        Waited::Done(()) => After::WindowGone,
        Waited::Timeout | Waited::Lost(_) => After::Probed(request.state(element).await),
    };
    let (observed, changes, detail) = activation(checked.states, after);
    let (states_set, states_cleared) = changes.unwrap_or_default();
    let acted = Acted {
        role: model::role_name(checked.role),
        action: Some(action),
        states_set,
        states_cleared,
        characters: None,
    };
    Ok(done(
        (observed, detail),
        lost,
        waiter.view(),
        checked.focus,
        acted,
    ))
}

/// Replaces the element's whole text with `text` and reads back how many characters it
/// holds.
pub(crate) async fn set_text(
    input: Input<'_>,
    element: &ElementRef,
    text: &str,
    gate: Gate<'_, impl Recheck>,
) -> Result<Outcome, CallError> {
    check_text(text)?;
    let a11y = accessible(input)?;
    let mut waiter = niri::waiter(input.niri.events).await?;
    owner_focused(input.policy, waiter.view(), element, &gate.expect)?;
    let request = a11y.request(a11y::BUDGET).await?.names_only();
    identify(&request, element, owner(waiter.view(), element)?).await?;
    let editable = request
        .editable_text(element)
        .await
        .map_err(gone_is_stale)?;
    if !editable {
        return Err(CallError::InvalidArguments(
            "the element has no EditableText interface, so its text can't be set".to_owned(),
        ));
    }
    let checked = gate
        .last(input.policy, element, &request, &mut waiter)
        .await?;
    if !checked.states.has(State::Editable) {
        return Err(CallError::InvalidArguments(
            "the element isn't in the `editable` state, so its text can't be set".to_owned(),
        ));
    }
    let dispatched = request
        .set_text(element, text)
        .await
        .map_err(gone_is_stale)?;
    let lost = taken(dispatched, "the new text")?;
    let count = request.character_count(element).await;
    let (observed, characters, detail) = written(text.chars().count(), count);
    let acted = Acted {
        role: model::role_name(checked.role),
        action: None,
        states_set: Vec::new(),
        states_cleared: Vec::new(),
        characters,
    };
    Ok(done(
        (observed, detail),
        lost,
        waiter.view(),
        checked.focus,
        acted,
    ))
}

/// Refuses text over `MAX_TEXT` bytes before anything is asked.
fn check_text(text: &str) -> Result<(), CallError> {
    if text.len() > MAX_TEXT {
        return Err(ToolError::new(
            ErrorName::TextTooLong,
            format!(
                "{} bytes; at most {MAX_TEXT} bytes of UTF-8 per call; nothing was set",
                text.len()
            ),
        )
        .into());
    }
    Ok(())
}

fn accessible(input: Input<'_>) -> Result<&A11y, ToolError> {
    input
        .a11y
        .ok_or_else(|| a11y::not_accessible("this session has no accessibility bus".to_owned()))
}

/// Checks the element's window in niri's `view`: still the one the ref was listed in, not
/// denied, and the window with keyboard focus, which `expect` must name as well. A shell
/// panel or a lock screen holds keyboard focus with no window focused, so an element
/// under it is refused here.
fn owner_focused(
    policy: &Loaded,
    view: &View,
    element: &ElementRef,
    expect: &Expect,
) -> Result<Focus, CallError> {
    if *expect == Expect::Unchecked {
        return Err(CallError::InvalidArguments(
            "element actions always check focus: give `expect` as {\"window_id\"} or {\"app_id\"}, not \"none\"".to_owned(),
        ));
    }
    let window = owner(view, element)?;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused.into());
    }
    if view.focused_window() != Some(window.id) {
        let focused = view.focused_window().map_or_else(
            || "no window has keyboard focus".to_owned(),
            |id| format!("window {id} has it"),
        );
        return Err(ToolError::new(
            ErrorName::FocusMismatch,
            format!(
                "the element's window {} doesn't have keyboard focus; {focused}",
                window.id
            ),
        )
        .into());
    }
    Ok(keyboard::check_expect(expect, view)?)
}

/// The element's window in niri's `view`, if it is still the one the ref was listed in.
fn owner<'a>(view: &'a View, element: &ElementRef) -> Result<&'a Window, ToolError> {
    let kept = &element.kept;
    view.windows()
        .get(&kept.window)
        .filter(|window| window.pid == Some(kept.pid))
        .ok_or_else(|| stale(&format!("window {} is gone", kept.window)))
}

/// The element's role and states now, if it is still the element listed, not a password
/// field, and showing.
async fn current(request: &Request, element: &ElementRef) -> Result<(u32, States), ToolError> {
    let (role, states) = request.state(element).await.map_err(gone_is_stale)?;
    // Before the role is compared, so a field that became a password field says so.
    if let Some(refused) = policy::refuse_secret_field(model::role_name(role)) {
        return Err(refused);
    }
    if role != element.kept.role {
        return Err(stale(&format!(
            "the element was a {} and is now a {}",
            model::role_name(element.kept.role),
            model::role_name(role)
        )));
    }
    if !(states.has(State::Showing) && states.has(State::Visible)) {
        return Err(super::unmappable(model::Unmappable::NotShowing));
    }
    Ok((role, states))
}

/// What became of an activated element after the settle.
#[derive(Debug)]
enum After {
    /// niri closed its window.
    WindowGone,
    /// Its role and states, or why they couldn't be read.
    Probed(Result<(u32, States), Failed>),
}

/// The states set and cleared, by name.
type Changes = (Vec<&'static str>, Vec<&'static str>);

/// `gone` when the window or the element went away, `present` with the states that
/// changed since `before`, or `unknown` with why the element couldn't be read.
fn activation(before: States, after: After) -> (Observed, Option<Changes>, Option<String>) {
    match after {
        After::WindowGone | After::Probed(Err(Failed::Gone(_))) => (Observed::Gone, None, None),
        After::Probed(Ok((_, now))) => (Observed::Present, Some(now.changes(before)), None),
        After::Probed(Err(Failed::Refused(error) | Failed::Error(error))) => {
            (Observed::Unknown, None, Some(error.detail))
        }
    }
}

/// `matched` when the element holds as many characters as were set, `differs` when an
/// app kept other text, as one that filters input does, or `unknown` with why the count
/// couldn't be read.
fn written(expected: usize, count: Result<i32, Failed>) -> (Observed, Option<i32>, Option<String>) {
    match count {
        Ok(count) if usize::try_from(count) == Ok(expected) => {
            (Observed::Matched, Some(count), None)
        }
        Ok(count) => (Observed::Differs, Some(count), None),
        Err(failed) => (
            Observed::Unknown,
            None,
            Some(ToolError::from(failed).detail),
        ),
    }
}

/// What was observed after the call, and why it is `unknown`.
type Seen = (Observed, Option<String>);

/// The outcome: what was `seen` afterwards, or, when the call's reply was `lost`,
/// `uncertain` with `accepted: null`, saying what was seen in `detail`.
fn done(seen: Seen, lost: Option<String>, view: &View, focus: Focus, acted: Acted) -> Outcome {
    let (observed, detail) = seen;
    let Some(lost) = lost else {
        return Outcome {
            focus: Some(focus),
            element: Some(acted),
            detail,
            ..Outcome::seen(observed, view, Vec::new())
        };
    };
    let afterwards = serde_json::to_value(observed)
        .ok()
        .and_then(|name| name.as_str().map(str::to_owned))
        .unwrap_or_default();
    let why = detail.map_or_else(String::new, |detail| format!(" ({detail})"));
    Outcome {
        focus: Some(focus),
        element: Some(acted),
        ..Outcome::uncertain(
            None,
            Some(view),
            format!("{lost}; afterwards: {afterwards}{why}"),
        )
    }
}

/// Whether the app took the request: `None` when it answered that it did, why its reply
/// was lost when that is unknown, and `upstream_error` when it refused `what`.
fn taken(dispatched: Dispatched<bool>, what: &str) -> Result<Option<String>, ToolError> {
    match dispatched {
        Dispatched::Answered(true) => Ok(None),
        Dispatched::Answered(false) => Err(refused_by_app(what)),
        Dispatched::Lost(why) => Ok(Some(why)),
    }
}

fn refused_by_app(what: &str) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("the app refused {what}; it may have been taken, so take a screenshot to check"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::model::Kept;
    use crate::niri::waiter::tests::{view, window};
    use crate::policy::parse;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    fn element(window: u64, actions: &[&str]) -> ElementRef {
        ElementRef {
            bus: ":1.7".to_owned(),
            path: "/org/a11y/atspi/accessible/9".to_owned(),
            frame: "/org/a11y/atspi/accessible/1".to_owned(),
            kept: Kept {
                role: 43,
                window,
                pid: 1,
                actions: names(actions),
                lineage: Vec::new(),
            },
        }
    }

    #[test]
    fn the_default_action_is_the_elements_first_click_press_activate_or_toggle() {
        let gtk = names(&["click"]);
        assert_eq!(choose_action(&gtk, None), Ok(0));
        // Qt Quick's button: Press, then SetFocus.
        let qt = names(&["SetFocus", "Press"]);
        assert_eq!(choose_action(&qt, None), Ok(1));
        // The element's own order decides, not the list's.
        let entry = names(&["activate", "click"]);
        assert_eq!(choose_action(&entry, None), Ok(0));
        assert!(choose_action(&[], None).is_err());
    }

    #[test]
    fn a_requested_action_must_be_one_the_element_listed_exactly() {
        let frame = names(&["window.close", "default.activate"]);
        assert_eq!(choose_action(&frame, Some("default.activate")), Ok(1));
        assert!(choose_action(&frame, Some("Window.Close")).is_err());
    }

    #[test]
    fn a_mistake_counts_the_actions_and_names_their_kinds_never_the_apps_names() {
        let label = names(&["SECRET-1 copy", "SECRET-2", "Toggle"]);
        for requested in [None, Some("SECRET-3")] {
            let mistake = choose_action(&label[..2], requested).unwrap_err();
            assert!(!mistake.contains("SECRET"), "{mistake}");
            assert!(
                mistake.contains("2 actions, of the kinds other;"),
                "{mistake}"
            );
        }
        let mistake = choose_action(&label, Some("SECRET-3")).unwrap_err();
        assert!(
            mistake.contains("3 actions, of the kinds other, toggle;"),
            "{mistake}"
        );
    }

    #[test]
    fn only_the_focused_window_that_listed_the_element_and_expect_names_is_acted_on() {
        let policy = Loaded::Valid(parse("deny_input_app_ids = [\"secret\"]", false).unwrap());
        let windows = |focused: u64| {
            view(vec![
                window(3, Some("app"), 1, focused == 3),
                window(4, Some("secret"), 1, focused == 4),
                window(5, Some("other"), 1, focused == 5),
            ])
        };
        let check = |focused: u64, element: &ElementRef, expect: Expect| {
            owner_focused(&policy, &windows(focused), element, &expect)
        };
        let error = |result: Result<Focus, CallError>| match result {
            Err(CallError::Tool(error)) => error.name,
            other => panic!("{other:?}"),
        };
        let mine = element(3, &[]);
        assert_eq!(check(3, &mine, Expect::Window(3)).unwrap(), Focus::Matched);
        assert_eq!(
            check(3, &mine, Expect::App("app".to_owned())).unwrap(),
            Focus::Matched
        );
        // Another window has focus, or none does, as under a shell panel.
        assert_eq!(
            error(check(5, &mine, Expect::Window(3))),
            ErrorName::FocusMismatch
        );
        assert_eq!(
            error(check(0, &mine, Expect::Window(3))),
            ErrorName::FocusMismatch
        );
        // The element's window has focus, but `expect` names another.
        assert_eq!(
            error(check(3, &mine, Expect::App("other".to_owned()))),
            ErrorName::FocusMismatch
        );
        assert!(matches!(
            check(3, &mine, Expect::Unchecked),
            Err(CallError::InvalidArguments(_))
        ));
        // A denied window is refused even with focus.
        assert_eq!(
            error(check(4, &element(4, &[]), Expect::Window(4))),
            ErrorName::AppDenied
        );
        // The window closed, or its id now belongs to another process.
        assert_eq!(
            error(check(3, &element(9, &[]), Expect::Window(9))),
            ErrorName::ElementStale
        );
        let reused = ElementRef {
            kept: Kept {
                pid: 2,
                ..mine.kept.clone()
            },
            ..mine
        };
        assert_eq!(
            error(check(3, &reused, Expect::Window(3))),
            ErrorName::ElementStale
        );
    }

    #[test]
    fn an_activation_is_gone_present_with_its_state_changes_or_unknown() {
        let unchecked = States::from_words(&[(1 << 25) | (1 << 30)]);
        let checked = States::from_words(&[(1 << 4) | (1 << 25) | (1 << 30)]);
        assert_eq!(
            activation(unchecked, After::WindowGone),
            (Observed::Gone, None, None)
        );
        assert_eq!(
            activation(
                unchecked,
                After::Probed(Err(Failed::Gone("gone".to_owned())))
            ),
            (Observed::Gone, None, None)
        );
        assert_eq!(
            activation(unchecked, After::Probed(Ok((43, checked)))),
            (Observed::Present, Some((vec!["checked"], vec![])), None)
        );
        assert_eq!(
            activation(checked, After::Probed(Ok((43, checked)))),
            (Observed::Present, Some((vec![], vec![])), None)
        );
        let late = ToolError::new(ErrorName::DeadlineExceeded, "no answer within 1 s");
        assert_eq!(
            activation(unchecked, After::Probed(Err(Failed::Error(late)))),
            (
                Observed::Unknown,
                None,
                Some("no answer within 1 s".to_owned())
            )
        );
    }

    #[test]
    fn written_text_is_matched_by_its_character_count() {
        assert_eq!(written(4, Ok(4)), (Observed::Matched, Some(4), None));
        // An app that filters input keeps less, or more.
        assert_eq!(written(4, Ok(2)), (Observed::Differs, Some(2), None));
        assert_eq!(written(0, Ok(-1)), (Observed::Differs, Some(-1), None));
        let refused = ToolError::new(ErrorName::UpstreamError, "no Text interface");
        assert_eq!(
            written(4, Err(Failed::Refused(refused))),
            (
                Observed::Unknown,
                None,
                Some("no Text interface".to_owned())
            )
        );
    }

    #[test]
    fn text_over_64_kib_of_utf8_is_refused_before_anything_is_asked() {
        assert!(check_text(&"a".repeat(MAX_TEXT)).is_ok());
        assert!(check_text("").is_ok());
        // 21846 three-byte characters are 65538 bytes.
        let over = "€".repeat(21_846);
        assert!(matches!(
            check_text(&over),
            Err(CallError::Tool(error)) if error.name == ErrorName::TextTooLong
        ));
    }

    #[test]
    fn the_log_keeps_the_role_actions_kind_and_index_and_length_never_a_name_or_text() {
        let button = element(3, &["SECRET-1", "Press"]);
        let expect = json!({"window_id": 3});
        assert_eq!(
            logged("elem-t-1", Some(&button), None, None, &expect),
            json!({"element": "elem-t-1", "role": "button", "action": "press", "action_index": 1, "expect": expect})
        );
        assert_eq!(
            logged("elem-t-1", Some(&button), Some("SECRET-1"), None, &expect),
            json!({"element": "elem-t-1", "role": "button", "action": "other", "action_index": 0, "expect": expect})
        );
        // An action the element didn't list, or an unknown ref, logs the kind asked for.
        assert_eq!(
            logged("elem-t-1", Some(&button), Some("SECRET-2"), None, &expect),
            json!({"element": "elem-t-1", "role": "button", "action": "other", "action_index": null, "expect": expect})
        );
        assert_eq!(
            logged("elem-t-9", None, Some("Click"), None, &json!("none")),
            json!({"element": "elem-t-9", "role": null, "action": "click", "action_index": null, "expect": "none"})
        );
        assert_eq!(
            logged(
                "elem-t-1",
                Some(&button),
                None,
                Some(12),
                &json!({"app_id": "a"})
            ),
            json!({"element": "elem-t-1", "role": "button", "text_len": 12, "expect": {"app_id": "a"}})
        );
    }
}

//! The element tools against a mock application on a private accessibility bus: what
//! reaches the application, what is refused before anything does, and what the result and
//! the audit log say. Every name and text is synthetic.

use std::time::Duration;

use serde_json::{Value, json};

use crate::atspi::{self, Bus, Mock, Object, PASSWORD_TEXT, TEXT};
use crate::client::{Server, mistake, tool_error};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};

/// An action name an app could put private text in.
const ODD_ACTION: &str = "open ACTION-SENTINEL";

/// The mock application's window.
const WINDOW: u64 = 1;

/// A server holding the lease on a fake niri, whose focused window 1 is the mock
/// application's, beside window 2 of another app.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct Desk {
    server: Server,
    _niri: Niri,
    stream: Stream,
    noctalia: noctalia::Reply,
    bus: Bus,
    mock: Mock,
    fixture: Fixture,
}

/// The mock's objects: a frame of 800x600 holding a panel with a button, a text field and
/// a password field.
fn objects() -> Vec<(&'static str, Object)> {
    vec![
        ("/frame", Object::frame("Mock", (800, 600), &["/panel"])),
        (
            "/panel",
            Object::panel(&["/button", "/odd", "/entry", "/password"]),
        ),
        ("/button", Object::button("Safe", &["click"])),
        ("/odd", Object::button("Odd", &[ODD_ACTION, "click"])),
        ("/entry", Object::entry(TEXT, "Field")),
        ("/password", Object::entry(PASSWORD_TEXT, "Secret")),
    ]
}

/// A window of the mock application, the test's own process.
fn app_window(id: u64, focused: bool) -> Value {
    let mut window = window_on(id, Some("org.ncu.Mock"), 1, focused);
    window["pid"] = json!(std::process::id());
    window["title"] = json!("Mock");
    window
}

/// Workspace 1 on `DP-1`, as `initial` sends it.
fn workspace() -> Value {
    json!({
        "id": 1, "idx": 1, "name": null, "output": "DP-1", "is_urgent": false,
        "is_active": true, "is_focused": true, "active_window_id": null
    })
}

impl Desk {
    async fn start(name: &str) -> Self {
        let windows = [
            app_window(WINDOW, true),
            window_on(2, Some("other"), 1, false),
        ];
        Self::start_with(name, &["/frame"], objects(), &windows).await
    }

    /// A desk whose mock application has `frames` under its root, holding `objects`, and
    /// whose niri has `windows`.
    async fn start_with(
        name: &str,
        frames: &[&str],
        objects: Vec<(&'static str, Object)>,
        windows: &[Value],
    ) -> Self {
        let mut fixture = Fixture::new(name);
        fixture.program("noctalia", "exit 0");
        fixture.grim(&jpeg(1280, 720, b"evidence"));
        let (bus, mock) = atspi::start(&mut fixture, frames, objects).await;
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let mut niri = Niri::start(&fixture);
        niri.set_windows(windows, &[workspace()]);
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(windows);
        server.structured("acquire_desktop").await;
        Self {
            server,
            _niri: niri,
            stream,
            noctalia,
            bus,
            mock,
            fixture,
        }
    }

    /// The `screenshot_ref` of a screenshot of the focused output.
    async fn screenshot_ref(&mut self) -> String {
        let shot = self
            .server
            .call("screenshot", json!({"target": "focused_output"}))
            .await;
        let id = &shot["structuredContent"]["screenshot_ref"];
        id.as_str().unwrap_or_else(|| panic!("{shot}")).to_owned()
    }

    /// The `element_ref` of the listed element named `name`.
    async fn element(&mut self, name: &str) -> String {
        let listing = self
            .server
            .structured_with("elements", json!({"window_id": WINDOW}))
            .await;
        let elements = listing["elements"].as_array().unwrap();
        let element = elements
            .iter()
            .find(|element| element["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {listing}"));
        element["element_ref"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn an_element_of_the_focused_window_is_activated_and_its_text_set_once_each() {
    let mut desk = Desk::start("el-acts").await;
    let button = desk.element("Safe").await;
    let activated = desk
        .server
        .structured_with(
            "activate_element",
            json!({"element": button, "expect": {"window_id": WINDOW}}),
        )
        .await;
    assert_eq!(activated["accepted"], true, "{activated}");
    assert_eq!(activated["observed"], "present", "{activated}");
    assert_eq!(desk.mock.calls("DoAction"), 1);
    let entry = desk.element("Field").await;
    let set = desk
        .server
        .structured_with(
            "set_element_text",
            json!({"element": entry, "text": "abc", "expect": {"window_id": WINDOW}}),
        )
        .await;
    assert_eq!(set["observed"], "matched", "{set}");
    assert_eq!(desk.mock.calls("SetTextContents"), 1);
    assert_eq!(desk.mock.object("/entry").text, Some(3));
    let password = desk.element("Secret").await;
    let refused = desk
        .server
        .call(
            "set_element_text",
            json!({"element": password, "text": "abc", "expect": {"window_id": WINDOW}}),
        )
        .await;
    assert_eq!(tool_error(&refused).0, "secret_field");
    assert_eq!(desk.mock.calls("SetTextContents"), 1);
}

/// Text an application put in its error messages, which must reach neither the agent nor
/// the audit log.
const SENTINEL: &str = "PRIVATE-REVIEW-SENTINEL";

#[tokio::test]
async fn the_apps_error_messages_reach_neither_the_result_nor_the_audit_log() {
    let mut desk = Desk::start("el-sentinel").await;
    let button = desk.element("Safe").await;
    let entry = desk.element("Field").await;
    let activate = json!({"element": button, "expect": {"window_id": WINDOW}});
    let set = json!({"element": entry, "text": "abc", "expect": {"window_id": WINDOW}});
    let message = format!("refused private text {SENTINEL}");
    let mut results = Vec::new();
    // Before the call that acts, the call itself, and the look afterwards.
    for (tool, member) in [
        ("activate_element", "GetActions"),
        ("activate_element", "DoAction"),
        ("set_element_text", "GetInterfaces"),
        ("set_element_text", "SetTextContents"),
        ("set_element_text", "CharacterCount"),
    ] {
        desk.mock.fail(member, &message);
        let arguments = if tool == "activate_element" {
            &activate
        } else {
            &set
        };
        results.push(desk.server.call(tool, arguments.clone()).await);
    }
    let held = desk.mock.hold("DoAction");
    let id = desk.server.start_call("activate_element", activate).await;
    held.arrived().await;
    desk.mock.fail("GetState", &message);
    held.release();
    results.push(desk.server.response(id).await["result"].clone());
    let unknown = &results[5]["structuredContent"];
    assert_eq!(unknown["observed"], "unknown", "{unknown}");
    assert_eq!(results[4]["structuredContent"]["observed"], "unknown");
    for result in &results {
        assert!(!result.to_string().contains(SENTINEL), "{result}");
    }
    let (name, detail) = tool_error(&results[3]);
    assert_eq!(name, "upstream_error");
    assert!(
        detail.contains("org.freedesktop.DBus.Error.Failed"),
        "{detail}"
    );
    let audit = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!audit.contains(SENTINEL), "{audit}");
}

#[tokio::test]
async fn an_apps_action_names_reach_neither_errors_nor_the_audit_log() {
    let mut desk = Desk::start("el-names").await;
    let odd = desk.element("Odd").await;
    let expect = json!({"window_id": WINDOW});
    let named = json!({"element": odd, "action": ODD_ACTION, "expect": expect});
    let taken = desk
        .server
        .structured_with("activate_element", named.clone())
        .await;
    assert_eq!(
        taken["element"]["action"],
        json!({"kind": "other", "index": 0})
    );
    let by_default = json!({"element": odd, "expect": expect});
    let defaulted = desk
        .server
        .structured_with("activate_element", by_default)
        .await;
    assert_eq!(
        defaulted["element"]["action"],
        json!({"kind": "click", "index": 1})
    );
    let unlisted = json!({"element": odd, "action": "ACTION-SENTINEL", "expect": expect});
    let mistake = mistake(&desk.server.call("activate_element", unlisted).await);
    assert_eq!(
        mistake,
        "invalid arguments: `action` isn't one of the element's actions: the element has 2 actions, of the kinds other, click; elements lists them"
    );
    desk.mock.fail("DoAction", "no");
    let refused = desk.server.call("activate_element", named.clone()).await;
    assert_eq!(tool_error(&refused).0, "upstream_error");
    let mut unknown = named;
    unknown["element"] = json!("elem-00000000-999");
    let unknown = desk.server.call("activate_element", unknown).await;
    assert_eq!(tool_error(&unknown).0, "element_stale");
    for result in [&refused, &unknown] {
        assert!(!result.to_string().contains("SENTINEL"), "{result}");
    }
    let audit = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!audit.contains("SENTINEL"), "{audit}");
    assert_eq!(desk.mock.done(), [0, 1, 0]);
}

/// An outcome whose reply was lost: `uncertain`, with nothing said accepted, a
/// screenshot, and the audit log saying the same.
fn assert_uncertain(desk: &Desk, tool: &str, result: &Value) {
    assert_eq!(result["isError"], false, "{result}");
    let outcome = &result["structuredContent"];
    assert_eq!(outcome["accepted"], Value::Null, "{outcome}");
    assert_eq!(outcome["observed"], "uncertain", "{outcome}");
    assert!(
        outcome["detail"]
            .as_str()
            .unwrap()
            .contains("the call went out, so the app may have acted"),
        "{outcome}"
    );
    assert_eq!(result["content"][1]["type"], "image", "{result}");
    let line = desk.fixture.audit_lines().pop().unwrap();
    assert_eq!(line["tool"], tool);
    assert_eq!(line["accepted"], Value::Null, "{line}");
    assert_eq!(line["observed"], "uncertain", "{line}");
}

#[tokio::test]
async fn a_reply_that_comes_too_late_is_uncertain_and_never_retried() {
    let mut desk = Desk::start("el-late").await;
    let button = desk.element("Safe").await;
    let entry = desk.element("Field").await;
    let expect = json!({"window_id": WINDOW});
    desk.mock.delay("DoAction", Duration::from_millis(1500));
    let activated = desk
        .server
        .call(
            "activate_element",
            json!({"element": button, "expect": expect}),
        )
        .await;
    assert_uncertain(&desk, "activate_element", &activated);
    assert_eq!(desk.mock.calls("DoAction"), 1);
    desk.mock
        .delay("SetTextContents", Duration::from_millis(1500));
    let set = json!({"element": entry, "text": "abc", "expect": expect});
    let set = desk.server.call("set_element_text", set).await;
    assert_uncertain(&desk, "set_element_text", &set);
    assert_eq!(desk.mock.calls("SetTextContents"), 1);
}

#[tokio::test]
async fn a_bus_lost_after_the_call_went_out_is_uncertain() {
    for (tool, member) in [
        ("activate_element", "DoAction"),
        ("set_element_text", "SetTextContents"),
    ] {
        let mut desk = Desk::start("el-lost").await;
        let name = if tool == "activate_element" {
            "Safe"
        } else {
            "Field"
        };
        let element = desk.element(name).await;
        let held = desk.mock.hold(member);
        let arguments = json!({"element": element, "text": "abc", "expect": {"window_id": WINDOW}});
        let id = desk.server.start_call(tool, arguments).await;
        held.arrived().await;
        desk.bus.kill().await;
        let result = desk.server.response(id).await["result"].clone();
        assert_uncertain(&desk, tool, &result);
        assert_eq!(desk.mock.calls(member), 1);
    }
}

/// Activates `element` in window `window`, expecting the error named `error`, with nothing
/// sent.
async fn refused(desk: &mut Desk, element: &str, window: u64, error: &str) {
    let arguments = json!({"element": element, "expect": {"window_id": window}});
    let result = desk.server.call("activate_element", arguments).await;
    assert_eq!(tool_error(&result).0, error, "{result}");
    assert_eq!(desk.mock.calls("DoAction"), 0);
}

#[tokio::test]
async fn another_object_at_a_listed_elements_path_is_stale() {
    let mut desk = Desk::start("el-same-path").await;
    let button = desk.element("Safe").await;
    // A virtualized list reuses the object for another record and moves it among its
    // parent's children: same path, role and action.
    desk.mock.change("/panel", |panel| {
        panel.children.retain(|child| child != "/button");
        panel.children.push("/button".to_owned());
    });
    refused(&mut desk, &button, WINDOW, "element_stale").await;
    // No longer among its parent's children at all.
    desk.mock.change("/panel", |panel| {
        panel.children.retain(|child| child != "/button");
    });
    refused(&mut desk, &button, WINDOW, "element_stale").await;
    // Back in place, it is the element listed again.
    desk.mock.change("/panel", |panel| {
        panel.children.insert(0, "/button".to_owned());
    });
    let arguments = json!({"element": button, "expect": {"window_id": WINDOW}});
    let result = desk.server.call("activate_element", arguments).await;
    assert_eq!(result["structuredContent"]["accepted"], true, "{result}");
}

/// Calls `held`'s tool on `element` and checks it is refused with `element_stale` and
/// nothing reaches the app.
async fn stale(desk: &mut Desk, held: &Held, element: &str) {
    let before = desk.mock.calls(held.acts);
    let result = desk.server.call(held.tool, held.arguments(element)).await;
    let (name, detail) = tool_error(&result);
    assert_eq!(name, "element_stale", "{result}");
    assert!(detail.contains("since `elements` listed it"), "{detail}");
    assert_eq!(desk.mock.calls(held.acts), before, "{result}");
}

// A virtualized list reuses an object for another record in place: the same path, index,
// role and actions, another name.
#[tokio::test]
async fn an_object_reused_in_place_for_another_record_is_stale() {
    let mut desk = Desk::start("el-reused").await;
    let button = desk.element("Safe").await;
    let entry = desk.element("Field").await;
    desk.mock.change("/button", |object| {
        object.name = "Delete different record".to_owned();
    });
    desk.mock.change("/entry", |object| {
        object.name = "Different record".to_owned();
    });
    stale(&mut desk, &ACTIVATE, &button).await;
    stale(&mut desk, &SET_TEXT, &entry).await;
}

// The row a list reused holds the record's name; the button in it keeps its own.
#[tokio::test]
async fn a_parent_renamed_in_place_makes_its_elements_stale_for_every_tool() {
    let mut desk = Desk::start("el-parent-renamed").await;
    let button = desk.element("Safe").await;
    let entry = desk.element("Field").await;
    let shot = desk.screenshot_ref().await;
    desk.mock.change("/panel", |object| {
        object.name = "Another record".to_owned();
    });
    stale(&mut desk, &ACTIVATE, &button).await;
    stale(&mut desk, &SET_TEXT, &entry).await;
    let aimed = json!({"screenshot_ref": shot, "element": button});
    let result = desk.server.call("pointer_move", aimed).await;
    let (name, detail) = tool_error(&result);
    assert_eq!(name, "element_stale", "{result}");
    assert!(
        detail.contains("a parent of the element changed its name"),
        "{detail}"
    );
}

// A counter's activation relabels it; the ref keeps the name read after each one. A name
// that changes on its own makes the ref stale, but a pointer aim doesn't check the
// element's own name, since a click doesn't read it afterwards.
#[tokio::test]
async fn a_counter_stays_valid_through_its_own_relabelling_and_only_through_that() {
    let mut objects = objects();
    objects.push((
        "/counter",
        Object {
            counter: Some(0),
            ..Object::button("Count: 0", &["click"])
        },
    ));
    let windows = [
        app_window(WINDOW, true),
        window_on(2, Some("other"), 1, false),
    ];
    if let Some((_, panel)) = objects.iter_mut().find(|(path, _)| *path == "/panel") {
        panel.children.push("/counter".to_owned());
    }
    let mut desk = Desk::start_with("el-counter", &["/frame"], objects, &windows).await;
    let counter = desk.element("Count: 0").await;
    let shot = desk.screenshot_ref().await;
    let arguments = ACTIVATE.arguments(&counter);
    for count in 1..=20 {
        let result = desk
            .server
            .call("activate_element", arguments.clone())
            .await;
        assert_eq!(
            result["structuredContent"]["accepted"], true,
            "{count}: {result}"
        );
        assert_eq!(desk.mock.object("/counter").name, format!("Count: {count}"));
    }
    desk.mock.change("/counter", |object| {
        object.name = "Count: 99".to_owned();
    });
    let aimed = json!({"screenshot_ref": shot, "element": counter});
    let moved = desk.server.call("pointer_move", aimed).await;
    // Past the element's checks: the mock's button can't be placed on the fake desktop.
    assert_eq!(tool_error(&moved).0, "element_unmappable", "{moved}");
    stale(&mut desk, &ACTIVATE, &counter).await;
}

#[tokio::test]
async fn an_element_moved_to_another_window_of_its_app_is_stale() {
    let mut desk = two_windows("el-moved").await;
    let button = desk.element("Safe").await;
    // The window that listed it is still focused; the button now sits in the other one.
    desk.move_element("/button", Moved::ToTheOtherWindow);
    refused(&mut desk, &button, WINDOW, "element_stale").await;
    // The other window's elements are refused while it isn't focused.
    let listing = desk
        .server
        .structured_with("elements", json!({"window_id": 3}))
        .await;
    let moved = listing["elements"][0]["element_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    refused(&mut desk, &moved, 3, "focus_mismatch").await;
}

/// What changes while an element action is still reading the element.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// Keyboard focus leaves the element's window.
    FocusMoved,
    ScreenLocked,
    /// The user sets the stop flag.
    Stopped,
    /// Another server's input may be stuck.
    InputDirty,
    /// The lease file is replaced, so another server could lock the new one.
    LeaseReplaced,
    /// The element becomes a password field.
    BecamePassword,
}

impl Desk {
    /// Makes `change` happen to the element at `path`, seen by the server before this
    /// returns. Returns the error it must refuse with.
    async fn make(&mut self, change: Change, path: &str) -> &'static str {
        match change {
            Change::FocusMoved => {
                self.stream
                    .send(&json!({"WindowFocusChanged": {"id": null}}));
                while !self.server.structured("desktop_state").await["focused_window"].is_null() {
                    tokio::task::yield_now().await;
                }
                "focus_mismatch"
            }
            Change::ScreenLocked => {
                self.noctalia.set(LOCKED);
                "screen_locked"
            }
            Change::Stopped => {
                std::fs::write(self.fixture.runtime_dir().join("stop"), "").unwrap();
                "stopped"
            }
            Change::InputDirty => {
                std::fs::write(self.fixture.runtime_dir().join("input-dirty"), "").unwrap();
                "recovery_required"
            }
            Change::LeaseReplaced => {
                let lease = self.fixture.runtime_dir().join("lease");
                std::fs::remove_file(&lease).unwrap();
                std::fs::write(&lease, "").unwrap();
                "lease_required"
            }
            Change::BecamePassword => {
                self.mock.change(path, |object| object.role = PASSWORD_TEXT);
                "secret_field"
            }
        }
    }
}

const CHANGES: [Change; 6] = [
    Change::FocusMoved,
    Change::ScreenLocked,
    Change::Stopped,
    Change::InputDirty,
    Change::LeaseReplaced,
    Change::BecamePassword,
];

/// An element action of `tool` on the element named and at `element`, whose `read` of the
/// element the test holds, and whose call that acts is `acts`.
struct Held {
    tool: &'static str,
    element: (&'static str, &'static str),
    read: &'static str,
    acts: &'static str,
}

/// `activate_element` on the button.
const ACTIVATE: Held = Held {
    tool: "activate_element",
    element: ("Safe", "/button"),
    read: "GetActions",
    acts: "DoAction",
};

/// `set_element_text` on the text field.
const SET_TEXT: Held = Held {
    tool: "set_element_text",
    element: ("Field", "/entry"),
    read: "GetInterfaces",
    acts: "SetTextContents",
};

impl Held {
    /// The same action, holding `read` instead.
    const fn reading(&self, read: &'static str) -> Self {
        Self {
            tool: self.tool,
            element: self.element,
            read,
            acts: self.acts,
        }
    }

    /// A short name for fixtures: the call that acts and the read held.
    fn tag(&self) -> String {
        let read = self.read.chars().skip(3).take(4);
        self.acts.chars().take(2).chain(read).collect()
    }

    fn arguments(&self, element: &str) -> Value {
        let mut arguments = json!({"element": element, "expect": {"window_id": WINDOW}});
        if self.tool == "set_element_text" {
            arguments["text"] = json!("abc");
        }
        arguments
    }
}

/// Holds `held`'s first read of its element, makes `change` meanwhile, and checks that the
/// call is refused and the call that acts never reaches the app.
async fn refused_at_the_last_gate(held: &Held, change: Change) {
    let (name, path) = held.element;
    let mut desk = Desk::start(&format!("el-g-{}-{}", held.tag(), change as u8)).await;
    let element = desk.element(name).await;
    let reading = desk.mock.hold_on(held.read, path);
    let id = desk
        .server
        .start_call(held.tool, held.arguments(&element))
        .await;
    reading.arrived().await;
    let expected = desk.make(change, path).await;
    reading.release();
    let result = desk.server.response(id).await["result"].clone();
    assert_eq!(tool_error(&result).0, expected, "{change:?}: {result}");
    assert_eq!(desk.mock.calls(held.acts), 0, "{change:?}");
}

#[tokio::test]
async fn an_activation_is_refused_when_anything_changed_while_the_element_was_read() {
    for change in CHANGES {
        refused_at_the_last_gate(&ACTIVATE, change).await;
    }
}

#[tokio::test]
async fn setting_text_is_refused_when_anything_changed_while_the_element_was_read() {
    for change in CHANGES {
        refused_at_the_last_gate(&SET_TEXT, change).await;
    }
}

// The role is the app's last answer before the call that acts: holding it back can't make
// the checks of focus, the lock screen and the desk that follow it stale.
#[tokio::test]
async fn an_activation_is_refused_when_anything_changed_while_the_app_held_its_last_answer() {
    for change in CHANGES {
        refused_at_the_last_gate(&ACTIVATE.reading("GetRole"), change).await;
    }
}

#[tokio::test]
async fn setting_text_is_refused_when_anything_changed_while_the_app_held_its_last_answer() {
    for change in CHANGES {
        refused_at_the_last_gate(&SET_TEXT.reading("GetRole"), change).await;
    }
}

// The role is read after the states, not beside them, so a field that becomes a password
// field while its states are read is refused.
#[tokio::test]
async fn a_field_that_becomes_a_password_field_while_its_states_are_read_is_refused() {
    for held in [&ACTIVATE, &SET_TEXT] {
        refused_at_the_last_gate(&held.reading("GetState"), Change::BecamePassword).await;
    }
}

/// How an element moves in its app's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moved {
    /// To the end of its parent's children, as when a list inserts a row before it.
    InItsParent,
    /// Into the app's other window, which isn't focused.
    ToTheOtherWindow,
}

/// A desk whose mock application has a second window, 3, holding an empty panel.
async fn two_windows(name: &str) -> Desk {
    let mut objects = objects();
    objects.extend([
        ("/frame2", Object::frame("Other", (640, 480), &["/panel2"])),
        ("/panel2", Object::panel(&[])),
    ]);
    let mut other = app_window(3, false);
    other["title"] = json!("Other");
    other["layout"]["window_size"] = json!([640, 480]);
    let windows = [app_window(WINDOW, true), other];
    Desk::start_with(name, &["/frame", "/frame2"], objects, &windows).await
}

impl Desk {
    fn move_element(&self, path: &str, moved: Moved) {
        self.mock.change("/panel", |panel| {
            panel.children.retain(|child| child != path);
            if moved == Moved::InItsParent {
                panel.children.push(path.to_owned());
            }
        });
        if moved == Moved::ToTheOtherWindow {
            self.mock.change("/panel2", |panel| {
                panel.children.push(path.to_owned());
            });
        }
    }
}

// The last gate finds the element again in its window's tree, so one that moved after the
// first look is refused.
#[tokio::test]
async fn an_element_moved_after_it_was_first_found_is_stale() {
    for held in [&ACTIVATE, &SET_TEXT] {
        for moved in [Moved::InItsParent, Moved::ToTheOtherWindow] {
            let (name, path) = held.element;
            let mut desk = two_windows(&format!("el-move-{}-{}", held.tag(), moved as u8)).await;
            let element = desk.element(name).await;
            let reading = desk.mock.hold_on(held.read, path);
            let id = desk
                .server
                .start_call(held.tool, held.arguments(&element))
                .await;
            reading.arrived().await;
            desk.move_element(path, moved);
            reading.release();
            let result = desk.server.response(id).await["result"].clone();
            assert_eq!(
                tool_error(&result).0,
                "element_stale",
                "{moved:?}: {result}"
            );
            assert_eq!(desk.mock.calls(held.acts), 0, "{moved:?}");
        }
    }
}

// The index sent is read at the last gate, and must still name the action listed.
#[tokio::test]
async fn an_action_list_changed_before_the_last_gate_never_sends_another_action() {
    for (actions, sent) in [
        (["delete", "click"], vec![]),
        (["click", "delete"], vec![0]),
    ] {
        let mut desk = Desk::start("el-action-list").await;
        let button = desk.element("Safe").await;
        let first = desk.mock.hold_on("GetActions", "/button");
        let id = desk
            .server
            .start_call("activate_element", ACTIVATE.arguments(&button))
            .await;
        first.arrived().await;
        // The last gate's first read: the frame's child on the way down to the element.
        let last = desk.mock.hold_on("GetChildAtIndex", "/frame");
        first.release();
        last.arrived().await;
        desk.mock.change("/button", |object| {
            object.actions = actions.map(str::to_owned).to_vec();
        });
        last.release();
        let result = desk.server.response(id).await["result"].clone();
        assert_eq!(desk.mock.done(), sent, "{result}");
        if sent.is_empty() {
            assert_eq!(tool_error(&result).0, "element_stale", "{result}");
        } else {
            let outcome = &result["structuredContent"];
            assert_eq!(outcome["accepted"], true, "{result}");
            assert_eq!(
                outcome["element"]["action"],
                json!({"kind": "click", "index": 0})
            );
        }
    }
}

/// The calls that read the element.
const READS: [&str; 4] = ["GetState", "GetRole", "CharacterCount", "GetActions"];

// A stop or a lease change once the call that acts went out can't recall it: the action is
// `uncertain`, with why, and nothing more is asked, not even a screenshot. Before it went
// out, the same changes refuse it (the tests above).
#[tokio::test]
async fn a_stop_or_lost_lease_after_the_call_went_out_leaves_it_uncertain() {
    for held in [&ACTIVATE, &SET_TEXT] {
        for (change, why) in [
            (Change::Stopped, "the user's stop flag cancelled the action"),
            (
                Change::LeaseReplaced,
                "the lease file was removed or replaced",
            ),
        ] {
            let (name, path) = held.element;
            let mut desk = Desk::start(&format!("el-cut-{}-{}", held.tag(), change as u8)).await;
            let element = desk.element(name).await;
            let acting = desk.mock.hold_on(held.acts, path);
            let id = desk
                .server
                .start_call(held.tool, held.arguments(&element))
                .await;
            acting.arrived().await;
            let mock = desk.mock.clone();
            let reads = || READS.map(|read| mock.calls(read));
            let read_before = reads();
            desk.make(change, path).await;
            let result = desk.server.response(id).await["result"].clone();
            assert_eq!(reads(), read_before, "{change:?}");
            acting.release();
            assert_eq!(result["isError"], false, "{result}");
            assert_eq!(result["content"].as_array().unwrap().len(), 1, "{result}");
            let outcome = &result["structuredContent"];
            assert_eq!(outcome["accepted"], Value::Null, "{outcome}");
            assert_eq!(outcome["observed"], "uncertain", "{outcome}");
            let detail = outcome["detail"].as_str().unwrap();
            assert!(detail.contains(why), "{detail}");
            assert!(detail.contains("may have taken effect"), "{detail}");
            assert_eq!(desk.mock.calls(held.acts), 1);
            let line = desk.fixture.audit_lines().pop().unwrap();
            assert_eq!(line["tool"], held.tool);
            assert_eq!(line["accepted"], Value::Null, "{line}");
            assert_eq!(line["observed"], "uncertain", "{line}");
        }
    }
}

#[tokio::test]
async fn a_password_field_is_never_activated() {
    let mut desk = Desk::start("el-password-activate").await;
    desk.mock.change("/password", |object| {
        object.actions = vec!["activate".to_owned()];
    });
    let password = desk.element("Secret").await;
    let arguments = json!({"element": password, "expect": {"window_id": WINDOW}});
    let refused = desk.server.call("activate_element", arguments).await;
    assert_eq!(tool_error(&refused).0, "secret_field");
    assert_eq!(desk.mock.calls("DoAction"), 0);
}

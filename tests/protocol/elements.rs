//! The element tools against a mock application on a private accessibility bus: what
//! reaches the application, what is refused before anything does, and what the result and
//! the audit log say. Every name and text is synthetic.

use serde_json::{Value, json};

use crate::atspi::{self, Bus, Mock, Object, PASSWORD_TEXT, TEXT};
use crate::client::{Server, mistake, tool_error};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, UNLOCKED};

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
    _stream: Stream,
    _noctalia: noctalia::Reply,
    _bus: Bus,
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
        let mut fixture = Fixture::new(name);
        fixture.program("noctalia", "exit 0");
        fixture.grim(&jpeg(1280, 720, b"evidence"));
        let (bus, mock) = atspi::start(&mut fixture, &["/frame"], objects()).await;
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let mut niri = Niri::start(&fixture);
        let windows = [
            app_window(WINDOW, true),
            window_on(2, Some("other"), 1, false),
        ];
        niri.set_windows(&windows, &[workspace()]);
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&windows);
        server.structured("acquire_desktop").await;
        Self {
            server,
            _niri: niri,
            _stream: stream,
            _noctalia: noctalia,
            _bus: bus,
            mock,
            fixture,
        }
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
    assert_eq!(desk.mock.calls("DoAction"), 3);
}

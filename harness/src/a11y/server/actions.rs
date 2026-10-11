//! M9b's acceptance: `activate_element` and `set_element_text` on each toolkit's fixture,
//! counted and read back by the fixture itself, and each refusal checked to have sent
//! nothing by the same counters. The text is synthetic, so reading it back is safe.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Checks, element_ref, expect, read_count};
use crate::a11y::{Bus, Fixture, Toolkit};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;

/// The acceptance rule: every one of twenty activations counted once and observed.
const ACTIVATIONS: u32 = 20;
/// How long a refused action has to show up anyway.
const SETTLE: Duration = Duration::from_millis(500);
const WAIT: Duration = Duration::from_secs(5);
/// The action kinds the audit log may hold instead of an action's name.
const KINDS: [&str; 5] = ["click", "press", "activate", "toggle", "other"];
/// `set_element_text`'s most text, in bytes.
const LARGEST: usize = 64 * 1024;
/// The `app_id` the policy file denies, which the GTK 4 fixture's `Rename` button takes.
pub(super) const DENIED: &str = "org.ncu.Denied";

/// What the checks need of one toolkit's fixture: its counted button and the files its
/// counter, entry text and password changes are reported in.
struct Files {
    button: &'static str,
    count: &'static str,
    entry: &'static str,
    /// The password fields, by a part of their name where there are several, and the
    /// files that count their changes.
    passwords: &'static [(Option<&'static str>, &'static str)],
}

const fn files(toolkit: Toolkit) -> Files {
    match toolkit {
        Toolkit::Gtk4 => Files {
            button: "Primary",
            count: "primary-count",
            entry: "a11y-entry-text",
            passwords: &[
                (Some("Password entry"), "password-changes"),
                (Some("Pin entry"), "pin-changes"),
            ],
        },
        Toolkit::Gtk3 => Files {
            button: "Gtk3",
            count: "gtk3-count",
            entry: "gtk3-entry-text",
            passwords: &[(None, "gtk3-password-changes")],
        },
        Toolkit::Qt => Files {
            button: "Qt",
            count: "qt-count",
            entry: "qt-entry-text",
            // Qt gives its password field no accessible name.
            passwords: &[(None, "qt-password-changes")],
        },
    }
}

impl Checks<'_, '_> {
    /// Every toolkit's element actions and refusals, then the deny list on GTK 4.
    pub(super) fn element_actions(&mut self, bus: &Bus, server: &str) -> Result<()> {
        for toolkit in Toolkit::ALL {
            self.toolkit_actions(bus, server, toolkit)?;
        }
        self.denied(bus)?;
        self.audited()
    }

    fn toolkit_actions(&mut self, bus: &Bus, server: &str, toolkit: Toolkit) -> Result<()> {
        let fixture = Fixture::start(self.session, toolkit)?;
        let window = fixture.window(self.session, bus)?.id;
        let files = files(toolkit);
        self.counted(files.count, 0)?;
        self.call("focus_window", json!({"id": window}))?;
        self.describe(toolkit, window)?;
        if toolkit == Toolkit::Gtk4 {
            crate::a11y::hidden::record(self.session, bus, fixture.pid())?;
        }
        let button = self.named(window, Some("button"), files.button)?;
        self.activated(toolkit, window, &button, files.count)?;
        self.set_text(toolkit, window, files.entry)?;
        for (name, changes) in files.passwords {
            self.secret(window, *name, changes)?;
        }
        self.unfocused(window, &button, files.count)?;
        let button = self.stopped_action(server, window, &button, &files)?;
        let button = self.without_lease(window, &button, &files)?;
        self.dismissed(window, &button)?;
        fixture.stop().map(drop)
    }

    /// Logs the role, states and actions of each of the window's elements, never their
    /// names, as the record of what each toolkit exposes.
    fn describe(&mut self, toolkit: Toolkit, window: u64) -> Result<()> {
        let listing = self.list(window, None, None)?;
        let described: Vec<Value> = field(&listing, "/elements")
            .as_array()
            .into_iter()
            .flatten()
            .map(|element| {
                json!([
                    field(element, "/role"),
                    field(element, "/states"),
                    field(element, "/actions")
                ])
            })
            .collect();
        self.session.log(&format!(
            "M9b: {toolkit:?} elements (role, states, actions): {}",
            Value::from(described)
        ))
    }

    /// Twenty activations, each counted once by the fixture and observed `present`.
    fn activated(
        &mut self,
        toolkit: Toolkit,
        window: u64,
        button: &str,
        counter: &str,
    ) -> Result<()> {
        let started = Instant::now();
        let mut took = Vec::new();
        let mut on_return = 0;
        for count in 1..=ACTIVATIONS {
            let asked = Instant::now();
            let outcome = self.call("activate_element", activation(button, window))?;
            took.push(asked.elapsed().as_millis());
            on_return += u32::from(read_count(self.session, counter)? == Some(count));
            expect(
                field(&outcome, "/accepted") == true
                    && field(&outcome, "/observed") == "present"
                    && field(&outcome, "/focus") == "matched"
                    && field(&outcome, "/element/role") == "button"
                    && field(&outcome, "/element/name").is_null(),
                "an activation observed present",
                &outcome,
            )?;
            self.counted(counter, count)?;
        }
        took.sort_unstable();
        self.session.log(&format!(
            "M9b: {toolkit:?} {ACTIVATIONS}/{ACTIVATIONS} activations counted once and present ({on_return} already counted when the call returned), in {:?}; call ms min {:?} median {:?} max {:?}",
            started.elapsed(),
            took.first(),
            took.get(took.len() / 2),
            took.last()
        ))
    }

    /// The plain entry gets synthetic text, which the fixture reads back whole.
    fn set_text(&mut self, toolkit: Toolkit, window: u64, file: &str) -> Result<()> {
        let entry = self.named(window, None, "Plain entry")?;
        let text = format!("ncu m9b {toolkit:?} entry");
        let asked = Instant::now();
        let outcome = self.call(
            "set_element_text",
            json!({"element": entry, "text": text, "expect": {"window_id": window}}),
        )?;
        let took = asked.elapsed();
        expect(
            field(&outcome, "/observed") == "matched"
                && field(&outcome, "/element/characters") == text.chars().count(),
            "set_element_text matched",
            &outcome,
        )?;
        let path = self.session.test_dir().root().join(file);
        let session = &mut *self.session;
        session.wait_until("m9b-entry", "the entry's text read back", WAIT, |_| {
            let now = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
            Ok((now == text).then_some(()))
        })?;
        self.session.log(&format!(
            "M9b: {toolkit:?} entry set and read back by the fixture, matched, in {took:?}: {outcome}"
        ))?;
        if toolkit == Toolkit::Gtk4 {
            self.largest(window, &entry)?;
        }
        Ok(())
    }

    /// The most text a call takes, 64 KiB, goes out; a GTK entry keeps at most 65534 bytes
    /// of it (`GTK_ENTRY_BUFFER_MAX_SIZE`), so it `differs`. A byte more is refused.
    fn largest(&mut self, window: u64, entry: &str) -> Result<()> {
        let text = "x".repeat(LARGEST);
        let asked = Instant::now();
        let outcome = self.call(
            "set_element_text",
            json!({"element": entry, "text": text, "expect": {"window_id": window}}),
        )?;
        let took = asked.elapsed();
        expect(
            field(&outcome, "/observed") == "differs"
                && field(&outcome, "/element/characters") == LARGEST - 2,
            "64 KiB set, of which GTK keeps 65534 characters",
            &outcome,
        )?;
        let over = self.refused(
            "set_element_text",
            json!({"element": entry, "text": format!("{text}x"), "expect": {"window_id": window}}),
        )?;
        expect(
            field(&over, "/error") == "text_too_long",
            "text_too_long past 64 KiB",
            &over,
        )?;
        self.session.log(&format!(
            "M9b: 64 KiB set in {took:?}: {outcome}; a byte more refused: {over}"
        ))
    }

    /// A password field is refused, and the fixture saw no change.
    fn secret(&mut self, window: u64, name: Option<&str>, changes: &str) -> Result<()> {
        let field_ref = self.named(window, Some("password_text"), name.unwrap_or(""))?;
        let refused = self.refused(
            "set_element_text",
            json!({"element": field_ref, "text": "ncu m9b secret", "expect": {"window_id": window}}),
        )?;
        expect(
            field(&refused, "/error") == "secret_field",
            "secret_field for a password field",
            &refused,
        )?;
        self.unchanged(changes, 0)?;
        self.session.log(&format!(
            "M9b: password field {changes} refused, nothing changed: {refused}"
        ))
    }

    /// With another window focused, the element's window isn't, so nothing is sent.
    fn unfocused(&mut self, window: u64, button: &str, counter: &str) -> Result<()> {
        let decoy = Fixture::start_decoy(self.session)?;
        let decoy_window = decoy.decoy_window(self.session)?;
        self.call("focus_window", json!({"id": decoy_window}))?;
        let refused = self.refused("activate_element", activation(button, window))?;
        expect(
            field(&refused, "/error") == "focus_mismatch",
            "focus_mismatch with another window focused",
            &refused,
        )?;
        self.unchanged(counter, ACTIVATIONS)?;
        decoy.stop()?;
        self.call("focus_window", json!({"id": window}))?;
        self.session.log(&format!(
            "M9b: unfocused window refused, nothing sent: {refused}"
        ))
    }

    /// With the stop flag set, nothing is sent; once resumed, the lease is taken again and
    /// the button listed again, since a stop drops the refs. Returns the new ref.
    fn stopped_action(
        &mut self,
        server: &str,
        window: u64,
        button: &str,
        files: &Files,
    ) -> Result<String> {
        self.session.run(server, &["stop".into()])?;
        let refused = self.refused("activate_element", activation(button, window))?;
        expect(
            field(&refused, "/error") == "stopped",
            "stopped with the stop flag set",
            &refused,
        )?;
        self.unchanged(files.count, ACTIVATIONS)?;
        self.session.run(server, &["resume".into()])?;
        let client = &mut self.client;
        self.session
            .wait_until("m9b-resume", "the lease taken again", WAIT, |session| {
                let result = client.call(session, "acquire_desktop", json!({}))?;
                Ok((field(&result, "/isError") == false).then_some(()))
            })?;
        self.session
            .log(&format!("M9b: stop flag refused, nothing sent: {refused}"))?;
        self.named(window, Some("button"), files.button)
    }

    /// Without the lease nothing is sent; the lease is taken again after. Returns a ref
    /// listed under the new lease.
    fn without_lease(&mut self, window: u64, button: &str, files: &Files) -> Result<String> {
        self.call("release_desktop", json!({"restore_focus": false}))?;
        let refused = self.refused("activate_element", activation(button, window))?;
        expect(
            field(&refused, "/error") == "lease_required",
            "lease_required without the lease",
            &refused,
        )?;
        self.unchanged(files.count, ACTIVATIONS)?;
        self.call("acquire_desktop", json!({}))?;
        self.session
            .log(&format!("M9b: no lease refused, nothing sent: {refused}"))?;
        self.named(window, Some("button"), files.button)
    }

    /// `Dismiss` closes its window: `gone`. Then a ref of that window is stale.
    fn dismissed(&mut self, window: u64, button: &str) -> Result<()> {
        let dismiss = self.named(window, Some("button"), "Dismiss")?;
        let outcome = self.call("activate_element", activation(&dismiss, window))?;
        expect(
            field(&outcome, "/observed") == "gone",
            "gone after a button that closes its window",
            &outcome,
        )?;
        let refused = self.refused("activate_element", activation(button, window))?;
        expect(
            field(&refused, "/error") == "element_stale",
            "element_stale after the window closed",
            &refused,
        )?;
        self.session.log(&format!(
            "M9b: Dismiss observed {}, then the window's element refused: {refused}",
            field(&outcome, "/observed")
        ))
    }

    /// GTK 4's `Rename` gives its window a denied `app_id`: the next activation in it is
    /// refused and sends nothing.
    fn denied(&mut self, bus: &Bus) -> Result<()> {
        let fixture = Fixture::start(self.session, Toolkit::Gtk4)?;
        let window = fixture.window(self.session, bus)?.id;
        self.counted("primary-count", 0)?;
        self.call("focus_window", json!({"id": window}))?;
        let primary = self.named(window, Some("button"), "Primary")?;
        let rename = self.named(window, Some("button"), "Rename")?;
        let renamed = self.call("activate_element", activation(&rename, window))?;
        let session = &mut *self.session;
        session.wait_until(
            "m9b-renamed",
            "the window's denied app_id",
            WAIT,
            |session| {
                let now = super::super::window(session, window)?;
                Ok(now
                    .filter(|now| now.app_id.as_deref() == Some(DENIED))
                    .map(drop))
            },
        )?;
        let refused = self.refused("activate_element", activation(&primary, window))?;
        expect(
            field(&refused, "/error") == "app_denied",
            "app_denied once the window's app is denied",
            &refused,
        )?;
        self.unchanged("primary-count", 0)?;
        self.session.log(&format!(
            "M9b: Rename observed {}, then the denied window refused, nothing sent: {refused}",
            field(&renamed, "/observed")
        ))?;
        fixture.stop().map(drop)
    }

    /// The audit log names the role, and the action or the text's length, of each element
    /// action that went through, and never the text set or an element's name. A ref the
    /// stop flag or a release revoked has no role. Kept as `audit-m9b.jsonl`.
    fn audited(&mut self) -> Result<()> {
        let path = self
            .session
            .test_dir()
            .state()
            .join("niri-computer-use/audit.jsonl");
        let audit = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
        fs::write(self.session.artifact("audit-m9b.jsonl"), &audit)
            .context("copy the audit log")?;
        let secrets = [
            "ncu m9b", "xxxxxxxx", "entry", "Primary", "Dismiss", "Rename",
        ];
        let mut acted: u32 = 0;
        for line in audit.lines() {
            let entry: Value = serde_json::from_str(line).context("parse an audit line")?;
            let detail = match field(&entry, "/tool").as_str() {
                Some("activate_element") => "/args/action",
                Some("set_element_text") => "/args/text_len",
                _ => continue,
            };
            let leaked = secrets.iter().any(|secret| line.contains(secret));
            expect(
                !leaked,
                "an element action logged with no text or name",
                &entry,
            )?;
            if !field(&entry, "/error").is_null() {
                continue;
            }
            expect(
                !field(&entry, "/args/role").is_null() && !field(&entry, detail).is_null(),
                "an element action logged with its role, and its action or length",
                &entry,
            )?;
            let kind = field(&entry, "/args/action");
            expect(
                kind.is_null() || KINDS.iter().any(|known| kind == known),
                "an element action logged with its action's kind, not its name",
                &entry,
            )?;
            acted += 1;
        }
        let wanted = 3 * ACTIVATIONS;
        if acted < wanted {
            return Err(Failure::new(format!(
                "M9b: expected at least {wanted} element actions in the audit log; saw {acted}"
            )));
        }
        self.session.log(&format!(
            "M9b: audit log: {acted} element actions with their role, none with text or a name"
        ))
    }

    /// The one element of `window` whose name contains `name`, with `role` if given.
    fn named(&mut self, window: u64, role: Option<&str>, name: &str) -> Result<String> {
        let listing = self.list(window, role, Some(name))?;
        let found = field(&listing, "/elements")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let [element] = found.as_slice() else {
            return Err(Failure::new(format!(
                "M9b: expected one element named {name:?} in window {window}; saw {} of them",
                found.len()
            )));
        };
        element_ref(element)
    }

    fn list(&mut self, window: u64, role: Option<&str>, name: Option<&str>) -> Result<Value> {
        let mut arguments = serde_json::Map::new();
        arguments.insert("window_id".to_owned(), json!(window));
        arguments.insert("limit".to_owned(), json!(100));
        if let Some(role) = role {
            arguments.insert("role".to_owned(), json!(role));
        }
        if let Some(name) = name {
            arguments.insert("name_contains".to_owned(), json!(name));
        }
        self.call("elements", Value::Object(arguments))
    }

    /// Calls `tool`, which must fail; returns the error.
    fn refused(&mut self, tool: &str, arguments: Value) -> Result<Value> {
        let result = self.client.call(self.session, tool, arguments)?;
        expect(field(&result, "/isError") == true, "a refusal", &result)?;
        Ok(field(&result, "/structuredContent").clone())
    }

    /// The counter stays at `count` for a while.
    fn unchanged(&self, counter: &str, count: u32) -> Result<()> {
        let session = &*self.session;
        session.still_absent("m9b-nothing-sent", SETTLE, || {
            Ok(read_count(session, counter)? != Some(count))
        })
    }
}

fn activation(element: &str, window: u64) -> Value {
    json!({"element": element, "expect": {"window_id": window}})
}

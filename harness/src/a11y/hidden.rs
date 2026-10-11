//! What GTK 4 puts on the accessibility bus for an entry that only hides its text
//! (`Gtk.Entry(visibility=False)`, free-form input purpose), beside the plain entry: its
//! role, states, interfaces, attributes and `Text` contents. `secret_field` refuses the
//! role `password_text`; this records whether anything else could tell such an entry
//! apart. The fixture's text is synthetic, so reading it is safe.

use serde_json::{Value, json};

use super::{Bus, ROOT, busctl};
use crate::failure::{Failure, Result};
use crate::mcp::field;
use crate::session::Session;

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT_INTERFACE: &str = "org.a11y.atspi.Text";
/// AT-SPI's role `text`.
const TEXT: u64 = 61;
/// How deep the walk goes; the fixture's entries sit a few levels down.
const DEPTH: usize = 8;

/// Records both entries of the GTK 4 fixture `pid` and requires the hidden one to have
/// the plain one's role: the server can't tell it apart by role, as its docs say.
pub(crate) fn record(session: &mut Session<'_>, bus: &Bus, pid: i32) -> Result<()> {
    let app = app(session, bus, pid)?;
    let mut entries = Vec::new();
    walk(session, bus, (&app, ROOT), DEPTH, &mut entries)?;
    let find = |name: &str| {
        entries
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, path)| path.clone())
            .ok_or_else(|| Failure::new(format!("M9b: no {name} among {entries:?}")))
    };
    let hidden = describe(session, bus, &app, &find("Hidden entry")?);
    let plain = describe(session, bus, &app, &find("Plain entry")?);
    session.log(&format!(
        "M9b: GTK 4 hidden entry {hidden}; plain entry {plain}"
    ))?;
    if field(&hidden, "/role") != field(&plain, "/role") {
        return Err(Failure::new(format!(
            "M9b: the hidden entry's role differs from the plain entry's now: {hidden}; update the secret_field docs"
        )));
    }
    Ok(())
}

/// The fixture's connection on the bus.
fn app(session: &Session<'_>, bus: &Bus, pid: i32) -> Result<String> {
    for app in bus.apps(session)? {
        if bus.pid(session, &app)? == pid {
            return Ok(app);
        }
    }
    Err(Failure::new(format!("M9b: no application with pid {pid}")))
}

/// Collects the name and path of every object with the role `text` under `at`.
fn walk(
    session: &Session<'_>,
    bus: &Bus,
    at: (&str, &str),
    depth: usize,
    found: &mut Vec<(String, String)>,
) -> Result<()> {
    let (app, path) = at;
    let role = call(session, bus, app, path, &[ACCESSIBLE, "GetRole"])?;
    if role.pointer("/data/0").and_then(Value::as_u64) == Some(TEXT) {
        found.push((name(session, bus, app, path)?, path.to_owned()));
    }
    if depth == 0 {
        return Ok(());
    }
    let children = call(session, bus, app, path, &[ACCESSIBLE, "GetChildren"])?;
    let children = children
        .pointer("/data/0")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for child in children {
        if let (Some(child_app), Some(child_path)) = (
            child.pointer("/0").and_then(Value::as_str),
            child.pointer("/1").and_then(Value::as_str),
        ) {
            walk(session, bus, (child_app, child_path), depth - 1, found)?;
        }
    }
    Ok(())
}

fn name(session: &Session<'_>, bus: &Bus, app: &str, path: &str) -> Result<String> {
    let args = [
        format!("--address={}", bus.address),
        "--json=short".to_owned(),
        "get-property".to_owned(),
        app.to_owned(),
        path.to_owned(),
        ACCESSIBLE.to_owned(),
        "Name".to_owned(),
    ]
    .map(Into::into);
    let output = session.run("busctl", &args)?;
    let reply: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Failure::new(format!("parse busctl's reply: {error}")))?;
    Ok(reply
        .pointer("/data")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned())
}

/// What the object at `path` exposes; a call that fails is recorded as its error.
fn describe(session: &Session<'_>, bus: &Bus, app: &str, path: &str) -> Value {
    let read = |call_args: &[&str]| {
        call(session, bus, app, path, call_args).map_or_else(
            |error| json!({"error": error.to_string()}),
            |reply| reply.pointer("/data/0").cloned().unwrap_or(Value::Null),
        )
    };
    json!({
        "role": read(&[ACCESSIBLE, "GetRole"]),
        "states": read(&[ACCESSIBLE, "GetState"]),
        "interfaces": read(&[ACCESSIBLE, "GetInterfaces"]),
        "attributes": read(&[ACCESSIBLE, "GetAttributes"]),
        // busctl would take -1, "to the end", for an option.
        "text": read(&[TEXT_INTERFACE, "GetText", "ii", "0", "100"]),
        "text_attributes": read(&[TEXT_INTERFACE, "GetDefaultAttributes"]),
    })
}

fn call(session: &Session<'_>, bus: &Bus, app: &str, path: &str, call: &[&str]) -> Result<Value> {
    let mut args = vec![app, path];
    args.extend_from_slice(call);
    busctl(session, &bus.address, &args)
}

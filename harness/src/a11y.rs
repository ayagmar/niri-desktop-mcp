//! M9's nested acceptance: a private accessibility bus in the nested session, with the
//! fixtures registered on it. The bus launcher claims `org.a11y.Bus` on the nested
//! session bus, which has no service directories, so the registry daemon is started by
//! hand; nothing is activated from the host. Before any check reads the bus, its socket
//! must resolve under `TEST_DIR/run`, and every application the registry lists must be a
//! process of the nested session.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use niri_ipc::{Request, Response, Window};
use serde_json::Value;

mod hidden;
mod server;

use crate::config::Decorations;
use crate::failure::{Context as _, Failure, Result};
use crate::nested;
use crate::runner::Process;
use crate::session::Session;

const LAUNCHER: &str = "/usr/lib/at-spi-bus-launcher";
const REGISTRY: &str = "/usr/lib/at-spi2-registryd";
/// Within what is left of the run's deadline.
const BUS_DEADLINE: Duration = Duration::from_secs(230);
const STARTUP: Duration = Duration::from_secs(10);
const FIXTURE_DEADLINE: Duration = Duration::from_secs(220);
const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";

pub(crate) fn run(session: &mut Session<'_>, server: &str, decorations: Decorations) -> Result<()> {
    session.log(&format!("M9: decorations {decorations:?}"))?;
    let bus = Bus::start(session)?;
    bus.only_nested_apps(session, "before the fixtures")?;
    for toolkit in Toolkit::ALL {
        let fixture = Fixture::start(session, toolkit)?;
        let window = fixture.window(session, &bus)?;
        session.log(&format!(
            "M9: {toolkit:?} window {} app_id {:?}, size {:?}",
            window.id, window.app_id, window.layout.window_size
        ))?;
        bus.only_nested_apps(session, &format!("with the {toolkit:?} fixture"))?;
        fixture.stop()?;
    }
    server::run(session, &bus, server, decorations)?;
    bus.only_nested_apps(session, "after the server's checks")?;
    bus.stop()
}

/// The nested accessibility bus: its launcher, the registry daemon, and its address.
#[derive(Debug)]
pub(crate) struct Bus {
    launcher: Process,
    registry: Process,
    pub(crate) address: String,
}

impl Bus {
    /// Starts the launcher, waits for the address on the session bus and checks that its
    /// socket is under `TEST_DIR/run`, then starts the registry and waits for its name.
    pub(crate) fn start(session: &mut Session<'_>) -> Result<Self> {
        let session_bus =
            std::env::var("DBUS_SESSION_BUS_ADDRESS").context("read DBUS_SESSION_BUS_ADDRESS")?;
        // dbus-broker-launch, the launcher's first choice under systemd, logs to the
        // user's journal; dbus-daemon without `<syslog/>` logs to stderr only.
        let args = [
            "ATSPI_DBUS_IMPLEMENTATION=dbus-daemon",
            LAUNCHER,
            "--launch-immediately",
        ]
        .map(OsString::from);
        let launcher = session.start(
            "env",
            &args,
            session.artifact("at-spi-bus.log"),
            BUS_DEADLINE,
        )?;
        owned(session, &session_bus, "org.a11y.Bus")?;
        let reply = busctl(
            session,
            &session_bus,
            &[
                "org.a11y.Bus",
                "/org/a11y/bus",
                "org.a11y.Bus",
                "GetAddress",
            ],
        )?;
        let address = string_reply(&reply)?;
        let socket = nested::dbus_socket(&address)?;
        nested::resolve_under(&socket, &session.test_dir().run())?;
        session.log(&format!("M9: accessibility bus at {}", socket.display()))?;
        let registry = session.start(
            REGISTRY,
            &[],
            session.artifact("at-spi-registry.log"),
            BUS_DEADLINE,
        )?;
        owned(session, &address, REGISTRY_NAME)?;
        Ok(Self {
            launcher,
            registry,
            address,
        })
    }

    /// The registered applications: their unique bus names.
    pub(crate) fn apps(&self, session: &Session<'_>) -> Result<Vec<String>> {
        let reply = busctl(
            session,
            &self.address,
            &[
                REGISTRY_NAME,
                ROOT,
                "org.a11y.atspi.Accessible",
                "GetChildren",
            ],
        )?;
        children(&reply)
    }

    /// Requires every application on the bus to be a process of the nested session: a
    /// descendant of the nested niri, which is the supervisor's parent.
    pub(crate) fn only_nested_apps(&self, session: &mut Session<'_>, when: &str) -> Result<()> {
        let niri = rustix::process::getppid()
            .ok_or_else(|| Failure::new("the supervisor has no parent"))?
            .as_raw_nonzero()
            .get();
        let apps = self.apps(session)?;
        let mut pids = Vec::new();
        for app in &apps {
            let pid = self.pid(session, app)?;
            if !descends_from(pid, niri)? {
                return Err(Failure::new(format!(
                    "M9: application {app} on the accessibility bus is pid {pid}, outside the nested session"
                )));
            }
            pids.push(pid);
        }
        session.log(&format!(
            "M9: {when}, the registry lists {} applications, all nested: pids {pids:?}",
            apps.len()
        ))
    }

    /// The process ID of a bus connection, as the bus daemon knows it.
    pub(crate) fn pid(&self, session: &Session<'_>, name: &str) -> Result<i32> {
        let reply = busctl(
            session,
            &self.address,
            &[
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                "s",
                name,
            ],
        )?;
        let pid = field_u64(&reply)?;
        i32::try_from(pid).map_err(|_| Failure::new(format!("pid {pid} is out of range")))
    }

    /// Whether an application with process ID `pid` is on the bus.
    pub(crate) fn registered(&self, session: &Session<'_>, pid: i32) -> Result<bool> {
        for app in self.apps(session)? {
            if self.pid(session, &app)? == pid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn stop(self) -> Result<()> {
        self.registry.stop()?;
        self.launcher.stop().map(drop)
    }
}

/// Waits until `name` has an owner on the bus at `address`.
fn owned(session: &mut Session<'_>, address: &str, name: &str) -> Result<()> {
    let what = format!("{name} on the bus");
    session.wait_until("a11y-name", &what, STARTUP, |session| {
        let reply = busctl(
            session,
            address,
            &[
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                "s",
                name,
            ],
        )?;
        Ok((reply.pointer("/data/0") == Some(&Value::Bool(true))).then_some(()))
    })
}

/// One method call with `busctl`, whose reply comes back as JSON:
/// `{"type": <signature>, "data": [<values>]}`.
fn busctl(session: &Session<'_>, address: &str, call: &[&str]) -> Result<Value> {
    let mut args = vec![
        OsString::from(format!("--address={address}")),
        "--json=short".into(),
        "call".into(),
    ];
    args.extend(call.iter().map(OsString::from));
    let output = session.run("busctl", &args)?;
    serde_json::from_slice(&output.stdout).context("parse busctl's reply")
}

fn string_reply(reply: &Value) -> Result<String> {
    reply
        .pointer("/data/0")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("expected a string reply, got {reply}")))
}

fn field_u64(reply: &Value) -> Result<u64> {
    reply
        .pointer("/data/0")
        .and_then(Value::as_u64)
        .ok_or_else(|| Failure::new(format!("expected a number reply, got {reply}")))
}

/// The bus names of a `GetChildren` reply, `a(so)`.
fn children(reply: &Value) -> Result<Vec<String>> {
    let list = reply
        .pointer("/data/0")
        .and_then(Value::as_array)
        .ok_or_else(|| Failure::new(format!("expected a(so), got {reply}")))?;
    list.iter()
        .map(|child| {
            child
                .pointer("/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| Failure::new(format!("expected (so), got {child}")))
        })
        .collect()
}

/// Whether `pid` is `ancestor` or one of its descendants, following `/proc/<pid>/stat`.
fn descends_from(pid: i32, ancestor: i32) -> Result<bool> {
    let mut current = pid;
    while current > 1 {
        if current == ancestor {
            return Ok(true);
        }
        let stat = fs::read_to_string(format!("/proc/{current}/stat"))
            .context(format!("read /proc/{current}/stat"))?;
        current = parent_of(&stat)
            .ok_or_else(|| Failure::new(format!("unreadable /proc/{current}/stat")))?;
    }
    Ok(false)
}

/// The parent PID in a `/proc/<pid>/stat` line: the second field after the command, which
/// is in parentheses and may itself hold spaces and parentheses.
fn parent_of(stat: &str) -> Option<i32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The toolkits of the fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Toolkit {
    Gtk4,
    Gtk3,
    Qt,
}

impl Toolkit {
    pub(crate) const ALL: [Self; 3] = [Self::Gtk4, Self::Gtk3, Self::Qt];
}

/// A running fixture.
#[derive(Debug)]
pub(crate) struct Fixture {
    pub(crate) toolkit: Toolkit,
    process: Process,
}

impl Fixture {
    /// Writes the fixture's source into `TEST_DIR` and starts it. The Qt fixture gets
    /// accessibility switched on, its logs on stderr instead of the journal, and file
    /// writes from QML for its counter, in its own environment only.
    pub(crate) fn start(session: &Session<'_>, toolkit: Toolkit) -> Result<Self> {
        let root = session.test_dir().root();
        let (name, source) = match toolkit {
            Toolkit::Gtk4 => ("gtk.py", include_str!("../fixtures/gtk.py")),
            Toolkit::Gtk3 => ("gtk3.py", include_str!("../fixtures/gtk3.py")),
            Toolkit::Qt => ("qt.qml", include_str!("../fixtures/qt.qml")),
        };
        let file = root.join(name);
        fs::write(&file, source).context(format!("write {}", file.display()))?;
        let (program, args): (&str, Vec<OsString>) = match toolkit {
            Toolkit::Gtk4 => (
                "python3",
                vec!["-I".into(), file.into(), root.into(), "a11y".into()],
            ),
            Toolkit::Gtk3 => ("python3", vec!["-I".into(), file.into(), root.into()]),
            Toolkit::Qt => (
                "env",
                vec![
                    "QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1".into(),
                    "QT_FORCE_STDERR_LOGGING=1".into(),
                    "QML_XHR_ALLOW_FILE_WRITE=1".into(),
                    "qml6".into(),
                    file.into(),
                    "--".into(),
                    root.into(),
                ],
            ),
        };
        let log = session.artifact(&format!("fixture-{toolkit:?}.log").to_lowercase());
        let process = session.start(program, &args, log, FIXTURE_DEADLINE)?;
        Ok(Self { toolkit, process })
    }

    /// A GTK 4 window that isn't a fixture of the checks, to take focus from them.
    pub(crate) fn start_decoy(session: &Session<'_>) -> Result<Self> {
        let root = session.test_dir().root();
        let file = root.join("gtk.py");
        fs::write(&file, include_str!("../fixtures/gtk.py"))
            .context(format!("write {}", file.display()))?;
        let args = vec!["-I".into(), file.into(), root.into(), "button".into()];
        let log = session.artifact("fixture-decoy.log");
        let process = session.start("python3", &args, log, FIXTURE_DEADLINE)?;
        Ok(Self {
            toolkit: Toolkit::Gtk4,
            process,
        })
    }

    pub(crate) const fn pid(&self) -> i32 {
        self.process.pid()
    }

    /// Waits for the decoy's window, whatever its size.
    pub(crate) fn decoy_window(&self, session: &mut Session<'_>) -> Result<u64> {
        let pid = self.pid();
        let window = session.wait_until("a11y-decoy", "the decoy window", STARTUP, |session| {
            Ok(windows(session)?
                .into_iter()
                .find(|window| window.pid == Some(pid)))
        })?;
        Ok(window.id)
    }

    /// Waits for the fixture's 400x300 window and for the fixture on the bus.
    pub(crate) fn window(&self, session: &mut Session<'_>, bus: &Bus) -> Result<Window> {
        let pid = self.pid();
        let window = session.wait_until(
            "a11y-window",
            &format!("the {:?} fixture's window", self.toolkit),
            STARTUP,
            |session| {
                Ok(windows(session)?.into_iter().find(|window| {
                    window.pid == Some(pid)
                        && window.layout.window_size == (400, 300)
                        && window.layout.tile_pos_in_workspace_view.is_some()
                }))
            },
        )?;
        let what = format!("the {:?} fixture on the bus", self.toolkit);
        session.wait_until("a11y-registered", &what, STARTUP, |session| {
            Ok(bus.registered(session, pid)?.then_some(()))
        })?;
        Ok(window)
    }

    pub(crate) fn stop(self) -> Result<()> {
        self.process.stop().map(drop)
    }
}

fn windows(session: &mut Session<'_>) -> Result<Vec<Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}

/// niri's window `id`, while it has one.
fn window(session: &mut Session<'_>, id: u64) -> Result<Option<Window>> {
    Ok(windows(session)?.into_iter().find(|window| window.id == id))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_busctl_replies() {
        let address = json!({"type": "s", "data": ["unix:path=/t/run/at-spi/bus,guid=1"]});
        assert_eq!(
            string_reply(&address).unwrap(),
            "unix:path=/t/run/at-spi/bus,guid=1"
        );
        assert_eq!(
            field_u64(&json!({"type": "u", "data": [4711]})).unwrap(),
            4711
        );
        let apps = json!({"type": "a(so)", "data": [[
            [":1.3", "/org/a11y/atspi/accessible/root"],
            [":1.7", "/org/a11y/atspi/accessible/root"]
        ]]});
        assert_eq!(children(&apps).unwrap(), [":1.3", ":1.7"]);
        assert_eq!(
            children(&json!({"type": "a(so)", "data": [[]]})).unwrap(),
            Vec::<String>::new()
        );
        assert!(string_reply(&json!({"type": "s", "data": [3]})).is_err());
        assert!(field_u64(&json!({})).is_err());
        assert!(children(&json!({"type": "a(so)", "data": [[[7, "/"]]]})).is_err());
    }

    #[test]
    fn reads_the_parent_after_a_command_with_spaces_and_parentheses() {
        let stat = "4711 (qml (x) 6) S 4700 4711 4711 0 -1 4194560";
        assert_eq!(parent_of(stat), Some(4700));
        assert_eq!(parent_of("1 (init) S 0 1 1"), Some(0));
        assert_eq!(parent_of("garbage"), None);
    }

    #[test]
    fn a_process_descends_from_itself_and_its_ancestors_only() {
        let me = i32::try_from(std::process::id()).unwrap();
        let parent = rustix::process::getppid().unwrap().as_raw_nonzero().get();
        assert!(descends_from(me, me).unwrap());
        assert!(descends_from(me, parent).unwrap());
        assert!(!descends_from(parent, me).unwrap());
    }
}

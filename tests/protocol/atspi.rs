//! A private D-Bus daemon with no service directories, serving as both the session bus and
//! the accessibility bus, and a mock application on it that answers AT-SPI the way a
//! toolkit does. The test changes the mock's objects, and holds, delays or fails a method's
//! next reply, to check the element tools against an application without any desktop's
//! accessibility bus. Every name and text is synthetic.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, oneshot};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, fdo, interface};

use crate::client::WAIT;
use crate::fixture::Fixture;

pub(crate) const ROOT: &str = "/org/a11y/atspi/accessible/root";
pub(crate) const FRAME: u32 = 23;
pub(crate) const PANEL: u32 = 39;
pub(crate) const PASSWORD_TEXT: u32 = 40;
pub(crate) const BUTTON: u32 = 43;
pub(crate) const TEXT: u32 = 61;
const APPLICATION: u32 = 75;
/// `ATSPI_COORD_TYPE_WINDOW`, the only coordinates the server asks for.
const WINDOW_COORDS: u32 = 1;
/// `enabled`, `sensitive`, `showing` and `visible`.
pub(crate) const SHOWING: u32 = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30);
pub(crate) const EDITABLE: u32 = 1 << 7;

/// One accessible object.
#[derive(Debug, Clone, Default)]
pub(crate) struct Object {
    pub(crate) role: u32,
    pub(crate) name: String,
    pub(crate) states: u32,
    pub(crate) children: Vec<String>,
    /// With a Component interface: its WINDOW extents.
    pub(crate) extents: Option<(i32, i32, i32, i32)>,
    /// With an Action interface: its actions' names.
    pub(crate) actions: Vec<String>,
    /// With `Text` and `EditableText` interfaces: how many characters it holds.
    pub(crate) text: Option<i32>,
}

impl Object {
    pub(crate) fn frame(name: &str, size: (i32, i32), children: &[&str]) -> Self {
        Self {
            role: FRAME,
            name: name.to_owned(),
            states: SHOWING,
            children: children.iter().map(|&child| child.to_owned()).collect(),
            extents: Some((0, 0, size.0, size.1)),
            ..Self::default()
        }
    }

    pub(crate) fn panel(children: &[&str]) -> Self {
        Self {
            role: PANEL,
            states: SHOWING,
            children: children.iter().map(|&child| child.to_owned()).collect(),
            extents: Some((0, 0, 100, 100)),
            ..Self::default()
        }
    }

    pub(crate) fn button(name: &str, actions: &[&str]) -> Self {
        Self {
            role: BUTTON,
            name: name.to_owned(),
            states: SHOWING,
            extents: Some((10, 10, 80, 30)),
            actions: actions.iter().map(|&action| action.to_owned()).collect(),
            ..Self::default()
        }
    }

    pub(crate) fn entry(role: u32, name: &str) -> Self {
        Self {
            role,
            name: name.to_owned(),
            states: SHOWING | EDITABLE,
            extents: Some((10, 50, 200, 30)),
            actions: vec!["activate".to_owned()],
            text: Some(0),
            ..Self::default()
        }
    }

    fn interfaces(&self) -> Vec<String> {
        let mut interfaces = vec!["org.a11y.atspi.Accessible"];
        if self.extents.is_some() {
            interfaces.push("org.a11y.atspi.Component");
        }
        if !self.actions.is_empty() {
            interfaces.push("org.a11y.atspi.Action");
        }
        if self.text.is_some() {
            interfaces.extend(["org.a11y.atspi.Text", "org.a11y.atspi.EditableText"]);
        }
        interfaces.into_iter().map(str::to_owned).collect()
    }
}

/// What a method's next call gets instead of its answer.
#[derive(Debug)]
enum Next {
    /// Its reply waits until the test releases it.
    Hold(Arc<Notify>, oneshot::Receiver<()>),
    /// `org.freedesktop.DBus.Error.Failed` with this message.
    Fail(String),
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<String, Object>,
    /// The application's unique name on the bus.
    app: String,
    next: BTreeMap<&'static str, Next>,
    calls: BTreeMap<&'static str, usize>,
}

/// The mock application, which the test changes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Mock(Arc<Mutex<State>>);

/// A call held until the test releases it.
#[derive(Debug)]
pub(crate) struct Held {
    arrived: Arc<Notify>,
    release: oneshot::Sender<()>,
}

impl Held {
    /// Waits, within `WAIT`, until the call arrives.
    pub(crate) async fn arrived(&self) {
        tokio::time::timeout(WAIT, self.arrived.notified())
            .await
            .expect("the held call never came");
    }

    pub(crate) fn release(self) {
        self.release.send(()).ok();
    }
}

impl Mock {
    /// Holds the next call of `member` until the returned handle releases it.
    pub(crate) fn hold(&self, member: &'static str) -> Held {
        let arrived = Arc::new(Notify::new());
        let (release, released) = oneshot::channel();
        self.set_next(member, Next::Hold(Arc::clone(&arrived), released));
        Held { arrived, release }
    }

    /// Fails the next call of `member` with `message`.
    pub(crate) fn fail(&self, member: &'static str, message: &str) {
        self.set_next(member, Next::Fail(message.to_owned()));
    }

    /// How many calls of `member` arrived.
    pub(crate) fn calls(&self, member: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .calls
            .get(member)
            .copied()
            .unwrap_or(0)
    }

    /// Changes the object at `path`.
    pub(crate) fn change(&self, path: &str, change: impl FnOnce(&mut Object)) {
        change(self.0.lock().unwrap().objects.get_mut(path).unwrap());
    }

    /// The object at `path` as it is now.
    pub(crate) fn object(&self, path: &str) -> Object {
        self.0.lock().unwrap().objects[path].clone()
    }

    fn set_next(&self, member: &'static str, next: Next) {
        self.0.lock().unwrap().next.insert(member, next);
    }

    /// Counts a call of `member` and applies what the test set for it.
    async fn called(&self, member: &'static str) -> fdo::Result<()> {
        let next = {
            let mut state = self.0.lock().unwrap();
            *state.calls.entry(member).or_default() += 1;
            state.next.remove(member)
        };
        match next {
            None => {}
            Some(Next::Hold(arrived, released)) => {
                arrived.notify_one();
                released.await.ok();
            }
            Some(Next::Fail(message)) => return Err(fdo::Error::Failed(message)),
        }
        Ok(())
    }

    fn read<T>(&self, path: &str, read: impl FnOnce(&Object, &str) -> T) -> fdo::Result<T> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .objects
            .get(path)
            .map(|object| read(object, &state.app))
            .ok_or_else(|| fdo::Error::UnknownObject(format!("no object at {path}")))
    }
}

/// The daemon and the mock's connections. Dropping it kills the daemon.
#[derive(Debug)]
pub(crate) struct Bus {
    _daemon: Child,
    _registry: Connection,
    _app: Connection,
}

/// Starts a daemon in the fixture's directory, makes it the server's session bus, and puts
/// the mock application on it with `objects` under its root, whose children are `frames`.
pub(crate) async fn start(
    fixture: &mut Fixture,
    frames: &[&str],
    objects: Vec<(&str, Object)>,
) -> (Bus, Mock) {
    let (daemon, address) = daemon(fixture).await;
    fixture.set("DBUS_SESSION_BUS_ADDRESS", &address);
    let mock = Mock::default();
    let app = connect(&address).await;
    let unique = app.unique_name().unwrap().to_string();
    let root = Object {
        role: APPLICATION,
        name: "mock".to_owned(),
        children: frames.iter().map(|&frame| frame.to_owned()).collect(),
        ..Object::default()
    };
    {
        let mut state = mock.0.lock().unwrap();
        state.app.clone_from(&unique);
        state.objects.insert(ROOT.to_owned(), root);
        for (path, object) in objects {
            state.objects.insert(path.to_owned(), object);
        }
    }
    let paths: Vec<(String, Object)> = mock.0.lock().unwrap().objects.clone().into_iter().collect();
    for (path, object) in paths {
        serve(&app, &mock, &path, &object).await;
    }
    let registry = connect(&address).await;
    let server = registry.object_server();
    let apps = vec![(unique, OwnedObjectPath::try_from(ROOT).unwrap())];
    server.at(ROOT, Registry { apps }).await.unwrap();
    server
        .at("/org/a11y/bus", A11yBus { address })
        .await
        .unwrap();
    registry
        .request_name("org.a11y.atspi.Registry")
        .await
        .unwrap();
    registry.request_name("org.a11y.Bus").await.unwrap();
    let bus = Bus {
        _daemon: daemon,
        _registry: registry,
        _app: app,
    };
    (bus, mock)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test starts its own D-Bus daemon, outside the server's runner"
)]
async fn daemon(fixture: &Fixture) -> (Child, String) {
    let config = fixture.path("bus.conf");
    let config_text = format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        fixture.path("bus").display()
    );
    std::fs::write(&config, config_text).unwrap();
    let mut daemon = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(daemon.stdout.take().unwrap()).lines();
    let address = tokio::time::timeout(WAIT, stdout.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    (daemon, address)
}

async fn connect(address: &str) -> Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap()
}

/// Serves `object`'s interfaces at `path`.
async fn serve(app: &Connection, mock: &Mock, path: &str, object: &Object) {
    let server = app.object_server();
    let at = || Node {
        path: path.to_owned(),
        mock: mock.clone(),
    };
    server.at(path, Accessible(at())).await.unwrap();
    if object.extents.is_some() {
        server.at(path, Component(at())).await.unwrap();
    }
    if !object.actions.is_empty() {
        server.at(path, Action(at())).await.unwrap();
    }
    if object.text.is_some() {
        server.at(path, Text(at())).await.unwrap();
        server.at(path, EditableText(at())).await.unwrap();
    }
}

/// An object's place in the mock.
#[derive(Debug)]
struct Node {
    path: String,
    mock: Mock,
}

impl Node {
    fn read<T>(&self, read: impl FnOnce(&Object, &str) -> T) -> fdo::Result<T> {
        self.mock.read(&self.path, read)
    }
}

fn reference(app: &str, path: &str) -> (String, OwnedObjectPath) {
    (app.to_owned(), OwnedObjectPath::try_from(path).unwrap())
}

struct Accessible(Node);

#[interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_role(&self) -> fdo::Result<u32> {
        self.0.mock.called("GetRole").await?;
        self.0.read(|object, _| object.role)
    }

    async fn get_state(&self) -> fdo::Result<Vec<u32>> {
        self.0.mock.called("GetState").await?;
        self.0.read(|object, _| vec![object.states, 0])
    }

    async fn get_children(&self) -> fdo::Result<Vec<(String, OwnedObjectPath)>> {
        self.0.mock.called("GetChildren").await?;
        self.0.read(|object, app| {
            object
                .children
                .iter()
                .map(|child| reference(app, child))
                .collect()
        })
    }

    async fn get_child_at_index(&self, index: i32) -> fdo::Result<(String, OwnedObjectPath)> {
        self.0.mock.called("GetChildAtIndex").await?;
        self.0.read(|object, app| {
            let child = usize::try_from(index)
                .ok()
                .and_then(|index| object.children.get(index));
            child.map_or_else(
                || reference("", "/org/a11y/atspi/null"),
                |child| reference(app, child),
            )
        })
    }

    async fn get_interfaces(&self) -> fdo::Result<Vec<String>> {
        self.0.mock.called("GetInterfaces").await?;
        self.0.read(|object, _| object.interfaces())
    }

    #[zbus(property, name = "Name")]
    fn accessible_name(&self) -> fdo::Result<String> {
        self.0.read(|object, _| object.name.clone())
    }
}

struct Component(Node);

#[interface(name = "org.a11y.atspi.Component")]
impl Component {
    async fn get_extents(&self, coord_type: u32) -> fdo::Result<(i32, i32, i32, i32)> {
        self.0.mock.called("GetExtents").await?;
        if coord_type != WINDOW_COORDS {
            return Err(fdo::Error::InvalidArgs(
                "only WINDOW coordinates".to_owned(),
            ));
        }
        self.0.read(|object, _| object.extents.unwrap_or_default())
    }
}

struct Action(Node);

#[interface(name = "org.a11y.atspi.Action")]
impl Action {
    async fn get_actions(&self) -> fdo::Result<Vec<(String, String, String)>> {
        self.0.mock.called("GetActions").await?;
        self.0.read(|object, _| {
            object
                .actions
                .iter()
                .map(|action| (action.clone(), String::new(), String::new()))
                .collect()
        })
    }

    async fn do_action(&self, index: i32) -> fdo::Result<bool> {
        self.0.mock.called("DoAction").await?;
        self.0.read(|object, _| {
            usize::try_from(index).is_ok_and(|index| index < object.actions.len())
        })
    }
}

struct Text(Node);

#[interface(name = "org.a11y.atspi.Text")]
impl Text {
    #[zbus(property)]
    async fn character_count(&self) -> fdo::Result<i32> {
        self.0.mock.called("CharacterCount").await?;
        self.0.read(|object, _| object.text.unwrap_or_default())
    }
}

struct EditableText(Node);

#[interface(name = "org.a11y.atspi.EditableText")]
impl EditableText {
    async fn set_text_contents(&self, text: String) -> fdo::Result<bool> {
        self.0.mock.called("SetTextContents").await?;
        let characters = i32::try_from(text.chars().count()).unwrap();
        self.0
            .mock
            .change(&self.0.path, |object| object.text = Some(characters));
        Ok(true)
    }
}

/// The registry, which lists the applications on the bus.
struct Registry {
    apps: Vec<(String, OwnedObjectPath)>,
}

#[interface(name = "org.a11y.atspi.Accessible")]
impl Registry {
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.apps.clone()
    }
}

/// The session bus's pointer to the accessibility bus: this same bus.
struct A11yBus {
    address: String,
}

#[interface(name = "org.a11y.Bus")]
impl A11yBus {
    fn get_address(&self) -> String {
        self.address.clone()
    }
}

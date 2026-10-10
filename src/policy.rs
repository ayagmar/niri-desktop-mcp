//! The policy file, `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, and the decisions it
//! feeds: launch presets, the app deny list, where screenshots may be saved, whether a
//! server may take the lease, which niri actions need `unrestricted = true`, and which output
//! setups the pointer tools may run on. Also the Noctalia panels the shell tools may open,
//! which no file changes.
//! Everything here is pure; the caller reads the file. The preset rules catch common
//! mistakes. They are a guardrail, not a boundary: a wrapper script or a symlink with
//! another name gets past any list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use niri_ipc::{Action, Output, Transform};
use serde::{Deserialize, Serialize};

use crate::control::LockState;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::events::StreamState;
use crate::niri::version::Compat;

/// Programs that run whatever command they are given, so no preset may start them.
const COMMAND_RUNNERS: [&str; 35] = [
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "tcsh",
    "csh",
    "nu",
    "elvish",
    "xonsh",
    "env",
    "sudo",
    "doas",
    "pkexec",
    "su",
    "run0",
    "setsid",
    "nohup",
    "systemd-run",
    "timeout",
    "xargs",
    "nice",
    "python",
    "python3",
    "perl",
    "ruby",
    "node",
    "lua",
    "busybox",
    "toybox",
    "uwsm",
    "distrobox",
    "toolbox",
];

/// Terminals run their trailing arguments as a command, so a preset may start one only
/// without arguments.
const TERMINALS: [&str; 17] = [
    "foot",
    "footclient",
    "alacritty",
    "kitty",
    "wezterm",
    "ghostty",
    "gnome-terminal",
    "kgx",
    "konsole",
    "xterm",
    "uxterm",
    "urxvt",
    "st",
    "terminator",
    "tilix",
    "xfce4-terminal",
    "rio",
];

/// The parsed file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    /// Input tools refuse while the focused window has one of these `app_id`s.
    #[serde(default)]
    pub(crate) deny_input_app_ids: Vec<String>,
    #[serde(default, rename = "preset")]
    pub(crate) presets: Vec<Preset>,
    /// Where `screenshot` may save files: an absolute path or one under `~/`. Without it,
    /// nothing is saved.
    #[serde(default)]
    pub(crate) capture_dir: Option<String>,
    /// Lets `niri_action` send the gated actions, such as `Spawn`. Off unless the user
    /// turns it on.
    #[serde(default)]
    pub(crate) unrestricted: bool,
    /// Serves this client through the instance's shared engine.
    #[serde(default)]
    pub(crate) shared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preset {
    /// What `launch` takes.
    pub(crate) name: String,
    /// Fixed; without `unrestricted`, no shells, interpreters or terminals with arguments.
    pub(crate) argv: Vec<String>,
    /// For observing the launched window and for `reuse`.
    pub(crate) app_id: String,
    /// Variables added to the app's environment; only with `unrestricted`.
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

impl Preset {
    /// What niri spawns. niri's `Spawn` takes no environment, so variables go through
    /// `env`, which niri finds on its `PATH`.
    pub(crate) fn command(&self) -> Vec<String> {
        if self.env.is_empty() {
            return self.argv.clone();
        }
        let assignments = self
            .env
            .iter()
            .map(|(name, value)| format!("{name}={value}"));
        ["env".to_owned(), "--".to_owned()]
            .into_iter()
            .chain(assignments)
            .chain(self.argv.iter().cloned())
            .collect()
    }
}

/// The file's state, as loaded once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Loaded {
    /// No file: no presets and an empty deny list, which is valid.
    Missing,
    Valid(Policy),
    /// Action tools refuse with `read_only` until the file is fixed and the server
    /// restarted.
    Invalid(String),
}

/// What `status` reports about the policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PolicyStatus {
    state: &'static str,
    presets: usize,
    /// What `launch` takes.
    preset_names: Vec<String>,
    denied_app_ids: usize,
    /// Where `screenshot`'s `save_path` writes, as the file says it; null when saving is off.
    capture_dir: Option<String>,
    error: Option<String>,
}

/// What reading the policy file gave, before it is checked: what a session brings. A
/// bridge sends it to the engine in its hello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is set, so there is no directory to look in.
    NoConfigDir,
    Missing,
    Unreadable {
        path: PathBuf,
        error: String,
    },
    Text {
        path: PathBuf,
        text: String,
    },
}

impl Source {
    /// Reads `<config_dir>/niri-computer-use/policy.toml` now.
    pub(crate) fn read(config_dir: Option<&Path>) -> Self {
        let Some(dir) = config_dir else {
            return Self::NoConfigDir;
        };
        let path = dir.join("niri-computer-use").join("policy.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::Text { path, text },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(error) => Self::Unreadable {
                path,
                error: error.to_string(),
            },
        }
    }
}

impl Loaded {
    /// Checks the file `source` read. No directory to look in counts as invalid, because
    /// a file might exist. `unrestricted_env` is the session's variable turning
    /// `unrestricted` on, which lifts the preset rules as the file's own key does.
    pub(crate) fn from_source(source: &Source, unrestricted_env: bool) -> Self {
        match source {
            Source::NoConfigDir => {
                Self::Invalid("neither XDG_CONFIG_HOME nor HOME is set".to_owned())
            }
            Source::Missing => Self::Missing,
            Source::Unreadable { path, error } => {
                Self::Invalid(format!("read {}: {error}", path.display()))
            }
            Source::Text { path, text } => parse(text, unrestricted_env).map_or_else(
                |error| Self::Invalid(format!("{}: {error}", path.display())),
                Self::Valid,
            ),
        }
    }

    /// The preset `launch` names, or `unknown_preset`.
    pub(crate) fn preset(&self, name: &str) -> Result<&Preset, ToolError> {
        let presets = match self {
            Self::Valid(policy) => policy.presets.as_slice(),
            Self::Missing | Self::Invalid(_) => &[],
        };
        presets
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| {
                let names: Vec<&str> = presets.iter().map(|preset| preset.name.as_str()).collect();
                ToolError::new(
                    ErrorName::UnknownPreset,
                    format!("no preset named {name:?}; the policy file has {names:?}"),
                )
            })
    }

    /// Whether the file turns `unrestricted` on. A missing or invalid file doesn't.
    pub(crate) const fn unrestricted(&self) -> bool {
        match self {
            Self::Valid(policy) => policy.unrestricted,
            Self::Missing | Self::Invalid(_) => false,
        }
    }

    pub(crate) fn status(&self) -> PolicyStatus {
        let (state, policy, error) = match self {
            Self::Missing => ("missing", None, None),
            Self::Valid(policy) => ("loaded", Some(policy), None),
            Self::Invalid(error) => ("invalid", None, Some(error.clone())),
        };
        PolicyStatus {
            state,
            presets: policy.map_or(0, |policy| policy.presets.len()),
            preset_names: policy.map_or_else(Vec::new, |policy| {
                policy
                    .presets
                    .iter()
                    .map(|preset| preset.name.clone())
                    .collect()
            }),
            denied_app_ids: policy.map_or(0, |policy| policy.deny_input_app_ids.len()),
            capture_dir: policy.and_then(|policy| policy.capture_dir.clone()),
            error,
        }
    }

    /// Where `screenshot` saves `save_path`: `save_not_enabled` without `capture_dir`, an
    /// argument mistake for a path that breaks the rules.
    pub(crate) fn save_target(
        &self,
        home: Option<&Path>,
        save_path: &str,
    ) -> Result<SaveTarget, CallError> {
        let not_enabled = |detail: &str| ToolError::new(ErrorName::SaveNotEnabled, detail);
        let Self::Valid(Policy {
            capture_dir: Some(dir),
            ..
        }) = self
        else {
            return Err(not_enabled(
                "saving screenshots is off: the policy file has no capture_dir",
            )
            .into());
        };
        let dir = match dir.strip_prefix("~/") {
            Some(rest) => home
                .ok_or_else(|| not_enabled("capture_dir starts with ~/ but HOME is not set"))?
                .join(rest),
            None => PathBuf::from(dir),
        };
        let path = SavePath::parse(save_path).map_err(CallError::InvalidArguments)?;
        Ok(SaveTarget { dir, path })
    }
}

/// Where a screenshot is saved: the capture directory, and a path checked to stay in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveTarget {
    pub(crate) dir: PathBuf,
    pub(crate) path: SavePath,
}

/// A `save_path` that follows the rules: relative, made only of plain names, naming a
/// `.png` file. Symlinks are for the writer to refuse, since only the filesystem knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavePath {
    /// The subdirectories, outermost first, which must already exist.
    pub(crate) dirs: Vec<String>,
    pub(crate) file: String,
}

impl SavePath {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        if text.starts_with('/') {
            return Err(format!(
                "save_path {text:?} must be relative to capture_dir"
            ));
        }
        let mut names: Vec<String> = Vec::new();
        for name in text.split('/') {
            if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
                return Err(format!(
                    "save_path {text:?} must be plain names joined by /, without ., .. or empty parts"
                ));
            }
            names.push(name.to_owned());
        }
        let file = names.pop().unwrap_or_default();
        let png = Path::new(&file)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if !png {
            return Err(format!("save_path {text:?} must name a .png file"));
        }
        Ok(Self { dirs: names, file })
    }
}

/// Parses the file and checks every preset, by the looser rules when the file or the
/// environment (`unrestricted_env`) turns `unrestricted` on.
pub(crate) fn parse(text: &str, unrestricted_env: bool) -> Result<Policy, String> {
    let policy: Policy = toml::from_str(text).map_err(|error| error.to_string())?;
    if let Some(dir) = &policy.capture_dir
        && !Path::new(dir).is_absolute()
        && dir.strip_prefix("~/").is_none_or(str::is_empty)
    {
        return Err(format!(
            "capture_dir {dir:?} must be an absolute path or start with ~/"
        ));
    }
    let mut names = BTreeSet::new();
    let unrestricted = policy.unrestricted || unrestricted_env;
    for preset in &policy.presets {
        check(preset, unrestricted)?;
        if !names.insert(preset.name.as_str()) {
            return Err(format!("two presets are named {:?}", preset.name));
        }
    }
    Ok(policy)
}

fn check(preset: &Preset, unrestricted: bool) -> Result<(), String> {
    let name = &preset.name;
    if name.is_empty() || preset.app_id.is_empty() {
        return Err("a preset needs a non-empty name and app_id".to_owned());
    }
    let Some(program) = preset.argv.first().filter(|program| !program.is_empty()) else {
        return Err(format!("preset {name:?} has an empty argv"));
    };
    if unrestricted {
        return check_env(preset, program);
    }
    if !preset.env.is_empty() {
        return Err(format!(
            "preset {name:?} has env, which needs unrestricted = true"
        ));
    }
    let base = Path::new(program)
        .file_name()
        .map_or_else(String::new, |base| base.to_string_lossy().into_owned());
    // `python3.13` is `python3`, and `perl5.40` is `perl`.
    let unversioned = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if COMMAND_RUNNERS.contains(&base.as_str()) || COMMAND_RUNNERS.contains(&unversioned) {
        return Err(format!(
            "preset {name:?} starts {base}, which runs any command it is given"
        ));
    }
    if base == "flatpak" && preset.argv.iter().any(|arg| arg.starts_with("--command")) {
        return Err(format!(
            "preset {name:?} starts flatpak with --command, which runs any command"
        ));
    }
    if TERMINALS.contains(&base.as_str()) && preset.argv.len() > 1 {
        return Err(format!(
            "preset {name:?} starts the terminal {base} with arguments, which it can run as a command"
        ));
    }
    Ok(())
}

/// With `env`, the program goes after the assignments `env` reads, so it can't look like
/// one, and each name must be one `env` can set.
fn check_env(preset: &Preset, program: &str) -> Result<(), String> {
    let name = &preset.name;
    if preset.env.is_empty() {
        return Ok(());
    }
    if program.contains('=') {
        return Err(format!(
            "preset {name:?} has env, so its program can't contain ="
        ));
    }
    for (variable, value) in &preset.env {
        if variable.is_empty() || variable.contains(['=', '\0']) || value.contains('\0') {
            return Err(format!(
                "preset {name:?} has env variable {variable:?}, which env can't set"
            ));
        }
    }
    Ok(())
}

/// Whether `unrestricted` is on, and which source turned it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unrestricted {
    /// The policy file's key.
    pub(crate) policy: bool,
    /// `NIRI_COMPUTER_USE_UNRESTRICTED`: on for `1`, off when unset or empty, and an error for
    /// anything else, which leaves it off.
    pub(crate) env: Result<bool, String>,
}

/// Whether `serve` bridges its client to the shared engine: the policy file's `shared`, or
/// `NIRI_COMPUTER_USE_SHARED` set to `1`, given as `env`. Any other value of the variable
/// leaves it as the file says, and is the error when that is off.
pub(crate) fn shared(policy: &Loaded, env: Option<&std::ffi::OsStr>) -> Result<bool, String> {
    let from_file = matches!(policy, Loaded::Valid(policy) if policy.shared);
    match env {
        Some(value) if value == "1" => Ok(true),
        Some(value) if !value.is_empty() && !from_file => Err(format!(
            "NIRI_COMPUTER_USE_SHARED is \"{}\"; only 1 turns shared mode on",
            value.display()
        )),
        _ => Ok(from_file),
    }
}

/// What `status` reports about `unrestricted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UnrestrictedStatus {
    enabled: bool,
    /// `policy`, `env` or `both`; null while off.
    source: Option<&'static str>,
    /// Why the variable was ignored.
    error: Option<String>,
}

impl Unrestricted {
    /// The variable's value, read as `Unrestricted::env` says.
    pub(crate) fn parse_env(value: Option<&std::ffi::OsStr>) -> Result<bool, String> {
        match value {
            None => Ok(false),
            Some(value) if value.is_empty() => Ok(false),
            Some(value) if value == "1" => Ok(true),
            Some(value) => Err(format!(
                "NIRI_COMPUTER_USE_UNRESTRICTED is \"{}\"; only 1 turns unrestricted on, so it stays off",
                value.display()
            )),
        }
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.policy || matches!(self.env, Ok(true))
    }

    pub(crate) fn status(&self) -> UnrestrictedStatus {
        let source = match (self.policy, matches!(self.env, Ok(true))) {
            (true, true) => Some("both"),
            (true, false) => Some("policy"),
            (false, true) => Some("env"),
            (false, false) => None,
        };
        UnrestrictedStatus {
            enabled: self.enabled(),
            source,
            error: self.env.clone().err(),
        }
    }
}

/// What the control decision looks at, gathered when `acquire_desktop` or an action tool
/// is called.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts<'a> {
    /// The version rule's answer; none when niri's version couldn't be read.
    pub(crate) compat: Option<Compat>,
    /// Why niri's version couldn't be read.
    pub(crate) niri_error: Option<&'a ToolError>,
    pub(crate) event_stream: Option<StreamState>,
    pub(crate) policy: &'a Loaded,
    pub(crate) lock: LockState,
}

/// Why a server may not take the lease or act now, checked after the stop flag, the
/// input-dirty marker and, for actions, the lease: niri unreachable, then read-only (niri's version, its event schema,
/// or the policy file), then a screen that is locked or whose lock state is unknown. Input
/// only ever goes to a screen known to be unlocked.
pub(crate) fn refuse_control(facts: Facts<'_>) -> Option<ToolError> {
    let read_only = |reason: String| Some(ToolError::new(ErrorName::ReadOnly, reason));
    if let Some(error) = facts.niri_error {
        return Some(error.clone());
    }
    match facts.compat {
        None => {
            return Some(ToolError::new(
                ErrorName::NiriUnavailable,
                "niri's version is unknown",
            ));
        }
        Some(Compat::ReadOnly) => {
            return read_only("this build doesn't support the running niri version".to_owned());
        }
        Some(Compat::Ok | Compat::PatchWarning) => {}
    }
    if facts.event_stream == Some(StreamState::SchemaIncompatible) {
        return read_only("niri sent events this build can't parse".to_owned());
    }
    if let Loaded::Invalid(error) = facts.policy {
        return read_only(format!("the policy file is invalid: {error}"));
    }
    match facts.lock {
        LockState::Unlocked => None,
        LockState::Locked => Some(ToolError::new(
            ErrorName::ScreenLocked,
            "the screen is locked",
        )),
        LockState::Unknown => Some(ToolError::new(
            ErrorName::ScreenLocked,
            "the lock state is unknown: neither logind nor Noctalia answered (see status.lock)",
        )),
    }
}

/// `app_denied` when the window with keyboard focus belongs to an app the policy file
/// denies input to (plan §9). Its `app_id` is the client's own claim, and a click can land
/// on another window, so this is a guardrail, not a boundary.
pub(crate) fn refuse_input(policy: &Loaded, focused_app_id: Option<&str>) -> Option<ToolError> {
    denied(policy, focused_app_id?, "the focused window")
}

/// `app_denied` when `window`, such as the one that owns an accessible element or the one
/// being closed, belongs to an app on the deny list, whatever has focus.
pub(crate) fn refuse_window(
    policy: &Loaded,
    window: u64,
    app_id: Option<&str>,
) -> Option<ToolError> {
    denied(policy, app_id?, &format!("window {window}"))
}

/// `secret_field` for an element the toolkit marks as a password field, which the element
/// actions never act on, since activating one can submit it: its role is `password_text`.
/// That is the one mark GTK 4 (`GtkPasswordEntry`, or an entry whose input purpose is a
/// password or PIN), GTK 3 (an entry that hides its text) and Qt (a field in password echo
/// mode) give one; no AT-SPI state says it. A GTK 4 entry that only hides its text has the
/// role, states, interfaces and attributes of a plain entry (M9b fixes' nested run), so it
/// can't be told apart and isn't refused.
pub(crate) fn refuse_secret_field(role: &str) -> Option<ToolError> {
    (role == "password_text").then(|| {
        ToolError::new(
            ErrorName::SecretField,
            "the element is a password field; element actions never act on one, and nothing was sent",
        )
    })
}

fn denied(policy: &Loaded, app_id: &str, whose: &str) -> Option<ToolError> {
    let Loaded::Valid(policy) = policy else {
        return None;
    };
    policy
        .deny_input_app_ids
        .iter()
        .any(|denied| denied == app_id)
        .then(|| {
            ToolError::new(
                ErrorName::AppDenied,
                format!("{whose}'s app_id {app_id:?} is on the policy's deny list"),
            )
        })
}

/// Whether `niri_action` may send an action without `unrestricted = true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionGate {
    /// Changes only niri's layout, focus or views.
    Allowed,
    /// Refused unless `unrestricted = true`, for this reason.
    Gated(&'static str),
}

/// Sorts niri 26.04's actions into the ones that only change niri's layout, focus or views
/// and the ones that run programs, write files or change state outside niri's layout.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive match over niri's actions without a wildcard arm, so an action a \
              new niri-ipc adds fails to compile instead of being allowed"
)]
pub(crate) const fn action_gate(action: &Action) -> ActionGate {
    match action {
        Action::Spawn { .. } | Action::SpawnSh { .. } => ActionGate::Gated("it runs any program"),
        Action::Quit { .. } => ActionGate::Gated("it ends the niri session"),
        Action::PowerOffMonitors { .. } | Action::PowerOnMonitors { .. } => {
            ActionGate::Gated("it powers the monitors off or on")
        }
        Action::LoadConfigFile { .. } => {
            ActionGate::Gated("it loads a niri config file, which can bind keys and start programs")
        }
        Action::Screenshot { .. }
        | Action::ScreenshotScreen { .. }
        | Action::ScreenshotWindow { .. } => ActionGate::Gated(
            "it writes a file and replaces the clipboard; the screenshot tool's save_path saves without either",
        ),
        Action::ToggleKeyboardShortcutsInhibit { .. } => ActionGate::Gated(
            "it changes whether niri's keybinds, the stop key's among them, reach niri",
        ),
        Action::SwitchLayout { .. } => {
            ActionGate::Gated("it changes the keyboard layout the user types with")
        }
        Action::SetDynamicCastWindow { .. }
        | Action::SetDynamicCastMonitor { .. }
        | Action::ClearDynamicCastTarget { .. }
        | Action::StopCast { .. } => {
            ActionGate::Gated("it changes what a screencast shows, or stops it")
        }
        Action::ToggleDebugTint { .. }
        | Action::DebugToggleOpaqueRegions { .. }
        | Action::DebugToggleDamage { .. } => {
            ActionGate::Gated("it changes niri's debug rendering, not the layout or focus")
        }
        Action::DoScreenTransition { .. } => ActionGate::Gated(
            "it freezes what the user sees on every output, and in screencasts, for up to 65 s",
        ),
        Action::CloseWindow { .. }
        | Action::FullscreenWindow { .. }
        | Action::ToggleWindowedFullscreen { .. }
        | Action::FocusWindow { .. }
        | Action::FocusWindowInColumn { .. }
        | Action::FocusWindowPrevious { .. }
        | Action::FocusColumnLeft { .. }
        | Action::FocusColumnRight { .. }
        | Action::FocusColumnFirst { .. }
        | Action::FocusColumnLast { .. }
        | Action::FocusColumnRightOrFirst { .. }
        | Action::FocusColumnLeftOrLast { .. }
        | Action::FocusColumn { .. }
        | Action::FocusWindowOrMonitorUp { .. }
        | Action::FocusWindowOrMonitorDown { .. }
        | Action::FocusColumnOrMonitorLeft { .. }
        | Action::FocusColumnOrMonitorRight { .. }
        | Action::FocusWindowDown { .. }
        | Action::FocusWindowUp { .. }
        | Action::FocusWindowDownOrColumnLeft { .. }
        | Action::FocusWindowDownOrColumnRight { .. }
        | Action::FocusWindowUpOrColumnLeft { .. }
        | Action::FocusWindowUpOrColumnRight { .. }
        | Action::FocusWindowOrWorkspaceDown { .. }
        | Action::FocusWindowOrWorkspaceUp { .. }
        | Action::FocusWindowTop { .. }
        | Action::FocusWindowBottom { .. }
        | Action::FocusWindowDownOrTop { .. }
        | Action::FocusWindowUpOrBottom { .. }
        | Action::MoveColumnLeft { .. }
        | Action::MoveColumnRight { .. }
        | Action::MoveColumnToFirst { .. }
        | Action::MoveColumnToLast { .. }
        | Action::MoveColumnLeftOrToMonitorLeft { .. }
        | Action::MoveColumnRightOrToMonitorRight { .. }
        | Action::MoveColumnToIndex { .. }
        | Action::MoveWindowDown { .. }
        | Action::MoveWindowUp { .. }
        | Action::MoveWindowDownOrToWorkspaceDown { .. }
        | Action::MoveWindowUpOrToWorkspaceUp { .. }
        | Action::ConsumeOrExpelWindowLeft { .. }
        | Action::ConsumeOrExpelWindowRight { .. }
        | Action::ConsumeWindowIntoColumn { .. }
        | Action::ExpelWindowFromColumn { .. }
        | Action::SwapWindowRight { .. }
        | Action::SwapWindowLeft { .. }
        | Action::ToggleColumnTabbedDisplay { .. }
        | Action::SetColumnDisplay { .. }
        | Action::CenterColumn { .. }
        | Action::CenterWindow { .. }
        | Action::CenterVisibleColumns { .. }
        | Action::FocusWorkspaceDown { .. }
        | Action::FocusWorkspaceUp { .. }
        | Action::FocusWorkspace { .. }
        | Action::FocusWorkspacePrevious { .. }
        | Action::MoveWindowToWorkspaceDown { .. }
        | Action::MoveWindowToWorkspaceUp { .. }
        | Action::MoveWindowToWorkspace { .. }
        | Action::MoveColumnToWorkspaceDown { .. }
        | Action::MoveColumnToWorkspaceUp { .. }
        | Action::MoveColumnToWorkspace { .. }
        | Action::MoveWorkspaceDown { .. }
        | Action::MoveWorkspaceUp { .. }
        | Action::MoveWorkspaceToIndex { .. }
        | Action::SetWorkspaceName { .. }
        | Action::UnsetWorkspaceName { .. }
        | Action::FocusMonitorLeft { .. }
        | Action::FocusMonitorRight { .. }
        | Action::FocusMonitorDown { .. }
        | Action::FocusMonitorUp { .. }
        | Action::FocusMonitorPrevious { .. }
        | Action::FocusMonitorNext { .. }
        | Action::FocusMonitor { .. }
        | Action::MoveWindowToMonitorLeft { .. }
        | Action::MoveWindowToMonitorRight { .. }
        | Action::MoveWindowToMonitorDown { .. }
        | Action::MoveWindowToMonitorUp { .. }
        | Action::MoveWindowToMonitorPrevious { .. }
        | Action::MoveWindowToMonitorNext { .. }
        | Action::MoveWindowToMonitor { .. }
        | Action::MoveColumnToMonitorLeft { .. }
        | Action::MoveColumnToMonitorRight { .. }
        | Action::MoveColumnToMonitorDown { .. }
        | Action::MoveColumnToMonitorUp { .. }
        | Action::MoveColumnToMonitorPrevious { .. }
        | Action::MoveColumnToMonitorNext { .. }
        | Action::MoveColumnToMonitor { .. }
        | Action::SetWindowWidth { .. }
        | Action::SetWindowHeight { .. }
        | Action::ResetWindowHeight { .. }
        | Action::SwitchPresetColumnWidth { .. }
        | Action::SwitchPresetColumnWidthBack { .. }
        | Action::SwitchPresetWindowWidth { .. }
        | Action::SwitchPresetWindowWidthBack { .. }
        | Action::SwitchPresetWindowHeight { .. }
        | Action::SwitchPresetWindowHeightBack { .. }
        | Action::MaximizeColumn { .. }
        | Action::MaximizeWindowToEdges { .. }
        | Action::SetColumnWidth { .. }
        | Action::ExpandColumnToAvailableWidth { .. }
        | Action::ShowHotkeyOverlay { .. }
        | Action::MoveWorkspaceToMonitorLeft { .. }
        | Action::MoveWorkspaceToMonitorRight { .. }
        | Action::MoveWorkspaceToMonitorDown { .. }
        | Action::MoveWorkspaceToMonitorUp { .. }
        | Action::MoveWorkspaceToMonitorPrevious { .. }
        | Action::MoveWorkspaceToMonitorNext { .. }
        | Action::MoveWorkspaceToMonitor { .. }
        | Action::ToggleWindowFloating { .. }
        | Action::MoveWindowToFloating { .. }
        | Action::MoveWindowToTiling { .. }
        | Action::FocusFloating { .. }
        | Action::FocusTiling { .. }
        | Action::SwitchFocusBetweenFloatingAndTiling { .. }
        | Action::MoveFloatingWindow { .. }
        | Action::ToggleWindowRuleOpacity { .. }
        | Action::ToggleOverview { .. }
        | Action::OpenOverview { .. }
        | Action::CloseOverview { .. }
        | Action::ToggleWindowUrgent { .. }
        | Action::SetWindowUrgent { .. }
        | Action::UnsetWindowUrgent { .. } => ActionGate::Allowed,
    }
}

/// `unrestricted_required` for a gated action while `unrestricted` is off.
pub(crate) fn refuse_action(action: &Action, unrestricted: bool) -> Option<ToolError> {
    let ActionGate::Gated(reason) = action_gate(action) else {
        return None;
    };
    if unrestricted {
        return None;
    }
    // niri's JSON for an action is an object with the action's name as its one key.
    let name = serde_json::to_value(action)
        .ok()
        .and_then(|json| json.as_object()?.keys().next().cloned())
        .unwrap_or_default();
    Some(ToolError::new(
        ErrorName::UnrestrictedRequired,
        format!("{name} needs unrestricted = true in the user's policy file: {reason}"),
    ))
}

/// The Noctalia panels `shell_open` and `shell_close` may name (plan §6.1). Never the
/// session menu, which powers off; the launcher, which runs whatever is typed into it;
/// polkit's authentication prompt; the clipboard history, which may hold secrets; the
/// setup wizard; or Noctalia's test panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Panel {
    ControlCenter,
    Wallpaper,
    TrayDrawer,
}

impl Panel {
    const ALL: [Self; 3] = [Self::ControlCenter, Self::Wallpaper, Self::TrayDrawer];

    /// Noctalia's id for the panel.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::ControlCenter => "control-center",
            Self::Wallpaper => "wallpaper",
            Self::TrayDrawer => "tray-drawer",
        }
    }
}

/// The allowlisted panel with Noctalia's id `name`, or `panel_not_allowed`.
pub(crate) fn panel(name: &str) -> Result<Panel, ToolError> {
    Panel::ALL
        .into_iter()
        .find(|panel| panel.id() == name)
        .ok_or_else(|| {
            let allowed: Vec<&str> = Panel::ALL.into_iter().map(Panel::id).collect();
            ToolError::new(
                ErrorName::PanelNotAllowed,
                format!(
                    "panel {name:?} isn't allowed; the shell tools take only {}",
                    allowed.join(", ")
                ),
            )
        })
}

/// Whether the pointer tools may run on these outputs (plan §8): exactly one enabled
/// output, either a monitor with transform `Normal` or nested niri's `winit` window, which
/// niri always shows `Flipped180`. Those are the setups live tests cover; anything else,
/// including a monitor really rotated to `Flipped180`, is `untested_output_config`.
pub(crate) fn pointer_support<'a>(
    outputs: impl IntoIterator<Item = &'a Output>,
) -> Result<(), ToolError> {
    let enabled: Vec<(&str, Transform)> = outputs
        .into_iter()
        .filter_map(|output| Some((output.name.as_str(), output.logical?.transform)))
        .collect();
    match enabled.as_slice() {
        [("winit", Transform::Flipped180)] => Ok(()),
        [(name, Transform::Normal)] if *name != "winit" => Ok(()),
        _ => Err(ToolError::new(
            ErrorName::UntestedOutputConfig,
            format!(
                "the pointer runs only with one enabled output, a monitor at transform Normal or nested niri's winit window; enabled outputs and transforms: {enabled:?}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
deny_input_app_ids = ["org.keepassxc.KeePassXC"]

[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"

[[preset]]
name = "terminal"
argv = ["/usr/bin/foot"]
app_id = "foot"
"#;

    #[test]
    fn shared_mode_comes_from_the_file_or_the_variable_set_to_one() {
        let file = |shared| {
            Loaded::Valid(Policy {
                shared,
                ..Policy::default()
            })
        };
        let env = |value: &'static str| Some(std::ffi::OsStr::new(value));
        assert_eq!(shared(&Loaded::Missing, None), Ok(false));
        assert_eq!(shared(&file(true), None), Ok(true));
        assert_eq!(shared(&file(false), env("1")), Ok(true));
        assert_eq!(
            shared(&Loaded::Invalid("bad".to_owned()), env("1")),
            Ok(true)
        );
        assert_eq!(shared(&file(false), env("")), Ok(false));
        assert!(
            shared(&file(false), env("yes"))
                .unwrap_err()
                .contains("\"yes\"")
        );
        assert_eq!(shared(&file(true), env("yes")), Ok(true));
        assert_eq!(
            Loaded::from_source(
                &Source::Text {
                    path: "/p.toml".into(),
                    text: "shared = true".to_owned()
                },
                false
            ),
            file(true)
        );
    }

    #[test]
    fn reads_the_plan_example() {
        let policy = parse(EXAMPLE, false).unwrap();
        assert_eq!(policy.deny_input_app_ids, ["org.keepassxc.KeePassXC"]);
        assert_eq!(policy.presets.len(), 2);
        assert_eq!(policy.presets[1].argv, ["/usr/bin/foot"]);
        assert_eq!(parse("", false).unwrap(), Policy::default());
        // An app that only looks versioned, and a flatpak app, are fine.
        let fine = "[[preset]]\nname = \"a\"\nargv = [\"gimp-2.10\"]\napp_id = \"gimp\"\n\n[[preset]]\nname = \"b\"\nargv = [\"flatpak\", \"run\", \"org.mozilla.firefox\"]\napp_id = \"firefox\"\n";
        assert_eq!(parse(fine, false).unwrap().presets.len(), 2);
    }

    #[test]
    fn refuses_presets_that_could_run_any_command() {
        let preset = |argv: &str| {
            parse(
                &format!("[[preset]]\nname = \"x\"\nargv = {argv}\napp_id = \"x\""),
                false,
            )
            .unwrap_err()
        };
        assert!(preset(r#"["bash", "-c", "rm -rf ~"]"#).contains("runs any command"));
        assert!(preset(r#"["/usr/bin/env", "firefox"]"#).contains("starts env"));
        assert!(preset(r#"["python3", "-m", "http.server"]"#).contains("python3"));
        assert!(preset(r#"["foot", "-e", "sh"]"#).contains("terminal foot"));
        assert!(preset(r#"["kitty", "sh"]"#).contains("terminal kitty"));
        assert!(preset(r#"["python3.13", "x.py"]"#).contains("python3.13"));
        assert!(preset(r#"["/usr/bin/perl5.40"]"#).contains("perl5.40"));
        assert!(preset(r#"["busybox", "sh"]"#).contains("busybox"));
        assert!(preset(r#"["uwsm", "app", "--", "firefox"]"#).contains("uwsm"));
        assert!(preset(r#"["flatpak", "run", "--command=sh", "org.x.Y"]"#).contains("--command"));
        assert!(preset("[]").contains("empty argv"));
        assert!(preset(r#"[""]"#).contains("empty argv"));
    }

    #[test]
    fn refuses_unknown_keys_duplicates_and_blank_names() {
        assert!(parse("deny_apps = []", false).is_err());
        assert!(
            parse(
                "[[preset]]\nname = \"a\"\nargv = [\"a\"]\napp_id = \"a\"\nshell = true",
                false
            )
            .is_err()
        );
        let twice = "[[preset]]\nname = \"a\"\nargv = [\"a\"]\napp_id = \"a\"\n".repeat(2);
        assert_eq!(
            parse(&twice, false).unwrap_err(),
            "two presets are named \"a\""
        );
        assert!(
            parse(
                "[[preset]]\nname = \"\"\nargv = [\"a\"]\napp_id = \"a\"",
                false
            )
            .is_err()
        );
    }

    #[test]
    fn a_missing_file_is_valid_and_others_are_reported() {
        let path = PathBuf::from("/c/policy.toml");
        let text = |text: &str| Source::Text {
            path: path.clone(),
            text: text.to_owned(),
        };
        let loaded = Loaded::from_source(&Source::Missing, false);
        assert_eq!(loaded, Loaded::Missing);
        assert_eq!(loaded.status().state, "missing");
        let status = Loaded::from_source(&text(EXAMPLE), false).status();
        assert_eq!(
            (status.state, status.presets, status.denied_app_ids),
            ("loaded", 2, 1)
        );
        assert_eq!(status.preset_names, ["firefox", "terminal"]);
        let invalid = Loaded::from_source(&text("nonsense"), false).status();
        assert_eq!(invalid.state, "invalid");
        assert!(invalid.error.unwrap().starts_with("/c/policy.toml: "));
        let unreadable = Source::Unreadable {
            path: path.clone(),
            error: "Permission denied".to_owned(),
        };
        assert!(
            matches!(Loaded::from_source(&unreadable, false), Loaded::Invalid(error) if error.starts_with("read /c/policy.toml"))
        );
        assert_eq!(
            Loaded::from_source(&Source::NoConfigDir, false)
                .status()
                .state,
            "invalid"
        );
    }

    #[test]
    fn the_file_is_read_from_the_config_directory() {
        let dir = crate::test_support::fresh_dir("policy-source");
        assert_eq!(Source::read(None), Source::NoConfigDir);
        assert_eq!(Source::read(Some(&dir)), Source::Missing);
        let path = dir.join("niri-computer-use/policy.toml");
        std::fs::create_dir(dir.join("niri-computer-use")).unwrap();
        std::fs::write(&path, "unrestricted = true").unwrap();
        assert_eq!(
            Source::read(Some(&dir)),
            Source::Text {
                path,
                text: "unrestricted = true".to_owned()
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn launch_names_a_preset_from_the_file() {
        let loaded = Loaded::Valid(parse(EXAMPLE, false).unwrap());
        assert_eq!(loaded.preset("terminal").unwrap().app_id, "foot");
        let unknown = loaded.preset("Firefox").unwrap_err();
        assert_eq!(unknown.name, ErrorName::UnknownPreset);
        assert_eq!(
            unknown.detail,
            "no preset named \"Firefox\"; the policy file has [\"firefox\", \"terminal\"]"
        );
        assert_eq!(
            Loaded::Missing.preset("firefox").unwrap_err().detail,
            "no preset named \"firefox\"; the policy file has []"
        );
    }

    #[test]
    fn saving_needs_a_capture_dir_in_the_file() {
        let home = Some(Path::new("/home/u"));
        for loaded in [
            Loaded::Missing,
            Loaded::Valid(parse(EXAMPLE, false).unwrap()),
            Loaded::Invalid("bad".to_owned()),
        ] {
            let Err(CallError::Tool(error)) = loaded.save_target(home, "a.png") else {
                panic!("saved without capture_dir");
            };
            assert_eq!(error.name, ErrorName::SaveNotEnabled);
        }
        let tilde =
            Loaded::Valid(parse("capture_dir = \"~/Pictures/agent-shots\"", false).unwrap());
        assert_eq!(
            tilde.save_target(home, "readme/one.png"),
            Ok(SaveTarget {
                dir: PathBuf::from("/home/u/Pictures/agent-shots"),
                path: SavePath {
                    dirs: vec!["readme".to_owned()],
                    file: "one.png".to_owned(),
                },
            })
        );
        assert!(matches!(
            tilde.save_target(None, "a.png"),
            Err(CallError::Tool(ToolError {
                name: ErrorName::SaveNotEnabled,
                ..
            }))
        ));
        assert!(matches!(
            tilde.save_target(home, "/etc/a.png"),
            Err(CallError::InvalidArguments(_))
        ));
        let absolute = Loaded::Valid(parse("capture_dir = \"/srv/shots\"", false).unwrap());
        assert_eq!(
            absolute.save_target(home, "a.png").unwrap().dir,
            PathBuf::from("/srv/shots")
        );
        assert_eq!(absolute.status().capture_dir.as_deref(), Some("/srv/shots"));
        for relative in ["shots", "~", "~/", "~user/shots", ""] {
            let file = format!("capture_dir = {relative:?}");
            assert!(
                parse(&file, false).unwrap_err().contains("absolute"),
                "{relative:?}"
            );
        }
    }

    #[test]
    fn a_save_path_stays_inside_the_capture_dir_and_names_a_png() {
        assert_eq!(
            SavePath::parse("shot.png"),
            Ok(SavePath {
                dirs: Vec::new(),
                file: "shot.png".to_owned()
            })
        );
        assert_eq!(SavePath::parse("a/b/c.png").unwrap().dirs, ["a", "b"]);
        assert!(SavePath::parse("Shot.PNG").is_ok());
        for refused in [
            "/tmp/x.png",
            "../x.png",
            "a/../../x.png",
            "./x.png",
            "a//x.png",
            "a/",
            "",
            ".png",
            "x.jpg",
            "x.png/",
            "x\0.png",
        ] {
            assert!(SavePath::parse(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn the_lease_is_refused_in_order() {
        let unreachable = ToolError::new(ErrorName::NiriUnavailable, "gone");
        let invalid = Loaded::Invalid("bad".to_owned());
        let ok = Facts {
            compat: Some(Compat::Ok),
            niri_error: None,
            event_stream: Some(StreamState::Connected),
            policy: &Loaded::Missing,
            lock: LockState::Unlocked,
        };
        let name = |facts| refuse_control(facts).map(|error| error.name);
        assert_eq!(name(ok), None);
        assert_eq!(
            name(Facts {
                compat: Some(Compat::PatchWarning),
                ..ok
            }),
            None
        );
        assert_eq!(
            name(Facts {
                lock: LockState::Unknown,
                ..ok
            }),
            Some(ErrorName::ScreenLocked)
        );
        assert_eq!(
            name(Facts {
                event_stream: Some(StreamState::Disconnected),
                ..ok
            }),
            None
        );
        assert_eq!(
            name(Facts {
                compat: None,
                niri_error: Some(&unreachable),
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::NiriUnavailable)
        );
        assert_eq!(
            name(Facts {
                compat: Some(Compat::ReadOnly),
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                event_stream: Some(StreamState::SchemaIncompatible),
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                policy: &invalid,
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::ScreenLocked)
        );
    }

    #[test]
    fn input_to_a_denied_app_is_refused() {
        let policy = Loaded::Valid(parse(EXAMPLE, false).unwrap());
        let refused = refuse_input(&policy, Some("org.keepassxc.KeePassXC")).unwrap();
        assert_eq!(refused.name, ErrorName::AppDenied);
        assert!(
            refused.detail.contains("org.keepassxc.KeePassXC"),
            "{}",
            refused.detail
        );
        assert_eq!(refuse_input(&policy, Some("firefox")), None);
        assert_eq!(refuse_input(&policy, None), None);
        assert_eq!(
            refuse_input(&Loaded::Missing, Some("org.keepassxc.KeePassXC")),
            None
        );
    }

    #[test]
    fn elements_of_a_denied_apps_window_are_refused_whatever_has_focus() {
        let policy = Loaded::Valid(parse(EXAMPLE, false).unwrap());
        let refused = refuse_window(&policy, 7, Some("org.keepassxc.KeePassXC")).unwrap();
        assert_eq!(refused.name, ErrorName::AppDenied);
        assert!(
            refused.detail.starts_with("window 7's app_id"),
            "{}",
            refused.detail
        );
        assert_eq!(refuse_window(&policy, 7, Some("firefox")), None);
        assert_eq!(refuse_window(&policy, 7, None), None);
    }

    #[test]
    fn a_password_field_is_never_acted_on() {
        let refused = refuse_secret_field("password_text").unwrap();
        assert_eq!(refused.name, ErrorName::SecretField);
        for role in ["text", "entry", "terminal", "document_text"] {
            assert_eq!(refuse_secret_field(role), None, "{role}");
        }
    }

    fn output(name: &str, transform: Option<Transform>) -> Output {
        serde_json::from_value(serde_json::json!({
            "name": name, "make": "", "model": "", "serial": null, "physical_size": null,
            "modes": [], "current_mode": null, "is_custom_mode": false,
            "vrr_supported": false, "vrr_enabled": false,
            "logical": transform.map(|transform| serde_json::json!({
                "x": 0, "y": 0, "width": 960, "height": 720, "scale": 1.0,
                "transform": transform
            }))
        }))
        .unwrap()
    }

    #[test]
    fn the_pointer_runs_on_one_tested_output() {
        let supported = |outputs: &[Output]| pointer_support(outputs).map_err(|error| error.name);
        let monitor = output("DP-1", Some(Transform::Normal));
        let nested = output("winit", Some(Transform::Flipped180));
        let off = output("HDMI-A-1", None);
        assert_eq!(supported(std::slice::from_ref(&monitor)), Ok(()));
        assert_eq!(supported(std::slice::from_ref(&nested)), Ok(()));
        // A disabled output doesn't count.
        assert_eq!(supported(&[monitor.clone(), off]), Ok(()));
        let untested = Err(ErrorName::UntestedOutputConfig);
        assert_eq!(supported(&[]), untested);
        assert_eq!(supported(&[monitor, nested]), untested);
        assert_eq!(
            supported(&[output("DP-1", Some(Transform::Flipped180))]),
            untested
        );
        assert_eq!(supported(&[output("DP-1", Some(Transform::_90))]), untested);
        assert_eq!(
            supported(&[output("winit", Some(Transform::Normal))]),
            untested
        );
        let error = pointer_support(&[output("eDP-1", Some(Transform::_270))]).unwrap_err();
        assert!(
            error.detail.ends_with(r#"[("eDP-1", _270)]"#),
            "{}",
            error.detail
        );
    }

    fn action(json: serde_json::Value) -> Action {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn actions_that_run_programs_or_reach_past_the_layout_are_gated() {
        use serde_json::json;
        for gated in [
            json!({"Spawn": {"command": ["foot"]}}),
            json!({"SpawnSh": {"command": "foot"}}),
            json!({"Quit": {"skip_confirmation": true}}),
            json!({"PowerOffMonitors": {}}),
            json!({"PowerOnMonitors": {}}),
            json!({"LoadConfigFile": {"path": "/tmp/other.kdl"}}),
            json!({"Screenshot": {"show_pointer": false, "path": null}}),
            json!({"ScreenshotScreen": {"write_to_disk": true, "show_pointer": false, "path": null}}),
            json!({"ScreenshotWindow": {"id": null, "write_to_disk": true, "show_pointer": false, "path": "/tmp/w.png"}}),
            json!({"ToggleKeyboardShortcutsInhibit": {}}),
            json!({"SwitchLayout": {"layout": "Next"}}),
            json!({"SetDynamicCastWindow": {"id": 3}}),
            json!({"SetDynamicCastMonitor": {"output": null}}),
            json!({"ClearDynamicCastTarget": {}}),
            json!({"StopCast": {"session_id": 1}}),
            json!({"ToggleDebugTint": {}}),
            json!({"DebugToggleOpaqueRegions": {}}),
            json!({"DebugToggleDamage": {}}),
            json!({"DoScreenTransition": {"delay_ms": 65535}}),
        ] {
            assert!(
                matches!(action_gate(&action(gated.clone())), ActionGate::Gated(_)),
                "{gated}"
            );
        }
        for allowed in [
            json!({"FullscreenWindow": {"id": 12}}),
            json!({"SetWindowWidth": {"id": 12, "change": {"SetFixed": 1600}}}),
            json!({"ToggleWindowFloating": {"id": 12}}),
            json!({"MaximizeColumn": {}}),
            json!({"CloseWindow": {"id": null}}),
            json!({"FocusWorkspace": {"reference": {"Index": 2}}}),
            json!({"SetWorkspaceName": {"name": "demo", "workspace": null}}),
            json!({"ToggleOverview": {}}),
        ] {
            assert_eq!(
                action_gate(&action(allowed.clone())),
                ActionGate::Allowed,
                "{allowed}"
            );
        }
    }

    #[test]
    fn a_gated_action_needs_unrestricted() {
        let spawn = action(serde_json::json!({"Spawn": {"command": ["foot"]}}));
        let refused = refuse_action(&spawn, false).unwrap();
        assert_eq!(refused.name, ErrorName::UnrestrictedRequired);
        assert_eq!(
            refused.detail,
            "Spawn needs unrestricted = true in the user's policy file: it runs any program"
        );
        assert_eq!(refuse_action(&spawn, true), None);
        let float = action(serde_json::json!({"ToggleWindowFloating": {"id": null}}));
        assert_eq!(refuse_action(&float, false), None);
    }

    #[test]
    fn unrestricted_is_off_unless_the_file_turns_it_on() {
        assert!(!Loaded::Missing.unrestricted());
        assert!(!Loaded::Valid(parse(EXAMPLE, false).unwrap()).unrestricted());
        assert!(Loaded::Valid(parse("unrestricted = true", false).unwrap()).unrestricted());
        assert!(parse("unrestricted = \"yes\"", false).is_err());
    }

    #[test]
    fn unrestricted_lifts_the_preset_rules_and_allows_env() {
        let shot = "[[preset]]\nname = \"shot\"\nargv = [\"kitty\", \"--class\", \"shot\"]\napp_id = \"shot\"\nenv = { GDK_SCALE = \"2\" }\n";
        assert!(
            parse(shot, false)
                .unwrap_err()
                .contains("needs unrestricted = true")
        );
        let plain_env =
            "[[preset]]\nname = \"a\"\nargv = [\"gimp\"]\napp_id = \"gimp\"\nenv = { A = \"1\" }\n";
        assert_eq!(
            parse(plain_env, false).unwrap_err(),
            "preset \"a\" has env, which needs unrestricted = true"
        );
        // The environment or the file's own key turns it on.
        let preset = &parse(shot, true).unwrap().presets[0];
        assert_eq!(
            preset.command(),
            ["env", "--", "GDK_SCALE=2", "kitty", "--class", "shot"]
        );
        let keyed = format!("unrestricted = true\n{shot}");
        assert_eq!(parse(&keyed, false).unwrap().presets.len(), 1);
        let runner =
            "[[preset]]\nname = \"b\"\nargv = [\"bash\", \"-c\", \"obs\"]\napp_id = \"obs\"\n";
        assert!(parse(runner, true).is_ok());
        assert_eq!(
            parse(EXAMPLE, true).unwrap().presets[0].command(),
            ["firefox"]
        );
        for bad in [r#"{ "" = "1" }"#, r#"{ "A=B" = "1" }"#] {
            let file =
                format!("[[preset]]\nname = \"c\"\nargv = [\"x\"]\napp_id = \"x\"\nenv = {bad}\n");
            assert!(
                parse(&file, true).unwrap_err().contains("env can't set"),
                "{bad}"
            );
        }
        let assignment =
            "[[preset]]\nname = \"d\"\nargv = [\"A=1\"]\napp_id = \"x\"\nenv = { B = \"2\" }\n";
        assert!(
            parse(assignment, true)
                .unwrap_err()
                .contains("can't contain =")
        );
    }

    #[test]
    fn the_variable_turns_unrestricted_on_only_with_1() {
        use std::ffi::OsStr;
        let read = |value: Option<&str>| Unrestricted::parse_env(value.map(OsStr::new));
        assert_eq!(read(None), Ok(false));
        assert_eq!(read(Some("")), Ok(false));
        assert_eq!(read(Some("1")), Ok(true));
        for wrong in ["0", "yes", "true", " 1"] {
            assert!(
                read(Some(wrong)).unwrap_err().contains("stays off"),
                "{wrong}"
            );
        }
        let status =
            |policy, env| serde_json::to_value(Unrestricted { policy, env }.status()).unwrap();
        assert_eq!(
            status(false, Ok(false)),
            serde_json::json!({"enabled": false, "source": null, "error": null})
        );
        assert_eq!(status(true, Ok(false))["source"], "policy");
        assert_eq!(status(false, Ok(true))["source"], "env");
        assert_eq!(status(true, Ok(true))["source"], "both");
        // A wrong value can't turn off the file's true.
        let wrong = status(true, Err("bad".to_owned()));
        assert_eq!(
            (&wrong["enabled"], &wrong["source"], &wrong["error"]),
            (
                &serde_json::json!(true),
                &serde_json::json!("policy"),
                &serde_json::json!("bad")
            )
        );
    }

    #[test]
    fn only_the_allowlisted_panels_can_be_named() {
        for allowed in ["control-center", "wallpaper", "tray-drawer"] {
            assert_eq!(panel(allowed).map(Panel::id), Ok(allowed));
        }
        for refused in [
            "session",
            "launcher",
            "polkit",
            "clipboard",
            "setup-wizard",
            "test",
            "Control-Center",
            "control-center audio",
            "",
        ] {
            let error = panel(refused).unwrap_err();
            assert_eq!(error.name, ErrorName::PanelNotAllowed, "{refused:?}");
            assert!(
                error
                    .detail
                    .contains("control-center, wallpaper, tray-drawer")
            );
        }
    }
}

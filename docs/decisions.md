# Decisions

Dependency and design decisions, newest last. Each entry says what was chosen, why, and what was considered instead.

Version rule: use the newest stable release that is at least 7 days old, and record its publish date here.

## 2026-10-06: Rust 1.99.0

- Pinned in `rust-toolchain.toml`, with `rust-version = "1.99"` in `Cargo.toml`.
- Released 2026-09-28. That's the newest stable release at least 7 days old.
- This is an application, so there's no reason to support older compilers. Both values move together, by hand, in one `build:` commit.

## 2026-10-06: `const fn main` in the skeleton

- Clippy 1.99 reports `missing_const_for_fn` for an empty `main`. Marking it `const fn` satisfies the lint without an `#[expect]`.
- It goes away once `main` does real work. Don't copy it as a pattern.

## 2026-10-06: CI actions

| Action | Pin | Latest release when chosen |
|---|---|---|
| `actions/checkout` | `v7` | v7.0.1, 2026-07-20 |
| `taiki-e/install-action` | `v2` | v2.87.26, 2026-10-06 |

- Actions are pinned to a major tag, so patch releases arrive without the 7-day delay. `install-action` publishes almost daily.
- Dependabot only proposes major-version bumps for these, with a 7-day cooldown.
- Pinning exact versions or commit SHAs would enforce the delay, at the cost of frequent update PRs. Revisit if that trade-off changes.

## 2026-10-06: CI tool versions

These match the versions installed locally.

| Tool | Version | Published |
|---|---|---|
| cargo-deny | 0.20.2 | 2026-07-09 |
| cargo-machete | 0.9.2 | 2026-04-15 |
| typos-cli | 1.50.3 | 2026-09-25 |
| shellcheck | 0.11.0 | 2025-08-04 |

- typos-cli 1.51.0 came out on 2026-10-06, so it's too new.
- Dependabot doesn't update these pins. Bump them by hand in `.github/workflows/ci.yml`, following the version rule.

## 2026-10-07: harness dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `niri-ipc` | `=26.4.0` | 2026-04-25 | niri's own request and response types. Pinned to the installed niri 26.04, as the version rule's exception requires. |
| `serde_json` | 1.0.151 | 2026-07-20 | Encodes requests and decodes replies. `niri-ipc` already depends on it. |
| `rustix` | 1.1.5 | 2026-09-16 | Waits for a child without reaping it (`waitid` with `WNOWAIT`) and kills its process group, without `unsafe`. Feature `process` only. |
| `signal-hook` | 0.4.4 | 2026-04-04 | Sets a flag on Ctrl+C, SIGTERM and SIGHUP, so `harness run` can kill its children's process groups. They don't get the terminal's signals because each has its own group. 0.4.5 (2026-10-04) is too new. |

- `niri_ipc::socket::Socket` has no timeouts, so the harness writes the same JSON line to a `UnixStream` with read and write timeouts.
- The harness is synchronous. It waits for a child on a separate thread and uses `recv_timeout` for the deadline, so it needs neither `tokio` nor `std::thread::sleep`.
- The runner kills the child's process group before reaping the child. Until then the child's ID can't be reused, so the kill can't reach an unrelated group.
- Captured output is read on its own threads. Once the child is gone, the runner waits at most 2 seconds for both streams together, because a descendant that left the group can hold a pipe open.
- After killing a child that timed out or was interrupted, the runner waits at most 2 seconds for it to exit. If it hasn't, the run fails without reaping it.
- `Cargo.lock` was checked against the version rule, transitive crates included. `libc` 0.2.190 (2026-10-02) was too new, so it is held at 0.2.189 with `cargo update -p libc --precise 0.2.189`.

## 2026-10-07: private bus config for the nested harness

- `dbus-run-session` normally uses `/usr/share/dbus-1/session.conf`, which listens with `unix:tmpdir=/tmp`. That put the bus socket in the host's `/tmp` (seen as `unix:path=/tmp/dbus-…`).
- The harness passes `--config-file` with `<listen>unix:dir=$TEST_DIR/run</listen>` and no service directories, so the socket lives in `TEST_DIR` and nothing on the host can be activated through the bus.
- Unix socket paths are limited to 108 bytes. A test with a long directory failed with `Socket name too long`. The harness refuses a `TEST_DIR/run` longer than 78 bytes, which leaves room for niri's socket name.

## 2026-10-07: the supervisor quits the nested niri

- niri starts its `--` command with a double fork and with stdin, stdout and stderr set to null (`src/utils/spawning.rs` at `v26.04`). It doesn't wait for that command or exit when it does.
- So the supervisor writes its log and status to files, and sends `Action::Quit` to the nested niri when it is done. It sends that only after `NIRI_SOCKET` resolves to a path under `TEST_DIR/run` and niri has reported `winit` as its only output. The host niri drives real outputs, so it can't pass the second check. If either check fails it sends nothing more, and the harness deadline kills the process group.
- The supervisor keeps one niri connection from the version request through `Quit`. niri answers any number of requests on one connection (`src/ipc/server.rs`, `handle_client`), and `Quit` then reaches the niri that was identified even if the socket path changed in between. The plan's one-connection-per-request rule is for the MCP server's actions; the harness doesn't need it.
- `harness supervise` takes the artifacts directory as an extra argument, so the supervisor can write there.

## 2026-10-07: `vpointer` probe dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | Connects to the compositor, binds the seat and outputs, and reads `wl_output.name`. |
| `wayland-protocols-wlr` | 0.3.12 | 2026-03-31 | The `zwlr_virtual_pointer_v1` bindings. Feature `client` only. |

- With default features, `wayland-client` uses its pure-Rust backend. The system libwayland isn't needed: `ldd` on the probe lists only libc and libgcc_s. The `system` feature would switch to libwayland.
- Every crate in `probes/vpointer/Cargo.lock` was checked against the version rule, transitive crates included. `libc` 0.2.190 (2026-10-02) and `cc` 1.6.0 (2026-10-03) were too new, so they are held at `libc` 0.2.189 (2026-07-21) and `cc` 1.5.1 (2026-09-25) with `cargo update --precise`. `cc` is a build dependency of `wayland-backend` and only compiles C when that crate's `log` feature is on, which it isn't.
- The rest of the lock, with publish dates from crates.io. Each is the newest release at least 7 days old that its dependents allow. `downcast-rs`, `quick-xml` and `windows-link` have newer releases outside the ranges `wayland-backend` (`^1.2`), `wayland-scanner` (`^0.41`) and `windows-sys` (`^0.2.1`) accept.

  | Crate | Version | Published |
  |---|---|---|
  | `bitflags` | 2.13.2 | 2026-09-10 |
  | `downcast-rs` | 1.2.1 | 2024-04-07 |
  | `errno` | 0.3.14 | 2025-09-09 |
  | `find-msvc-tools` | 0.1.14 | 2026-09-25 |
  | `linux-raw-sys` | 0.12.1 | 2025-12-23 |
  | `memchr` | 2.8.3 | 2026-07-08 |
  | `pkg-config` | 0.3.34 | 2026-08-14 |
  | `proc-macro2` | 1.0.107 | 2026-07-19 |
  | `quick-xml` | 0.41.0 | 2026-06-29 |
  | `quote` | 1.0.47 | 2026-07-19 |
  | `rustix` | 1.1.5 | 2026-09-16 |
  | `shlex` | 2.0.1 | 2026-05-17 |
  | `smallvec` | 1.16.2 | 2026-09-25 |
  | `unicode-ident` | 1.0.26 | 2026-09-17 |
  | `wayland-backend` | 0.3.17 | 2026-08-14 |
  | `wayland-protocols` | 0.32.13 | 2026-06-19 |
  | `wayland-scanner` | 0.31.11 | 2026-07-22 |
  | `wayland-sys` | 0.31.11 | 2026-03-31 |
  | `windows-link` | 0.2.1 | 2025-10-06 |
  | `windows-sys` | 0.61.2 | 2025-10-06 |
- The probe has its own `Cargo.lock`, committed, so its results can be reproduced.

## 2026-10-07: pointer and capture steps in the supervisor

- wev 1.1.0 never flushes stdout (no `fflush` or `setvbuf` in `wev.c`), and with stdout in a file, glibc buffers it in blocks. The supervisor runs `stdbuf -oL wev` (coreutils), so each event reaches `wev.log` as a line.
- `wait_until` polls every 50 ms with `std::thread::sleep`, behind one `#[expect(clippy::disallowed_methods)]`. The ban exists so nothing blocks an async runtime, and the harness has none. Polling niri state and the `wev` log is how plan §13 describes `wait_until`. `thread::park_timeout` would avoid the lint without saying why.
- Each probe call sends its own event time. niri passes it through to the client's `wl_pointer` events (a probe motion with time 4001 shows up in `wev.log` as `motion: time: 4001`), so each check reads only its own events.
- C3 asks grim for a PPM, which is a short header followed by raw RGB, so the harness needs no image decoder. C15 reads the size from the PNG `IHDR` chunk or the JPEG start-of-frame segment, as plan §8 says the server will. Both take a few dozen lines, so neither needs a crate.
- The runner can now start a process and stop it later, which `wev` needs. A watchdog thread kills the process at its deadline. It shares a lock with the code that reaps the child, and only kills while the child is unreaped, so it can't signal a reused process ID. Any ending seen at or after the deadline, including a clean exit, counts as a timeout. Dropping the handle kills the process.
- `wait_until` ignores a value its condition returns after the deadline. A condition that asks niri can take that request's own 5-second deadline, so a wait can end up to 5 seconds late, but it then fails instead of passing.
- C15 on a host output is a separate command, `harness host-capture`, run from your shell. The supervisor can only reach the nested niri.

## 2026-10-07: keyboard checks and the stdin gate

- The existing runner holds and feeds stdin, so C5(b) needs no new probe or dependency. A writer runs on a thread and waits only until the child's original deadline; the existing watchdog bounds a child that doesn't read. All wtype children start through the checked nested `Session`.
- Keyboard checks use byte offsets in the append-only `wev` log, because wtype 0.4 sends key time 0 (`main.c`, `type_keycode` and `run_key`). The parser includes the key and modifier continuation lines from wev 1.1.0 (`wev.c`, `wl_keyboard_key` and `wl_keyboard_modifiers`). A partial record waits for its final newline; a malformed complete record fails.
- `still_absent` checks for the full interval and once at its end. C5(b) watches keys and modifiers for two seconds while stdin is open, and C9 watches `bind-fired` for one second after the chord. Both passed at scale 1 and 1.5.
- Keep the stdin gate, and keep the 100-character cap and three-second wtype deadline. The slowest of the ten C10 calls was 624.112 ms, below the 1500 ms decision threshold. Keeping wtype remains conditional on C6's human keymap-restoration check.
- Sources: [wtype 0.4](https://raw.githubusercontent.com/atx/wtype/v0.4/main.c) and [wev 1.1.0](https://git.sr.ht/~sircmpwn/wev/blob/1.1.0/wev.c).

## 2026-10-07: no system bus in the nested session

- PARENT sets `DBUS_SYSTEM_BUS_ADDRESS` to `TEST_DIR/run/no-system-bus`, where nothing listens. A nested Noctalia on the real system bus registered itself as BlueZ's default pairing agent (`RequestDefaultAgent` in `bluetooth_agent.cpp`) and tried to register a NetworkManager secret agent. With its default lock screen settings it also takes a logind sleep-delay inhibitor, and sets the locked hint of the logind session it finds by its PID when it locks (`logind_service.cpp`, `application_ui.cpp`). That session is the host's wherever the harness runs inside the session scope. The config can turn off the lock screen parts but not the two agents, and all of them act on the host.
- With no system bus, Noctalia logs `system dbus disabled` and runs without those services. The nested niri loses its read-only `login1` (lid switch) and `locale1` (keyboard layout) watchers and logs a warning for each. `X11 Layout` is unset on this machine, so the nested keymap doesn't change.
- Containment and the supervisor's endpoint checks require exactly that address, and that nothing, not even a symlink, exists at its path. A path that merely lies under `TEST_DIR` could be a symlink to the host bus or a socket someone listens on.

## 2026-10-07: `noctalia-socket` probe

- The probe uses only the standard library: a `UnixStream` with read and write timeouts. It has no dependencies, and its `Cargo.lock` lists only itself.
- It sends `/`, `\x1e` and one of three fixed commands. Noctalia 5.2.1 erases everything up to the first `\x1e` before it parses a command (`src/ipc/ipc_service.cpp`), so text from elsewhere must never reach the payload. The probe picks a constant by exact match and sends that constant, never its argument.

## 2026-10-07: Noctalia in the nested harness

- No new dependencies. The harness reads `status` with `serde_json`, which it already depends on, and drives Noctalia through the `noctalia-socket` probe.
- The harness generates a Noctalia config instead of using the defaults. Without a setup marker in its fresh state directory, Noctalia opens its setup wizard as a panel on startup (`application_ui.cpp`), so `activePanelId` wouldn't start as null. Weather is off so a test run makes no weather requests. The config also lists no plugin sources (`[plugins] source = []`). By default Noctalia has two git sources, the official and community plugin repositories on GitHub, and at every start it clones each one that isn't cached yet (`ensureEnabledMaterialized` in `plugin_manager.cpp`), whatever `auto_update` says. It runs the clones in a process group of its own (`setpgid` in `process.cpp`), so they escape the harness's group kill: a reviewer's run left both clones writing under the removed `TEST_DIR` for about 20 seconds after the harness exited. The defaults apply only when the `source` array is absent (`config_service.cpp`), so an explicit empty array removes them. `auto_update = "none"` alone was tried first; the community clone still outlived the run. `noctalia config validate` exits 0 even when it warns about an unknown key, so the harness requires its plain success line.
- Noctalia starts after the `wev` checks. Its bar reserves 34 logical pixels at the top of the output, and `wev`'s position is relative to the top-left of the working area.
- C13's pass rule only reads `status`. The supervisor also compares captures of the output, because a screenshot taken right after `status` reports the panel open showed no panel: the panel was not drawn yet. Noctalia also answers `status` before it has drawn its wallpaper, and a capture taken then is the magenta background: in one review run the wallpaper alone made the panel look drawn and never gone. So the supervisor first waits until less than half the output is magenta and two consecutive captures differ in fewer than 1% of their pixels, and keeps that capture. It waits until at least 1% of the pixels differ from it after `panel-open` and saves the screenshot then. These capture waits allow five seconds rather than C13's two, because Noctalia can report the panel open long before it draws it: in one run (1791393079-462290) it logged `queued surface rendering took 1935.1ms`, and the panel wasn't drawn within two seconds. Both plugin clones had just finished in that run, so they may have caused the stall. A change of 90% or more fails at once: the control center changed 22–24% of the output at scale 1 and 50–67% at scale 1.5 in the runs so far, so a change that large means a bad baseline. After `panel-close`, it waits until fewer than 1% of the pixels differ from that capture. Exact equality would fail whenever the bar's clock ticks between the two captures.
- After each run, `harness run` lists processes whose command line names `TEST_DIR` and fails if any are left. It reports them and doesn't kill them, because they are outside the run's process groups and matching by command line is not proof of ownership.
- Noctalia binds its IPC socket before it listens (`IpcService::start`), so a `status` sent as soon as the socket exists can be refused. The supervisor retries the first `status` until its startup deadline and reports the last failure if none succeeds.

## 2026-10-08: supervised input and recovery evidence

- No dependencies or lockfiles changed. The sitting reuses `Session`, the bounded runner, keyboard log offsets and pointer output binding. Human confirmation files are separate from observed physical events. Only an explicit sitting mode extends deadlines; automatic checks retain their original path.
- Keep wtype 0.4: C6 observed broadcasts to an unfocused keyboard and restoration of its original keymap format and size after physical `x`. The stdin gate, 100-character cap and three-second child deadline stay. wtype's 100-character batching is in `main.c:367–418`; its key and modifier requests are in `main.c:318–342`. Keymap identity here means format and size, as C6 specifies, not a content hash.
- C7 matched recovery by the original logged keycode. Fresh `wtype -p a` cleared the one-symbol synthetic code 9; physical `a` used code 38 and did not release code 9. Physical Shift cleared the modifier. wev's key logger (`wev.c:383–401`) does not track held keys or client repeat, so these event results do not prove application behavior. wtype chooses its own keycodes; a fresh one-symbol release is not evidence of generic recovery for arbitrary multi-symbol calls.
- C8 independently verified fresh-device release and physical click after observed press then SIGKILL. The later human-only `recover` should send a fresh virtual-pointer release for the interrupted button. It should retain manual modifier/click checks and interactive confirmation, warn about ordinary-key limitations, and keep the dirty marker when the human cannot confirm the application's state. Only Shift and the left button were physically verified here.
- C9's physical chord fired the nested bind while wtype's chord did not. Smithay sends virtual keys directly to focused keyboards (`src/wayland/virtual_keyboard/virtual_keyboard_handle.rs:104–120` at `ff5fa7df`) and performs no release on destruction (`:160–173`). niri's virtual-pointer destruction hook is the trait's empty default (`src/protocols/virtual_pointer.rs:278–280` at `v26.04`), which niri's `State` doesn't override (`src/handlers/mod.rs:666–690`); `destroyed` only drops the resource from the manager's set (`virtual_pointer.rs:530–544`). C8 supplies the runtime release result.
- C14(a) returned `no` for session 3. The optional physical lock/unlock check was skipped, so no runtime claim is made about the locked transition.
- The first sitting stopped because its pointer check expected motion where wev logged enter. The sitting now reuses the automatic enter-or-motion helper. An explicit C8 restart mode preserves the completed keyboard evidence and prevents unnecessary repeated human checks.

## 2026-10-08: M0 outcome

- Input: wtype 0.4 for the keyboard, behind the stdin gate, with the 100-character cap and three-second deadline. The pointer stays a native `zwlr_virtual_pointer_v1` bound to an output (plan §4), which `vpointer` exercises.
- Screenshots: grim honours `-s` below 1 on niri, so `max_width` becomes a lower capture scale and the server needs no resize step. The default format is JPEG. The default `max_width` of 1280 stays provisional until M1's image delivery check.
- Recovery: send the interrupted button's release from a fresh virtual pointer, keep the manual checks and interactive confirmation, and keep the input-dirty marker when recovery is uncertain. The evidence and its limits are in `docs/results/m0.md`.

## 2026-10-08: niri-ipc's license

- `niri-ipc` 26.4.0 is GPL-3.0-or-later, like niri. The license gate allowed only permissive licenses, and it hadn't checked the harness, because cargo-deny skips workspace crates with `publish = false`. The gate first failed when the server started to depend on `niri-ipc`.
- The repository stays MIT. `deny.toml` allows GPL-3.0-or-later for `niri-ipc` alone; any other GPL crate still fails. MIT code can be combined into a GPL program, but a built binary includes `niri-ipc`, so a distributed binary follows GPL-3.0 terms. The README says so.
- Considered: relicensing the project to GPL-3.0-or-later, which changes little for binaries but gives up MIT for code that doesn't need GPL, and dropping `niri-ipc` for hand-written copies of niri's types, which would have to track niri by hand and are derived from GPL source anyway.

## 2026-10-08: MCP server dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `rmcp` | 3.5.0 | 2026-09-28 | The official Rust MCP SDK. Default features (`server`, `macros`, `base64`) plus `transport-io` for stdio. 3.5.1 (2026-10-05) is too new. |
| `tokio` | 1.53.1 | 2026-07-20 | rmcp's runtime. Features `rt`, `macros`, `net` (niri's Unix socket), `time` (deadlines) and `io-util`. 1.53.2 (2026-10-03) is too new. |
| `serde` | 1.0.229 | 2026-07-18 | `derive` for the status report and error bodies. |
| `serde_json` | 1.0.151 | 2026-07-20 | Same version as the harness. |
| `niri-ipc` | `=26.4.0` | 2026-04-25 | Pinned to the installed niri, as the version rule's exception requires. |

- Every crate rmcp and tokio added to `Cargo.lock` was checked against the version rule. `cc` 1.6.0 (2026-10-03), `mio` 1.2.4 (2026-10-03) and `uuid` 1.27.0 (2026-10-02) were too new, so they are held at `cc` 1.5.1, `mio` 1.2.3 and `uuid` 1.26.1 with `cargo update --precise`. The other new crates were published on or before 2026-10-01.
- The runtime is Tokio's current-thread flavour. The server handles one client over stdio, and one thread keeps the order of its work easy to follow.
- rmcp's `#[tool_handler]` generates an `async fn list_tools` with no `.await`, which Clippy's `unused_async_trait_impl` rejects. The handler impl carries one `#[expect]` for it. It is the first lint exception the rmcp macros have needed.
- Each niri request uses a new connection, so a reply that arrives after its deadline can't be mistaken for the next reply. The harness keeps one connection instead, because it needs to reach the niri it identified.
- With tokio in the build but its `process` feature off, Clippy warns that the `tokio::process::Command::new` entry in `clippy.toml` doesn't resolve. The warning doesn't fail the gate and goes away when the server starts its first subprocess.

## 2026-10-08: niri event stream

- No new crate. tokio's `sync` feature adds the `watch` channel between the reader task and the tools; the version is unchanged (1.53.1).
- The server replays the stream with niri-ipc's `EventStreamState` instead of keeping its own copy of niri's state, so niri's own reducer decides what each event changes.
- The state counts as initialized when the workspaces, windows and overview events have arrived. niri sends its whole state as one burst on connect (`EventStreamState::replicate` in niri-ipc 26.4.0), but it marks no end of the burst. The overview event comes after the workspaces, windows and keyboard layouts, which are what `desktop_state` returns.
- One unparsable event reconnects; a second stops the stream until restart, as plan §7 says. The counter doesn't reset, so two bad events far apart also stop it. Ordinary disconnects reconnect after one second and don't count.
- Matching niri's `Event` in the server uses `matches!` for the three initialization events. Everything else goes to niri-ipc's reducer, so the server has no `match` on `Event` that a niri-ipc bump would need to extend.

## 2026-10-08: deterministic timing tests

- tokio's `test-util` feature is a dev-dependency feature only (same version, 1.53.1). Tests that depend on deadlines and reconnect delays run on Tokio's paused clock, which moves on only when every task is waiting, so their outcome doesn't depend on how fast the machine is.

## 2026-10-08: cancellation

- rmcp 3.5.0 runs each request in its own task and only cancels the request's token when the client sends a cancellation (`service.rs`, the request branch of the serve loop); it doesn't stop the task. Each tool takes rmcp's `RequestContext` and races its work against that token, so a cancelled call drops its niri connection or wait at once instead of at its deadline.

## 2026-10-08: screenshots, clipboard and the runner

| Crate | Version | Published | Why |
|---|---|---|---|
| `base64` | 0.23.1 | 2026-08-04 | Encodes screenshot images for MCP image content. rmcp already depends on the same version. |
| `rustix` | 1.1.5 | 2026-09-16 | Kills a child's process group on timeout or cancellation, without `unsafe`. Feature `process`. Same version as the harness. |

- tokio gains the `process` feature (same version). No new package entered `Cargo.lock`.
- grim truncates its image size (`render.c:145–146` at grim v1.5.0), where plan §8 says `round(logical × s)`. M0's C15 only used scales whose products are whole numbers, so it couldn't tell the two apart. The server expects truncation and nudges a lowered scale up to the next representable double until the width is exactly `max_width`; a unit test checks every width from 1281 to 3999 at four output scales.
- `max_width` defaults to 1280 image pixels (plan §4, provisional until M1's image delivery check), so an omitted `max_width` on a 2560-pixel output gives a 1280-pixel image. A capture keeps its full detail only when its logical width times the output's scale fits; a 1000-pixel region at scale 2 is still lowered to 1280. On DP-1 the default made JPEG capture take about 100 ms instead of 14 ms.
- Screenshot refs (plan §8) are not stored yet. Only the pointer tools use them, and they arrive with the input milestones; until then a stored ref would be dead code. The metadata returns everything a ref will hold.
- Arguments that don't fit the desktop (an unknown output, a region that crosses outputs, a malformed target) come back as a tool result with `isError` and plain text, not a new name in the stable error list. rmcp 3.5.0 already answers arguments that don't fit the schema that way (`handler/server/router/tool.rs`, `into_tool_argument_error`), so both kinds of argument mistake look the same to the model, which can then correct the call.
- `clipboard_read` runs `wl-paste --no-newline --type text`. wl-paste 2.3.0 exits 1 both for an empty clipboard and when nothing copied is text (`src/wl-paste.c:222–245`, `:265–268`); the tool returns those as `text: null` with `reason` `nothing_copied` or `no_text`. Text is capped at 1 MiB and must be UTF-8.
- The harness includes `src/image_header.rs` with `#[path]` instead of keeping its own copy. The server is a binary crate, so there is no library to depend on, and one header parser keeps the server and C15's checks in agreement.

## 2026-10-08: Noctalia detection and the lock state

- No new crate. The server's Noctalia client is async and sends one fixed payload, `/\x1estatus`, framed the way M0's `noctalia-socket` probe and Noctalia's own client frame it. Panel commands wait for the milestone that needs them.
- `shell_status` is removed from the tool router at startup when `noctalia` isn't on `PATH` (plan §6.1), using rmcp's `ToolRouter::remove_route`. An installed Noctalia that isn't answering gives `noctalia_unavailable`, a new name in the stable list, as plan §6.2 defines it.
- `status` reports Noctalia's failure detail as `noctalia_error` and logind's as `lock.logind_error`. Plan §6's shape allows added fields. niri's version and Noctalia's status are read concurrently, and the lock read follows because it may need Noctalia's reply, so `status` waits at most two deadlines, not three.
- `XDG_SESSION_ID` is passed to `loginctl` as an argument, so a value that isn't plain letters and digits is refused instead of risking it being read as an option.
- Locked wins between the two sources (review finding). niri sets logind's locked hint only on its own session (`src/niri.rs` at v26.04), so a server started from an SSH login, a TTY or an environment without the graphical session's ID reads a hint that never changes. Plan §9 ordered the sources but didn't say what to do when they disagree. Checking that the session is the graphical one is left for the action tools' lock gate.
- Whether Noctalia is installed is decided once at startup and used for both the tool list and `status`.

## 2026-10-08: the audit log

| Crate | Version | Published | Why |
|---|---|---|---|
| `chrono` | 0.4.45 | 2026-06-04 | RFC 3339 timestamps with milliseconds in UTC. rmcp already depends on this version with the `now` feature, which is the only one the server enables, so no package was added to `Cargo.lock`. |

- One line per call, opened in append mode and written with one `write_all`, so servers sharing the file don't interleave inside a line. The directory is created with mode `0700` and the file with `0600` (plan §12).
- The session label is the MCP client's `clientInfo.name` and the server's PID, as plan §10 defines it; `unknown` when the client sent no initialization.
- The outcome comes only from the result's error name, never from its content, so a result holding clipboard text or an image can't leak into the log. Outcomes beyond plan §6.2's names are `invalid_arguments`, `cancelled`, and `internal` when the tool couldn't build its result.
- A failed write doesn't fail the tool call, because the read-only tools have nothing to protect; `status` shows the last failure. The action tools may need a stricter rule.
- rmcp rejects arguments that don't fit a tool's schema before the tool runs, so those calls aren't logged.
- `screenshot`'s arguments are logged as given, `target` included, even when it names no output. They are the model's own identifiers, never desktop content, and serde_json escapes them.

## 2026-10-08: protocol tests

- The protocol tests write JSON-RPC by hand instead of using rmcp's client, so they see every byte the server writes to stdout and send each cancellation at the moment they choose. No crate was added.

## 2026-10-08: the MCP Inspector

| Tool | Version | Published | Why |
|---|---|---|---|
| `@modelcontextprotocol/inspector` | 2.9.0 | 2026-09-30 | The pinned Inspector for `make inspect` and `make inspect-check` (plan §13). 2.10.0 was published on 2026-10-07, too recently for the version rule. It runs through `npx`, so nothing is added to the repository. |

- The Inspector starts a stdio server with a minimal environment. `NIRI_SOCKET`, `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY` and `XDG_SESSION_ID` are not passed on, and `status` then reports `NIRI_SOCKET is not set`. `scripts/inspector.sh` passes those four on explicitly. Since 2026-10-10 the server finds the first three itself; see below.
- The Inspector's `--strict` schema check flagged `screenshot`'s `max_width`, typed `["integer", "null"]`, as less portable: clients that map tool schemas onto a single-type dialect may reject it. The optional `screenshot` arguments are now described as their own types and left out of `required`. The server still accepts `null` for them.

## 2026-10-08: the name

- The project is `niri-computer-use`: the crate, the binary, the MCP server's name, the skill, the audit log's directory and the GitHub repository. "Computer use" is the term agents and their users look for, and the milestones after M1 add input. The server name is the same everywhere, so clients register it as `niri-computer-use`. `docs/results/m0.md` keeps the commands as they were run, under the old name `niri-desktop-mcp`.

## 2026-10-08: the docs site

| Package or action | Version | Published | Why |
|---|---|---|---|
| `astro` | 7.3.5 | 2026-09-24 | The site generator Starlight runs on (plan §21.3). 7.3.6 and 7.3.7 are too new for the version rule. |
| `@astrojs/starlight` | 0.42.5 | 2026-10-01 | Documentation theme with navigation and search. |
| `actions/setup-node` | v7.0.0 | 2026-07-14 | Node.js for the Pages build. v7.1.0 was published on 2026-10-08, so the major tag isn't used. |
| `actions/upload-pages-artifact` | v5 (5.0.0) | 2026-04-10 | Uploads the built site. |
| `actions/deploy-pages` | v5 (5.0.1) | 2026-09-01 | Deploys it to GitHub Pages. |

- `site/package-lock.json` was created with `npm install --before=2026-10-01T09:30:00Z`, so every transitive package also follows the version rule. Dependabot watches `site/` with the same seven-day cooldown.
- `npm audit` reports two advisories in the build tooling. `http-cache-semantics` 4.2.0, through Astro, can leak cached responses between users of a server's HTTP cache (GHSA-ch52-4w7c-c8xp); the static build serves nobody, and the fix, 4.3.0, came out on 2026-10-04. `postcss-selector-parser` 6.1.4, through Starlight's code-block styling, has quadratic parsing on crafted selectors (GHSA-rj75-hqrm-r3gf), and only our own CSS goes through it; no 6.x release fixes it. Neither is overridden.
- The site is plain Markdown pages in `site/src/content/docs/`, built to `site/dist/` and deployed by `.github/workflows/pages.yml` on pushes to `main` that touch `site/`.
- npm instead of the pnpm with `minimumReleaseAge` that plan §21.5 names: npm ships with Node.js, and the site has two direct dependencies. npm keeps no release-age setting, so a manual `npm install` or `npm update` in `site/` must pass `--before` with a date seven days back, as Cargo updates use `--precise`; Dependabot's cooldown covers routine bumps.
- `actions/setup-node` and `npm ci` instead of Astro's `withastro/action`, so CI and the deploy build the site with the same steps.
- CI builds the site on every push and pull request (`ci.yml`, job `site`), so a broken page fails before the deploy does. There is no link checker yet; the internal links were checked by hand against the build.
- The landing page has no demo. Plan §21.3 asks for a screenshot or recording of a real run, never a mock-up, and none has been taken for publication.

## 2026-10-08: the lease and the stop watcher

- No new crate. The lease lock is `std::fs::File::try_lock`, an `flock` on Linux, in the standard library since Rust 1.89. The stop watcher uses inotify through rustix's `fs` feature (same version, 1.1.5) and Tokio's `AsyncFd`, which the `net` feature already brings.
- The watcher doesn't interpret event names: any change in the runtime directory makes it check whether `stop` exists. That avoids reasoning about event order, renames and coalesced events.
- The flags fail closed (review finding): a runtime directory that can't be read counts as stopped for the watcher and refuses `acquire_desktop`, instead of reading as "no flag".
- `lease.json` is display only. The holder empties it before unlocking, and readers ignore a record whose PID has no `/proc` entry, so a crashed holder doesn't show up as holding the lease.
- Removing the runtime directory or the `lease` file under a holder (review finding) is caught by inode checks: the watcher compares the directory's path with the inode it watches, on every event and once a second, and the holder does the same for `lease`. A plain inotify watch isn't enough, because the kernel delays a directory's deletion event while a file in it, the held lease, is open. After the directory is lost the server refuses the lease until it restarts.
- The release task reads the latest flag on every change (level-triggered), so a resume followed quickly by a stop can't be missed, and `acquire_desktop` checks both the watcher's view and the file.
- `release_desktop` when not holding the lease succeeds with `released: false` rather than failing, so an agent whose lease the stop flag already took back isn't told it made a mistake.
- Whether `acquire_desktop` also refuses while the screen is locked is decided with the gate in a later step; for now only the stop flag and the input-dirty marker refuse it.

## 2026-10-08: the input-dirty marker and `recover`

- No new crate. `/proc` is read directly: the start time (field 22 of `stat`, counted from the last `)` because the command name may contain one) pins a PID to one process, and `comm` plus the real UID find the user's `wtype` processes.
- Only the reader of the marker exists in M2. The input tools that write it arrive in M3 and M4; until then the tests write it as a fixture. A marker that can't be parsed blocks like any other, so a malformed file can never unblock input.
- Plan §11 step 3 has `recover` send the release of a pressed pointer button from a fresh virtual pointer. The pointer module arrives in M4, so this version asks the human to press and release the buttons the marker names instead, and the manual check already asks for a click. The automatic release is added with the pointer.
- The plan's M2 exit lists `recover` against a live owner and with a marker naming a delayed-exit child as nested tests. Neither touches niri, so they run as protocol tests against the binary with a fixture runtime directory: the child is a `sh` that ignores SIGTERM in a process group of its own, and a marker whose start time doesn't match proves a reused PID is never killed. The `pending` path, which scans the user's real `/proc` for `wtype`, is unit-tested against a fake `/proc` whose entries link to processes the test started, so no test can end a process it doesn't own.
- `recover` pins every process it may kill by PID and start time when it lists it, and kills it only if it is still that process; a process that is already gone counts as ended (review finding). A child that doesn't lead its own process group is ended alone rather than by group.

## 2026-10-08: the lock state follows niri's session

- Plan §9 asked for a check that `XDG_SESSION_ID` names the graphical session before the lock gate trusts logind. On this machine the session in `XDG_SESSION_ID` is of type `tty` and niri runs as a systemd user service outside any session scope, so checks on the session's type or on niri's cgroup would reject the normal setup. niri v26.04 sets `LockedHint` on the session in its own `XDG_SESSION_ID` (`update_locked_hint`, `src/niri.rs`). The server now asks logind about exactly that session: niri's PID comes from the socket's peer credentials, and its `XDG_SESSION_ID` from `/proc/<pid>/environ`, readable because niri runs as the same user. The server's own `XDG_SESSION_ID` is no longer read.
- The protocol tests run the fake niri for these cases as a process of its own, the test binary started with `--ignored --exact`, so its environment is exactly what the test sets; the in-process fake would show the test runner's environment, which differs between machines and CI.

## 2026-10-08: the policy file and the lease decision

| Crate | Version | Published | Why |
|---|---|---|---|
| `toml` | 1.1.6 | 2026-09-10 | Parses the policy file (plan §9). Features `std`, `serde` and `parse` only. 1.1.7 was published on 2026-10-08, too new for the version rule. |

- `toml` 1.1.6 pulled in `serde_spanned` 1.1.2, `toml_datetime` 1.1.2, `toml_parser` 1.1.4 and `toml_writer` 1.1.3, all published on 2026-10-08. They are held at `serde_spanned` 1.1.1 (2026-03-31), `toml_datetime` 1.1.1 (2026-03-31), `toml_parser` 1.1.3 (2026-07-27) and `toml_writer` 1.1.2 (2026-07-14) with `cargo update --precise`. `winnow` 1.0.4 (2026-07-13) is old enough.
- Plan §9 refuses presets whose `argv[0]` is "a known shell or terminal with `-e`/`-c`". The rule here is wider: any program that runs a command it is given (shells, interpreters, `env`, `sudo`, `doas`, `pkexec`, `su`, `run0`, `setsid`, `nohup`, `systemd-run`, `timeout`, `xargs`, `nice`) is refused, and a terminal is allowed only without arguments, because terminals such as `kitty` run trailing arguments as a command without any flag. The lists match the program's file name, so a path doesn't get around them, and a trailing version is removed before matching, so `python3.13` is `python3`. `busybox`, `toybox`, `uwsm`, `distrobox` and `toolbox` are on the list, and `flatpak` is refused only with `--command`, because `flatpak run <app>` is the usual way to start a Flatpak app (review finding). The rules are a guardrail, not a boundary: a wrapper script or a renamed link gets past any list.
- An unresolvable config directory (no `XDG_CONFIG_HOME` and no `HOME`) makes the policy `invalid` rather than `missing`: a file might exist that the server can't find.
- `acquire_desktop` now refuses while the screen is locked and while the server is read-only, so an agent learns at once that it can't act, instead of at its first action. An unknown lock state is allowed, as plan §9 says. The app deny list is loaded and counted, and enforced once the input tools exist.

## 2026-10-08: an unknown lock state refuses the lease

- Plan §9 reported `unknown` and allowed it. Since the lock state follows niri's own session, `unknown` has more causes: niri's environment can't be read, something else serves the socket, or niri wasn't started from a session, each without Noctalia running. The user chose to refuse: `acquire_desktop` returns `screen_locked` with a detail that says the state is unknown, so input only ever goes to a screen that a source niri actually updates says is unlocked. One case still reads a confident wrong answer: a plain `niri`, not `niri --session`, started inside a logind session inherits `XDG_SESSION_ID` but never sets the hint, so logind keeps saying `no`; reading `--session` from niri's command line closes it, and is planned before input arrives in M4. A desktop without logind's hint and without Noctalia can't be controlled until one of them answers.

## 2026-10-08: M2's nested acceptance

- The M2 exit criteria run in `make nested-control`: two servers competing for the lease, stop and resume, `recover` against a live owner and with a marker naming a delayed-exit child. The protocol tests check the same behaviour against fakes; the nested run adds a real niri (its version, its peer credentials), a real Noctalia as the lock source, and the stop sent through niri's `spawn` action, the path the stop keybind takes.
- The nested run starts Noctalia because the lease now needs an unlocked answer: a nested niri isn't a session instance and sets no logind hint.
- The harness has no interactive MCP client. Each server reads a `printf` of its requests followed by a `sleep` that keeps stdin open, so every process stays under the harness runner's deadlines and process groups.

## 2026-10-08: logind only for `niri --session`

- niri v26.04 sets logind's `LockedHint` only when it runs as the session instance (`update_locked_hint` returns early unless `is_session_instance`, `src/niri.rs`), which `niri --session` turns on. A plain `niri` started inside a logind session inherits `XDG_SESSION_ID`, so logind kept answering `no` on a locked screen. The server now reads niri's `/proc/<pid>/cmdline` and asks logind only when `--session` is among the arguments; otherwise logind counts as not answering, Noctalia decides, and without Noctalia the state is `unknown`, which refuses the lease.
- The protocol tests' fake niri is the test binary started with `--ignored --exact`. libtest rejects unknown options, so `--session` goes after `--`, where libtest reads it as one more test-name filter that matches no test. No separate fake-niri binary was needed.

## 2026-10-08: the niri action tools

- No new crate. Waiters use Tokio's `broadcast` channel, from the `sync` feature already enabled.
- Every action runs through one gate that holds the action mutex for the whole action (plan §9, §10). Its order is the stop flag, the input-dirty marker, the lease, then the same `refuse_control` as `acquire_desktop` (niri unreachable, `read_only`, `screen_locked`). Stop and recovery come before `lease_required`: a stop also takes the lease away, and an agent told `lease_required` would call `acquire_desktop` only to be told `stopped`. The readiness report behind `refuse_control` costs a niri request and a `loginctl` run, so it is gathered only after the cheap checks pass, and it is gathered again for every action, because the lock state can change while the lease is held.
- A stop during an action cancels it (plan §11: cancel, clean up, release). The gate races the action against the stop watcher; the watcher's task waits for the action mutex before it takes the lease back, so the lease is released only after the action has been dropped. The cancelled call returns `stopped`, whose detail says that what niri already accepted may have taken effect, because the server doesn't track whether the request was sent. A watcher that ends during the action, because the runtime directory was removed, cancels it too, but the call says `upstream_error` and to restart the server, as `acquire_desktop` does then (review finding). Cancelling the MCP request drops the action and keeps the lease: only the user's stop or `release_desktop` gives it up. `release_desktop` waits for a running action; an action has no overall deadline, so the wait is the sum of its steps' deadlines (see the lease section of `architecture.md`).
- Waiters apply every event themselves instead of reading the replica when it changes. A Tokio `watch` keeps only the latest value, so focus passing through another window between two reads would be missed, and `interrupted` would depend on timing. The stream task numbers each event and broadcasts it after the replica applies it; a waiter subscribes before copying the replica's windows and workspaces and skips events the copy already holds. niri-ipc's `EventStreamState` isn't `Clone`, but its window and workspace parts have public fields, so the waiter copies those two and applies events with niri-ipc's own reducer. Connections are numbered too, and the end of a connection carries its number, so a waiter whose copy came from a newer connection ignores an older one's end instead of reporting `uncertain` (review finding).
- A lost reply is told apart from a refused request. niri reads one request line and answers after carrying the action out (`src/ipc/server.rs` at v26.04), so a failed connect or write, and niri's error reply, mean nothing happened, and the call fails with that error. A reply lost after the whole request was written is the outcome `uncertain` with `accepted: null`, never an error and never retried (plan §6.2).
- An unknown window or workspace id is an argument mistake, checked against the waiter's state before anything is sent. niri silently ignores actions on ids it doesn't know (`do_action`, `src/input/mod.rs` at v26.04), which would otherwise show up only as a timeout.
- `focus_workspace` takes a workspace id from `desktop_state`, not the plan's `ref`. niri's index references count per output and its names are optional, so an id is the one reference that always names one workspace.
- `interrupted` applies to the focus tools and to `launch`, whose wait is the longest. For `launch`, only focus moving to a window that was already open counts, because a launched app may map and take focus before it sets its `app_id`. A window that was already open and has the preset's `app_id` doesn't count either: a single-instance app hands a second start to its running process, which focuses the window it has, so `launch` reports `focused` with that window rather than an intruder (review finding). `close_window` doesn't use it: focus moves away from a closing window by design. Focus leaving every window, for example to a shell panel, keeps waiting.
- `launch` keeps counting matching windows for half a second after the first appears, so an app that opens two windows reports `ambiguous` instead of `one`. The whole call can take up to five and a half seconds.
- A focus target that already has focus is reported as `focused` with `accepted: false`, and nothing is sent (plan §7 step 3). With `workspace-auto-back-and-forth` set, niri answers `FocusWorkspace` on the focused workspace by switching to the previous one (`switch_workspace_auto_back_and_forth`, `src/layout/monitor.rs` at v26.04), so sending it would move the agent away while the check already said `focused` (review finding). `focus_window` follows the same rule, so the two read alike.
- `close_window` is marked destructive and not idempotent: an app may answer a second close differently, for example while its unsaved-changes dialog is open. The focus tools are idempotent; `launch` isn't.
- `status.policy.preset_names` lists the preset names, because `launch` takes a name and an agent has no other way to learn them. The field is added beside the existing ones, as plan §6 allows.
- The audit log reads `accepted` and `observed` only from action tools' successful results, so a read-only tool whose content happens to have such a field, such as Noctalia's status, never fills them.
- The protocol tests run the actions against the in-process fake niri, which answers actions `Handled` or holds them, and hands each one to the test; the test then sends the events niri would. The lock state comes from a fake Noctalia whose reply the test can switch, since the in-process fake niri doesn't run as `niri --session`.

## 2026-10-08: a screenshot with outcomes in doubt

- Plan §6 attaches a fresh screenshot to an action result whose observation is a timeout, `pending`, `uncertain` or `interrupted`. `launch`'s `none` is a timeout under another name, so it gets one too. `one`, `ambiguous`, `focused` and `closed` don't.
- It is taken after the stop race but before the action mutex is released: a stop during the capture no longer throws away the outcome the server already observed, such as `pending` (review finding), and no other action of this server can change the desktop between the observation and the picture. A stop during the capture waits for it, at most grim's five seconds, before the lease is given back. It is the focused output at the default 1280 pixels wide, as JPEG, through the same code as the `screenshot` tool.
- The image comes after the outcome's text in the result's content, so a client that reads only the first block still gets the outcome. Its metadata is the `screenshot` field. A failed capture adds `screenshot_error` with the error's name and detail and leaves the outcome as it is; an output niri can't name as focused counts as an `upstream_error` there.
- The `screenshot_ref` that plan §6 mentions arrives with the pointer tools in M4, like the ref store.

## 2026-10-08: M3's nested acceptance

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | The harness's fixture app, `harness window`, connects to the nested niri and maps toplevels. Same version the `vpointer` probe uses. |
| `wayland-protocols` | 0.32.13 | 2026-06-19 | The `xdg_wm_base` bindings for those toplevels. Feature `client` only. |

- `rustix` (same version, 1.1.5) gains the harness features `fs`, for the fixture's `memfd` buffer, and `event`, for polling its connection with a deadline.
- The crates the two pull into `Cargo.lock` are the ones already vetted for the probe, each the newest release at least 7 days old that its dependents allow, rechecked on crates.io today: `wayland-backend` 0.3.17 (2026-08-14), `wayland-scanner` 0.31.11 (2026-07-22), `wayland-sys` 0.31.11 (2026-03-31), `smallvec` 1.16.2 (2026-09-25), `pkg-config` 0.3.34 (2026-08-14), `quick-xml` 0.41.0 (2026-06-29) and `downcast-rs` 1.2.1 (2024-04-07). `quick-xml` 0.42.0 and `downcast-rs` 2.x are outside the ranges `wayland-scanner` and `wayland-backend` accept. They are dev-only: the server binary doesn't depend on them.
- A fixture app of our own instead of installed apps: the checks need an `app_id` set after mapping, two windows from one start, a window that ignores close requests, and a start delayed long enough to move focus during the wait. No app on the machine does all of these, and none would be reproducible elsewhere. The fixture's buffer is never written, so it needs no `mmap`.
- The fixture is the harness binary itself, started by niri from presets whose program is its absolute path. The policy rules allow it: `harness` is neither a command runner nor a terminal. It checks its environment is the nested one before connecting, exits when niri goes away, and has its own 90-second deadline, so the run's leftover check stays clean.
- The harness talks MCP to one long-running server instead of the one-shot `printf` pipelines of M2's checks, because the lease has to stay with one server across many calls and the `interrupted` check moves focus while a call is in flight. The runner's `Process::send` writes to a held stdin without closing it, within the process's deadline, and replies are read back from the server's log file.


## 2026-10-08: the pointer tools

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | The virtual pointer's Wayland connection to niri. The harness already uses it; newest release, rechecked on crates.io today. |
| `wayland-protocols-wlr` | 0.3.12 | 2026-03-31 | The `zwlr_virtual_pointer_v1` bindings, as in the `vpointer` probe. Feature `client` only. Newest release. |

- The only new crate in `Cargo.lock` is `wayland-protocols-wlr`; everything it pulls in was vetted for the harness. `rustix` gains the `time` feature for the monotonic clock, and Tokio's `net` feature, already on, provides `AsyncFd`.
- The pointer lives under `niri/`: its Wayland connection talks to niri too. It connects to the display itself rather than through `connect_to_env`, which reads the process environment that `main` already read once, and it checks the socket's peer PID against the one serving `NIRI_SOCKET`, so a stale or foreign `WAYLAND_DISPLAY` can't receive input meant for this niri.
- The pointer is async: `AsyncFd` watches a copy of the connection's socket, and each round trip uses `prepare_read`, a readiness wait with a deadline, and `dispatch_pending`. A blocking round trip would stall the single-threaded server, including the stop watcher.
- One pointer per gesture, created after every check passed and destroyed at its end. A long-lived device would need its own cleanup on release, stop and output changes, and buys nothing at one action per observation.
- `observed` for input is `sent`: niri handled the input (a `wl_display.sync` round trip after the last step), and nothing more is claimed. Plan §6 called this `verified: false`; an outcome name keeps the audit log's `observed` field meaningful without a second field. A failure after the first step is `uncertain` with `accepted: null`, as for a lost niri reply.
- Only `click` and `drag` write the input-dirty marker. A motion or a wheel turn leaves nothing held, and the marker would block every server for nothing if the server died between writing and removing it. The pointer's marker stays `pending`: there is no child process for `running` to name, and its `buttons` field says what to release.
- Plan §6 lists refusals; the order here is: arguments, unknown ref, the deny list, untested outputs, then the ref against the outputs just requested (expired, reconnected stream, changed output, out of bounds). A disconnect of the event stream drops every ref (plan §7); the ref keeps the connection number from capture and is refused as `unknown_ref` when it differs, instead of a store cleared from the stream task.
- `click` takes `count` from 1 to 3 with no pause between clicks, so a double click arrives within any app's double-click time. `drag` waits 50 ms before and after the press and moves in ten steps 20 ms apart, so toolkits that start a drag after a motion threshold see one. `scroll` takes at most 10 notches per axis per call, so a mistaken argument can't scroll a page away.
- The live setups (plan §8) are both enabled in code: the nested `winit` output, which the nested acceptance tests, and one monitor at `Normal`, which the supervised real-session run on DP-1 passed on 2026-10-09 (`docs/results/m4.md`).
- `recover` sends the release of the marker's buttons from a fresh virtual pointer (plan §11 step 3), bound to niri's first enabled output: a release needs no position. C8 verified this for the left button (272); the right and middle buttons are released the same way but remain runtime-unverified. A marker with buttons and no child skips the `wtype` scan, which only fits a keyboard marker. If the release can't be sent, the human is asked to press and release the buttons, as before, and the confirmation is still required either way.

## 2026-10-09: the keyboard tools

- No new crate. `runner::gated` starts a child with a held stdin pipe; `wtype` is the only user.
- `expect` is required and takes `{"window_id": …}`, `{"app_id": …}` or the string `"none"`, plan §6's `{window_id}`, `{app_id}` and `{none}` written as JSON an agent can't leave out by accident.
- `combo` is modifiers and one keysym name joined by `+`. The key must be letters, digits and `_`, which every XKB keysym name is, so a symbol like `/` is spelled `slash` and nothing can be read as a wtype option. `super`, `logo` and `win` all mean wtype's `logo`.
- The wtype call runs in a spawned task that owns the child and the marker. Plan §11 lets a running wtype finish within its deadline rather than killing it on a stop; the action gate drops the tool's work on a stop or cancel, so the child can't live in that work. The task removes the marker when wtype exits by itself, whatever its exit code: wtype checks its arguments and connects before it types, and it releases each key it presses. A wtype ended by a signal, or killed at the deadline, leaves the marker.
- If wtype's PID or start time can't be recorded, wtype is killed while it still waits at the gate and the marker is removed: nothing could have been typed.
- `interrupted` for the keyboard tools means focus left the window that had it at any event between the check and wtype's exit, as the plan's before-and-after comparison, but without missing a focus change that came back.
- `observed` is `sent`, as for the pointer tools (2026-10-08), plus `focus`: `matched` or `unchecked`.
- `key` and `type_text` carry `destructiveHint`, like `click` and `drag`: a keystroke can delete or send anything. Plan §6 names only `close_window`; the hint tells clients the truth about input.

## 2026-10-09: M4 pointer review

- A pointer failure before any step reached niri's socket is now an error, not `uncertain`: nothing can have happened (review finding).
- A marker that can't be removed after a gesture niri handled is an `upstream_error` saying so, not `uncertain` (review finding).
- A gesture dropped midway sends its releases, then a spawned task waits for niri's `wl_display.sync` reply before removing the marker. `Drop` can't wait, and a flush only puts the release in the socket buffer (review finding). Until the task finishes, other servers see `recovery_required` for a moment, which errs on the safe side.
- The pointer marker records its output, and `recover` binds its fresh pointer there when that output is still enabled (review finding). niri ignores the bound output for a button, so this only makes the record exact.
- No protocol test drives a successful gesture: the fake niri has no Wayland display, and a fake Wayland compositor would test our own fake. The nested run checks the marker after each click and drag, and the stop checks cover the release on drop.
- `click`, `drag`, `key` and `type_text` carry `destructiveHint`; plan §6 names only `close_window`, but a click or a keystroke can delete or send anything.

## 2026-10-09: the `vpointer` probe is gone

- Plan §14 deletes the probes once real code replaces them, by the end of M4. The server's pointer replaces `vpointer`: its mapping is `coords.rs`, and `make nested-input` checks accuracy, clicks, a drag and the wheel's frames through the server, at scale 1 and 1.5. `make nested` no longer runs C4, the probe's click and C12; their M0 results stay in `docs/results/m0.md`.
- C8, the interrupted pointer in `make sitting`, needed the probe's `hold`, so the sitting runs C6, C7 and C9, and `--sitting-from-c8`, which only resumed C8 and C9, is gone. C8's automatic half, a button left down by a killed pointer and cleared by a fresh pointer's release, now runs against the real server in `make nested-input`'s crash check; its human half was recorded in M0.
- `noctalia-socket` stays until M5: it drives C13's panel commands, and the server's replacement for those, `shell_open` and `shell_close`, arrives in M5. A plan correction (revision 19) moves its deletion there.
- The C10 corpus is a test fixture, not a probe; it moves to `harness/corpus.txt`.

## 2026-10-09: M4 keyboard review

- wtype runs with `LC_ALL=C.UTF-8`. It decodes stdin with `setlocale(LC_CTYPE, "")` and `mbstowcs`, and in a non-UTF-8 locale it stops at the first byte above ASCII, types what it had and exits 0, so a server started with a trimmed environment would report `sent` for half a text (review finding). glibc has `C.UTF-8` built in.
- A broken pipe on wtype's stdin means wtype exited before reading, which with `-` first means before typing anything; the runner ignores that write error and reports wtype's exit status and stderr, and the marker comes off (review finding).
- A gesture that failed before its press, and was dropped, removes its marker at once instead of waiting for a sync on a connection that may be gone (review finding).
- When the event stream is lost after input was sent, the result is `uncertain` with `accepted: true`, as for the action tools, rather than `sent` with a stale focus (review finding).
- The protocol tests drive the keyboard path through a fake `wtype` on the fixture's `PATH`: its arguments, its stdin and locale, the marker in its `running` phase while it runs, and the marker kept after a kill or the deadline (review finding).
- A pixel inside the image that maps off the output stays `out_of_bounds`, with the distance in the detail, rather than a reason of its own: `observe` only captures rectangles inside one output, so it can't happen today, and the agent's remedy is the same.

## 2026-10-09: the shell tools

- No new crate. `shell_open` and `shell_close` use the server's Noctalia client from M1, which now sends three kinds of command: `status`, `panel-open <id>` and `panel-close <id>`.
- The panel allowlist is `policy::Panel`, an enum of the three panels plan §6.1 allows. Plan §6.1 asks the payload builder to reject `\x1e` and newlines; with commands built only from the enum's fixed words there is nothing to reject, so a unit test checks every payload instead.
- `panel` is a plain string in the schema, so a refused panel reaches the server and comes back as `panel_not_allowed`, the stable name plan §6.2 promises, rather than as a schema mistake. The check runs after the action gate, as `launch` checks its preset.
- `Unanswered`, the refused-or-lost split for niri requests, moves to `error.rs` and also describes a panel command: a failed connect or Noctalia's `error:` reply is an error, and a reply lost after the connect is `uncertain` with `accepted: null`, because Noctalia carries the command out before it replies (`PanelManager::registerIpc` in 5.2.1).
- The tools read `status` before sending, which is plan §6.1's "checked before every Noctalia-backed call", and send nothing when the panel is already open, or already not open, reporting `accepted: false` as the focus tools do.
- `opened` is a new `observed` value; `closed` now also means a panel closed. `shell_close` counts another panel being open as closed, since `activePanelId` names only one.
- Results carry `shell.active_panel`, absent when it is unknown: after a lost reply, or a timeout before any `status` read answered, and `focused_window` from niri's event stream, which an open panel leaves null.
- They have no `interrupted`: Noctalia sends no events, and plan §6 defines `interrupted` by window focus.
- They take the lock gate like every action, before Noctalia is asked. Where only Noctalia can say whether the screen is locked, as in the nested session, a stopped Noctalia makes the lock state unknown, so they answer `screen_locked`, and `shell_status` answers `noctalia_unavailable`.

## 2026-10-09: C13 through the server, and M5's nested acceptance

- `make nested NOCTALIA=1` drives C13 through `shell_status`, `shell_open` and `shell_close`, so the `noctalia-socket` probe is deleted, and with it `probes/` (plan revision 19). C13's rule, `activePanelId` seen within two seconds of each command, is the server's observation plus a check that each call returned within two seconds; the server's own wait starts only after Noctalia's reply. The capture checks stay in the harness. Before each panel call the harness still checks that Noctalia's socket resolves under `TEST_DIR/run`, which it used to check before handing the socket to the probe.
- The nested Noctalia's wallpaper directory is an empty `TEST_DIR/data/wallpapers`. With the default, the wallpaper panel lists the pictures directory of the host user, whose `HOME` the nested session keeps, and its screenshot would land in the run's artifacts.
- `make nested-shell` is M5's acceptance: the three panels, the refused ones, the Noctalia lock source, Noctalia stopped and Noctalia absent from `PATH`. The absent case runs a second server under `env PATH=<empty directory>`, since `PATH` decides the tool list.
- The tray drawer gets no pixel check: with no tray items it is one icon wide (`TrayDrawerPanel::preferredWidth` in 5.2.1), under the 1% a drawn panel must change, while the bar's clock alone can change a few hundred pixels.
- The OCR helper looks for `Home`, the control center's title on open (`control-center.tabs.home`, `control_center_panel.cpp`), in English, which is the `LANG` the harness keeps from the host here. The first runs skipped it because tesseract wasn't installed; with tesseract 5.5.3 it found the word (`docs/results/m5.md`).

## 2026-10-09: agent guidance and skill evals

- A session transcript showed an agent breaking rules the skill stated: screenshots sent alongside actions, text split by hand with Enter pressed after a failed part, apps started by typing into a terminal, focus left on the wrong window. The rules an agent needs at the moment of the call now sit in the tool descriptions and the server's `instructions`, which every client shows; the skill keeps the workflow and the reasons, and its tables move to `references/`.
- `type_text` takes up to 1000 characters and types them in parts of 100, one `wtype` call each, checking focus between parts. Splitting is the server's job because the agent got it wrong; parts keep each `wtype` well inside its three-second deadline. 1000 bounds one call to about thirty seconds. A result that stops early carries `typed`, and a failed part says in its detail how much went out, so an agent can tell a half-typed message from a whole one.
- `screenshot` waits for the action mutex before capturing. A screenshot sent in parallel with an action otherwise showed the screen before it, and agents read it as the action's effect. Evidence screenshots are taken inside the action and don't wait.
- `release_desktop` doesn't put focus back by itself. Many tasks end with another window in front on purpose, such as "open the notes for me", and the server can't tell which. The tool description and the skill tell the agent to do it with `focus_window`.
- `make nested-eval` runs `claude -p` inside the nested session against one scenario, with only the nested server (`--strict-mcp-config`), only the `Skill` and `Read` tools, and only project settings, so the agent has no shell and no host MCP servers. The skill under test is copied into the run's `TEST_DIR/agent/.claude/skills/`. Grading reads the nested server's audit log, which has every call's timing, outcome and `text_len`, and the scenario's fixture logs. The one write outside `TEST_DIR` is Claude Code's own transcript under `~/.claude/projects/`, which `claude -p` always writes; the run keeps its own copy of the stream as `transcript.jsonl`.
- The scenarios come from failures seen in a real session's transcript: a long message sent with one Enter, four keys with a screenshot after each, a click in another window with focus returned, a stopped server, and an app without a preset. Results go in `grading.json` and `timing.json` in the skill-creator's format, so its benchmark and viewer can compare skill versions.

## 2026-10-09: fewer turns per task

- About 98% of an eval run's wall time is the agent's own turns; the server's calls take milliseconds. So speed comes from fewer calls per task, not faster calls.
- Every action takes `screenshot: true`, which returns a screenshot of the focused output with the result. It is taken inside the action, under the action mutex, so no other action can start before it. It waits for the screen to stop changing: a first look after 50 ms, then a capture at least 100 ms after the previous one until two are the same bytes, for at most 1.5 seconds. 100 ms is several frames at 60 Hz, so two identical captures mean the app stopped redrawing rather than caught between frames; 1.5 seconds bounds a screen that never stops, such as a video, and `settled: false` says so. Comparing bytes needs no image decoding, and grim encodes the same pixels to the same bytes.
- `type_text` takes `submit: true`, which presses `Return` only once the whole text went out, and reports `submitted`. Pressing Enter after a call that stopped early was the worst mistake seen in transcripts, and with `submit` the server makes that call.
- `key` takes `keys`, a list of up to 16 combinations, pressed one `wtype` call each with the same focus check between them as `type_text`'s parts. Every combination is parsed before the first is pressed, so a typo presses nothing. 16 covers menu walks and form tabbing without letting one call run long.
- `wait_for` replaces polling with screenshots: a window appearing, closing or changing its title, from niri's event stream, or the screen stopping changing. It reads only, so it needs no lease and doesn't take the action mutex, except that `screen_stable` waits for a running action first. Up to 30 seconds, default 10: long enough for an app to start, short enough that the agent stays in control. Titles can hold private text, so the audit log keeps only their length.
- `acquire_desktop` keeps the window the user was on, and `release_desktop` takes a required `restore_focus`. It replaces the earlier rule that the agent puts focus back with `focus_window` itself, which needed the agent to remember the window across the whole task and was the step most often missed. The argument is required, not defaulted, because whether to leave another window in front is the task's decision and the agent must make it each time. Restoring runs through the action gate, so a stop or a lock still refuses it, and the lease is given up whatever the outcome.
- The evals get harder: `dialog-midway` opens a window that takes focus as soon as the agent takes the lease, before its first action, and `errand` chains a launch, a message, a close and the return to the user's window. Every scenario also checks, from the agent's transcript, that no action was sent in the same turn as another desktop call, which the audit log alone can't see when the server ran them one after the other. `timing.json` adds the number of calls and of agent turns.

## 2026-10-09: M6.1 headless acceptance and application activation

- Automatic nested runs use the already-installed cage 0.3.1, the latest stable release listed upstream, published 2026-06-30T18:51:48Z (GitHub's `/repos/cage-kiosk/cage/releases/tags/v0.3.1` release metadata). It is more than seven days old. No package or Cargo dependency was installed or upgraded.
- Cage runs with an exact environment, `WLR_BACKENDS=headless`, `WLR_LIBINPUT_NO_DEVICES=1` and a short private runtime directory. It has runner deadlines and process-group cleanup. Client decorations are disabled to avoid consuming the headless output's dimensions. Initial M6.1 runs omitted host snapshots in this mode; the review follow-up below restores C1. Visible runs remain explicit for human sittings.
- M6.1 replaces M6's wait-and-drop screenshot synchronization with an observation held under the action mutex. It also changes settling to a wall-clock budget including capture work, and derives pointer space from the ceiled physical mode rather than truncated IPC dimensions. The older M6 entries describe the baseline, not these corrected guarantees.
- The application activation fixture uses installed Python 3, PyGObject and GTK 4 only when available. They are optional development tools, not server dependencies or pinned runtime requirements. Missing GTK produces an explicit skip. The fixture observes the actual button callback and its counter, not only pointer events; it does not cover every toolkit or gesture.

## 2026-10-09: review follow-up settle budgets

- Keep `wait_for`'s 100 ms minimum for compatibility and short event-condition waits. It is not a screenshot latency guarantee: settling starts samples after 50 ms and at least 100 ms apart, so stability needs at least the 150 ms second-sample start plus capture time. The recorded roughly 100 ms downscaled capture on a 2560-wide output can exhaust a 100 ms budget without an image. Callers needing visual evidence must choose a larger budget; a timed-out wait is an honest result, not a reason to raise every event wait's minimum.
- A pending capture is cancelled at its deadline, but a frame that already completed when the runtime resumes is retained even at or after that deadline, with `settled: false`. Only a timeout without any completed sample is a missing-frame error. The continuation deadline is checked at one point in the settle loop; the existing 50/150/250/350 ms sampling cadence is unchanged.

## 2026-10-09: C1 remains a headless contamination backstop

- The review explicitly authorizes read-only host metadata snapshots in headless mode. C1 now brackets the whole cage/nested lifecycle, including failure cleanup, in both modes. It reads niri outputs, host Noctalia `status` only, runtime/X11 entry names and dconf mtime. It never reads host pixels, clipboard contents or window titles and never sends input or other Noctalia commands.
- These metadata checks request no input lease and do not unlock the desktop; the headless control suite passed with C1 reporting unchanged host state. C1 is conservative while the host is in use: unrelated output, lock/panel, socket or dconf changes can fail the run, and a diff does not attribute causation to the harness. This limitation is preferable to silently disabling the backstop. Snapshot reads keep their runner deadlines; tests use disposable fake snapshots, not the real host.

## 2026-10-09: experimental native keyboard

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-protocols-misc` | 0.3.12 | 2026-03-31 | Upstream zwp_virtual_keyboard_v1 bindings; client only, compatible with existing wayland-client 0.31.15. |
| `xkbcommon` | 0.9.0 | 2025-08-09 | Safe libxkbcommon API for resolving the compositor's expanded XKB map. Default features disabled; uses installed libxkbcommon, not a generated layout. |
| `xkeysym` (transitive) | 0.2.1 | 2024-06-07 | Keysym/keycode types required by xkbcommon 0.9.0. |

- Verified latest stable versions and publish dates from crates.io API on 2026-10-09; all are older than seven days. No other dependency changed. xkbcommon requires libxkbcommon at build/runtime; pkg-config reported installed libxkbcommon 1.13.2. Upstream's `xkbcommon-1.13.2` GitHub release was published 2026-05-30T08:38:28Z and is the newest stable release listed upstream (1.14.0-beta1 is not stable). Its existing installed API passed compilation and nested tests with the 0.9.0 Rust wrapper. No package installation. Direct crates.io API checks also confirmed all three crate releases are non-yanked and the newest stable eligible versions at the 2026-10-02 cutoff; the web search could not retrieve those API dates.
- `NIRI_COMPUTER_USE_KEYBOARD=native` is experimental and explicit. Absent or `wtype` selects the existing gated wtype path. Invalid selection refuses input. No automatic fallback. Native preflights the entire request against the active layout, sends complete press/release pairs and checks focus/layout and stop before each key, with a bounded Wayland round trip per pair.
- A character absent as a direct symbol refuses the request before typing. Compose/dead-key text and arbitrary Unicode are not implemented; plain US cannot type the existing UTF-8 corpus. This blocks default promotion. Physical modifier state is not available to an unfocused wl_keyboard subscriber: native cannot yet promise preservation of physical modifiers. It must only be used in an isolated session without physical input.
- Normal, stop and cancellation explicitly release synthetic state; marker removal requires an acknowledgement. SIGKILL cannot run cleanup: a native marker records conservative evdev release codes and layout (never text); recover sends those releases and zero modifiers from a fresh keyboard and still requires human confirmation. Immediate crash release is not guaranteed, and remains a promotion blocker. wtype's existing recovery behavior is unchanged.
- Keymap unit tests use a committed, expanded, controlled XKB fixture with no environment names or system includes, not the host's layout configuration. Shared peer checking and deadline-bound Wayland dispatch live in niri/ for both native devices.

## 2026-10-10: native text beyond the active layout

- M0's C6 and C7 were about wtype's own keymap swap: wtype uploads a generated map, niri sends it to every `wl_keyboard` with the first key, and only a physical key brings the compositor's map back. niri 26.04's Smithay (`virtual_keyboard_handle.rs`, `send_keymap` at `ff5fa7df`) sends a virtual keyboard's map when its id, a hash of the serialized text, differs from the active one, and sends it to every known `wl_keyboard`.
- This replaces the 2026-10-09 refusal of every missing symbol. Native keeps the compositor's map whenever the text fits the layout, and otherwise uploads, for the call only, the compositor's map with the missing symbols on keys that have none. Existing keys keep their keycodes, symbols and level modifiers, so a key pressed physically during the call means the same thing, and recover's codes stay meaningful. The extension is libxkbcommon-checked, then serialized by libxkbcommon so it matches the text niri re-serializes and sends.
- Spare keys are limited to keycodes up to 255, because Xwayland clients cannot receive higher ones. A US evdev map has 14. More distinct missing symbols refuse the whole request rather than rotate maps within one call; splitting the text works. Missing control characters and `key` keysyms still refuse.
- Restoration is proved, not assumed: the device uploads the compositor's map again and sends zero modifiers, which makes niri send that map, and the call fails unless the device's own `wl_keyboard` then received it byte for byte. The equal text means equal id, so niri's active map is the compositor's again and later keys send nothing. Any map other than the compositor's that niri sends after the upload needs this proof, even one that differs from the extension only in serialization. A failed confirmation fails the call and keeps the marker; a cancelled call clears its marker only on the same proof. A SIGKILLed call leaves the extended map in clients until recover, whose fresh keyboard uploads the compositor's map before releasing, so no physical key is needed, unlike wtype. The nested suite checks both with an independent client that saves every map's contents.
- Keymap broadcasts that are neither the compositor's map nor the device's extension still count as compositor keymap changes and end the call.
- Application-level native text uses the existing optional GTK fixture, now one file with a `button` and an `entry` mode: `python3 -I` keeps a shared helper module off `sys.path`, so a second file would have copied the nested-session guard. The entry reports its own text, so the check reads what GTK inserted, not what wev decoded.

## 2026-10-10: crash guardian

- A SIGKILLed server can't release what it holds, and niri releases nothing when a virtual device goes. Until now only a human's `recover` sent the releases, so a key, modifier or button stayed down, and an extended keymap stayed in clients, until someone noticed.
- `serve` starts `niri-computer-use guard <pid>` through the runner, with a pipe as stdin whose only write end the server holds. End of file is the server's death, whatever kills it; no polling, no pidfd, no signal handler, and nothing to clean up on a normal exit. A separate process is needed because nothing inside a SIGKILLed process runs. Its own process group keeps a signal to the server's group from killing it too.
- It acts only on a marker its own server wrote naming native keycodes or pointer buttons, with the same fresh-device releases and peer-PID check `recover` uses, under the lease and within one deadline. A `wtype` child finishes by itself, so its marker stays `recover`'s alone.
- The marker stays after the guardian's releases, with their time in `released`. Releasing keys can't undo what the input did to an application, so a human still confirms through `recover` before any server acts again; the lease, the resume gate and `recovery_required` are unchanged.
- No dependency was added.


## 2026-10-10: clipboard-preserving paste

- `paste` talks wlr data-control itself instead of running `wl-copy` and `wl-paste`. `wl-copy` offers one MIME type, so it can't put back a selection that offered several, and neither tool says when another client reads what it serves, which is how `paste` knows the target took the text. The bindings come from `wayland-protocols-wlr` 0.3.12, already a dependency for the virtual pointer; no dependency was added. `clipboard_read` keeps `wl-paste`.
- The paste combination is a required argument, `ctrl+v`, `ctrl+shift+v` or `shift+Insert`, not a guess from the `app_id` or a policy table. The agent sees the app, and a wrong guess pastes nothing or presses an app's own binding.
- Nothing is left to the clipboard's next reader: the saved selection must fit in 16 MiB and arrive within two seconds, or `paste` refuses with the new error name `clipboard_unsaved` before changing anything. A selection whose owner set `x-kde-passwordManagerHint` to `secret` is refused too: a password manager clears its own selection after a while only while it still owns it, and after a restore the keeper would own it indefinitely.
- The restored selection has to be served by someone after the call, so a keeper process holds it, our own binary through the runner, not killed with the server, like the crash guardian. Serving has no deadline, as with `wl-copy`; each transfer has one, and the keeper ends when another client takes the selection or niri goes away. Its stdin tells it when the key went out, or, by its end, that the call is over, however it ended, so it restores at once rather than holding the text. Superseded for the end of stdin after `k`: see "the paste keeper's commands and the key's admission" below.
- A read proves only that some client asked for the text after the key: a `send` names no client. The keeper offers the text with `x-kde-passwordManagerHint: secret` so clipboard managers that honour it don't read or keep it; Noctalia's does, by the log line its binary has ("ignoring clipboard selection: password-hint MIME advertised").
- In the nested session Noctalia's clipboard service adopts the last selection whenever the clipboard goes empty, so `wl-copy --clear` can't empty it once something was copied. The empty-clipboard check therefore runs before the suite's first copy.
- The harness's clipboard owner uses ext-data-control, through the `staging` feature of the harness's existing `wayland-protocols` 0.32.13, so the nested test also shows the two data-control protocols share one selection. No crate was added.

## 2026-10-10: saving screenshots

- `screenshot` writes a file only when the policy file sets `capture_dir`. Agents without a shell otherwise have no way to write files, so it is opt-in and off by default.
- The saved PNG is its own capture at the output's scale rather than the returned image re-encoded, because downscaling happens in grim and the server decodes no images. That costs one more grim call (about 14 to 100 ms on a 2560-wide output, see Screenshots) and lets the two images differ by a few milliseconds.
- The path is confined with `openat` and `O_NOFOLLOW` through the existing `rustix` `fs` feature, not by canonicalizing and comparing prefixes, which a symlink swapped in between could get past. No new dependency.
- `screenshot` keeps `read_only_hint: true`: the hint describes the desktop, and a save only adds a new file in a directory the user chose.

## 2026-10-10: zbus for the accessibility bus

- `elements` and `element:` read AT-SPI over D-Bus, which needs a D-Bus client. Spawning `busctl` per call would cost a process for each of the up to 2000 objects a walk reads, so the server keeps one connection to the accessibility bus instead.
- `zbus` 5.19.0, published 2026-08-09, the newest stable release at least 7 days old. MIT, MSRV 1.87. `default-features = false` with only the `tokio` feature: it runs on the server's runtime with no thread of its own, and `cargo tree -e features -i zbus` resolves no other feature (no `p2p`, no `async-io`).
- Not `atspi` 0.30.0 (2026-05-06). Its `atspi-connection` defaults to `p2p`, whose connect awaits every application on the bus in turn with no deadline, so one stopped app hangs it (the research report measured this). Its `atspi-proxies` 0.14.0 always compiles in zbus's `async-io`, even with `tokio`. It would add 48 crates to the lock file, against 26 for zbus alone.
- The calls are hand-written with `Connection::call_method`, not zbus proxies: a proxy caches properties and adds match rules, which means more round trips and no per-call deadline. Each call has a 1 s deadline and each request a 3 s budget. Only the application whose PID is niri's window's PID is called; the registry and the bus answer the rest.
- `async-recursion` (a zbus dependency) is held at 1.1.1: 1.2.0 came out on 2026-10-03, less than 7 days ago. `toml_edit` resolves to 0.25.15 (2026-09-11) for the same reason.
- Cost: `Cargo.lock` grows from 112 to 138 packages, and the server's normal dependency tree on Linux from 83 to 106 crates. The stripped release binary grows from 4,422,464 to 6,279,608 bytes.

## 2026-10-10: elements

- Role and state names are AT-SPI's own (`atspi-constants.h`) in snake case, such as `push_button_menu` or `check_box`, so they match what other AT-SPI tools show. An unknown `role` argument is an argument mistake.
- An element's place is `output origin + tile position + window offset in the tile + its WINDOW-relative extents`. It is trusted only when the app's accessible frame is niri's window size, within a pixel. A frame of another size, as with client-side decorations that draw shadows, makes every element of that window `unmappable: frame_size_mismatch` rather than a guess.
- New error names: `not_accessible` when the window's application has no accessible window for it, and `ambiguous_window` when several of its windows fit.
- A walk that runs out of the three-second budget returns what it read, possibly nothing, with `capped: true` and `capped_reason: budget_exhausted`, instead of `deadline_exceeded` with nothing. Each object costs six D-Bus calls, so a browser or office document's tree ran out of time long before the 2000-object cap and lost everything already read. Only the whole budget counts: one call's own one-second deadline, as for a hung app, stays `deadline_exceeded`, and a bus failure stays `upstream_error`, wherever in the walk they happen (Sol's design check: a first-node failure doesn't prove a hung app, since finding the app and frame spend the same budget). The full match predicate (showing, the name-or-action rule, `role`, `name_contains`) goes into the walk, which stops once it found one more match than `limit`, so `truncated` means a further match was found. It keeps walking through showing elements that don't match, and the 2000-object cap still bounds it. Added after Fable's review (B3).

## 2026-10-10: aiming at elements

- A pointer tool given `element` checks it again right before acting, with no retry: the window still exists with the same process, one round of calls to the element answers within its budget, its role is unchanged, it is still showing, the frame still fits, and its centre, from niri's geometry now, lies inside the screenshot. New error names: `element_stale` and `element_unmappable`.
- `screenshot_ref` stays required with `element`: the screenshot names the output the virtual pointer binds to and the rectangle the point must fall in, so an element ref never aims at something the agent hasn't seen.
- A drag interpolates in layout coordinates, so a drag between an element and a pixel, or two elements, needs no image pixel in between. `scroll` keeps pixels only.
- Element refs belong to the lease, as screenshot refs do, and the last 1000 are kept: a walk lists up to 500 elements, so two lists' worth stay usable.

## 2026-10-10: the docs site for agents

- No new npm dependencies. `llms.txt`, `llms-full.txt`, a `.md` address for every page and the skill under `skill/` are Astro static endpoints in `site/src/pages/`, built from the docs collection and from `skills/niri-computer-use/`. Starlight plugins for llms.txt would add a dependency for about a hundred lines of code, and the published skill must come from the one copy in `skills/`, not a second one in `site/`.
- The sidebar and the agent files take their groups from `site/src/lib/groups.mjs`, so both list the same pages in the same order.
- The Markdown versions turn relative links into absolute ones and images into their alt text, so they read correctly outside the site.
- `pages.yml` also deploys on changes to `skills/`, since the site publishes the skill.

## 2026-10-10: checks in the site build

- `npm run build` runs `site/scripts/check-reference.mjs` before Astro and `site/scripts/check-links.mjs` after it, so CI and the Pages deploy both fail on a broken page.
- `check-reference.mjs` reads `ErrorName` in `src/error.rs` and the `Policy` and `Preset` structs in `src/policy.rs`, and fails when the error reference or the configuration page documents a name the source doesn't have, or misses one it has.
- `check-links.mjs` resolves every internal link and image in the built HTML, and every link to the site in the Markdown and llms files, to a file in `dist/`, checks `#fragment`s against the target's element ids, and checks that `llms.txt` links every page's Markdown and `llms-full.txt` contains it. starlight-links-validator would do the HTML part as a dependency; it doesn't check the agent files.
- The old addresses `guides/getting-started/` and `reference/tools/` redirect to `start/install/` and `tools/overview/` through Astro's `redirects`.

## 2026-10-10: finding the session without client configuration

- A client should need no configuration beyond the command. Codex forwards only a short allow-list of variables, so its users had to add `env_vars` for `NIRI_SOCKET`, `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`. The server now discovers whichever of the three is missing, from the names logind and niri use: `/run/user/<euid>`, and `niri.<display>.<pid>.sock` (`IpcServer::start` in niri v26.04 `src/ipc/server.rs`).
- Set variables always win, so a nested niri, a test fixture or a second session is chosen by setting them. They are also constraints: what discovery fills in must belong to the same niri, or the server refuses with a detail naming the variable to set. It never falls back to another session.
- Without the variables, a server started over SSH, from a TTY or as a systemd service attaches to the user's only niri session. That is intended: requiring the server's login session to be niri's would also rule out services started on purpose. The lease, the stop flag and the lock check still apply, but none of them shows the user asked, and the client setup page says so.
- Discovery never chooses between niri instances. A socket counts only if it is a socket owned by the effective user, its PID is a running process called `niri`, and a connection to it, within 500 ms, has that PID as its peer (`SO_PEERCRED`). The PID check alone isn't enough: a crashed niri leaves its socket behind, and once a new niri reuses its PID, the dead socket would look live and make discovery refuse the one usable niri as ambiguous, or pick a socket nobody listens on. Discovery deletes no socket. Several live ones make niri `niri_unavailable` with their names, because driving the wrong session is worse than asking for `NIRI_SOCKET`.
- The runtime directory must be owned by the effective user with mode `0700`, so a directory someone else made can't steer the server to their sockets.
- A given `NIRI_SOCKET` names the runtime directory when `XDG_RUNTIME_DIR` is missing. niri binds its socket in `socket_dir()`, its own runtime directory (niri v26.04 `src/ipc/server.rs`), and the lease, stop flag and input-dirty marker live in the runtime directory, so taking `/run/user/<euid>` for a nested niri would let a second server hold a lease of its own and miss the stop flag. A socket directory that fails the ownership and mode checks leaves the runtime directory missing rather than falling back.
- When both are given and the socket lies elsewhere, the server keeps them and `status` warns. niri only ever puts its socket in its runtime directory, but a symlink or another spelling of the same directory would look the same, so refusing could break a working setup.
- The protocol fixture refuses to unset `XDG_RUNTIME_DIR`, so no protocol test can discover the host's session. Unit tests pass discovery roots of their own.
- The runner gives every child the resolved variables. Setting them in the server's own environment would need `unsafe` (`std::env::set_var` in edition 2024), which the crate forbids, so `main` hands them to the runner once, in a `OnceLock`, before any child starts. The crash guardian and the paste keeper therefore read the same niri socket as the server, even if another niri starts meanwhile.
- No dependency was added: `rustix::process::geteuid` comes from the `process` feature already enabled.
- A given `WAYLAND_DISPLAY` limits discovery to niri sockets named for that display. Without it, a client that forwarded only the display got the one niri found in the runtime directory, even when that niri ran on another display.
- The display is checked against niri at each use, by the peer-PID check the server's own Wayland connections already make. `wtype`, `grim` and `wl-paste` reach the display by name, so a `WAYLAND_DISPLAY` of another compositor sent input there and read its pixels and clipboard while focus, policy and the lease were checked on niri. A mismatch is the stable error `session_mismatch`, and nothing replaces either variable.
- A check at startup alone wasn't enough: a server outlives a niri restart, for example one started over SSH or as a service, and another compositor can then take the display's name while the old proof stays cached. So right before each `wtype`, `grim` and `wl-paste` and before the paste keeper starts, the server connects to niri's socket and to the display, each within its deadline, and compares the peers. That is two local connects, so it costs nothing worth caching. Once niri is gone the check gives `niri_unavailable`. Native input, the pointer, held modifiers, the paste keeper and `recover` make the same check on the connection they then use, so for them no window is left. `status` reports the current check, not one from startup, and `stop` and `resume` never make it, so the stop flag never waits on a compositor.
- A window remains between the check and a child's own connect: the child opens the display by name a few milliseconds later, and a compositor that replaces the display in exactly that moment would get it. Closing it would mean handing the checked connection to the child as `WAYLAND_SOCKET`, which libwayland uses before `WAYLAND_DISPLAY`. The runner removes that variable from every child instead, so an inherited one can never skip the check, and a niri restart landing in those milliseconds isn't worth a second way to start children.
- The runner removes an inherited `WAYLAND_SOCKET` from every child. libwayland connects to that file descriptor before it reads `WAYLAND_DISPLAY`, so a child would skip the display the server checked.

## 2026-10-10: `niri_action` and the gated actions

- One tool takes any niri action in niri's own JSON form, `niri_ipc::Action`, instead of a tool per layout action. niri's actions are already a stable, documented vocabulary, and agents recording stills and demos needed fullscreen, floating and widths, which no tool had.
- The schema is a plain object, not niri-ipc's `json-schema` feature: that would add a feature and put a schema of 141 variants in every client's tool list.
- `policy::action_gate` sorts every variant with one exhaustive `match` and no wildcard arm, so an action a new niri-ipc adds fails to compile until someone decides it. The workspace also denies wildcard arms, so the window-target match in `act/compositor.rs` is exhaustive too. Both carry `#[expect(clippy::too_many_lines)]`, because splitting a match without a wildcard isn't possible.
- Gated, decided from niri 26.04's `niri-ipc/src/lib.rs` and refused with `unrestricted_required` unless `unrestricted = true`:
  - `Spawn`, `SpawnSh`: run any program.
  - `Quit`: ends the session.
  - `PowerOffMonitors`, `PowerOnMonitors`: DPMS, outside the layout.
  - `LoadConfigFile`: loads any path as niri's config, which can bind keys and start programs.
  - `Screenshot`, `ScreenshotScreen`, `ScreenshotWindow`: write to any absolute `path` and replace the clipboard. `screenshot` with `save_path` saves under `capture_dir` without either.
  - `ToggleKeyboardShortcutsInhibit`: changes whether niri's binds reach niri, and the stop key is a niri bind.
  - `SwitchLayout`: changes the layout the user types with after the lease. The native keyboard backend also builds on the active layout.
  - `SetDynamicCastWindow`, `SetDynamicCastMonitor`, `ClearDynamicCastTarget`, `StopCast`: change what a screencast shows or end it, which could show a window to a call.
  - `ToggleDebugTint`, `DebugToggleOpaqueRegions`, `DebugToggleDamage`: rendering debug state, not layout or focus; no agent needs them.
  - `DoScreenTransition`: niri renders the frozen frame on every output and screencast for `delay_ms`, up to 65.5 s with no clamp, while the agent could keep acting through `elements` and the keyboard. Rendering state like the debug toggles, and no agent task needs it. Added after the gate review.
- Everything else only changes niri's layout, focus or views and is allowed, including `CloseWindow`, `SetWorkspaceName`, the overview and urgency.
- `CloseWindow`, and the `close_window` tool, refuse a window whose app is on the deny list, named or focused, with `app_denied` (`policy::refuse_window`). Closing can end the user's password manager or settings window; focus and layout actions leave the app's content alone and stay unchecked. Added after the gate review.
- `CloseWindow` with a null id goes out with the id of the focused window that was checked, so focus moving to a denied window before niri handles it can't redirect the close. With no window focused nothing is sent, and the call is an argument mistake. From Sol's design check.
- For an action about one window, the result reports that window as niri's event stream shows it: within 1 s of the action, plus 200 ms for a resize that arrives in steps. niri 26.04's IPC has no fullscreen or maximized flag, so those show as sizes. A 1 s wait on an action that changes nothing is the cost.
- The gate is checked after the lease, stop and lock checks, so a refused action is audit-logged like any other.

## 2026-10-10: `unrestricted`, off by default

- One top-level key, `unrestricted = true`, instead of lists. Turning it on lets `niri_action` send the gated actions and lifts the preset rules: terminals with arguments, programs that run commands, and an `env` table.
- It is off by default. Agents without a shell, such as desktop apps, can't run programs except through this server, and with it off the server starts only what the user put in a preset. Turning it on means an agent can run any program through the server. The lease, the stop key and the audit log still apply to every call.
- `NIRI_COMPUTER_USE_UNRESTRICTED=1` turns it on as well, so a user can allow it for one MCP client in that client's server settings without a policy file. Either source enables it; nothing in the environment turns off a file's `true`. Any other value counts as off and is reported in `status.unrestricted.error`, so a typo such as `true` isn't silently ignored. It doesn't make the server read-only, because nothing gets looser.
- `status.unrestricted` reports `enabled`, `source` (`policy`, `env` or `both`) and `error`.
- A preset's `env` is applied by spawning `env -- NAME=value … argv` through niri, because niri 26.04's `Spawn` has no environment field. niri still starts it with the session's environment, plus these variables. Names must be non-empty and free of `=` and NUL, and with `env` the program can't contain `=`, which `env` would read as an assignment. Without `unrestricted`, a preset with `env` makes the file invalid.
- An agent never adds arguments or variables: `launch` still takes only a preset name.
- With `unrestricted` on and no preset for an app, the skill, `launch`'s description and the server instructions tell the agent to start it with `niri_action`'s `Spawn` and the program's argv, never a shell or `SpawnSh`, then `wait_for` its window by `app_id`. The user turned `unrestricted` on for this, and the earlier "ask for a preset" text kept a compliant agent from using it. A preset still comes first, and with `unrestricted` off the agent asks for one.

## 2026-10-10: the `noctalia` passthrough, and OBS through it

- With `unrestricted` on and Noctalia installed, the `noctalia` tool sends any Noctalia command: its arguments joined with spaces after the `/\x1e` prefix, exactly as Noctalia 5.2.1's `noctalia msg` builds it (`src/ipc/cli.cpp`, `src/ipc/ipc_client.cpp`). Only `notification-show`, which the CLI rewrites as JSON, differs.
- It uses the socket `noctalia.rs` already speaks, with the same 2 s deadline, not a `noctalia msg` subprocess. The socket gives Noctalia's reply, which is what the CLI prints. The CLI's exit status follows from the reply alone, 1 for a reply starting `error:`, which the tool returns as `upstream_error` with Noctalia's text, and the CLI writes nothing to stderr for a reply. So a subprocess would add a process per call and nothing else.
- Replies are cut at 64 KiB, with `truncated`. A lost reply is `uncertain`, because Noctalia carries a command out before replying.
- The tool is listed only when `unrestricted` is on, decided at startup like the other optional tools. Noctalia commands include the launcher, the session menu and plugins, which can run programs.
- Screen recording goes through the user's own Noctalia OBS plugin (`plugin ayagmar/obs-control:controller all toggle-record`), the command their niri bind runs, instead of a built-in recorder. That adds no runtime dependency, and the user's OBS scenes, encoder and output folder apply.

## 2026-10-10: one shared engine per niri instance

- `serve` in shared mode (`shared = true` in the policy file, or `NIRI_COMPUTER_USE_SHARED=1`) relays its client's MCP lines to one `niri-computer-use engine` per niri instance, over `engine.sock` in the instance's runtime directory. The engine holds what used to exist once per client: the desk, the crash guardian, the event stream, the accessibility connection and the audit writer. Shared mode is off by default; standalone `serve` is unchanged, and both kinds of server share one lease.
- No dependency was added. The bridge and the engine use tokio's Unix sockets and the crates already in the tree.
- What each setting belongs to:

  | Setting | Scope | Why |
  |---|---|---|
  | `NIRI_COMPUTER_USE_UNRESTRICTED` | session, from the hello | A per-client opt-in that another client must not inherit. |
  | The policy file | session: the bridge reads its own and sends the text, at most 64 KiB, or that it is missing or unreadable | A client sees policy edits made before it started and keeps its snapshot, as in standalone. An invalid file makes that session alone `read_only`. |
  | `NIRI_COMPUTER_USE_KEYBOARD` | session, from the hello | The backend applies per call, and only the lease owner types. |
  | `HOME`, for `capture_dir` | session, from the hello | `~/` resolves against the client's own home, as in standalone. |
  | `PATH` | engine | See below. |
  | `XDG_STATE_HOME` (the audit log), the session bus | engine, from the environment of the bridge that started it | They describe the user's session, not the client. |
  | `XDG_RUNTIME_DIR`, `NIRI_SOCKET`, `WAYLAND_DISPLAY` | must match | The hello carries the bridge's niri socket and Wayland socket; a mismatch is refused with `session_mismatch`, and the bridge serves standalone. |

  The engine never reads its own `NIRI_COMPUTER_USE_*` variables: a session's settings come only from its hello, so one client's opt-in can't leak into another's session.
- The hello carries no `PATH`. The engine runs `grim`, `wtype` and the other programs, so only its own `PATH`, the first bridge's, decides what it finds. A later bridge's `PATH` would change nothing, so it isn't compared or refused: refusing would push that client to standalone, where the same programs would be missing. The plan kept a copy for `status` to compare; that comparison wasn't built, so the field would be dead.
- The hello is one JSON line of at most 512 KiB, sent within two seconds of connecting: hello version 1, the identity of `/proc/self/exe` (device, inode, size, modification time), the niri and Wayland sockets, the session's settings above and its policy file. The same file means the same build, so there is no version file or build script. After a reinstall, new bridges meet the old engine's `engine_version` refusal and serve standalone until the old engine's clients leave and it exits; the next client starts a new one. Only a process of the same user (`SO_PEERCRED`) gets an answer, and the engine takes at most 64 connections (`engine_busy`).
- Ref ids carry an eight-hex-digit tag random for each process, `shot-<tag>-<n>` and `elem-<tag>-<n>`, from `std::hash::RandomState` (std only). A bridge that reconnects to a new engine, or a client that restarts its standalone server, could otherwise pass an id the new process issued itself for another capture. Agents treat ids as opaque strings.
- The engine exits after two seconds of continuous idleness: no connection, no lease held and no input cleanup pending. It closes and removes `engine.sock` first, still holding `engine.lock`. MCP clients stay connected for a whole agent session, so the grace only absorbs restarts and churn; an engine start costs one guardian, an event-stream connection and the accessibility lookup. It is fixed, with no setting.
- New error names, both from the bridge: `engine_lost` when the engine ends with calls in flight or since the session's last call, and `engine_unavailable` when, after that, no engine can be reached or started within five seconds. A bridge that is relaying never turns standalone, because the client's session state (lease, refs) would silently differ. The refusals in the hello (`engine_version`, `session_mismatch`, `engine_busy`, `bad_hello`) never reach an agent: at start the bridge serves standalone instead, and after an engine was lost they become `engine_unavailable` with the refusal as `detail`.
- The stop key's bound: the engine's one stop watcher cancels the running action whichever session runs it, non-owners never wait for the action mutex, and an owner's queued calls each fail at once. A stop can wait for at most the readiness deadlines and one grim deadline, because the readiness check isn't raced against the stop while the work is.
- Each session runs at most 16 tool calls at once; one more gets a JSON-RPC error at once and runs nothing. It bounds what one client can queue in the engine every client shares.
- Risk, not addressed: an engine started by a client inherits that client's cgroup. Its own process group protects it from the client's terminal signals, but stopping the client's systemd scope kills the engine for every client. That is the crash path: `engine_lost`, a new engine on the next call, and `recovery_required` only if input was held. The follow-up is to start the engine in its own transient scope with `systemd-run --user --scope --collect`; it isn't built, and there is no systemd unit.
- An engine socket path over the 108 bytes a Unix socket allows makes the bridge serve standalone at once, with the reason on stderr, instead of trying for five seconds. The nested harness's `TEST_DIR` moved from `$XDG_RUNTIME_DIR/niri-computer-use-test` to `$XDG_RUNTIME_DIR/ncu-test`, so the engine socket of a nested run fits.

## 2026-10-10: the runtime directory follows niri's resolved socket

- The runtime directory is `<dir>/niri-computer-use/<instance>/` beside niri's socket with every symlink resolved, not under `XDG_RUNTIME_DIR` (review finding). Before, a standalone server and a bridge whose clients passed different `XDG_RUNTIME_DIR` spellings, or a `NIRI_SOCKET` symlink with another name, locked different `lease` files for one niri and missed each other's stop flag.
- The socket is resolved once per process and connections use the resolved path, so a symlink retargeted later can't move a running server to another niri or another lease.
- Resolution fails closed: a socket that doesn't resolve, isn't the user's, has more than one hard link, or lies in a directory that isn't the user's with mode `0700` leaves the process with no runtime directory. Picking `/run/user/<euid>` or the unresolved path instead could split the lease again.
- A restarted niri gets a new directory with its new socket name. What the old instance left there, including an input-dirty marker, stays for a human: once the old socket is gone nothing resolves to it, so `recover` can't reach it. No dependency was added.
- A socket with more than one hard link is refused by every name (review finding, fix round 4). Resolving symlinks can't tell two hard links apart, so each name would get its own lease and stop flag for one niri. Keying the directory on the socket's device and inode instead would keep working through a link, but a name that has to stay stable for the instance is simpler, and a link to niri's socket is no setup worth supporting. A server that resolved the socket before the link was made keeps its directory: the check runs once per process, like the resolution.

## 2026-10-10: stop and resume don't connect to niri

- `stop` and `resume` parse the command before discovery and find niri's socket without connecting (review finding): a socket counts when niri names it, it belongs to the user and its PID is a running `niri`. A hung niri accepts no connection, so the connect check made the stop key fail exactly when it was needed.
- This is weaker than the connect check, which tells a live socket from one a crashed niri left whose PID was reused. It is accepted only for these two commands, which set or clear a flag; serving, `recover` and the guardian still connect. Several candidates are still refused, and `resume` still refuses while the input-dirty marker exists.

## 2026-10-10: one writer for the bridge's stdout

- The bridge wrote each line to stdout inside its relay loop, so a client that stopped reading stopped the relay too, and its closing stdin went unseen while it held the lease (review finding). One task now writes stdout from a queue of 32 lines, which the relay fills without waiting; a full queue, a write error, or a line not written and flushed within 30 seconds ends the bridge.
- Ending that way closes the engine connection first and then calls `std::process::exit`, under `#[expect(clippy::exit)]`: tokio's stdout writes on a blocking thread that can't be cancelled, and the runtime waits for blocking threads when `main` returns. A nonblocking stdout would avoid that, but stdout may be a regular file, and the exit is simpler.
- The queue's bound comes from the existing 256 MiB engine line limit rather than a byte budget: 8 GiB at worst. A client that reads keeps few lines waiting, since the engine's lines come one at a time; the call limit doesn't bound them, as requests other than `tools/call` aren't limited.
- The `engine_lost` answers to everything in flight were queued in one loop without waiting, so with more than 32 in flight the queue filled before the writer ran, and a client that was reading lost its bridge as if it had stopped (review finding, fix round 5). Each of these answers now waits for room, within a line's 30 seconds, until the client's stdout fails or its input overflows. The queue wasn't enlarged: any size has a burst that fills it. The client's lines wait in a 32 MiB backlog, twice the client line limit; a full backlog ends the bridge (see "a full input backlog ends the bridge" below). No dependency was added.

## 2026-10-10: the native keyboard restores the latest base map

- The native keyboard uploaded the map it was bound with at the end of a call, so a compositor keymap that changed during an extension was replaced by the stale one, and the layout captured at the start was sent back with it (review finding). It now keeps the latest base map apart from its own extensions' echoes and restores that, in the layout niri has active (see "native cleanup restores niri's active layout" below).
- Maps are compared by their libxkbcommon serialization, which `input/keymap.rs` already produces, because niri re-serializes uploaded maps; byte comparison took a re-serialized echo for a new map. A map niri sends is not labelled as the compositor's. No dependency was added.
- The keyboard tracks the map installed on its device, from the bind on, and a map equal to it, or to an extension it installed before, is its echo; any other is a base map. The smithay revision niri 26.04 uses sends a virtual keyboard's own map with its key and modifiers requests whenever another map is active, so after a configuration change the device's next key sends the old base back, and taking that for a new base put the stale map back (review finding). Restoration counts only when the last map received is the latest base and the device holds it too. Cleanup uploads the latest base whenever the device holds another, an extension or the base it was bound with, and a second time for a base that arrived during the first upload. Before, the proof compared clients with the base last uploaded, so a new base during a call with no extension failed it for good and kept the marker for `recover` (review finding).
- A configuration put back to exactly the map the device holds can't be told from the device's echo, so the newer map goes back to clients until a physical key sends niri's own. Checked by unit tests and by the nested input suite's ASCII call during a configuration change, which failed on the previous code with the old map left in clients.

## 2026-10-10: the passthroughs log metadata only

- `niri_action` and `noctalia` logged their arguments whole, so a `Spawn` command, a workspace name or a `noctalia` argument could carry clipboard text or a title into `audit.jsonl`, against the log's promise. Sol's review reproduced it with a sentinel through MCP, on a refused call.
- `niri_action` now logs the parsed action: its name and field names as niri-ipc's serde spells them, numbers, booleans and nulls as they are, every string as its length in bytes and every list of strings as a count and byte lengths. JSON that isn't an action logs a fixed `invalid_action` category with its key count and size, never its keys, since an unknown variant or field name is the agent's text.
- `noctalia` logs only the argument count and byte lengths. A first draft kept a first argument of lowercase letters and `-` as a command word; Sol's design check showed clipboard text such as `synthetic-private-content` fits that shape, so syntax can't tell a command from private text. A fixed list of Noctalia's command names would be the way to get readable commands back.
- Lengths are in bytes, the size the log can state without decoding anything.
- The log no longer says which program `Spawn` started; the promise that the log holds no desktop text wins over that.

## 2026-10-10: the paste keeper's commands and the key's admission

- The keeper's stdin carries `k` right before the key, then `p` once the key went out or `n` if it didn't. Before `k`, end of stdin means no key, and the keeper restores at once. After `k`, only `p` or `n` says the key can't still arrive: end of stdin, or ten seconds without either, leaves the pasted text on the clipboard with `clipboard: kept` (review finding). A late key would otherwise paste the restored clipboard, perhaps a password, into the app; losing the user's earlier copy is the lesser harm.
- The server's death is the same case: its end closes the keeper's stdin after `k`, and wtype, in a process group of its own, may still type. So the keeper keeps rather than restores.
- `k` is acknowledged (review finding). A keeper that waited ten seconds for `k` restored and went on serving the saved selection with its stdin open, so a delayed server's `k` was written without error and the key pasted the user's clipboard. The keeper now answers `k` with `armed` only from that wait, while the text still holds the selection, after a round trip to niri so a replacement already sent counts; it never reads a command after the wait ends. The server sends the key only on `armed` within 500 ms: a round trip to niri takes milliseconds, and the wait runs inside wtype's three-second deadline. No answer, an error or any other report fails the call with nothing pasted, and the server says `n` unless the keeper already reported. No dependency was added.

## 2026-10-10: the marker's lock and owner

- Every change to `input-dirty`, by a server, the crash guardian or `recover`, takes `input-dirty.lock`, a file that stays, and checks under it that the marker is still the one the change is for (review finding): a server's own by its `owner`, unique to each input, and the guardian's and `recover`'s by the bytes they read. Before, a late cleanup could compare, lose the race and remove the next input's marker. The lock is taken without blocking, retried for 500 ms, and held only for the file operations; a lock not taken fails the change and keeps the marker. `std::fs::File::try_lock` is in the standard library, so no dependency was added.
- Markers written before `owner` existed are read and recovered, but never match a server's own clear or update.
- Builds from before the lock don't take it. Running one beside this build leaves their changes unarbitrated, so every server, bridge and engine of the old build has to end before the new one starts. A compatibility layer for mixed versions isn't worth it.
- The refusal while a marker is set says "still finishing" only for a marker whose input this process still holds (review finding). Matching the marker's `server_pid` to the process sent the agent to retry forever after a cleanup had failed and left the marker, as when `wtype` is killed by a signal, and a shared engine's PID matches every client's. Each process now keeps the owners of the markers its input holds and drops one when that input clears or gives up on its marker; any other marker tells the agent to call `release_desktop`, because `recover` needs the lease, and the user to run `recover`.

## 2026-10-10: release preparation

- The package has `description`, `repository`, `readme`, `keywords` and `categories`. Only `description` (with the existing `license`) is required to publish on crates.io; the rest helps people find it. Nothing was published.
- The workspace's `cargo_common_metadata = "allow"` is gone. The lint only checks packages that can be published, and the harness is `publish = false`, so `make check` passes with it at the workspace's `cargo` level and `-D warnings`.
- `--version` is handled before the server reads its environment or looks for a session, so it works with no desktop and never connects to a niri socket.

## 2026-10-10: a full input backlog ends the bridge

- A line that found the bridge's 32 MiB input backlog full waited for room, and while it waited the reader looked for the end of stdin only when nothing more was buffered, so a client that sent more and then closed stdin went unseen while the relay waited on the engine (review finding). The reader now never waits: a line with no room ends the bridge as a full output queue does, closing the engine connection, so the engine ends the session and frees the lease, then saying why on stderr and exiting.
- The same policy as the output queue: input the engine hasn't taken is bounded, and a client past the bound loses its session rather than holding the lease while the bridge can't see it. Watching for the end behind buffered bytes would need the reader to keep reading past the bound anyway, which is what the backlog exists to stop. No dependency was added.

## 2026-10-10: native cleanup restores niri's active layout

- A native call dropped by a cancellation or a failed stroke, the crash guardian and `recover` sent their zero modifiers in the layout the call started in, so a layout the user switched to meanwhile was undone in the focused application until its next key (review finding). Only the end of a call used the layout niri last reported.
- Every path now asks niri with a `KeyboardLayouts` request right before it sends them. niri's active layout follows the user's physical keys and `SwitchLayout`, not a virtual keyboard's group, so it is the layout the user's own next key types in, and the one the focused application should show. The event stream's view is no substitute in the separate guardian and `recover` processes, and even in the server it lags niri by the stream's delivery.
- A dropped call can't wait for niri in `Drop`, so it releases at once in the layout it last knew. Its cleanup task, once niri has taken that, restores the base map with zero modifiers in the active layout and waits, then sends zero modifiers in that layout once more and waits again. The second input goes out even when nothing changed: in the nested trials, a dropped call that checked for the restored map after one wait didn't see it come back and kept its marker, and once the focused client got no modifiers event for one of the two inputs. Neither cause was found in niri 26.04 or the smithay revision it uses. When niri doesn't answer, a call falls back to the layout the stream last reported, and the guardian and `recover` to the marker's `group`; `recover` prints that, and the guardian writes it to the server's stderr. Recording each layout change in the marker under its lock was the other option; it costs a marker write per change, and still lags niri.
- Checked in the nested input suite: a call cancelled after a switch, retried until one is dropped before it sees the switch, and a server killed after a switch, whose guardian and then `recover` must leave the focused client in the layout switched to. wev printed a dropped call's last modifiers only when later input woke it, so the cancelled call's check runs a one-character call after it and reads what wev printed before that call's keymap. No dependency was added.

## 2026-10-10: acquiring the lease, in order

- Another session's `acquire_desktop` waited behind the owner's running action, for the action mutex, before it was refused (review finding). The stop flag, the input-dirty marker and another session of the same server holding the lease are now checked before the mutex, so that refusal comes at once and before any readiness I/O.
- Under the mutex the order is: the session's end; the same session's own lease, returned as it is; the stop flag and the marker again; another session's grant; locking the `lease` file, so another server's lease refuses before the slow part; readiness and the user's focused window; then the session's end, the stop flag and the marker once more, and only then the grant. A refusal after the lock gives the lock back. Readiness can take seconds, and a stop or the session's end meanwhile must not hand out a lease nobody wants. The `lease` file's inode isn't checked again here; the next action checks it.
- No dependency was added.

## 2026-10-10: every completed line counts against the line limit

- The 16 MiB line limit counted from the last newline in each read, so a read that ended an oversized line and started a short one passed the long one (review finding). `session::LineLimit` now checks every line a read completes, newline included, before resetting, and the standalone server, the engine's sessions and the bridge's reader share it.
- An oversized line ends the session wherever it falls among others, and lines after it are not answered: a client that sent one has lost track of its framing, so nothing after it can be trusted to be what the client meant. No dependency was added.

## 2026-10-10: each input cleanup ends by its own deadline

- `cleanup::LIMIT`, the server's wait at its end, was a sum written in a comment, and a native paste's cleanup had since gained a `KeyboardLayouts` request and two Wayland round trips: up to 13.5 seconds against the ten (review finding). The extra keymap upload of "the native keyboard restores the latest base map" would have made it 15.5, and a dropped call's cleanup that took over from a failed release started a new count.
- Every cleanup now runs through `cleanup::spawn`, which drops it at its deadline, `LIMIT` after it started; a native cleanup that takes over from a failed release keeps that release's deadline. So no cleanup outlasts the server's wait however its steps add up, and a sum can't drift from the steps again. Computing `LIMIT` from the step constants was the other option; it would still miss a step added later, and the native worst case would have meant a wait of over fifteen seconds at every exit with a cleanup pending.
- A cleanup cut short leaves its marker, as any failed cleanup does, so the next action sends the user to `recover`; a call waiting for it gets `deadline_exceeded` saying so. Each step takes milliseconds when niri and the keeper answer, so only a niri slow on every round trip reaches the deadline. The Wayland round trips can't be slowed in a test without a fake compositor, which the fixtures don't have; the deadline is tested at `cleanup::spawn` with a cleanup that never ends.
- No dependency was added.

## 2026-10-10: a copy made while the keeper saves refuses the paste

- After saving the selection and before taking it, the keeper makes a round trip to niri and handles every selection it announced meanwhile; if one came, it refuses with `clipboard_unsaved` and changes nothing (review finding). Before, it took the selection over a copy made during the save and later restored the older one, so that copy was lost, and a newer copy marked secret never met the secret check.
- It refuses rather than saving again: a client that keeps copying could hold it in a loop, and the agent can type the text instead or try again.
- A copy niri handles between that round trip and the take is still overwritten, as one between the last check and the restore is: data-control has no request that sets the selection only if it is still the one seen.
- The nested keeper checks cover it with `harness clipboard --hold`, an owner that answers no read until another client copies. The selection code talks to niri's Wayland socket, and the repository has no fake Wayland server to drive it in `make check`.
- Removing the refusal used to leave `make check` green (review finding). The decision is now a pure step, `keeper::previous`, given the saved selection and whether it was still current, and `Saved::still_current` compares niri's announcement count; both are unit-tested, as is that `State::select` counts every announcement, an empty one included, since it can't tell our own source's offer from another client's. What stays nested-only is the wiring: that `take` makes the round trip after the save and passes its answer to `previous`. Testing that in `make check` needs a fake data-control server for the protocol fixture, a Wayland server written for the tests; that is the accepted gap.

## 2026-10-10: held pointer modifiers restore like native typing

- Modifiers held with `keys` on a native `click`, `drag` or `scroll` were released in the layout the gesture began in, with the device's own map, and the marker came off after one round trip (review finding). A keyboard configuration change or a layout switch during the gesture left clients with the old map and layout until a physical key, and nothing kept the marker.
- `Held` now ends through native typing's own cleanup, `native::restore_active`, which also gained the `restored` check it was always followed by: release, ask niri for the active layout, upload the latest base map, zero modifiers, wait, and fail unless clients and the device hold that map. A failure keeps the marker for `recover`. Sharing the function keeps the two paths from drifting again; the cleanup still runs under `cleanup::spawn`'s deadline.
- The check is in the nested input suite: a held drag whose serving process is stopped right after the press while niri's config gains a layout and niri switches to it. With the old release, clients kept the old keymap and the check failed. A unit test would need a fake Wayland compositor that echoes virtual-keyboard maps, which the repository doesn't have. No dependency was added.

## 2026-10-10: a paste after `kept` replaces the kept text

- After a `kept` outcome the keeper keeps serving the agent's text marked `x-kde-passwordManagerHint: secret`, so every later `paste` refused with `clipboard_unsaved`, naming a password manager that wasn't there (review finding).
- The keeper now also offers `application/x-niri-computer-use-paste`. A later keeper that saves a selection carrying it doesn't refuse on the hint and doesn't put it back afterwards: it clears the clipboard and reports `cleared`, with a `detail` saying the earlier paste's text was dropped. The user's copy that text stood in for is already lost, and the late key it guarded against has long landed; restoring it would only put the agent's old text back. A selection with the hint and without that type is still refused.
- Only a keeper offers the type, but nothing stops another client from offering it; such a selection would be dropped rather than restored, which costs that client's copy, not a secret's safety. No dependency was added.

## 2026-10-10: the keeper's ready budget follows its steps

- The server gave the keeper five seconds to report `ready`, while its steps, finding niri's PID and then binding, saving, the round trip after the save and the take, each had two (review finding). With a niri slow on every step, the server failed the call and the keeper still took the selection and then restored it.
- `keeper::TAKE` is now the sum of those step deadlines, built from `niri::PID_LIMIT` and `selection::STEP`, and the server waits `READY`, that plus a second for the keeper to start: eleven seconds. Giving the keeper one overall deadline was the other option; cancelling it partway through the take could leave the selection with a source that is about to go away. No dependency was added.
- The wait for the keeper's last report after `p` had the same gap: five seconds for the two-second wait for a read and two round trips of two seconds each, so a slow niri turned a finished restore into `clipboard: unknown` (review finding). `keeper::FINISH` is that sum, and the server waits it plus half a second for the report line. Both budgets have a test with a keeper stand-in that answers after five and a half seconds.

## 2026-10-10: a dead server's marker goes straight to recover

- The refusal for a marker another server wrote said that server's input may still be finishing, and to try once more, even when that server had died and its crash guardian had sent the releases (review finding, fix round 5). Now that wording needs the marker's `server_pid` running, as `/proc/<pid>/stat` tells, and no `released` note; any other gets the `recover` wording at once.
- The marker records no start time for its server, so a PID reused since that server died reads as running and costs one retry, as before. Recording it would change the marker's format, which every server and `recover` read with unknown fields refused, for a wording only. No dependency was added.

## 2026-10-10: element actions

- `activate_element` (AT-SPI `Action.DoAction`) and `set_element_text` (`EditableText.SetTextContents`) reach an app with no input event, through any window on the bus, focused or not (research A4). They are not gated behind `unrestricted` (the supervisor's decision): they get the gates `type_text` has, on the element's own window, so they can do no more than a click and typing can, and a second gate would only add friction. Those are the lease, the stop flag, the input-dirty marker and the lock state through `Desk::act`, checked right before the work; the element's window, found by the ref's window id and PID, must have keyboard focus and `expect` must name it (`focus_mismatch` otherwise, which also refuses an element under a Noctalia panel, since a panel holds keyboard focus); the deny list on that window's `app_id`; and the ref checked again right before the call by the A5 rule, as `element:` on the pointer tools is.
- `expect: "none"` is refused as an argument mistake instead of accepted and ignored: the focus check can't be skipped, and accepting it would suggest it was.
- No frame guard: nothing is aimed, so the windows `elements` can't place, GTK 3 and Qt with client-side decorations, can still be acted on.
- `activate_element` takes an action the element listed, or by default its first one among `click`, `press`, `activate` and `toggle`, compared without case since Qt says `Press`. It reads the element again 300 ms after the call: GTK 3 and GTK 4 buttons show an activation as a 250 ms press before `clicked` (`ACTIVATE_TIMEOUT` in `gtkbutton.c`, measured: all 20 activations were already counted when each call returned, in GTK 4, GTK 3 and Qt). The brief's cap was 300 ms. With 100 ms, the first choice, GTK 4's `Dismiss` button still read `present`: its window hadn't closed yet.
- `set_element_text` takes at most 64 KiB of UTF-8. The text goes out as one D-Bus message that the app handles on its main thread within the one-second call deadline; 64 KiB is more than any single-line field holds (a GTK entry keeps at most 65534 bytes, `GTK_ENTRY_BUFFER_MAX_SIZE`) and took 55 ms in the nested GTK 4 fixture, the same as a short text. `paste` remains for longer text.
- Password fields are refused with a new error name, `secret_field`, by the role `password_text`. In the nested fixtures that is what GTK 4 gives `GtkPasswordEntry` and an entry whose input purpose is a password or PIN (`gtk_accessible_is_password_text`), GTK 3 an entry that hides its text, and Qt Quick a `TextField` in password echo mode. No AT-SPI state marks a protected field. A GTK 4 entry that only hides its text, with a free-form purpose, shows on the bus as a plain `text` with the same states and can't be told apart; it is not refused. Its own role was chosen over `app_denied` because the agent can't do anything about it by changing the window.
- Both tools are `destructiveHint: true`. `set_element_text` replaces everything the field held, and is `idempotentHint: true`: the same text twice leaves the same content.
- The audit log has the role and the action from the element ref, and `text_len`, never the name or the text. No dependency was added.

## 2026-10-11: element actions after review

- An application writes the message that comes with a D-Bus error, and may echo the text it was given or a field's content there. For element actions only, a failed AT-SPI call's `detail` keeps the call and the D-Bus error's name and drops that message: a deliberate exception to AGENTS.md rule 5, which otherwise keeps upstream detail whole. The name is the stable part an agent or a human can act on; the message is the app's own data, which these tools promise never to return or log. Connection errors keep their OS error text, which no app writes. `elements` and the pointer tools keep the whole message, as before.
- An action's name is the app's string too. The audit log, the result and every error say an action's kind (`click`, `press`, `activate`, `toggle` or `other`) and its index among the element's actions instead, and an argument mistake counts the actions and lists their kinds instead of their names. `elements` still returns the names, as data for the agent, never logged; an agent picks an action by the name it read there.
- A ref keeps a private lineage of the element, never logged or returned: each object's path and index among its parent's children from the frame down. Before an element action or a pointer aim, the window's frame is found again and must be the ref's, and each parent must answer the kept child at the kept index (`GetChildAtIndex`, so the parent vouches for the child instead of the child naming its parent); otherwise `element_stale`. The same role and action names alone didn't show that a virtualized list's reused row was still the record listed. A shifted index refuses something that may be the same element: listing again costs one call, acting on another record could delete it. The name was not part of it at first: a first version hashed it, and the nested run showed every counter button going stale after one click, as GTK, Qt and apps relabel buttons when used, while a recycled row's `Delete` button keeps its name anyway. The second review brought names back, differently (below). The description isn't read: `elements` doesn't read it either, and every node of the walk would pay for it.
- The acting call's reply lost once the call may have gone out is the `uncertain` outcome (`accepted: null`) with the usual look afterwards and a screenshot, not an error: an error suggests nothing happened, and an agent that repeats a destructive activation could do it twice. Failures before anything is sent, a spent budget or a closed connection, stay errors. zbus 5.19 offers no public way to send a method call and wait for its reply separately (`call_method_raw` is crate-private), so a write that fails partway counts as lost too; reading replies off a `MessageStream` ourselves would need the `futures-core` stream trait as a new direct dependency for that one distinction, which wasn't worth it.
- Element actions read the element over several AT-SPI calls before they act, up to the request's three-second budget, while the action gate checked focus, the lock screen, the marker and the policy before them. So a last gate, `Gate::last`, asks again right before the acting call; at first slowest first (the second review changed the order, below): a fresh readiness report through the policy (the lock screen among it), the element's role and states, then niri's events received so far applied to the waiter, with focus, the deny list and `expect` checked on that view, and last the desk's `checkpoint` (stop flag, marker, lease held and its file unchanged), which reads the lease's owner without the action mutex the running action holds. The report the result gives is that last view's focus. The engine hands the checks in through a `Recheck` trait, so `elements/actions.rs` holds no engine or desk. The stream's task tells waiters its connection ended before it says so anywhere else, so applying the pending events is how the gate sees a lost stream (`niri_unavailable`). niri, the desk and the app can't be read in one step, so a change between the gate and the app taking the call isn't seen; the second review's entry below says what that leaves.
- `secret_field` applies to `activate_element` too, at that gate: a password field's `activate` can submit its form. The role is read fresh there, and checked before it is compared with the listed one, so a field that turned into a password field says `secret_field`, not `element_stale`.
- A GTK 4 entry that only hides its text (`Gtk.Entry(visibility=False)`, free-form purpose) stays unrefused, and the docs, the skill and the changelog now say `secret_field` covers fields the toolkit marks as passwords (role `password_text`), not every password field. The nested GTK 4 fixture's `Hidden entry`, read with `busctl` beside the `Plain entry`, has the same role (`text`), states, interfaces, `toolkit` attribute and default text attributes, `invisible: false` included; only its text differs, ten `●` (U+25CF) for ten synthetic characters. An empty hidden field, the one an agent would fill, shows nothing, and checking for that character would mean reading a field's text and refusing any field that holds one, so it isn't a signal. Refusing every GTK 4 entry would end `set_element_text` for GTK 4. The skill tells agents never to fill or activate a field that may hold a secret, and the harness requires the two entries' roles to stay the same, so the claim fails loudly if GTK starts marking them.

## 2026-10-11: element actions after the second review

- The last gate asks the app before it checks the server's own state, never after. The first version checked the readiness report first and read the element's role and states after, slowest first; but the app decides how long each answer takes, up to the call deadline, so a held `GetState` let the screen lock unseen after the lock check (reproduced by the review). The rule now is that no wait the app controls comes after a check of state the app doesn't control. The gate reads, one call at a time, `identify`, the tool's own read (the action list, from which the index sent is taken, or the interfaces), the states, and the role last; then the readiness report, niri's events, owner, deny list, focus and `expect`, and the desk. Role and states were read side by side, so a role read before a held state read was stale when the gate went on; reading them one after the other with the role last costs one round trip. The early checks stay, as fast refusals.
- The action's index is no longer the one found before the gate: the gate reads the action list again and needs the chosen action still at the index it was listed at, else `element_stale`. Taking the name's new index instead would also be safe, but refusing keeps the result and the audit log's index the one listed.
- What the gate can't see is said precisely in the architecture: the app changing the element after answering, during the rest of the gate and the readiness report, which its own niri and Noctalia deadlines bound, not the app; and anything between the end of the gate and the app taking the call, an in-memory check and one D-Bus message. The app is the one that acts on the call, so a change it makes to its own element in between is no more than it could do on receiving the call.
- A ref keeps a hash of the name of every object between the frame and the element, and of the element's own, in memory only (std's `DefaultHasher`; never logged or returned). An object reused in place for another record, at the same index under the same parents with the same role, passed the lineage check, and the docs promised a name check the code didn't make (both reviews). A reused row's own name is its record's, and rows don't relabel when a button in them is pressed, so the parents' names catch the recycled row even where the button keeps its name. The frame's name, the window title, changes on its own and isn't compared; niri's window finds the frame.
- The element's own name is compared for `activate_element` and `set_element_text`, which read the name again after acting and keep that hash in the ref: their own effect, a counter relabelling itself, keeps the ref valid; a name that changes later on its own makes it stale, failing closed, and the agent lists again. Ancestors' hashes are never updated. A pointer aim compares the ancestors only: a click doesn't read the element afterwards, so a counter clicked through the pointer would go stale after each click, which M9's hundred clicks per ref show. So the pointer tools keep the gap for the element's own name, and the role check and the agent's screenshot cover it.

# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Upgrading

- Before upgrading, end every running `niri-computer-use` process, servers, bridges and shared engines alike, then start your clients again. `pgrep -a '^niri-computer-u'` lists them. Older builds keep their runtime directory under `XDG_RUNTIME_DIR` rather than beside niri's socket, so an old and a new server on one niri share neither the lease nor the stop flag, and they don't take the input-dirty marker's lock.

### Added

- Optional GTK 4 button-activation acceptance in `make nested-input`: 100 actual activations with observed-counter latency percentiles.

- `niri-computer-use serve`: an MCP server over stdio with two read-only tools, `status` and `outputs`.
- `niri-computer-use status`: prints the same readiness report as the `status` tool.
- `screenshot` tool: one output or a region inside one output, through grim, as JPEG or PNG with its geometry and scale.
- `clipboard_read` tool: the clipboard's text through `wl-paste`.
- An audit log of every tool call at `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, with argument metadata and outcomes but no contents. `status` reports its path and last write error.
- `shell_status` tool, present when Noctalia is installed. `status` reports whether Noctalia is running and the lock state from logind and Noctalia; the screen counts as locked if either says so. logind is asked about the session niri itself runs in, read from niri's environment, so the answer doesn't depend on where the server was started, and only when niri runs as `niri --session`, the only niri that sets logind's hint.
- `desktop_state` tool: windows, workspaces, focus, overview and keyboard layouts from niri's event stream. `status` reports whether the stream is connected.
- The `niri-computer-use` skill in `skills/`, for agents using the read-only tools.
- `make inspect` and `make inspect-check`: the server under the pinned MCP Inspector.
- `niri-computer-use stop` and `resume`: set and clear a stop flag in the niri instance's runtime directory, `niri-computer-use/<instance>/` beside niri's socket. `status` reports it as `stop`.
- `acquire_desktop` and `release_desktop`: one server at a time holds the lease on a niri instance, an exclusive `flock` in the runtime directory. The stop flag takes it back. `status` reports the holder. New error names `lease_held`, `stopped` and `recovery_required`.
- The policy file, `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`: launch presets and an app deny list, checked at startup and reported by `status`. `acquire_desktop` refuses with `read_only` while it is invalid or niri's version isn't supported, and with `screen_locked` while the screen is locked or its lock state is unknown.
- The `niri-computer-use` skill covers the lease tools and tells agents to leave stops, recovery and locks to the user.
- `make nested-control`: M2's acceptance in a nested niri, with the nested Noctalia as the lock source.
- `make nested-actions`: M3's acceptance in a nested niri: launch, reuse, focus, close and `interrupted`, against fixture windows the harness provides.
- `focus_window`, `focus_workspace`, `launch` and `close_window`: the first tools that act on the desktop, through niri's IPC. They require the lease and check the stop flag, the input-dirty marker and the lock state before each action, and a stop cancels a running one. Each result says whether niri `accepted` the action and what was `observed` on niri's event stream: `focused`, `closed`, `pending`, `one`, `ambiguous`, `none`, `timeout`, `interrupted` when someone else moved focus, or `uncertain` when niri's reply was lost. `launch` only starts presets from the policy file, and with `reuse` focuses an existing window. An outcome in doubt comes with a fresh screenshot of the focused output. New error names `lease_required` and `unknown_preset`. The audit log records `accepted` and `observed` for these tools.
- `status.policy.preset_names`: the names `launch` takes.
- `niri-computer-use recover`: clears the input-dirty marker after ending the input child it names and asking the human to confirm that no input is held. `status` reports the marker as `input_dirty`.
- `shell_open` and `shell_close`, present when Noctalia is installed: open or close the Noctalia panels `control-center`, `wallpaper` and `tray-drawer`, then poll Noctalia's `activePanelId` every 100 ms for up to two seconds, reporting `opened`, `closed`, `timeout` or `uncertain`. Every other panel is refused with the new error name `panel_not_allowed`. They need the lease and pass the same gate as the other actions.
- `make nested-eval`: runs an agent (`claude -p`) with a skill against one scenario in a nested niri and grades it from the audit log.
- `screenshot: true` on every action: the result comes with a screenshot taken once the screen stopped changing, with `settled` in its metadata.
- `wait_for` tool: waits until a window appears, closes or changes its title, or the screen stops changing.
- `type_text`'s `submit`: presses Enter only once all of the text went out, and reports `submitted`.
- `acquire_desktop` returns `users_window`, and `release_desktop` takes `restore_focus` to give focus back to it.
- `make nested-eval` scenarios `dialog-midway` and `errand`, a check for calls sent together, and call and turn counts in `timing.json`.
- `pointer_move`, `click`, `drag` and `scroll`: input through niri's virtual pointer, aimed at pixels of a screenshot this lease took through its `screenshot_ref`. A ref expires after 60 seconds or when its output changes. They run only on one enabled output at transform `Normal`, and refuse others with `untested_output_config`.
- `key` and `type_text`: keyboard input through `wtype`, into the window named in `expect`, refused with `focus_mismatch` elsewhere and with `app_denied` for an app on the policy's deny list.
- An input-dirty marker written before input and removed once it is released, so a crash mid-input leaves `recovery_required` until `recover`.
- An experimental native keyboard backend, `NIRI_COMPUTER_USE_KEYBOARD=native`: niri's virtual keyboard protocol with focus checked before every key, symbols the layout lacks typed through a temporary keymap that is proved restored, and modifiers held through `click`, `drag` and `scroll` with `keys`. `wtype` stays the default.
- A crash guardian for each server: when the server dies holding native keys or pointer buttons, it releases them at once. The marker still waits for `recover`.
- `paste`: up to 1 MiB through the clipboard, with a keeper process that saves every type the clipboard offers, holds the text marked as a secret for clipboard managers, and puts the saved clipboard back, unless the server dies once the key may be on its way, when it keeps the pasted text and reports `clipboard: kept`. It refuses with `clipboard_unsaved` before changing anything when it can't save the clipboard, the clipboard is marked secret, or something else is copied while it saves.
- `screenshot`'s `save_path`: a full-resolution PNG under the policy file's `capture_dir`, created new, never through a symlink, refused with `save_not_enabled` without a `capture_dir`.
- `elements`: one window's accessible elements over AT-SPI, with roles, names, states, actions and boxes in layout coordinates, listed when the session has an accessibility bus. Each element has an `element_ref` under the lease that `pointer_move`, `click` and `drag` take as `element`; it is checked again before aiming. New error names `not_accessible`, `ambiguous_window`, `element_stale` and `element_unmappable`.
- Session discovery: `XDG_RUNTIME_DIR`, `NIRI_SOCKET` and `WAYLAND_DISPLAY` are found when the client doesn't pass them, from the user's only running niri, so clients need no environment settings. Given variables win, and discovery never chooses between niri instances.
- `niri_action`: any niri action in niri's IPC JSON, such as fullscreen, floating or a window's width, with the window's state as niri reports it after. Actions that run programs, write files or reach past the layout are refused with `unrestricted_required`.
- `unrestricted = true` in the policy file, or `NIRI_COMPUTER_USE_UNRESTRICTED=1` for one client: lets `niri_action` send the gated actions, lists the `noctalia` tool, and lifts the preset rules. Off by default. `status.unrestricted` reports it.
- `noctalia`: any Noctalia command, as `noctalia msg` sends it, listed only with Noctalia installed and `unrestricted` on.
- A shared engine per niri instance, with `shared = true` or `NIRI_COMPUTER_USE_SHARED=1`: each client's `serve` relays to one `niri-computer-use engine`, which holds one desk, guardian, event stream and accessibility connection for every client, while policy, `unrestricted`, keyboard backend and home stay per client.
- The documentation site, with `llms.txt` and a Markdown address for every page.
- `niri-computer-use --version`.

### Changed

- Automatic nested suites run inside isolated headless cage by default, without host snapshots. `VISIBLE=1` or `--visible` keeps the human path; sittings remain visible.
- Safety documentation explicitly distinguishes focus-based app denial from pointer-target isolation, and sensitive read-only observations from private data.

- `key` takes `keys`, a list of up to 16 combinations pressed in order, instead of one `combo`, and stops with `interrupted` and a `pressed` count if focus moves.
- `release_desktop` requires `restore_focus`.
- `type_text` takes up to 1000 characters and types them in parts of 100, stopping with `interrupted` and a `typed` count if focus moves between parts.
- `screenshot` waits for a running action of the same server before capturing.
- The tool descriptions and the server's instructions carry the rules agents most often broke: no screenshot alongside an action, no Enter after text that didn't fully go out, apps only through presets, focus back to the user's window when done.
- The `niri-computer-use` skill is rewritten: a description that says when to load it, a shorter workflow with reasons and examples, and the tool and error tables moved to `references/`.
- The project is renamed from `niri-desktop-mcp` to `niri-computer-use`, including the binary, the MCP server's name and the audit log's directory.
- `niri_action`'s `DoScreenTransition` needs `unrestricted`: it can freeze every output and screencast for up to 65 seconds.
- `activate_element` and `set_element_text`, listed with `elements`: an element's own action, or its whole text replaced, through the accessibility bus instead of the pointer or keyboard. They need the lease and pass the same gate as the other actions; the element's window must have keyboard focus and `expect` must name it, and the deny list applies to it. `activate_element` reports what it saw 300 ms later (`present` with the states that changed, `gone` or `unknown`), `set_element_text` whether the field holds as many characters as were set (`matched`, `differs` or `unknown`); a reply lost after the call went out is `uncertain` with `accepted: null`, as is a stop or a lost lease once the call went out, then with no screenshot. Right before the call that acts, the element, its action or editable text, its states and its role are read again, one at a time, and only then the focus, `expect`, the deny list, the lock screen, the stop flag, the input-dirty marker and the lease, so an app slow to answer can't make those checks stale. The element and the elements it sits in must still have the names they were listed with, so a list row reused for another record is `element_stale`; the element's name read after each element action is kept, so a counter that relabels itself stays valid. Fields the toolkit marks as passwords (role `password_text`) are refused by both with the new error name `secret_field`; a GTK 4 entry that only hides its text isn't marked and isn't refused. Text over 64 KiB is refused with `text_too_long`. The audit log keeps the role, the action's kind and index and the text's length, never names, action names or text.
- `elements` returns what it read, with `capped_reason: budget_exhausted`, when a large tree runs out of its three seconds, instead of failing with `deadline_exceeded`; `role`, `name_contains` and `limit` stop the walk early.
- With `unrestricted` on and no preset for an app, the skill and the tool text have agents start it with `niri_action`'s `Spawn` and wait for its window, instead of asking for a preset.
- The runtime directory that holds the lease, the stop flag and the input-dirty marker is `niri-computer-use/<instance>/` beside niri's socket with every symlink resolved, no longer under `XDG_RUNTIME_DIR`. A socket that doesn't resolve to the user's own leaves the server with no runtime directory rather than one picked another way.
- `stop` and `resume` find niri's socket without connecting to it, so the stop key works while niri hangs.
- A client that stops reading its server's output loses its session and the lease: once 32 lines wait for it, or one has waited 30 seconds, the bridge exits. So does a client that sends more than 32 MiB the shared engine hasn't taken yet, so its closed stdin can't go unseen behind that input.

### Fixed

- Standalone screenshots and `wait_for` capture sequences hold the action mutex throughout capture, excluding subsequent server actions and lease changes.
- Settled capture budgets include the initial delay and capture work. A timed-out later capture returns the last completed sample unsettled; no completed sample gives `deadline_exceeded`.
- Fractional-scale pointer motion uses niri's ceiled physical-mode space rather than truncated IPC dimensions. Unknown or changed pointer geometry refuses input.
- C15's nested capture assertions use grim's truncated image size instead of rounding odd dimensions.
- `close_window` and `niri_action`'s `CloseWindow` refuse with `app_denied` to close a window whose app is on the policy's deny list, named or focused.
- The audit log no longer keeps text an agent passes to `niri_action` or `noctalia`, such as a `Spawn` command or a notification body: it logs the action's name, field names, numbers and booleans with every string as its length in bytes, and only Noctalia's argument count and byte lengths.
- Another session's `acquire_desktop` refuses with `lease_held` at once, instead of waiting for the owner's running action. After its readiness check it looks again at whether the session has ended, the stop flag and the input-dirty marker before granting the lease.
- A stopped, cancelled or disconnected `paste` no longer restores the clipboard before a key already on its way arrives. If the server dies once the key may be on its way, the keeper keeps the pasted text and reports `clipboard: kept` rather than risk pasting the saved clipboard.
- `paste` sends its key only once the keeper has answered that it still holds the text. A server delayed past the keeper's ten-second wait could paste the user's restored clipboard instead; now the call fails, says nothing was pasted, and the clipboard stays restored.
- A copy made while `paste`'s keeper saves the clipboard refuses the paste with `clipboard_unsaved`, instead of being lost to the restore of the older one.
- Servers for one niri share its lease and stop flag whatever `XDG_RUNTIME_DIR` or `NIRI_SOCKET` spelling their clients pass. A socket with a hard link, which would have split them, gets no lease by any name, and `status` says why under `lease.error`.
- A shared-mode client that reads its output keeps its session when the engine is lost with more than 32 requests in flight, and each gets `engine_lost`; before, the bridge took the burst of answers for a client that had stopped reading and exited.
- Every change to the input-dirty marker takes one lock, so an older call's cleanup can't remove a newer call's marker.
- A marker left by a cleanup that failed, such as a `wtype` killed by a signal, now sends the user to `recover` instead of saying the input is still finishing; that answer is kept for input the server is still finishing. A marker another server wrote says its input may still be finishing and to try again in a few seconds before `recover`, while that server runs and its crash guardian hasn't sent the releases.
- The native keyboard restores the compositor's latest keymap, not the map from the start of the call, whether or not the call extended it, with zero modifiers in the layout niri has active: at the end of a call, after a cancelled one, and from the crash guardian and `recover`, which used the layout the marker recorded at the call's start. If niri doesn't answer, a call uses the layout niri last reported to it, and the guardian and `recover` the marker's, saying so.
- The 16 MiB line limit applies to every line a read ends, not only the last.
- An input cleanup that outlives its call, such as a dropped native `paste`'s release, ends within ten seconds of its start, as long as the server waits for it at exit. One still running then keeps its marker for `recover`, and a call waiting for it says so.
- Modifiers held with `keys` on a native `click`, `drag` or `scroll` end as native typing does: the compositor's latest keymap in the layout niri has active, proved before the marker comes off. They used to restore the map and layout from the gesture's start.
- A `paste` after a `clipboard: kept` outcome replaces the kept text instead of refusing with `clipboard_unsaved`, and clears the clipboard afterwards rather than put that text back. The keeper marks its text with `application/x-niri-computer-use-paste`.
- `paste` waits up to eleven seconds for the keeper to take the clipboard, as long as the keeper's steps can take, instead of five.
- `paste` waits six and a half seconds for the keeper's last report, as long as its wait for a read and its two round trips can take, instead of five, so a slow niri no longer turns a finished restore into `clipboard: unknown`.

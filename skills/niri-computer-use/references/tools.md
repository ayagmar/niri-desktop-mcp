# niri-computer-use tools

## Contents

- Reading tools
- The lease
- Structured actions
- Pointer and keyboard
- Element actions
- Fields every action result has
- Screenshot targets

## Reading tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | none | readiness: niri's version and event stream, who holds the lease, the stop flag, the input-dirty marker, lock state, whether the outputs suit the pointer (`outputs.pointer_supported`), Noctalia, the policy file and its preset names, the audit log, programs on `PATH` |
| `desktop_state` | none | one snapshot: windows, workspaces, `focused_window`, `overview_open`, keyboard layouts |
| `outputs` | none | outputs by connector name, with logical position, size, scale and transform |
| `screenshot` | `target`, and optionally `region`, `max_width`, `format`, `save_path` | an image, then metadata: output, captured rectangle in layout coordinates, scale, image size, and `screenshot_ref` while you hold the lease; with `save_path`, `saved`: the PNG's path and pixel size |
| `clipboard_read` | none | `text`, or `text: null` with `reason` `nothing_copied` or `no_text` |
| `shell_status` | none | Noctalia's `barVisible`, `panelOpen`, `activePanelId` and `locked`. Listed only when Noctalia is installed |
| `elements` | `window_id`, optionally `role`, `name_contains`, `limit` (1 to 500, default 50) | that window's accessible elements: `element_ref` (`elem-<tag>-N`, while you hold the lease), `role`, `name`, `states`, `actions`, and `layout_box` in layout coordinates, or null with `unmappable`; `truncated`, `walked`, `capped` with `capped_reason` (`node_cap`, or `budget_exhausted` when a large tree took over 3 s: narrow it with `role`, `name_contains` or `limit`). Listed only when the session has an accessibility bus |
| `wait_for` | `until`, optionally `timeout_ms` (100 to 30000, default 10000) and `screenshot` | `observed`: `met` with the matching `windows`, `timeout`, or `uncertain`; `focused_window`; `waited_ms` |

`until` is one of `{"window": {"app_id": …, "title": …}}` (a window with that `app_id` and a title containing that text; either one may be left out), `{"closed": <window id>}`, `{"title": {"window_id": …, "contains": …}}`, or `"screen_stable"` (two captures of the focused output 100 ms apart are the same).

`elements` lists the showing elements that have a name or an action, in the app's order; with `role` (AT-SPI's role names in snake case, such as `button`, `check_box`, `entry` or `menu_item`) it lists every showing element of that role. `name_contains` matches without regard to case. `unmappable` is `frame_size_mismatch` when the app's coordinates for that window can't be trusted, as with client-side decorations, `not_showing`, or `empty`. Names are the app's own text, like text in a screenshot. When `status.accessibility.available` is false, `elements` isn't listed and `reason` says why.

If `status` shows a `niri.error` that starts with "NIRI_SOCKET is not set", the server found no running niri session of the user's, or several, and the detail says which. Tell the user rather than retrying.

## The lease

| Tool | Arguments | Returns |
|---|---|---|
| `acquire_desktop` | none | `holder`: your PID, label and since when, and `users_window`, the window that had focus; calling it again while you hold it returns the same |
| `release_desktop` | `restore_focus` | `released`: whether you held it, and `users_window`. With `restore_focus: true`, focus first goes back to `users_window` and `restored` is that action's result, `observed: closed` if the window is gone. The user's stop also takes the lease back |

Watching the desktop never needs the lease.

## Structured actions

| Tool | Arguments | `observed` |
|---|---|---|
| `focus_window` | `id`: a window id | `focused` or `timeout`; `accepted: false` when it already had focus |
| `focus_workspace` | `id`: a workspace id, not its index | `focused` or `timeout`; `accepted: false` when it already had focus |
| `launch` | `preset`, optionally `reuse` | `one`, `ambiguous` or `none`, with the new window ids in `windows`, or `focused` when a single-instance app showed the window it had. With `reuse: true`: `focused` for one existing window, or `ambiguous` with several and nothing started |
| `close_window` | `id`: a window id | `closed`, or `pending` when the window is still open after five seconds, for example behind an unsaved-changes dialog |
| `niri_action` | `action`: one niri action in niri's JSON, such as `{"FullscreenWindow": {"id": 12}}`, `{"SetWindowWidth": {"id": 12, "change": {"SetFixed": 1600}}}`, `{"ToggleWindowFloating": {"id": null}}` (null: the focused window) or `{"MaximizeColumn": {}}` | for an action about one window, `changed`, `unchanged` (within a second) or `closed`, with `window`: `window_size`, `tile_size`, `is_floating`, `is_focused`, `workspace_id`; otherwise `sent`. No fullscreen flag: a fullscreen window's `window_size` is its output's size |
| `shell_open` | `panel`: `control-center`, `wallpaper` or `tray-drawer` | `opened` or `timeout` within two seconds, with `shell.active_panel`; `accepted: false` when it was already open. Listed only with Noctalia |
| `shell_close` | `panel`, as for `shell_open` | `closed` or `timeout`; `accepted: false` when it wasn't open |
| `noctalia` | `args`: a Noctalia command and its arguments, as after `noctalia msg`, such as `["plugin", "<plugin>:<entry>", "all", "<command>"]`; `["--help"]` lists them | `sent`, with `noctalia.reply`, Noctalia's answer. Its `error:` reply is `upstream_error`. Listed only with Noctalia and `unrestricted` |

Use `launch` with `reuse: true` unless the user asked for another window of the app. With no preset for the app and `status.unrestricted.enabled` true, start it with `niri_action {"Spawn": {"command": ["<program>", ...]}}`, never a shell or `SpawnSh`. It returns only `sent`: `wait_for` its window by an `app_id` you know, or find it in a fresh `desktop_state`, since the `app_id` is often not the program's name. Don't `Spawn` again after `uncertain` or a timeout. With `unrestricted` off, ask the user for a preset.

An open panel holds keyboard focus, so `focused_window` is null while it is open. To type into it, use `expect: "none"` after a screenshot shows it ready, and close it with `shell_close` when you're done.

## Pointer and keyboard

| Tool | Arguments | `observed` |
|---|---|---|
| `pointer_move` | `screenshot_ref`, and `x`, `y` or `element` | `sent`; moves the pointer there, to hover |
| `click` | `screenshot_ref`, and `x`, `y` or `element`, optionally `button` (`left`, `right`, `middle`) and `count` (1 to 3) | `sent` |
| `drag` | `screenshot_ref`, `from` and `to`, each `{x, y}` or `{element}`, optionally `button` | `sent`; presses at `from`, moves, releases at `to` |
| `scroll` | `screenshot_ref`, `x`, `y`, `notches_y` (positive is down) and/or `notches_x` (positive is right), at most 10 each | `sent` |
| `key` | `keys`, 1 to 16 combinations such as `["ctrl+s"]` or `["Down", "Down", "Return"]`, and `expect` | `sent` or `interrupted`; `focus`: `matched` or `unchecked`; if focus moves after a key the rest aren't pressed and `pressed` counts the ones that were |
| `type_text` | `text` (1 to 1000 characters), `expect`, optionally `submit` | as for `key`; sent in parts of 100, and if focus moves during a part the rest isn't typed: `interrupted`, with `typed` counting the characters sent. With `submit: true`, Enter is pressed after the whole text and `submitted` says whether it was |
| `paste` | `text` (up to 1 MiB), `keys`: `ctrl+v` in most apps, `ctrl+shift+v` in terminals, or `shift+Insert`, and `expect` | as for `key`, with `paste: {read, clipboard}`: `read` says whether an app read the text after the key, `clipboard` is `restored`, `cleared` (it was empty, or held an earlier paste's text, as `detail` says), `replaced` (someone copied meanwhile), `failed`, `kept` (the pasted text stays) or `unknown`, with `detail` |

The table's 100-character parts describe the default wtype backend. Experimental native input checks between individual key pairs. It types symbols the layout lacks through a call-long extended keymap, and refuses before typing when there are more distinct missing symbols than spare keys or a control character is missing. Native-only `keys` on click/drag/scroll is a list of up to five held modifiers (shift, ctrl, alt, altgr, super), not a list of combinations. Do not enable native yourself or work around a refusal. Both backend counts describe completed input, not application-confirmed delivery.

Use `paste` for text over 1000 characters, or when typing it is slow; it works with either backend. Pick `keys` from the app: a terminal needs `ctrl+shift+v`. It checks `expect` and the deny list before touching the clipboard. `read: false` means no app asked for the text: take a screenshot before anything else, and don't paste again on your own. `clipboard` says whether the user's clipboard was put back: `restored` or `cleared` when it was, `replaced` when someone copied meanwhile and their copy stays, and `kept` (the pasted text stays), `failed` or `unknown` when the user's copy may be lost; tell the user then. An error whose `detail` says nothing was pasted sent no key.

A combination uses keysym names (`a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`) and the modifiers `shift`, `ctrl`, `alt`, `altgr` and `super`.

Pixel coordinates are in the screenshot named by `screenshot_ref`, and a pixel targets its centre. A ref is good for 60 seconds and for the lease it was taken under.

`element` takes an `element_ref` from `elements` listed under this lease and aims at the element's centre. The server checks the element again just before: still there, at the same place in its window, its parents with the names they were listed with, the same role, showing, and its centre inside the screenshot named by `screenshot_ref`. `element_stale` or `element_unmappable` means nothing was sent. `scroll` takes pixels only.

## Element actions

Listed with `elements`, when the session has an accessibility bus. Both need the lease, and the element's window must have keyboard focus: `expect` names it as `{"window_id": …}` or `{"app_id": "…"}`, and `"none"` is refused. They send no input event, so no `screenshot_ref` is needed, and they work on elements with no `layout_box`.

| Tool | Arguments | `observed` |
|---|---|---|
| `activate_element` | `element`, `expect`, optionally `action`: one of the element's `actions`; by default its first of `click`, `press`, `activate` or `toggle` | 300 ms after the app took it: `present`, with `element.states_set` and `element.states_cleared` when states changed (`checked`, say); `gone` when the element or its window went away, as when a button closes its dialog; `unknown`, with `detail`; or `uncertain` (`accepted: null`) when the reply was lost after it went out |
| `set_element_text` | `element` (an editable field: `editable` among its `states`), `text` (up to 64 KiB, replacing all of it; empty clears it), `expect` | `matched` when the field then holds as many characters as were set; `differs` when it holds another number, `element.characters`, as when the app filters or caps input; `unknown`, with `detail`; or `uncertain` (`accepted: null`) when the reply was lost |

`element.role` is the element's role and `element.action` the action taken, as its `kind` (`click`, `press`, `activate`, `toggle` or `other`) and `index` in the element's `actions`. Neither tool returns or logs a name or the text. Right before acting the server checks everything again, the element (still there, at the same place in its window, it and its parents with the names they were listed with, the same role, showing: `element_stale` or `element_unmappable` otherwise), the window's focus and `expect`, the lock screen and the stop flag, and sends nothing if any changed; `focus` is what that last check saw. A field the toolkit marks as a password field (role `password_text`) is refused with `secret_field`; one that only hides its text may not be marked, so the server can't catch every secret field. An app that declines answers `upstream_error`, and `uncertain` means it may have happened: take a screenshot before doing anything else, and don't repeat it on your own. When its `detail` says the stop flag or the lease cut it short, stop and tell the user, as for `stopped` or `lease_required`. `set_element_text` sends no key events, so autocompletion and Enter don't happen: use `type_text` for those.

## Fields every action result has

- `accepted`: true once niri or Noctalia took the request, false when nothing was sent (already in that state), null when the reply was lost.
- `observed`: what the server saw by the end of its wait, as in the tables above, or `interrupted` (someone else moved focus) or `uncertain`.
- `focused_window` when the observation ended.
- `typed`, for `type_text` that stopped early: the characters sent; `pressed` likewise for `key`; `submitted` with `submit`; `paste` for `paste`; `window` for `niri_action`; `noctalia` for `noctalia`; `element` for the element actions.

Every action takes `screenshot: true`: the result then has an image of the focused output taken once the screen stopped changing (`settled` in its metadata says whether it did within 1.5 seconds), and its `screenshot_ref` works for the pointer tools.

Each action waits up to five seconds, the shell tools two, and `niri_action` one second for its window to change. With `timeout`, `pending`, `none`, `interrupted`, `uncertain` or `unknown`, the result has an image of the focused output with its metadata in `screenshot` even without asking, or `screenshot_error` if it couldn't be taken.

## Screenshot targets

- `focused_output`: the output with keyboard focus.
- `output:<name>`: an output by name from `outputs`, such as `output:DP-1`.
- `region`, with `region: {x, y, width, height}` in layout coordinates (the coordinates `outputs` uses). The rectangle must lie inside one output.

Images are JPEG at most 1280 pixels wide by default. `max_width` can go higher for a region wider than that, and a region keeps the output's native scale when it fits. `format` is `jpeg` (the default) or `png`.

`save_path` writes a full-resolution PNG, whatever `max_width` is, to a new file relative to the user's `capture_dir`, such as `readme/editor.png`. It works only when the user set `capture_dir` in the policy file (`status` shows `policy.capture_dir`); otherwise it fails with `save_not_enabled`. The path must be relative, without `..`, and end in `.png`; its directories must exist; an existing file is never replaced, so pick a new name rather than retrying the same one.

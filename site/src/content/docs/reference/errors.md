---
title: Error reference
description: Every error name niri-computer-use returns, what causes it and what to do.
sidebar:
  order: 1
---

A failed call sets `isError` and returns `{"error": <name>, "detail": <upstream detail>}`. The names below are a stable contract; the `detail` carries niri's, Noctalia's or a program's own message, so quote it when you report a problem. This list is checked against `ErrorName` in [`src/error.rs`](https://github.com/ayagmar/niri-computer-use/blob/main/src/error.rs) when the site builds.

A mistake in the arguments isn't one of these: it comes back as plain text starting `invalid arguments:`, and nothing was done.

## Refusals that are yours to clear

An agent should stop and tell you when it gets one of these. Calling the action again won't help.

### `stopped`

**Cause:** the stop flag is set for this niri instance, by the stop key or `niri-computer-use stop`. A stop during an action cancels it with this error; anything niri had already accepted may have taken effect. A runtime directory that can't be read also counts as stopped.

**What to do:** when you want agents to act again, run `niri-computer-use resume`. If `status` shows `input_dirty`, run `niri-computer-use recover` first.

### `recovery_required`

**Cause:** the input-dirty marker exists: an earlier input call couldn't confirm that every key and button was released, for example because the server was killed mid-click. The `detail` names the marker's operation and phase.

**What to do:** if the `detail` says a cancelled call's input is still finishing, that input removes the marker itself within seconds; try again then. Otherwise the agent calls `release_desktop`, since `recover` needs the lease, and you run `niri-computer-use recover` and answer its question. See [Safety](../../concepts/safety/#input-that-may-be-stuck-and-recover).

### `screen_locked`

**Cause:** the screen is locked, or neither logind nor Noctalia can say that it isn't. `status` shows `lock.state` and, under `lock.logind_error`, why logind couldn't answer.

**What to do:** unlock the screen. If it is unlocked and `lock.state` is `unknown`, run niri as `niri --session` or run Noctalia; see [Troubleshooting](../troubleshooting/#the-lease-is-refused-with-screen_locked-while-the-screen-is-unlocked).

### `session_mismatch`

**Cause:** the Wayland display isn't served by the niri on niri's socket, so `WAYLAND_DISPLAY` and `NIRI_SOCKET` name two different compositors. The server checks this right before each input, screenshot and clipboard read, and refuses them, because `wtype`, `grim` and `wl-paste` would reach the other compositor. A niri that restarted while the server ran gives this too when another compositor took over the display. `status` shows the current check under `display_error`.

**What to do:** pass the agent's client both variables from the same niri session, or neither, and start a new agent session.

### `lease_held`

**Cause:** another server holds the lease on this niri instance. The `detail` names its PID, its client and since when.

**What to do:** finish or close the other agent's session; its server gives the lease up when it releases or exits. The stop key also takes the lease back.

### `read_only`

**Cause:** this build doesn't support the running niri's major or minor version, niri sent events this build can't parse, or the policy file is invalid. The `detail` says which.

**What to do:** for niri's version, use a build that matches it (`status` shows `niri.version` and `niri.ipc_crate`). For the policy file, fix the error `status` shows under `policy.error`, then restart the agent's session.

### `app_denied`

**Cause:** the window with keyboard focus belongs to an app on `deny_input_app_ids`, or for `elements`, the window asked about does, or for `activate_element` and `set_element_text`, the element's window does, or for `close_window` and `niri_action`'s `CloseWindow`, the window it would close does.

**What to do:** nothing, if you meant it. Otherwise remove the app from the [policy file](../../concepts/configuration/#deny_input_app_ids) and restart the agent's session.

### `untested_output_config`

**Cause:** a pointer tool was called while niri's outputs are a setup no live test covers: more than one enabled output, or a monitor with a transform other than `Normal`. The `detail` lists the enabled outputs and their transforms.

**What to do:** the pointer tools work with one monitor at transform `Normal`. Structured actions and the keyboard work with any setup.

### `refused`

**Cause:** the selected keyboard backend can't send the requested input safely: an unknown `NIRI_COMPUTER_USE_KEYBOARD` value, a text with more symbols missing from the layout than the native backend has spare keys, a missing control character, a `key` keysym the layout lacks under native, or held pointer `keys` without the native backend.

**What to do:** split the text, or type it with the default backend; the `detail` says which limit was hit. An agent shouldn't change the backend itself.

## Mistakes an agent can correct

### `lease_required`

**Cause:** an action tool was called without holding the lease, or the lease file was removed or replaced before or during the action, so another server may hold the lease. The server then gives its lease up, and cancels an action that was running.

**What to do:** call `acquire_desktop` first, if you asked the agent to act.

### `unknown_preset`

**Cause:** `launch` named a preset the policy file doesn't have. The `detail` lists the names it has.

**What to do:** use one of those names, or add a [preset](../../concepts/configuration/#preset) and restart the agent's session.

### `ref_invalid`

**Cause:** a pointer tool's `screenshot_ref` can't be used. The `detail` starts with the reason: `unknown_ref` (not a screenshot of this lease, or niri's event stream reconnected since), `expired` (over 60 seconds old), `output_changed` (the output moved, resized, changed scale or transform, or is gone) or `out_of_bounds` (the pixel is outside the image).

**What to do:** take a new screenshot and aim from it; for `out_of_bounds`, use a pixel inside the image.

### `focus_mismatch`

**Cause:** a keyboard tool's `expect` doesn't match the window with keyboard focus, or for `activate_element` and `set_element_text`, the element's window doesn't have keyboard focus or `expect` doesn't name it. The `detail` says what has focus. Often a dialog or notification took focus; an open Noctalia panel leaves no window focused. Nothing was typed or sent.

**What to do:** look at `desktop_state` and a screenshot, focus the intended window with `focus_window` if that is what was meant, then type again.

### `text_too_long`

**Cause:** `type_text` with over 1000 characters, `paste` with over 1 MiB, or `set_element_text` with over 64 KiB of UTF-8. Nothing was typed or set.

**What to do:** split the text into calls of at most 1000 characters, or use `paste`.

### `clipboard_unsaved`

**Cause:** `paste` couldn't save the clipboard whole before replacing it: every type it offers, over 16 MiB in all, or not read within two seconds. Or the clipboard holds what its owner marked as a secret (`x-kde-passwordManagerHint`), or something else was copied while it was being saved. Nothing changed. An earlier paste's text, which a `kept` outcome leaves on the clipboard marked as a secret, isn't refused: the next `paste` replaces it and reports `clipboard: cleared`.

**What to do:** type the text with `type_text` instead, in parts of at most 1000 characters.

### `save_not_enabled`

**Cause:** `screenshot` was given `save_path`, but the policy file has no `capture_dir`, or `capture_dir` starts with `~/` and `HOME` isn't set.

**What to do:** add a [`capture_dir`](../../concepts/configuration/#capture_dir) and restart the agent's session, or take the screenshot without saving.

### `panel_not_allowed`

**Cause:** `shell_open` or `shell_close` named a panel other than `control-center`, `wallpaper` or `tray-drawer`. Nothing was sent.

**What to do:** use one of those three. The others, such as the session menu and the launcher, are deliberately out of reach.

### `not_accessible`

**Cause:** the window's app isn't on the accessibility bus, or has no accessible window that is this one.

**What to do:** aim at screenshot pixels for that window.

### `ambiguous_window`

**Cause:** the app has several accessible windows that could be the one asked about, so the server won't guess.

**What to do:** aim at screenshot pixels for that window.

### `element_stale`

**Cause:** an element ref's window, app or element is gone, the element is now something else, has another name, or sits elsewhere in its window's tree (an app may reuse an object for another record, as a long list does with its rows), or the ref isn't from this lease.

**What to do:** call `elements` again and aim from its new list.

### `element_unmappable`

**Cause:** the element is still there but can't be aimed at now. The `detail` starts with `frame_size_mismatch` (the app's coordinates for this window can't be trusted), `not_showing`, `empty` or `outside_screenshot` (its centre is outside the screenshot the pointer aims through).

**What to do:** for `frame_size_mismatch`, aim at a screenshot pixel or use `activate_element` instead. For `not_showing`, bring it into view first; for `outside_screenshot`, take a screenshot that shows it.

### `secret_field`

**Cause:** `activate_element` or `set_element_text` was aimed at a field the toolkit marks as a password field: an element whose role is `password_text`, as GTK 4's password entry and its entries for a password or PIN, GTK 3's entries that hide their text and Qt's password fields are. The role is read again right before acting, so a field that became one is refused too. Nothing was sent.

**What to do:** have the user type it. A field that only hides its text may not be marked: a GTK 4 entry that hides its text without a password purpose is a plain `text` and isn't refused, so don't fill or activate a field that may hold a secret.

### `unrestricted_required`

**Cause:** `niri_action` was given a gated action, such as `Spawn`, `Quit` or `LoadConfigFile`, and the user's policy doesn't turn on `unrestricted`. The `detail` names the action and why it is gated. Nothing was sent.

**What to do:** tell the user the action needs [`unrestricted = true`](../../concepts/configuration/#unrestricted). Don't reach the same result another way.

## Failures upstream

These mean niri, a program or Noctalia didn't answer as expected. Report the name and the `detail`; repeating the same call in a loop won't help.

### `niri_unavailable`

**Cause:** `NIRI_SOCKET` isn't set and the server found no running niri of yours, or found several; or niri's socket is missing, refused the connection or closed it.

**What to do:** `status` shows the reason under `niri.error`. With several niri sessions, start the agent from inside the one you want or pass `NIRI_SOCKET` on (see [Session variables](../../start/clients/#session-variables)).

### `deadline_exceeded`

**Cause:** niri, a program or an app didn't answer in time: two seconds for niri and `wl-paste`, five for `grim`, three for an `elements` walk.

**What to do:** check whether niri or the app is stuck. A stopped app makes `elements` time out; a large tree that is only slow gives a partial listing with `capped_reason: "budget_exhausted"` instead.

### `upstream_error`

**Cause:** niri, a program, Noctalia or an app on the accessibility bus answered with an error or with something unreadable; the `detail` keeps its message, exit status and stderr. For `activate_element` and `set_element_text`, also when the app refused the action or the text; their `detail` names the call and the D-Bus error's name but not the message that came with it, which the app writes and could hold a field's text. Also when the server's runtime directory was removed while it ran, which cancels a running action.

**What to do:** read the `detail`. After a removed runtime directory, restart the agent's session.

### `noctalia_unavailable`

**Cause:** Noctalia is installed but didn't answer on its socket within two seconds, usually because it isn't running.

**What to do:** start Noctalia, or ignore the shell tools. Without Noctalia running, the lock state relies on logind alone.

## Shared mode

These come only from a server in [shared mode](../../concepts/configuration/#shared), which relays its client to one engine per niri instance.

### `engine_lost`

**Cause:** the shared engine ended, killed or crashed, or stopped reading from this server for five seconds, while the call was in flight, or since the session's last call. What a call in flight did is unknown, and the session's lease, refs and focus record went with the engine. The `detail` names the engine's PID when the server knew it.

**What to do:** look at `desktop_state` or a screenshot before acting again, and call `acquire_desktop` again if you still need it. The next call reaches a new engine; the engine's own error, if any, is in `engine.log` in the runtime directory.

### `engine_unavailable`

**Cause:** after an engine was lost, the server couldn't reach or start a new one within five seconds, or the new one refused it. The `detail` says why. The next call tries again.

**What to do:** read the `detail` and `engine.log` in the runtime directory. A server that can't reach an engine when it starts serves its client on its own instead, so this happens only after a loss.

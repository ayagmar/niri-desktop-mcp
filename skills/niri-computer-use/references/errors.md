# niri-computer-use errors

A failed call comes back with `isError`. There are two shapes.

**Argument mistakes**, such as an unknown output or a window id that doesn't exist, are plain text starting `invalid arguments:`. Nothing was done. Fix the arguments and call again.

**Everything else** is `{"error": <name>, "detail": <upstream detail>}`. Quote the `detail` when you tell the user; it carries niri's or Noctalia's own message.

## Stop and tell the user

These are the user's decisions. Don't call the refused action again and don't try to work around it.

| Error | Cause |
|---|---|
| `stopped` | The user pressed the stop key or ran `niri-computer-use stop`; it also cancels a running action. Only the user's `niri-computer-use resume` clears it. |
| `recovery_required` | Input may be stuck from an earlier crash. Only the user's `niri-computer-use recover` clears it, and it needs the lease: call `release_desktop` first. The exception: when the `detail` says input is still finishing, or another server's may still be finishing, wait a few seconds and try once more. |
| `screen_locked` | The screen is locked, or no source can say it isn't. |
| `session_mismatch` | The server's `WAYLAND_DISPLAY` and `NIRI_SOCKET` belong to two different compositors, so input, screenshots and clipboard reads are refused. The user has to fix the client's environment. |
| `lease_held` | Another agent holds the lease (`acquire_desktop` only). |
| `read_only` | The niri version isn't supported, niri sent events this build can't read, or the policy file is invalid. |
| `app_denied` | The user's policy denies input to the focused app, or `elements`, element actions or closing for the window named. |
| `secret_field` | `set_element_text` was aimed at a password field. Its text is never set by the server; ask the user to type it themselves. |
| `untested_output_config` | The pointer only runs on one monitor at transform `Normal` (or in a nested niri). |
| `unrestricted_required` | `niri_action` was given a gated action (`Spawn`, `SpawnSh`, `Quit`, `LoadConfigFile`, niri's screenshot actions, monitor power, casts and a few more). It needs `unrestricted = true` in the user's policy file; tell the user. |
| `refused` | The selected keyboard backend cannot safely send the requested input: for example a text with more symbols missing from native's layout than it has spare keys, a missing control character, invalid backend selection, or held modifiers without native. Tell the user; do not change the backend or bypass the tools. |

## Fix and continue

| Error | What to do |
|---|---|
| `lease_required` | You called an action without the lease. Call `acquire_desktop` if the user asked you to act. |
| `unknown_preset` | `launch` named a preset that doesn't exist; the detail lists the ones that do. If none fits and `status.unrestricted.enabled` is true, start the app with `niri_action` `Spawn` and `wait_for` its window; otherwise ask the user to add one. |
| `ref_invalid` | The detail starts with `unknown_ref`, `expired`, `output_changed` or `out_of_bounds`. Take a new screenshot and aim again from it; for `out_of_bounds`, use a pixel inside the image. |
| `element_stale` | The element ref's window, app or element is gone, it is now another kind of element, has another name or moved in its window, or this lease didn't list it. Call `elements` again and aim from its new list. |
| `element_unmappable` | The element is there but can't be aimed at now. The detail starts with `frame_size_mismatch` (the app's coordinates for this window can't be trusted: aim at a screenshot pixel or use `activate_element` instead), `not_showing` (bring it into view first), `empty`, or `outside_screenshot` (take a screenshot that shows it). |
| `focus_mismatch` | The window in `expect` doesn't have keyboard focus, or for an element action the element's window doesn't (a shell panel may hold the keyboard). Look at `desktop_state` and a screenshot, focus the right window with `focus_window` if that's what you meant, then try again. |
| `text_too_long` | Over 1000 characters for `type_text`, 1 MiB for `paste`, or 64 KiB for `set_element_text`; nothing was typed or set. Split the text into calls of at most 1000 characters, pass `submit: true` only on the last, and send nothing more once a call comes back with `typed`. |
| `clipboard_unsaved` | `paste` couldn't save the user's clipboard whole (too large, an owner that didn't answer, something its owner marked secret other than an earlier paste's text, or a copy made while it saved), so it changed nothing. Use `type_text` instead, in parts of at most 1000 characters. |
| `save_not_enabled` | `screenshot`'s `save_path` needs a `capture_dir` in the user's policy file, and there is none. Ask the user to add one; don't save the image another way. |
| `not_accessible` | The window's app isn't on the accessibility bus, or has no accessible window that is this one. Use screenshots for that window. |
| `ambiguous_window` | The app has several accessible windows that could be this one. Use screenshots for that window. |
| `panel_not_allowed` | Only `control-center`, `wallpaper` and `tray-drawer` can be opened. Don't reach another panel some other way. |

## Report and don't loop

`niri_unavailable`, `deadline_exceeded`, `upstream_error` and `noctalia_unavailable` mean niri, a helper program, Noctalia or an app on the accessibility bus didn't answer as expected; for an element action, `upstream_error` can mean the app refused it, and its detail names only the D-Bus error, not the app's message. Tell the user the name and detail. Don't repeat the same call in a loop.

## Shared mode

`engine_lost` means the shared engine serving this session ended. What a call in flight did is unknown, and the lease is gone: look at `desktop_state` or a screenshot before acting again, and call `acquire_desktop` again if you still need it. `engine_unavailable` means no shared engine could be reached for this call; the next call tries again. Tell the user if it keeps happening.

## Not an error

`focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview.

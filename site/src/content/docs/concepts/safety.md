---
title: Safety
description: What niri-computer-use enforces, what it doesn't, and how you take the desktop back.
sidebar:
  order: 1
---

An agent using niri-computer-use works on your real desktop, with your apps and your files. A click or a key can do anything you could do. The server adds guardrails that catch mistakes and keep you in control; it is not a sandbox. To report a security problem, see [SECURITY.md](https://github.com/ayagmar/niri-computer-use/blob/main/SECURITY.md).

## The lease

Looking needs nothing: any connected agent can call `status`, `desktop_state`, `screenshot` and the other reading tools. Acting needs the lease. An agent takes it with `acquire_desktop` and gives it up with `release_desktop`. One agent session holds it per niri instance; another session's `acquire_desktop` is refused with `lease_held`, and its detail names the holder's PID, client and since when. That holds in [shared mode](../configuration/#shared) too, where one engine serves every client.

`acquire_desktop` notes the window you were on. `release_desktop` with `restore_focus: true` puts keyboard focus back there. The lease is also given up when the agent's session ends, because its client closed the connection or its server exited, and when you press the stop key.

## Stop and resume

Bind `niri-computer-use stop` to a key in your niri config (see [First session](../../start/first-session/#bind-the-stop-key)). Pressing it:

- cancels the action that is running, which ends with `stopped`; anything niri had already accepted may have taken effect. An element action whose call to the app had already gone out ends as `uncertain` with `accepted: null` instead, since the app may have acted
- takes the lease back from the agent, once a screenshot or readiness check under way has finished
- refuses every later `acquire_desktop` and action with `stopped`

The flag stays until you run `niri-computer-use resume`. An agent can't press the stop key: niri's keybinds don't fire from the virtual keyboard and pointer the server uses.

## Input that may be stuck, and `recover`

Before `click`, `drag`, `key`, `type_text` or `paste` send any input, the server writes an input-dirty marker, and it removes the marker once the input is released. `paste` takes the clipboard before it writes its marker, and its keeper process, not the marker, puts the clipboard back (see [`paste`](../../tools/keyboard/#paste)).

If the call is cancelled midway, the server releases any held button or key. A `wtype` already typing, or a paste key already sent, isn't stopped: it finishes after the cancellation, and the marker stays until it has, for `paste` until the keeper has reported. Meanwhile other calls refuse with `recovery_required` and a `detail` saying a cancelled call's input is still finishing; they can try again within seconds.

If the input can't be released, or its cleanup ended without removing the marker, for example because `wtype` was killed by a signal, the marker stays. Every action then refuses with `recovery_required`, and the `detail` tells the agent to call `release_desktop` and you to run `niri-computer-use recover`, which needs the lease. A server can only tell whether its own input is still finishing: for a marker another server wrote, such as a shared engine's seen by a standalone server, while that server still runs, the `detail` says that input may still be finishing and to try again in a few seconds before `recover`. A marker whose server has exited, or whose crash guardian has sent its releases, gets the `recover` wording at once.

`recover` ends the input program the marker names, releases any pointer button or key it names, then asks you to check that no key or button is held. It clears the marker only when you type `yes`. See the [CLI reference](../../reference/cli/#recover).

## The crash guardian

Each standalone `serve`, and each shared engine, starts a small guardian process. If the server or engine dies while it holds input, for example after `kill -9`, the guardian at once releases the native keyboard's keys and the pointer buttons that the marker records. It leaves alone a `wtype` call, which outlives the server and finishes on its own, and a marker it can't read, and a release can fail or time out. The marker still stays until you run `recover`.

## The lock gate

The server acts only on a screen it knows is unlocked. It asks logind for niri's session's `LockedHint` and, when Noctalia runs, Noctalia's own lock state. If either says locked, or neither can answer, `acquire_desktop` and every action refuse with `screen_locked`. The state is checked again before each action, because the screen can lock while an agent holds the lease.

logind only knows the lock state when niri runs as `niri --session`. Without that and without Noctalia, the state is `unknown` and the server won't act.

## Refusals

Before each action the server checks, in this order: the stop flag (`stopped`), the input-dirty marker (`recovery_required`), the lease (`lease_required`), niri's version and event stream and the policy file (`read_only`), and the lock state (`screen_locked`). The tools then check their own arguments:

- `launch` starts only presets from your [policy file](../configuration/), never a command the agent writes.
- `niri_action` refuses niri's actions that run programs, write files or reach past the window layout, such as `Spawn`, `Quit` and `LoadConfigFile`, with `unrestricted_required`, and the `noctalia` tool isn't listed, unless you turn on [`unrestricted`](../configuration/#unrestricted). Turning it on lets an agent run any program through the server; every call still needs the lease, stops at the stop key and is logged.
- The pointer and keyboard tools refuse with `app_denied` while the focused window's `app_id` is on your deny list. `elements`, `activate_element` and `set_element_text` refuse a denied app's window, and `close_window` and `niri_action`'s `CloseWindow` refuse to close one, whether named or focused. Focus and layout actions, such as `focus_window` or making a window fullscreen, still reach a denied window: they move it, not what is in it.
- The keyboard tools need `expect`, the window the agent means to type into, and refuse with `focus_mismatch` rather than type elsewhere. They stop if focus moves while they type.
- The pointer tools aim only at pixels of a screenshot the agent took under its lease, at most a minute old, of an output that hasn't changed since. They run only on one monitor at transform `Normal`.
- `shell_open` and `shell_close` open only three Noctalia panels.

A refusal sends nothing. The [error reference](../../reference/errors/) lists every refusal.

## Evidence screenshots

Every action reports `accepted`, whether niri took the request, and `observed`, what niri's event stream showed afterwards. When the outcome is in doubt, such as a timeout, focus moving to another window, or a lost reply, the result comes with a fresh screenshot of the focused output, so the agent sees what happened instead of guessing. The server never retries an action.

## The audit log

Every tool call is appended to `~/.local/state/niri-computer-use/audit.jsonl` (or under `$XDG_STATE_HOME`): the time, the client, the tool, its arguments and the outcome. The directory is `0700` and the file `0600`. Typed and pasted text, clipboard contents, window titles and images are never written; the log keeps their length. Text the agent passes on goes the same way: `niri_action` logs the action's name and field names, its numbers and booleans, and every string, such as a `Spawn` command, as its length in bytes, and JSON that isn't a niri action only as its size; `noctalia` logs how many arguments and how long.

## What isn't a boundary

- Screenshots, window titles, accessible names and the clipboard reach any connected agent without a lease, even while the screen is locked. Connect only agents you trust with what is on your screen.
- The deny list follows keyboard focus, and a click lands wherever the pointer is: it can hit a denied window while an allowed app has focus.
- Apps choose their own `app_id`, so any app can call itself anything.
- The preset rules refuse shells, interpreters and terminals with arguments, but a wrapper script gets past any list. With `unrestricted` on there are no preset rules.
- You can still type and click while an agent works. The server notices focus moving and stops typing, but nothing keeps you and an agent from moving the pointer at the same time.
- With the default `wtype` backend, a part of up to 100 characters that has started keeps typing after a stop or a focus change.
- The lease belongs to a server process. Anything that runs as your user can start `niri-computer-use` too.

For a stronger limit, give the agent a separate desktop holding only the apps it may use, such as a nested niri session.

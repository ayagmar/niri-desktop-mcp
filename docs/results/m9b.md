# M9b results

`activate_element` and `set_element_text`, on branch `m9b-actions` from `main` at `535fdd8`.

Environment: Rust 1.99.0; niri 26.04; cage 0.3.1; grim 1.5.0; Noctalia 5.2.1; at-spi2-core 2.62.0.1; GTK 4.24.1 and GTK 3.24.52 with PyGObject 3.58.1; Qt 6.12.0 (`qml6`, qt6-declarative). No dependency was added.

Every nested run below ran in the private headless cage, with its own session bus and accessibility bus under `TEST_DIR`. No run read the host's accessibility bus, sent input to the host, or viewed host pixels, clipboard or window titles. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes.

## Acceptance

| Command | Run | Result |
|---|---|---|
| `make check` | before every commit | pass: 291 server, 126 harness and 178 protocol tests, one protocol test ignored; formatting, Clippy, rustdoc, cargo-deny, cargo-machete, typos and ShellCheck pass |
| `make nested-a11y` | 1791664797-1903552, 1791665383-3224458 | pass |
| `make nested-a11y SSD=1` | 1791664846-2018448 | pass |
| `make nested-a11y SHARED=1` | 1791664908-2160141 | pass |
| `make nested-a11y SSD=1 SHARED=1` | 1791664956-2270723, 1791665432-3336754 | pass |
| `make nested-a11y SCALE=1.5` | 1791665030-2436052 | pass |
| `make nested-input` | 1791665205-2835952 | pass |
| `make nested-actions` | 1791665301-3045121 | pass |
| `make nested-shell` | 1791665317-3080077 | pass |
| `make nested-control` | 1791665325-3098396 | pass |

The audit log check was added after the first five `nested-a11y` runs; 1791665383-3224458 and 1791665432-3336754 ran it. Its first version, in runs 1791665094-2583997 and 1791665142-2695449, failed: it required a role for every element action, but a ref that the stop flag or a release had revoked no longer resolves, so the server logs its role as null. The check now requires the role on the calls that went through.

Each `nested-a11y` run passed these checks, for GTK 4, GTK 3 and Qt Quick, with client-side and server-side decorations alike:

- 20 activations of the counted button, each counted exactly once and observed `present`. All 20 were already counted when each call returned.
- The plain entry's text set and read back whole by the fixture, `matched`.
- Every field marked as a password field (role `password_text`) refused with `secret_field`, its change counter at 0.
- `focus_mismatch` with a decoy window focused, `stopped` with the stop flag set, and `lease_required` after a release, each leaving the counter unchanged for half a second.
- `Dismiss` observed `gone`, then the window's other ref `element_stale`.
- GTK 4: 64 KiB set and `differs` with 65534 characters kept, a byte more `text_too_long`; `Rename` to a denied `app_id`, then `app_denied`, counter at 0.
- With the audit check: 68 element actions logged with their role, and no element action entry holding the text set or an element's name.

What each toolkit's fixture exposes on the bus, from the runs' logs:

| | Button actions | Entry | Password field |
|---|---|---|---|
| GTK 4 | `Click` | `text`, `editable` | `GtkPasswordEntry` and the PIN entry: `password_text` |
| GTK 3 | `Click` | `text`, `editable` | entry with `visibility` off: `password_text` |
| Qt Quick | `Press`, `Set Focus` | `text`, `editable` | `TextField` in `Password` echo mode: `password_text`, no accessible name |

Artifacts are in `target/e2e/<run>/` and are not committed.

## Observed measurements

| Run | GTK 4 / GTK 3 / Qt activation, ms min / median / max (harness) | 20 activations, s | `set_element_text`, ms (harness) |
|---|---|---|---|
| CSD, 1791664797-1903552 | 314/318/366, 318/321/324, 320/326/328 | 6.39–6.50 | 56–58 |
| SSD, 1791664846-2018448 | 321/328/330, 325/332/334, 329/336/337 | 6.54–6.68 | 58–60 |
| SHARED, CSD, 1791664908-2160141 | 314/318/366, 317/322/324, 320/326/328 | 6.41–6.51 | 56–58 |
| SHARED, SSD, 1791664956-2270723 | 322/329/331, 325/331/334, 328/336/338 | 6.55–6.69 | 57–61 |
| Scale 1.5, CSD, 1791665030-2436052 | 314/319/366, 317/321/323, 320/326/328 | 6.41–6.50 | 56–58 |

By the server's audit log in runs 1791665383-3224458 and 1791665432-3336754: an activation observed `present` took 302–316 ms, nearly all of it the 300 ms settle; `gone` took 3–4 ms for GTK 3 and Qt, whose windows close at once, and 254 ms for GTK 4, whose button acts after 250 ms; `set_element_text` took 1–2 ms, and 18–22 ms for 64 KiB. The harness's numbers include its MCP client and the fixture's read-back. These are samples from a few runs, not latency guarantees.

## Not covered here

- Real applications on the user's accessibility bus.
- A GTK 4 entry that hides its text with a free-form input purpose isn't refused: it is a plain `text` on the bus. The fixes below record what it exposes.
- The deny list on GTK 3 and Qt windows: GTK 3 has no `GdkWayland` typelib here and Qt no per-window `app_id`, so only GTK 4 changes its `app_id`.
- `CharacterCount` for text outside the Basic Multilingual Plane, where a toolkit may count UTF-16 units: the synthetic text is ASCII.
- A layer-shell surface over the element: the focus check refuses an element while a panel holds keyboard focus, but nothing checks what is drawn on top.

## After review

A blind review found six problems; the follow-up commits fix them (see `docs/decisions.md`, "element actions after review"). What GTK 4 exposes for an entry that only hides its text, from `make nested-a11y` run 1791676844-2805663 (the same in the other three), read with `busctl` on the nested bus:

| | `Hidden entry` | `Plain entry` |
|---|---|---|
| Role | `text` (61) | `text` (61) |
| States | `[1124075648, 1024]` | `[1124075648, 1024]` |
| Interfaces | Accessible, Component, Text, EditableText, Action | the same |
| `GetAttributes` | `toolkit: GTK` | `toolkit: GTK` |
| `Text.GetDefaultAttributes` | `invisible: false` among 21 others | the same |
| `Text.GetText` | `●●●●●●●●●●` for the synthetic `ncu hidden` | empty |

Only the text tells them apart, and only while the field holds some. It isn't refused, and `secret_field` is documented as covering fields marked as passwords.

Runs at `9ff91c2`, the last code commit, each with C1's host snapshot unchanged, no host journal entries from the run and no leftover processes:

| Command | Run | Result |
|---|---|---|
| `make check` | before every commit | pass: 293 server, 126 harness and 188 protocol tests, one protocol test ignored |
| `make nested-a11y` | 1791676844-2805663 | pass |
| `make nested-a11y SHARED=1` | 1791676892-2923863 | pass |
| `make nested-a11y SSD=1` | 1791676941-3048006 | pass |
| `make nested-a11y SSD=1 SHARED=1` | 1791677003-3208484 | pass |

Activations took a median of 316–329 ms per call, as before the fixes; the last gate adds a readiness report, one AT-SPI read and a few file reads. An earlier run, 1791676674-2387079, passed every M9 and M9b check but failed the host journal check: three `sudo` entries from the terminal's own cgroup, which the harness runs in and which no nested process wrote; the check fails on anything in that cgroup by design.

## After the second review

The second round of fixes reads the element before the last gate checks the server's own state, checks the names of the element and the elements it sits in, and reports a stop or a lost lease after the acting call went out as `uncertain`. Each finding has a protocol test against the mock application; the nested runs check that the counter buttons, which relabel themselves on every activation, keep their refs through the 20 activations per toolkit, and that the 100 pointer clicks per ref of M9 still pass with the parents' names checked.

Runs at `23891c5`, the last code commit, each with C1's host snapshot unchanged, no host journal entries from the run and no leftover processes:

| Command | Run | Result |
|---|---|---|
| `make check` | before every commit | pass: 296 server, 126 harness and 197 protocol tests, one protocol test ignored |
| `make nested-a11y` | 1791680102-2769815 | pass |
| `make nested-a11y SHARED=1` | 1791680163-2929674 | pass |
| `make nested-a11y SSD=1` | 1791680211-3047320 | pass |
| `make nested-a11y SSD=1 SHARED=1` | 1791680273-3201259 | pass |

Activations took a median of 315–329 ms per call, as before; the last gate now reads the element's lineage, names, actions, states and role again, one call at a time, beside the readiness report.

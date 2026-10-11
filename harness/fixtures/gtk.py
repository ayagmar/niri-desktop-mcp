"""GTK fixtures; only connect to the harness's private Wayland session.

`button` counts a button's real activations. `entry` reports a text entry's text after
every change, and each activation (Enter) with the text it submitted, then clears it.
`a11y` is a 400x300 window for the accessibility checks: a label, the `Primary` and
`Second` buttons, which count their activations in `primary-count` and `second-count`,
the `Plain entry`, which reports its text in `a11y-entry-text`, the `Password entry` and
`Pin entry` (an entry for a PIN that hides it), which count their changes in
`password-changes` and `pin-changes` without their text, the `Hidden entry`, which
only hides its synthetic text, as a free-form entry can, a `Vanish` button that
removes itself when clicked, a `Dismiss` button that closes the window, and a `Rename`
button that changes the window's Wayland app_id to `org.ncu.Denied`.
"""

import json
import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve(strict=True)
mode = sys.argv[2]
if mode not in ("button", "entry", "a11y"):
    raise RuntimeError(f"unknown fixture {mode}")
run = root / "run"
for name in ("NIRI_SOCKET", "WAYLAND_DISPLAY"):
    endpoint = Path(os.environ[name])
    if not endpoint.is_absolute():
        endpoint = run / endpoint
    if not endpoint.resolve(strict=True).is_relative_to(run):
        raise RuntimeError(f"{name} is not nested")
if Path(os.environ["XDG_RUNTIME_DIR"]) != run:
    raise RuntimeError("runtime is not nested")

os.environ["HOME"] = str(root)
os.environ["GDK_BACKEND"] = "wayland"
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("GdkWayland", "4.0")
from gi.repository import GdkWayland, Gio, GLib, Gtk

counter = root / "activations"
app_id = {"button": "org.ncu.Activation", "entry": "org.ncu.Entry", "a11y": "org.ncu.A11y"}[mode]
app = Gtk.Application(application_id=app_id, flags=Gio.ApplicationFlags.NON_UNIQUE)
clicks = 0
submits = 0


def report(name, text):
    """Replaces the file whole, so a reader never sees half of it."""
    partial = root / f".{name}"
    partial.write_text(text, encoding="utf-8")
    partial.replace(root / name)


def clicked(button):
    global clicks
    clicks += 1
    button.set_label(f"Activations: {clicks}")
    counter.write_text(str(clicks), encoding="utf-8")


def changed(entry):
    report("entry-text", entry.get_text())


def submitted(entry):
    global submits
    submits += 1
    report("entry-submits", json.dumps({"count": submits, "text": entry.get_text()}))
    entry.set_text("")


def button_window(window):
    button = Gtk.Button(label="Activations: 0")
    button.connect("clicked", clicked)
    window.set_child(button)
    counter.write_text("0", encoding="utf-8")


def entry_window(window):
    entry = Gtk.Entry()
    entry.connect("changed", changed)
    entry.connect("activate", submitted)
    window.set_child(entry)
    report("entry-text", "")
    report("entry-submits", json.dumps({"count": 0, "text": ""}))
    entry.grab_focus()


counts = {"primary": 0, "second": 0}
changes = {"password": 0, "pin": 0}


def counted(button, key):
    counts[key] += 1
    button.set_label(f"{key.capitalize()}: {counts[key]}")
    report(f"{key}-count", str(counts[key]))


def secret_changed(entry, key):
    changes[key] += 1
    report(f"{key}-changes", str(changes[key]))


def labelled(widget, label):
    widget.update_property([Gtk.AccessibleProperty.LABEL], [label])
    return widget


def a11y_window(window):
    grid = Gtk.Grid(row_spacing=6, column_spacing=6, column_homogeneous=True)
    for side in ("top", "bottom", "start", "end"):
        getattr(grid, f"set_margin_{side}")(16)
    grid.attach(Gtk.Label(label="Accessibility fixture"), 0, 0, 2, 1)
    for column, key in enumerate(counts):
        button = Gtk.Button(label=f"{key.capitalize()}: 0")
        button.connect("clicked", counted, key)
        grid.attach(button, column, 1, 1, 1)
        report(f"{key}-count", "0")
    entry = labelled(Gtk.Entry(), "Plain entry")
    entry.connect("changed", lambda entry: report("a11y-entry-text", entry.get_text()))
    grid.attach(entry, 0, 2, 1, 1)
    report("a11y-entry-text", "")
    password = labelled(Gtk.PasswordEntry(), "Password entry")
    pin = labelled(
        Gtk.Entry(visibility=False, input_purpose=Gtk.InputPurpose.PIN), "Pin entry"
    )
    for column, (key, secret) in enumerate((("password", password), ("pin", pin))):
        secret.connect("changed", secret_changed, key)
        grid.attach(secret, column, 3, 1, 1)
        report(f"{key}-changes", "0")
    hidden = labelled(Gtk.Entry(visibility=False, text="ncu hidden"), "Hidden entry")
    grid.attach(hidden, 0, 5, 2, 1)
    vanish = Gtk.Button(label="Vanish")
    vanish.connect("clicked", lambda button: grid.remove(button))
    grid.attach(vanish, 1, 2, 1, 1)
    dismiss = Gtk.Button(label="Dismiss")
    dismiss.connect("clicked", lambda button: window.close())
    grid.attach(dismiss, 0, 4, 1, 1)
    rename = Gtk.Button(label="Rename")
    rename.connect(
        "clicked",
        lambda button: GdkWayland.WaylandToplevel.set_application_id(
            window.get_surface(), "org.ncu.Denied"
        ),
    )
    grid.attach(rename, 1, 4, 1, 1)
    window.set_child(grid)


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(*((400, 300) if mode == "a11y" else (320, 240)))
    window.set_title(f"{mode} fixture")
    {"button": button_window, "entry": entry_window, "a11y": a11y_window}[mode](window)
    window.present()


app.connect("activate", activate)
GLib.timeout_add_seconds(240 if mode == "a11y" else 45, app.quit)
app.run([])

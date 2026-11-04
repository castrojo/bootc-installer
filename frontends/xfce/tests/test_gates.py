"""Blocking hardware gates: which gate, and that each gate builds.

Mirrors GNOME's do_activate branches (RAM, CPU, then UEFI) and the
IGNORE_RAM / IGNORE_CPU escape hatches its Continue buttons relaunch with.
"""

import os
import sys

import gi

gi.require_version("Gtk", "3.0")

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tuna_installer_xfce import gates  # noqa: E402


def test_no_gate_when_everything_is_met(monkeypatch):
    assert gates.required_gate(unmet=[]) is None
    # A live probe that finds nothing unmet gates nothing either.
    import tuna_installer_xfce.core as core
    monkeypatch.setattr(core, "unmet_requirements", lambda: [])
    assert gates.required_gate() is None


def test_ram_before_cpu_before_unsupported():
    assert gates.required_gate(unmet=["ram", "cpu", "uefi"]) == "ram"
    assert gates.required_gate(unmet=["cpu", "uefi"]) == "cpu"
    assert gates.required_gate(unmet=["uefi"]) == "unsupported"


def test_ignore_env_clears_its_gate(monkeypatch):
    monkeypatch.setenv("IGNORE_RAM", "1")
    assert gates.required_gate(unmet=["ram", "cpu"]) == "cpu"
    monkeypatch.setenv("IGNORE_CPU", "1")
    assert gates.required_gate(unmet=["ram", "cpu"]) is None
    # UEFI has no escape hatch: an unbootable machine cannot continue.
    assert gates.required_gate(unmet=["uefi"]) == "unsupported"


def test_each_gate_builds_and_names_the_product():
    for name, builder in gates.GATE_BUILDERS.items():
        win = builder()
        assert win.get_title().startswith("TunaOS") or "Installer" in win.get_title(), name
        win.destroy()


def test_gates_mention_the_requirement():
    bodies = []
    for builder in gates.GATE_BUILDERS.values():
        win = builder()
        labels = [c.get_text() for c in win.get_children()[0].get_children()
                  if hasattr(c, "get_text")]
        bodies.append("\n".join(labels))
        win.destroy()
    assert any("memory" in b for b in bodies)
    assert any("UEFI" in b for b in bodies)


def test_exit_confirm_and_log_dialogs_build():
    import gi as _gi
    _gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk
    parent = Gtk.Window()
    for dialog_cls in (gates.ConfirmExitDialog, gates.PoweroffDialog,
                       gates.InstallLogDialog):
        dialog = dialog_cls(parent)
        dialog.destroy()
    parent.destroy()

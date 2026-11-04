"""Cancel-on-exit and --autoinstall (app.py).

- terminate_process_group() SIGTERMs the wrapper's group, like GNOME's
  BootcProgress.terminate, and tolerates dead or missing pids.
- _load_autoinstall_recipe() reads a JSON object or returns None.
- parse_argv() splits --autoinstall/-a out before GTK sees the argv.
- The window without a running install closes normally (no dialog).
"""

import json
import os
import signal
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tuna_installer_xfce import app  # noqa: E402


def test_terminate_kills_the_wrapper_group():
    pid, out, err = app.spawn_fisherman(["sleep", "30"])
    try:
        assert os.getpgid(pid) == pid
        app.terminate_process_group(pid)
        deadline = time.time() + 5
        while time.time() < deadline:
            try:
                os.kill(pid, 0)
            except OSError:
                break
            time.sleep(0.05)
        _, status = os.waitpid(pid, 0)
        assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGTERM
    finally:
        for fd in (out, err):
            os.close(fd)
        try:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass


def test_terminate_tolerates_dead_and_missing_pids():
    app.terminate_process_group(None)
    app.terminate_process_group(0)
    app.terminate_process_group(2 ** 30)  # nothing lives here


def test_load_autoinstall_recipe(tmp_path):
    good = tmp_path / "recipe.json"
    good.write_text(json.dumps({"disk": "/dev/vda"}))
    assert app.InstallerWindow._load_autoinstall_recipe(str(good)) == {
        "disk": "/dev/vda"}
    missing = tmp_path / "absent.json"
    assert app.InstallerWindow._load_autoinstall_recipe(str(missing)) is None
    broken = tmp_path / "broken.json"
    broken.write_text("{nope")
    assert app.InstallerWindow._load_autoinstall_recipe(str(broken)) is None
    array = tmp_path / "array.json"
    array.write_text("[1, 2]")
    assert app.InstallerWindow._load_autoinstall_recipe(str(array)) is None


def test_parse_argv():
    auto, rest = app.parse_argv(["--autoinstall", "r.json", "--g-fatal-warnings"])
    assert auto == "r.json"
    assert rest == ["--g-fatal-warnings"]
    auto, rest = app.parse_argv(["-a", "r.json"])
    assert auto == "r.json" and rest == []
    auto, rest = app.parse_argv(["--autoinstall=r.json"])
    assert auto == "r.json" and rest == []
    auto, rest = app.parse_argv([])
    assert auto is None and rest == []


def test_window_without_install_closes_normally():
    win = app.InstallerWindow(None)
    try:
        assert not win._install_running()
        # No install running: the delete-event handler lets the close
        # through instead of raising the confirm dialog.
        assert win._on_delete() is False
        win.terminate_install()  # no install: must not raise
    finally:
        win.destroy()


def test_bad_autoinstall_falls_back_to_wizard():
    win = app.InstallerWindow(None)
    try:
        start = win.index
        assert win._start_autoinstall("/definitely/not/a/recipe.json") is False
        assert win.index == start
        assert win._autoinstall_pending is None
    finally:
        win.destroy()

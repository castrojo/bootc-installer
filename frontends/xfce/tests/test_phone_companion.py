"""Phone companion (phone_companion.py): payload rules and the server.

The sanitizer mirrors GNOME's _FIELD_LIMITS/_USERNAME_RE/_HOSTNAME_RE: a
phone must not smuggle in what typing cannot.
"""

import json
import os
import sys
import urllib.error
import urllib.request

import gi

gi.require_version("Gtk", "3.0")

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tuna_installer_xfce import phone_companion  # noqa: E402


def _valid(**over):
    payload = {"fullname": "Ada Lovelace", "username": "ada",
               "password": "s3cret", "hostname": "analytical"}
    payload.update(over)
    return payload


def test_valid_payload_passes_and_normalizes():
    config = phone_companion.sanitize_config(
        _valid(hostname="  Analytical-Engine1  ", username=" ada "))
    assert config is not None
    assert config["hostname"] == "analytical-engine1"
    assert config["username"] == "ada"


def test_missing_or_bad_fields_reject():
    assert phone_companion.sanitize_config(_valid(username="")) is None
    assert phone_companion.sanitize_config(_valid(username="Root")) is None
    assert phone_companion.sanitize_config(_valid(hostname="-bad-")) is None
    assert phone_companion.sanitize_config(_valid(password=123)) is None
    assert phone_companion.sanitize_config(
        _valid(fullname="x" * 257)) is None
    assert phone_companion.sanitize_config("not a dict") is None
    assert phone_companion.sanitize_config(
        _valid(extra="dropped-ok", username="ada"))["username"] == "ada"


def test_html_names_the_product_not_a_literal():
    html = phone_companion.render_companion_html("TunaTest", "tuna", "tok")
    assert "TunaTest" in html and "__PRODUCT__" not in html
    assert "__HOSTNAME__" not in html and "__TOKEN__" not in html


def test_qr_is_none_without_segno(monkeypatch):
    monkeypatch.setattr(phone_companion, "segno", None)
    assert phone_companion.qr_png_bytes("http://127.0.0.1:1/abc") is None


def test_server_accepts_one_valid_submit():
    received = []
    server = phone_companion.CompanionServer(
        "TunaTest", "tuna", received.append, port=0)
    url = server.start()
    try:
        token = url.rsplit("/", 1)[1]
        body = json.dumps(_valid(token=token)).encode()
        req = urllib.request.Request(url.replace("/" + token, "/submit"),
                                     data=body,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as res:
            assert res.status == 200
        assert received and received[0]["username"] == "ada"
        # Wrong token is rejected and reaches no callback.
        bad = json.dumps(_valid(token="nope")).encode()
        req = urllib.request.Request(url.replace("/" + token, "/submit"),
                                     data=bad,
                                     headers={"Content-Type": "application/json"})
        try:
            urllib.request.urlopen(req, timeout=5)
            rejected = False
        except urllib.error.HTTPError as exc:
            rejected = exc.code == 403
        assert rejected
        assert len(received) == 1
    finally:
        server.stop()
    assert not server.running


def test_box_builds_without_serving():
    box = phone_companion.PhoneCompanionBox("TunaTest", "tuna", lambda c: None)
    assert box._server is None
    box.stop()  # stopping what never started is a no-op

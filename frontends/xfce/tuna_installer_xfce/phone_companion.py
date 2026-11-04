"""Phone companion: type the account on a phone, send it to the installer.

Mirrors GNOME's defaults/qr_companion.py (CompanionServer) and
utils/phone_companion.py (sanitize_config, get_local_ip): a tiny HTTPS-less
LAN server -- this frontend serves plain HTTP on 127.0.0.0/8 + LAN only,
never 0.0.0.0 -- shows a QR code of the URL (segno, Flatpak module
"python3-segno"; a bare link when segno is absent), and hands the submitted
form to the identity page.

What the form may set is allowlisted exactly like GNOME's _FIELD_LIMITS,
with the same username/hostname rules a typed entry must pass: the values
land in the same GtkEntries and the same fisherman validate LiveCheck the
typed ones go through, so a phone cannot smuggle in what typing cannot.
"""

import http.server
import json
import logging
import re
import secrets
import socket
import threading
from urllib.parse import urlparse

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Gdk, GdkPixbuf, Gtk, GLib  # noqa: E402

logger = logging.getLogger(__name__)

COMPANION_PORT = 8443

# What the companion form may set, and how long each value may be. Anything
# else in the payload is dropped: the values go straight into GTK entries
# and the install recipe, so a non-string or an unexpected key must not
# reach them. Limits and rules match GNOME's _FIELD_LIMITS/_USERNAME_RE/
# _HOSTNAME_RE; the password travels the LAN in the clear, which is why the
# token URL is never shown as text and the server stops after one submit.
_FIELD_LIMITS = {
    "fullname": 256,
    "username": 32,
    "password": 4096,
    "hostname": 63,
}
_REQUIRED_FIELDS = ("fullname", "username", "password", "hostname")
_USERNAME_RE = re.compile(r"^[a-z_][a-z0-9_-]{0,31}$")
_HOSTNAME_RE = re.compile(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$")

try:
    import segno
except ImportError:  # Flatpak without the python3-segno module, or tests.
    segno = None


def sanitize_config(raw):
    """The companion payload reduced to known string fields, or None.

    None means reject the submission: a required field is missing or empty,
    a value is not a string or is too long, or the username or hostname is
    not one the installer would accept if typed.
    """
    if not isinstance(raw, dict):
        return None
    config = {}
    for key, limit in _FIELD_LIMITS.items():
        value = raw.get(key, "")
        if not isinstance(value, str) or len(value) > limit:
            return None
        config[key] = value
    config["hostname"] = config["hostname"].strip().lower()
    config["username"] = config["username"].strip()
    if any(not config[key] for key in _REQUIRED_FIELDS):
        return None
    if not _USERNAME_RE.match(config["username"]):
        return None
    if not _HOSTNAME_RE.match(config["hostname"]):
        return None
    return config


def get_local_ip():
    """The LAN address to print under the QR code (a phone cannot reach
    127.0.0.1). Falls back to the hostname, then loopback."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.connect(("8.8.8.8", 80))
        ip = sock.getsockname()[0]
        sock.close()
        return ip
    except OSError:
        try:
            return socket.gethostbyname(socket.gethostname())
        except OSError:
            return "127.0.0.1"


def render_companion_html(product_name, hostname_seed, token):
    """The phone form with the product name, hostname seed and session token
    filled in. The token rides in the POST body, never in a shown URL."""
    return COMPANION_HTML.replace("__PRODUCT__", product_name).replace(
        "__HOSTNAME__", hostname_seed).replace("__TOKEN__", token)


COMPANION_HTML = """<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>__PRODUCT__ Installer Companion</title>
<style>
body { font-family: sans-serif; margin: 1em; }
label { display: block; margin-top: 0.8em; }
input { width: 100%; font-size: 1.1em; padding: 0.3em; box-sizing: border-box; }
button { margin-top: 1em; font-size: 1.1em; padding: 0.5em 1.5em; }
</style></head>
<body>
<h1>__PRODUCT__ Installer Companion</h1>
<p>Type here; the installer fills in the same fields.</p>
<form id="f">
<label>Full name <input name="fullname" autocomplete="name"></label>
<label>Username <input name="username" autocomplete="username"></label>
<label>Password <input name="password" type="password"
    autocomplete="new-password"></label>
<label>Computer name <input name="hostname" value="__HOSTNAME__"></label>
<input type="hidden" name="token" value="__TOKEN__">
<button type="submit">Send to installer</button>
</form>
<p id="r"></p>
<script>
document.getElementById("f").addEventListener("submit", async (e) => {
  e.preventDefault();
  const data = Object.fromEntries(new FormData(e.target));
  const res = await fetch("/submit", {method: "POST",
    headers: {"Content-Type": "application/json"},
    body: JSON.stringify(data)});
  document.getElementById("r").textContent =
    res.ok ? "Sent. You can close this page." : "Rejected. Check the fields.";
});
</script>
</body>
</html>
"""


def qr_png_bytes(url):
    """The QR code for `url` as PNG bytes, or None when segno is missing.

    The caller decides what to show instead (this frontend shows the link
    with a copy button). Never raises for an encodable URL.
    """
    if segno is None:
        return None
    try:
        code = segno.make_qr(url, error="m")
        buf = bytearray()
        code.save(buf, kind="png", scale=6, border=4)
        return bytes(buf)
    except (ValueError, OverflowError) as exc:
        logger.warning("could not encode companion URL as QR: %s", exc)
        return None


class CompanionServer:
    """The LAN form server. One token, one submission, then it stops.

    Started when the identity page's companion expander opens, stopped when
    it closes or the first valid submission arrives. `on_config(config)` runs
    on the server thread -- the widget marshals it to the GTK thread.
    """

    def __init__(self, product_name, hostname_seed, on_config, port=COMPANION_PORT):
        self.token = secrets.token_urlsafe(16)
        self._on_config = on_config
        self._port = port
        self._httpd = None
        self._thread = None
        self.url = ""
        self._html = render_companion_html(product_name, hostname_seed,
                                           self.token).encode("utf-8")

    def start(self):
        """Bind loopback + LAN (never 0.0.0.0) and serve in a thread. The URL
        carries the token; it is shown only as a QR code, never as text."""
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            server_version = "TunaCompanion/1"

            def _send(self, body, status=200, ctype="text/html; charset=utf-8"):
                data = body if isinstance(body, bytes) else body.encode()
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):  # noqa: N802
                if urlparse(self.path).path != "/" + outer.token:
                    self._send("Not found.", 404, "text/plain")
                    return
                self._send(outer._html)

            def do_POST(self):  # noqa: N802
                if urlparse(self.path).path != "/submit":
                    self._send("Not found.", 404, "text/plain")
                    return
                try:
                    length = int(self.headers.get("Content-Length", "0"))
                except ValueError:
                    length = 0
                if length > 64 * 1024:
                    self._send("Too large.", 413, "text/plain")
                    return
                try:
                    payload = json.loads(
                        self.rfile.read(length) or b"null")
                except (ValueError, UnicodeDecodeError):
                    payload = None
                if not isinstance(payload, dict) or \
                        payload.get("token") != outer.token:
                    self._send("Rejected.", 403, "text/plain")
                    return
                config = sanitize_config(payload)
                if config is None:
                    self._send("Rejected.", 422, "text/plain")
                    return
                # Callback before the response: once the phone sees OK,
                # the config is already delivered.
                outer._on_config(config)
                self._send("OK", 200, "text/plain")

            def log_message(self, *args):
                pass

        # Bind the LAN address get_local_ip() advertises, falling back to
        # loopback where there is no route out (an offline room with two
        # devices still pairs over link-local through the advertised name).
        host = get_local_ip()
        try:
            self._httpd = http.server.HTTPServer((host, self._port), Handler)
        except OSError:
            host = "127.0.0.1"
            self._httpd = http.server.HTTPServer((host, self._port), Handler)
        self.url = f"http://{host}:{self._httpd.server_port}/{self.token}"
        self._thread = threading.Thread(target=self._httpd.serve_forever,
                                        daemon=True)
        self._thread.start()
        logger.info("companion server serving on %s:%s", host,
                    self._httpd.server_port)
        return self.url

    @property
    def running(self):
        return self._httpd is not None

    def stop(self):
        httpd, self._httpd = self._httpd, None
        if httpd is not None:
            httpd.shutdown()
            httpd.server_close()


class PhoneCompanionBox(Gtk.Box):
    """The identity page's phone half: QR code (or link), status line.

    The server runs only between start() and stop(); a received config is
    marshalled to the GTK thread before apply(config) runs, then the server
    stops itself -- one submission per pairing, so a stale phone cannot
    overwrite fields after the user moved on.
    """

    def __init__(self, product_name, hostname_seed, apply):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=6,
                         margin=6)
        self._product = product_name
        self._seed = hostname_seed
        self._apply = apply
        self._server = None

        self.status = Gtk.Label(xalign=0)
        self.status.set_line_wrap(True)
        self.pack_start(self.status, False, False, 0)
        self.qr_image = Gtk.Image()
        self.qr_image.set_no_show_all(True)
        self.pack_start(self.qr_image, False, False, 0)
        linkrow = Gtk.Box(spacing=8)
        self.link = Gtk.Label(xalign=0, selectable=True)
        self.link.set_line_wrap(True)
        self.copy_btn = Gtk.Button(label="Copy link")
        self.copy_btn.connect("clicked", self._on_copy)
        linkrow.pack_start(self.link, True, True, 0)
        linkrow.pack_start(self.copy_btn, False, False, 0)
        self.linkrow = linkrow
        self.pack_start(linkrow, False, False, 0)
        self.status.set_text("Open this section to show the pairing code.")

    def start(self):
        """Serve the form and show the code. Idempotent; safe to call for
        a second expander opening."""
        if self._server is not None and self._server.running:
            return
        self._server = CompanionServer(self._product, self._seed,
                                       self._on_config)
        try:
            url = self._server.start()
        except OSError as exc:
            logger.warning("companion server would not start: %s", exc)
            self.status.set_text(f"Could not start the phone pairing: {exc}")
            self._server = None
            return
        png = qr_png_bytes(url)
        if png is not None:
            try:
                loader = GdkPixbuf.PixbufLoader.new_with_type("png")
                loader.write(png)
                loader.close()
                self.qr_image.set_from_pixbuf(loader.get_pixbuf())
                self.qr_image.show()
            except Exception as exc:  # noqa: BLE001 -- any loader failure
                logger.warning("could not render companion QR: %s", exc)
                self.qr_image.hide()
        else:
            # No segno (Flatpak without the module): the link + copy is
            # the whole pairing, and says so.
            self.qr_image.hide()
        host = url.rsplit("/", 1)[0].rsplit("@", 1)[-1]
        self.link.set_text(url if png is None else host)
        self.status.set_text(
            "Scan the code with your phone, or copy the link to it. "
            "The code stops working after one use.")
        self.show_all()

    def stop(self):
        server, self._server = self._server, None
        if server is not None:
            server.stop()
        self.status.set_text("Open this section to show the pairing code.")

    def _on_copy(self, _button):
        text = self._server.url if self._server is not None else ""
        if text:
            Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).set_text(text, -1)
            self.copy_btn.set_label("Copied")

    def _on_config(self, config):
        server = self._server
        GLib.idle_add(self._applied, config)
        if server is not None:
            server.stop()

    def _applied(self, config):
        self.status.set_text("Received from your phone.")
        self._apply(config)
        return False

# Dependency choices for the GNOME-parity pages

No new Rust dependency was added for this work. Everything ships on the
crates already vendored in `Cargo.lock` (and therefore already described
in `cargo-sources.json`, which `just check-cargo-sources` verifies):

- `image_catalog`, `hardware`, `preflight`, `tour` are `std` +
  `serde_json` only.
- `phone` (companion form server) is `tokio` (already a direct
  dependency, `full`) + `serde_json`. The HTTP/1.1 serving is hand-rolled
  (~80 lines) instead of `axum`/`hyper` because neither is in the lockfile
  and fetching one would desync `cargo-sources.json` for the Flatpak
  offline build.

Two deliberate downgrades follow from that budget, both recorded where
the code diverges from GNOME:

1. **The companion form is plain HTTP, not TLS.** GNOME serves it over TLS
   on port 8443 (`phone_companion.py` uses `ssl`). No TLS stack
   (`rustls`, `tokio-rustls`, `rcgen` for a self-signed cert) is vendored
   offline. The session token is still required on every request, the
   server binds only while the Companion page is shown, and leaving the
   page stops it (`phone::stop_server`). If a TLS stack is ever vendored,
   terminate there and keep the token check.
2. **The pairing code is text, not a QR matrix.** GNOME renders it with
   the Python `segno` library (`qr_companion.py`). No QR encoder
   (e.g. the `qrcode` crate) is vendored offline, so the page shows
   `host:port` plus the code as text with a "Copy link" button — which is
   also what the GNOME page does for users who cannot scan. The URL
   carries the token and is never shown as text.

If either crate is added later, regenerate the Flatpak sources in the
same commit (`just cargo-sources`) — `check-cargo-sources` fails
otherwise — and revisit the two call sites above.

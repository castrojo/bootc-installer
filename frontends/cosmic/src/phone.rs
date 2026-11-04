//! Phone companion: fill the account form from a phone on the same LAN.
//!
//! GNOME serves a small form over the LAN (`CompanionServer` in
//! `bootc_installer/utils/phone_companion.py`, QR code in
//! `defaults/qr_companion.py` via the `segno` library) and copies the
//! submitted values into the account entries. This ports the validation
//! half faithfully — [`sanitize_config`] is the same field limits, required
//! fields and username/hostname rules, so a value the phone sends is one the
//! installer would also accept typed — and serves the form with a minimal
//! HTTP/1.1 server built on tokio alone.
//!
//! Two deliberate downgrades, both from the offline dependency budget
//! (documented in `flatpak/DEPENDENCIES.md`):
//! - The form is plain HTTP, not TLS: no TLS stack is vendored in
//!   `Cargo.lock`, and fetching one would desync `cargo-sources.json`.
//!   The session token in the URL is still required for every request, the
//!   server binds only while its page is shown, and it stops the moment the
//!   wizard leaves the page.
//! - The pairing code is shown as text with a copy button, not a rendered
//!   QR matrix: no QR encoder is vendored either. The URL carries the token,
//!   so it is never shown as text — only `host:port` and the code are (as
//!   on the GNOME page, which copies the full link from a button).

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Largest form submission accepted (mirrors `MAX_CONFIG_BYTES`).
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// What the companion form may set, and how long each value may be.
/// Anything else in the payload is dropped.
pub const FIELD_LIMITS: [(&str, usize); 5] = [
    ("fullname", 256),
    ("username", 32),
    ("password", 4096),
    ("hostname", 63),
    ("sshkey", 16 * 1024),
];

/// The validated companion submission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhoneConfig {
    pub fullname: String,
    pub username: String,
    pub password: String,
    pub hostname: String,
    pub sshkey: String,
}

fn valid_username(s: &str) -> bool {
    // ^[a-z_][a-z0-9_-]{0,31}$ — the rule installer's own username field
    // enforces (user.py), so a submitted name is one typing would accept.
    let mut chars = s.chars();
    match chars.next() {
        Some(f) if f.is_ascii_lowercase() || f == '_' => {}
        _ => return false,
    }
    let mut len = 0;
    for c in chars {
        len += 1;
        if len > 31 {
            return false;
        }
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

fn valid_hostname(s: &str) -> bool {
    // ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ — the Systeminfo hostname rule.
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 63 || b[0] == b'-' || b[b.len() - 1] == b'-' {
        return false;
    }
    b.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The companion payload reduced to known string fields, or `None` to
/// reject the submission: a required field missing or empty, a value not a
/// string or too long, or a username/hostname typing would reject.
/// (Mirrors `sanitize_config` in phone_companion.py.)
pub fn sanitize_config(raw: &Value) -> Option<PhoneConfig> {
    let obj = raw.as_object()?;
    let get = |key: &str, limit: usize| -> Option<String> {
        let v = obj.get(key)?.as_str()?;
        if v.len() > limit {
            return None;
        }
        Some(v.to_string())
    };
    let mut config = PhoneConfig {
        fullname: get("fullname", 256)?,
        username: get("username", 32)?,
        password: get("password", 4096)?,
        hostname: get("hostname", 63)?.trim().to_lowercase(),
        sshkey: obj
            .get("sshkey")
            .map(|v| {
                v.as_str()
                    .filter(|s| s.len() <= 16 * 1024)
                    .map(str::to_string)
            })
            .unwrap_or(None)
            .unwrap_or_default(),
    };
    config.username = config.username.trim().to_string();
    if config.fullname.is_empty()
        || config.username.is_empty()
        || config.password.is_empty()
        || config.hostname.is_empty()
    {
        return None;
    }
    if !valid_username(&config.username) || !valid_hostname(&config.hostname) {
        return None;
    }
    let _ = FIELD_LIMITS;
    Some(config)
}

/// A fresh pairing token: 32 hex chars from the OS RNG (with a
/// time-seeded fallback that never panics, so pairing still works where
/// /dev/urandom is unavailable).
pub fn new_token() -> String {
    // Exactly 16 bytes: /dev/urandom never returns EOF, so a read-to-end
    // would fill memory until the OOM killer fires.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut bytes = [0u8; 16];
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    format!(
        "{:016x}{:016x}",
        h.finish(),
        h.finish().wrapping_mul(0x9e3779b97f4a7c15)
    )
}

/// The URL the phone opens. Carries the token, so the page shows only
/// `host:port` plus a copy button — never this string.
pub fn pairing_url(host: &str, port: u16, token: &str) -> String {
    format!("http://{host}:{port}/?token={token}")
}

/// The machine's LAN address for the pairing display, via the connected-
/// UDP-socket trick (no packet ever leaves). `None` when offline.
pub fn local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}

/// Minimal percent-decoding for form bodies (`+` becomes a space).
fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h << 4 | l);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
                i += 1;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse an `application/x-www-form-urlencoded` body into a JSON object.
fn parse_form(body: &str) -> Value {
    let mut map = serde_json::Map::new();
    for pair in body.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => continue,
        };
        map.insert(url_decode(k), Value::String(url_decode(v)));
    }
    Value::Object(map)
}

fn query_token(request_line: &str) -> &str {
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    match path.split_once("token=") {
        Some((_, rest)) => rest.split('&').next().unwrap_or(""),
        None => "",
    }
}

fn response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn form_page(token: &str) -> String {
    format!(
        "<!doctype html><html><body><h1>Installer companion</h1>\
        <form method=\"post\" action=\"/submit?token={token}\">\
        <label>Full name <input name=\"fullname\"></label><br>\
        <label>Username <input name=\"username\"></label><br>\
        <label>Password <input type=\"password\" name=\"password\"></label><br>\
        <label>Computer name <input name=\"hostname\"></label><br>\
        <input type=\"submit\" value=\"Send to installer\"></form></body></html>"
    )
}

/// The LAN form server. Binds on [`start`](Self::start), stops on
/// [`stop`](Self::stop) (or drop). Accepted configs arrive on the
/// tokio mpsc sender for the UI thread to apply to the account page.
pub struct CompanionServer {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl CompanionServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serve on `port` (0 picks one). Every request must carry
    /// `?token=<token>`.
    pub async fn start(
        port: u16,
        token: String,
        applied: tokio::sync::mpsc::UnboundedSender<PhoneConfig>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
        let actual = listener.local_addr()?.port();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            loop {
                let accept = listener.accept();
                tokio::select! {
                    _ = &mut stop_rx => break,
                    incoming = accept => {
                        let Ok((stream, _)) = incoming else { continue };
                        let token = token.clone();
                        let applied = applied.clone();
                        tokio::spawn(async move {
                            serve_one(stream, &token, &applied).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            port: actual,
            shutdown: Some(stop_tx),
            task: Some(task),
        })
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for CompanionServer {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn serve_one(
    mut stream: tokio::net::TcpStream,
    token: &str,
    applied: &tokio::sync::mpsc::UnboundedSender<PhoneConfig>,
) {
    let mut buf = vec![0u8; MAX_BODY_BYTES + 8192];
    let mut filled = 0;
    let header_end = loop {
        if filled >= buf.len() {
            return;
        }
        let n = match stream.read(&mut buf[filled..]).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        filled += n;
        if let Some(pos) = find_double_crlf(&buf[..filled]) {
            break pos;
        }
    };
    let header = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = header.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let method = request_line.split_whitespace().next().unwrap_or("");
    if query_token(&request_line) != token {
        let _ = stream
            .write_all(&response("403 Forbidden", "text/plain", "bad token"))
            .await;
        return;
    }
    if method == "GET" {
        let page = form_page(token);
        let _ = stream
            .write_all(&response("200 OK", "text/html", &page))
            .await;
        return;
    }
    if method != "POST" {
        let _ = stream
            .write_all(&response("405 Method Not Allowed", "text/plain", "no"))
            .await;
        return;
    }
    let content_length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        let _ = stream
            .write_all(&response("413 Payload Too Large", "text/plain", "too big"))
            .await;
        return;
    }
    let body_start = header_end;
    while filled - body_start < content_length {
        if filled >= buf.len() {
            return;
        }
        match stream.read(&mut buf[filled..]).await {
            Ok(0) | Err(_) => return,
            Ok(n) => filled += n,
        }
    }
    let body = String::from_utf8_lossy(&buf[body_start..body_start + content_length]).into_owned();
    match sanitize_config(&parse_form(&body)) {
        Some(config) => {
            let _ = applied.send(config);
            let _ = stream
                .write_all(&response("200 OK", "text/plain", "Sent to the installer."))
                .await;
        }
        None => {
            let _ = stream
                .write_all(&response("400 Bad Request", "text/plain", "rejected"))
                .await;
        }
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

// ------------------------------------------------- wizard lifecycle ----

/// The wizard keeps one server at a time, owned outside the UI thread: a
/// background thread runs its own runtime, and accepted configs land in
/// the inbox for the Companion page's heartbeat to drain.
struct ServerState {
    port: u16,
    token: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static STATE: std::sync::Mutex<Option<ServerState>> = std::sync::Mutex::new(None);
static INBOX: std::sync::Mutex<Vec<PhoneConfig>> = std::sync::Mutex::new(Vec::new());

/// Serve the companion form for `token` (starting the server the first
/// time, or when the token changed) and return its port. Idempotent per
/// token: entering the page twice does not rebind.
pub fn ensure_server(token: &str) -> u16 {
    let mut state = STATE.lock().unwrap();
    if let Some(running) = state.as_ref() {
        if running.token == token {
            return running.port;
        }
        stop_locked(&mut state);
    }
    let token = token.to_string();
    let (port_tx, port_rx) = std::sync::mpsc::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let thread_token = token.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let Ok(rt) = rt else {
            let _ = port_tx.send(0);
            return;
        };
        rt.block_on(async move {
            // GNOME's port first, then whatever the OS hands out.
            let mut server = None;
            for port in [8443, 0] {
                let (applied_tx, mut applied_rx) =
                    tokio::sync::mpsc::unbounded_channel::<PhoneConfig>();
                match CompanionServer::start(port, thread_token.clone(), applied_tx).await {
                    Ok(s) => {
                        let _ = port_tx.send(s.port());
                        let mut shutdown_rx = shutdown_rx;
                        server = Some(s);
                        loop {
                            tokio::select! {
                                _ = &mut shutdown_rx => break,
                                got = applied_rx.recv() => {
                                    match got {
                                        Some(config) => INBOX.lock().unwrap().push(config),
                                        None => break,
                                    }
                                }
                            }
                        }
                        break;
                    }
                    Err(_) => continue,
                }
            }
            drop(server);
        });
    });
    // A port of 0 reports a thread that never bound; the page shows its
    // retry state and the heartbeat keeps nothing.
    let port = port_rx.recv().unwrap_or(0);
    *state = Some(ServerState {
        port,
        token,
        shutdown: Some(shutdown_tx),
        thread: Some(thread),
    });
    port
}

fn stop_locked(state: &mut Option<ServerState>) {
    if let Some(mut running) = state.take() {
        if let Some(tx) = running.shutdown.take() {
            let _ = tx.send(());
        }
        // The thread owns its runtime; joining would wait out its sockets,
        // so detach and let the shutdown signal end it.
        let _ = running.thread.take();
    }
}

/// Stop the companion server, if any. Called when the wizard leaves the
/// companion page: the form must not outlive the install it feeds.
pub fn stop_server() {
    stop_locked(&mut STATE.lock().unwrap());
}

/// The port the server bound, if it is up.
#[allow(dead_code)]
pub fn server_port() -> Option<u16> {
    STATE.lock().unwrap().as_ref().map(|s| s.port)
}

/// Drain accepted phone configs for the UI thread to apply.
pub fn drain_inbox() -> Vec<PhoneConfig> {
    std::mem::take(&mut *INBOX.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid() -> Value {
        json!({
            "fullname": "Alex Example",
            "username": "alex",
            "password": "s3cret",
            "hostname": "My-PC",
            "sshkey": "ssh-ed25519 AAAA",
        })
    }

    #[test]
    fn valid_submission_is_accepted_and_hostname_normalized() {
        let config = sanitize_config(&valid()).expect("valid");
        assert_eq!(config.username, "alex");
        // Trimmed and lowercased, as the installer generates hostnames.
        assert_eq!(config.hostname, "my-pc");
        assert_eq!(config.sshkey, "ssh-ed25519 AAAA");
    }

    #[test]
    fn missing_or_empty_required_fields_reject() {
        for key in ["fullname", "username", "password", "hostname"] {
            let mut v = valid();
            v.as_object_mut().unwrap().remove(key);
            assert!(sanitize_config(&v).is_none(), "{key} missing");
            // Empty always rejects.
            let mut v = valid();
            v.as_object_mut().unwrap()[key] = Value::String("".into());
            assert!(sanitize_config(&v).is_none(), "{key} empty");
        }
        // Whitespace-only: username and hostname are stripped before the
        // required check (so they reject); fullname and password are not
        // (so they pass) — exactly as phone_companion.py orders it.
        for key in ["username", "hostname"] {
            let mut v = valid();
            v.as_object_mut().unwrap()[key] = Value::String("  ".into());
            assert!(sanitize_config(&v).is_none(), "{key} blank");
        }
        for key in ["fullname", "password"] {
            let mut v = valid();
            v.as_object_mut().unwrap()[key] = Value::String("  ".into());
            assert!(sanitize_config(&v).is_some(), "{key} blank");
        }
    }

    #[test]
    fn overlong_or_nonstr_values_reject() {
        let mut v = valid();
        v["username"] = Value::String("a".repeat(33));
        assert!(sanitize_config(&v).is_none());
        let mut v = valid();
        v["password"] = Value::Number(42.into());
        assert!(sanitize_config(&v).is_none());
        // sshkey is optional but still length-capped.
        let mut v = valid();
        v.as_object_mut().unwrap().remove("sshkey");
        assert!(sanitize_config(&v).is_some());
    }

    #[test]
    fn typed_rules_apply_to_submitted_names() {
        for bad_user in ["Alex", "0abc", "-x", "a b", "àlex", ""] {
            let mut v = valid();
            v["username"] = Value::String(bad_user.into());
            assert!(sanitize_config(&v).is_none(), "username {bad_user:?}");
        }
        for bad_host in ["-pc", "pc-", "my pc", ""] {
            let mut v = valid();
            v["hostname"] = Value::String(bad_host.into());
            assert!(sanitize_config(&v).is_none(), "hostname {bad_host:?}");
        }
        // Uppercase normalizes before the rule runs, as in Python
        // (strip().lower() precedes the regex).
        let mut v = valid();
        v["hostname"] = Value::String("My-PC".into());
        assert_eq!(sanitize_config(&v).unwrap().hostname, "my-pc");
        // Unknown keys are dropped, not fatal.
        let mut v = valid();
        v["admin"] = Value::Bool(true);
        assert!(sanitize_config(&v).is_some());
    }

    #[test]
    fn token_is_hex_and_url_carries_it() {
        let token = new_token();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        let url = pairing_url("192.168.1.5", 8443, &token);
        assert_eq!(url, format!("http://192.168.1.5:8443/?token={token}"));
    }

    /// Drive the server over a real socket with hand-written HTTP.
    fn roundtrip(path: String, method: String, body: Option<String>) -> String {
        use std::io::{Read, Write};
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut server = CompanionServer::start(0, "tok".into(), tx).await.unwrap();
            let port = server.port();
            let reply = tokio::task::spawn_blocking(move || {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n");
                if let Some(b) = body.as_deref() {
                    request.push_str(&format!(
                        "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
                        b.len()
                    ));
                }
                request.push_str("\r\n");
                if let Some(b) = body.as_deref() {
                    request.push_str(b);
                }
                stream.write_all(request.as_bytes()).unwrap();
                let mut reply = Vec::new();
                stream.read_to_end(&mut reply).unwrap();
                String::from_utf8_lossy(&reply).into_owned()
            })
            .await
            .unwrap();
            // Drain any accepted config so the channel cannot fill.
            while rx.try_recv().is_ok() {}
            server.stop();
            reply
        })
    }

    #[test]
    fn server_serves_the_form_only_with_the_token() {
        let ok = roundtrip("/?token=tok".to_string(), "GET".to_string(), None);
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("<form"));
        let bad = roundtrip("/?token=nope".to_string(), "GET".to_string(), None);
        assert!(bad.starts_with("HTTP/1.1 403"), "{bad}");
    }

    #[test]
    fn server_accepts_a_valid_form_and_rejects_a_bad_one() {
        let good = "fullname=Alex+Example&username=alex&password=s3cret&hostname=my-pc";
        let ok = roundtrip(
            "/submit?token=tok".to_string(),
            "POST".to_string(),
            Some(good.to_string()),
        );
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        let bad = roundtrip(
            "/submit?token=tok".to_string(),
            "POST".to_string(),
            Some("username=Alex&password=x".to_string()),
        );
        assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    }
}

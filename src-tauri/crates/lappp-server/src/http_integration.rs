//! Real-HTTP integration tests — the layer the object-level unit tests
//! cannot see. The production route table from `build_router` is served on
//! a random loopback port and exercised with a hand-rolled HTTP/1.1 client
//! (no tower `oneshot`, no mock), so middleware order, status mapping and
//! streaming behavior are pinned over the wire:
//!
//! * a valid token beats the 404 dispatch: an unknown command is 403
//!   "command not whitelisted" because `whitelist_middleware` answers
//!   before `bridge` ever sees the name — the bridge's 404 branch is
//!   unreachable through the assembled stack;
//! * the status contract (401/403/409/429/500/503 with `{"error": …}`);
//! * a full tag/collection/album CRUD round trip through the real bridge
//!   and the real default library in a scratch data dir;
//! * the §3.5 signed-media chain end to end: import bytes → sign → serve
//!   thumbnail bytes → tamper/missing-signature rejections;
//! * SSE frames on a live socket plus `Last-Event-ID` resume;
//! * rate-limit accounting: failing writes consume budget, reads don't;
//!   a shutdown request flips new commands to 503.
//!
//! Isolation model: every test spawns its own server (own `AuthService`
//! with its own limiter state and auth DB in a scratch dir), while the
//! command layer shares one scratch data dir pinned via
//! `t_config::set_data_dir_override` (first writer wins, process-global).
//! Only this module touches the global config layer inside this test
//! binary — every other lappp-server unit test is object-local.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rand::RngCore as _;

use super::build_router;
use crate::auth::AuthService;
use crate::sse::{EventBus, SseSink};
use crate::shutdown::ShutdownState;
use lappp_core::t_events::EventSink as _;

const ADMIN_PW: &str = "itest-admin-pass";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

// -- harness ---------------------------------------------------------------

/// Removes its directory when the test ends (best effort — a leftover under
/// the system temp dir is not fatal).
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn fresh(tag: &str) -> ScratchDir {
        let mut bytes = [0u8; 6];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("lap-server-it-{tag}-{hex}"));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        ScratchDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The process-wide scratch data dir backing the bridge's command layer
/// (app config + the default library DB), pinned once.
fn shared_library_root() -> &'static Path {
    static ROOT: OnceLock<ScratchDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let scratch = ScratchDir::fresh("root");
        lappp_core::t_config::set_app_identifier("com.julyx10.lap");
        lappp_core::t_config::set_data_dir_override(scratch.path().to_path_buf());
        lappp_core::t_state::init_process_states();
        lappp_core::t_config::load_app_config().expect("load app config");
        lappp_core::t_sqlite::create_db().expect("create default library db");
        scratch
    })
    .path()
}

struct TestServer {
    addr: SocketAddr,
    bus: Arc<EventBus>,
    shutdown: Arc<ShutdownState>,
    serve: tokio::task::JoinHandle<()>,
    /// Keeps the server's auth scratch dir alive until the test ends.
    _dir: ScratchDir,
}

impl TestServer {
    /// One server = one `AuthService` (own limiter state and auth DB in a
    /// scratch dir). `preset_password` pre-hashes the admin password so
    /// login-driven tests skip the setup flow; the setup flow test passes
    /// false.
    async fn spawn(preset_password: bool) -> TestServer {
        let dir = ScratchDir::fresh("auth");
        if preset_password {
            AuthService::set_admin_password(dir.path(), ADMIN_PW).expect("preset admin password");
        }
        let auth = Arc::new(AuthService::open(dir.path()).expect("open auth store"));
        let bus = EventBus::new();
        let shutdown = ShutdownState::new();
        let app = build_router(auth, bus.clone(), shutdown.clone(), None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let serve = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await;
        });
        TestServer { addr, bus, shutdown, serve, _dir: dir }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.serve.abort();
    }
}

fn uniq(prefix: &str) -> String {
    let mut bytes = [0u8; 5];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}-{hex}")
}

// -- minimal HTTP/1.1 client -------------------------------------------------

struct HttpReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpReply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("non-JSON body ({e}) status={}: {}", self.status, self.text()))
    }
}

/// One request per connection, `Connection: close`, read to EOF — no
/// chunked decoding needed on the JSON endpoints.
async fn send(
    server: &TestServer,
    method: &str,
    path_and_query: &str,
    headers: &[(&str, String)],
    body: Option<Vec<u8>>,
) -> HttpReply {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(server.addr))
            .await
            .expect("connect timeout")
            .expect("connect");

    let mut request = format!("{method} {path_and_query} HTTP/1.1\r\n");
    request.push_str("Host: 127.0.0.1\r\n");
    request.push_str("Content-Type: application/json\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    let body_bytes = body.unwrap_or_default();
    request.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    request.push_str("Connection: close\r\n\r\n");

    stream.write_all(request.as_bytes()).await.expect("write request");
    stream.write_all(&body_bytes).await.expect("write body");
    stream.flush().await.expect("flush");

    let mut raw = Vec::new();
    tokio::time::timeout(READ_TIMEOUT, stream.read_to_end(&mut raw))
        .await
        .expect("read timeout (server kept the connection open?)")
        .expect("read");
    parse_reply(&raw)
}

fn parse_reply(raw: &[u8]) -> HttpReply {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no header terminator in response");
    let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("empty response");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("unreadable status line: {status_line}"));
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    HttpReply { status, headers, body: raw[header_end + 4..].to_vec() }
}

async fn cmd(server: &TestServer, token: &str, name: &str, body: serde_json::Value) -> HttpReply {
    send(
        server,
        "POST",
        &format!("/api/cmd/{name}"),
        &[("Authorization", format!("Bearer {token}"))],
        Some(body.to_string().into_bytes()),
    )
    .await
}

async fn login(server: &TestServer, password: &str) -> HttpReply {
    send(
        server,
        "POST",
        "/api/auth/login",
        &[],
        Some(format!(r#"{{"password": "{password}", "deviceName": "it-client"}}"#).into_bytes()),
    )
    .await
}

/// A login carrying the Fetch Metadata headers a browser sends. The token
/// must NOT come back in the JSON body for such a caller — only the HttpOnly
/// cookie may carry it (§3.6: page script can read the body, not the cookie).
async fn browser_login(server: &TestServer, password: &str) -> HttpReply {
    send(
        server,
        "POST",
        "/api/auth/login",
        &[
            ("sec-fetch-mode", "cors".to_string()),
            ("sec-fetch-site", "same-origin".to_string()),
            ("origin", "http://127.0.0.1".to_string()),
        ],
        Some(format!(r#"{{"password": "{password}", "deviceName": "it-browser"}}"#).into_bytes()),
    )
    .await
}

async fn login_token(server: &TestServer) -> String {
    let reply = login(server, ADMIN_PW).await;
    assert_eq!(reply.status, 200, "login failed: {}", reply.text());
    reply.json()["token"].as_str().expect("token in login reply").to_string()
}

/// One SSE frame as read off the wire: the protocol-level `id:` line (what the
/// browser echoes back as `Last-Event-ID`) and the decoded `data:` envelope.
#[derive(Debug)]
struct SseFrameView {
    id: Option<String>,
    data: serde_json::Value,
}

/// Read `want` data frames off a live `/api/events` socket (no
/// `Connection: close` — the stream never ends, so we read until enough
/// frames arrived or the deadline passes), decoding `data: <json>` lines and
/// capturing each frame's `id:` line.
async fn sse_read_frames(
    server: &TestServer,
    token: &str,
    last_event_id: Option<u64>,
    want: usize,
) -> Vec<SseFrameView> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(server.addr))
            .await
            .expect("connect timeout")
            .expect("connect");
    let mut request = format!(
        "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n"
    );
    if let Some(id) = last_event_id {
        request.push_str(&format!("Last-Event-ID: {id}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.expect("write sse request");

    let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
    let mut buf: Vec<u8> = Vec::new();
    let mut frames: Vec<SseFrameView> = Vec::new();
    // A frame's `id:` line precedes its `data:` line; hold it until the data
    // arrives so the two always pair up.
    let mut pending_id: Option<String> = None;
    let mut chunk = [0u8; 4096];
    loop {
        while let Some(newline) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            let line = line.trim_end_matches('\r');
            if let Some(id) = line.strip_prefix("id: ") {
                pending_id = Some(id.to_string());
            } else if let Some(data) = line.strip_prefix("data: ") {
                let parsed: serde_json::Value = serde_json::from_str(data)
                    .unwrap_or_else(|e| panic!("unreadable SSE frame ({e}): {data}"));
                frames.push(SseFrameView { id: pending_id.take(), data: parsed });
            }
        }
        if frames.len() >= want {
            return frames;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("SSE deadline hit with {}/{} frames", frames.len(), want);
        }
        let read = tokio::time::timeout(remaining, stream.read(&mut chunk))
            .await
            .expect("sse read timeout")
            .expect("sse read");
        if read == 0 {
            panic!("SSE connection closed with {}/{} frames", frames.len(), want);
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// A real 8×8 red JPEG — the `image` crate is already a server dependency.
fn tiny_jpeg() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([220, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("encode tiny jpeg");
    out.into_inner()
}

// -- tests -------------------------------------------------------------------

#[tokio::test]
async fn setup_login_and_the_denial_matrix_over_live_http() {
    let server = TestServer::spawn(false).await; // no password yet — setup flow under test

    // Open discovery endpoints.
    let info = send(&server, "GET", "/api/server_info", &[], None).await;
    assert_eq!(info.status, 200);
    assert_eq!(info.json()["name"], "lappp-server");

    let status = send(&server, "GET", "/api/auth/status", &[], None).await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json()["passwordSet"], false, "fresh server must offer the setup screen");

    // No token → 401 before any whitelist consideration.
    let denied = send(&server, "POST", "/api/cmd/get_app_config", &[], Some(b"{}".to_vec())).await;
    assert_eq!(denied.status, 401);
    assert!(denied.json()["error"].as_str().unwrap().contains("authentication required"));

    // First-run setup from loopback, then it is burned.
    let setup = send(
        &server,
        "POST",
        "/api/auth/setup",
        &[],
        Some(format!(r#"{{"password": "{ADMIN_PW}"}}"#).into_bytes()),
    )
    .await;
    assert_eq!(setup.status, 200, "setup failed: {}", setup.text());
    assert_eq!(setup.json()["ok"], true);
    let again = send(
        &server,
        "POST",
        "/api/auth/setup",
        &[],
        Some(br#"{"password": "other"}"#.to_vec()),
    )
    .await;
    assert_eq!(again.status, 409, "second setup must conflict");

    // Wrong password → 401 with the uniform error body.
    let bad = login(&server, "wrong-password").await;
    assert_eq!(bad.status, 401);
    assert_eq!(bad.json()["error"], "invalid credentials");

    // Correct login: standard scope, cookie pinned per §3.6.
    let ok = login(&server, ADMIN_PW).await;
    assert_eq!(ok.status, 200);
    let body = ok.json();
    assert_eq!(body["scope"], "standard");
    assert!(body["token"].as_str().is_some());
    assert!(body["deviceId"].as_i64().is_some());
    let cookie = ok.header("set-cookie").expect("Set-Cookie on login");
    assert!(cookie.starts_with("lapd_token="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    let token = body["token"].as_str().unwrap().to_string();

    // A browser login gets the cookie but NOT the token in the body: page
    // script reads the body, so echoing a bearer token would defeat HttpOnly.
    let web = browser_login(&server, ADMIN_PW).await;
    assert_eq!(web.status, 200);
    let web_body = web.json();
    assert_eq!(web_body["scope"], "standard");
    assert!(web_body["token"].is_null(), "browser login must not serialize the token: {web_body}");
    let web_cookie = web.header("set-cookie").expect("Set-Cookie on browser login");
    assert!(web_cookie.starts_with("lapd_token="), "browser still gets the cookie");
    assert!(web_cookie.contains("HttpOnly"));

    // Config query through the bridge hits the real config layer.
    let config = cmd(&server, &token, "get_app_config", serde_json::json!({})).await;
    assert_eq!(config.status, 200, "get_app_config failed: {}", config.text());
    assert_eq!(config.json()["current_library_id"], "default");

    // Unknown command: the whitelist middleware answers 403 BEFORE dispatch —
    // the bridge's 404 `UnknownCommand` branch is unreachable over HTTP.
    // Pinned so a middleware reorder cannot silently change the contract.
    let unknown = cmd(&server, &token, "no_such_command", serde_json::json!({})).await;
    assert_eq!(unknown.status, 403, "unknown command: {}", unknown.text());
    assert_eq!(unknown.json()["error"], "command not whitelisted");

    // Malformed / non-object / argument-shape errors are 400s.
    let garbage = send(
        &server,
        "POST",
        "/api/cmd/create_tag",
        &[("Authorization", format!("Bearer {token}"))],
        Some(b"{ not json".to_vec()),
    )
    .await;
    assert_eq!(garbage.status, 400);
    assert!(garbage.json()["error"].as_str().unwrap().contains("invalid JSON body"));

    let non_object = send(
        &server,
        "POST",
        "/api/cmd/create_tag",
        &[("Authorization", format!("Bearer {token}"))],
        Some(b"[1, 2]".to_vec()),
    )
    .await;
    assert_eq!(non_object.status, 400);
    assert!(non_object.json()["error"].as_str().unwrap().contains("JSON object"));

    let missing = cmd(&server, &token, "create_tag", serde_json::json!({})).await;
    assert_eq!(missing.status, 400);
    assert!(missing.json()["error"].as_str().unwrap().contains("missing argument"));

    let wrong_type = cmd(&server, &token, "create_tag", serde_json::json!({ "name": 5 })).await;
    assert_eq!(wrong_type.status, 400);
    assert!(wrong_type.json()["error"].as_str().unwrap().contains("invalid argument"));

    // Admin endpoints honor loopback trust (standard token + loopback → ok).
    let devices = send(
        &server,
        "GET",
        "/api/auth/devices",
        &[("Authorization", format!("Bearer {token}"))],
        None,
    )
    .await;
    assert_eq!(devices.status, 200, "devices: {}", devices.text());

    // Revoking our own device invalidates the token immediately.
    let device_id = body["deviceId"].as_i64().unwrap();
    let revoke = send(
        &server,
        "DELETE",
        &format!("/api/auth/devices/{device_id}"),
        &[("Authorization", format!("Bearer {token}"))],
        None,
    )
    .await;
    assert_eq!(revoke.status, 200, "revoke: {}", revoke.text());
    let after = cmd(&server, &token, "get_app_config", serde_json::json!({})).await;
    assert_eq!(after.status, 401, "revoked token must stop working");
}

#[tokio::test]
async fn collection_tag_and_album_crud_roundtrip_over_http() {
    let server = TestServer::spawn(true).await;
    shared_library_root();
    let token = login_token(&server).await;

    // -- tags ----------------------------------------------------------------
    let tag_name = uniq("itag");
    let created = cmd(&server, &token, "create_tag", serde_json::json!({ "name": tag_name })).await;
    assert_eq!(created.status, 200, "create_tag: {}", created.text());
    let tag_id = created.json()["id"].as_i64().expect("tag id in reply");

    let list = cmd(&server, &token, "get_all_tags", serde_json::json!({ "sort": 0 })).await;
    assert_eq!(list.status, 200, "get_all_tags: {}", list.text());
    assert!(
        list.json().as_array().unwrap().iter().any(|t| t["id"] == tag_id && t["name"] == tag_name),
        "created tag missing from get_all_tags"
    );

    let new_name = uniq("itag-renamed");
    let renamed = cmd(
        &server,
        &token,
        "rename_tag",
        serde_json::json!({ "tagId": tag_id, "newName": new_name }),
    )
    .await;
    assert_eq!(renamed.status, 200, "rename_tag: {}", renamed.text());
    let name_now = cmd(&server, &token, "get_tag_name", serde_json::json!({ "tagId": tag_id })).await;
    assert_eq!(name_now.status, 200, "get_tag_name: {}", name_now.text());
    assert_eq!(name_now.json(), serde_json::json!(new_name));
    let list2 = cmd(&server, &token, "get_all_tags", serde_json::json!({ "sort": 0 })).await;
    let tags2 = list2.json();
    assert!(tags2.as_array().unwrap().iter().any(|t| t["name"] == new_name));
    assert!(
        !tags2.as_array().unwrap().iter().any(|t| t["name"] == tag_name),
        "old tag name must be gone after rename"
    );

    let deleted = cmd(&server, &token, "delete_tag", serde_json::json!({ "tagId": tag_id })).await;
    assert_eq!(deleted.status, 200, "delete_tag: {}", deleted.text());
    let list3 = cmd(&server, &token, "get_all_tags", serde_json::json!({ "sort": 0 })).await;
    assert!(
        !list3.json().as_array().unwrap().iter().any(|t| t["id"] == tag_id),
        "deleted tag still listed"
    );

    // Deleting a missing tag is idempotent: the bridge serializes the
    // affected-row count, so the reply is `200` with body `0` (unlike
    // rename_* which errors on a missing id — pinned by the rate-limit test).
    let gone = cmd(&server, &token, "delete_tag", serde_json::json!({ "tagId": 987654321 })).await;
    assert_eq!(gone.status, 200, "deleting a missing tag: {}", gone.text());
    assert_eq!(gone.json(), serde_json::json!(0), "idempotent delete reports zero affected rows");

    // -- collections -----------------------------------------------------------
    let col_name = uniq("icollection");
    let col = cmd(&server, &token, "create_collection", serde_json::json!({ "name": col_name })).await;
    assert_eq!(col.status, 200, "create_collection: {}", col.text());
    let col_id = col.json()["id"].as_i64().expect("collection id in reply");

    let col_list = cmd(&server, &token, "list_collections", serde_json::json!({})).await;
    assert_eq!(col_list.status, 200, "list_collections: {}", col_list.text());
    assert!(col_list.json().as_array().unwrap().iter().any(|c| c["id"] == col_id));

    let col_ren = uniq("icollection-renamed");
    let col_rename = cmd(
        &server,
        &token,
        "rename_collection",
        serde_json::json!({ "id": col_id, "name": col_ren }),
    )
    .await;
    assert_eq!(col_rename.status, 200, "rename_collection: {}", col_rename.text());
    let col_list2 = cmd(&server, &token, "list_collections", serde_json::json!({})).await;
    assert!(col_list2.json().as_array().unwrap().iter().any(|c| c["id"] == col_id && c["name"] == col_ren));

    let col_del = cmd(&server, &token, "delete_collection", serde_json::json!({ "id": col_id })).await;
    assert_eq!(col_del.status, 200, "delete_collection: {}", col_del.text());
    let col_list3 = cmd(&server, &token, "list_collections", serde_json::json!({})).await;
    assert!(!col_list3.json().as_array().unwrap().iter().any(|c| c["id"] == col_id));

    // -- albums (folder-backed; admin exposure, loopback-trusted here) ----------
    let photo_dir = ScratchDir::fresh("album");
    let album_name = uniq("ialbum");
    let album = cmd(
        &server,
        &token,
        "add_album_by_path",
        serde_json::json!({
            "folderPath": photo_dir.path().to_string_lossy(),
            "name": album_name,
            "description": "integration test album",
            "fileTypes": 1,
            "smallImageFilter": 0,
            "excludedFolders": [],
        }),
    )
    .await;
    assert_eq!(album.status, 200, "add_album_by_path: {}", album.text());
    let album_id = album.json()["id"].as_i64().expect("album id in reply");

    let albums = cmd(&server, &token, "get_all_albums", serde_json::json!({ "refreshAccessibility": false })).await;
    assert_eq!(albums.status, 200, "get_all_albums: {}", albums.text());
    assert!(albums.json().as_array().unwrap().iter().any(|a| a["id"] == album_id));

    let album_ren = uniq("ialbum-renamed");
    let album_edit = cmd(
        &server,
        &token,
        "edit_album",
        serde_json::json!({
            "id": album_id,
            "name": album_ren,
            "description": "",
            "fileTypes": 1,
            "smallImageFilter": 0,
            "excludedFolders": [],
        }),
    )
    .await;
    assert_eq!(album_edit.status, 200, "edit_album: {}", album_edit.text());
    let albums2 = cmd(&server, &token, "get_all_albums", serde_json::json!({ "refreshAccessibility": false })).await;
    assert!(
        albums2.json().as_array().unwrap().iter().any(|a| a["id"] == album_id && a["name"] == album_ren),
        "renamed album not reflected in get_all_albums"
    );

    let album_del = cmd(&server, &token, "remove_album", serde_json::json!({ "id": album_id })).await;
    assert_eq!(album_del.status, 200, "remove_album: {}", album_del.text());
    let albums3 = cmd(&server, &token, "get_all_albums", serde_json::json!({ "refreshAccessibility": false })).await;
    assert!(!albums3.json().as_array().unwrap().iter().any(|a| a["id"] == album_id));
}

#[tokio::test]
async fn signed_media_chain_serves_and_rejects_over_live_http() {
    let server = TestServer::spawn(true).await;
    shared_library_root();
    let token = login_token(&server).await;

    // An album with a real folder to import into.
    let photo_dir = ScratchDir::fresh("media");
    let album = cmd(
        &server,
        &token,
        "add_album_by_path",
        serde_json::json!({
            "folderPath": photo_dir.path().to_string_lossy(),
            "name": uniq("media-album"),
            "description": "",
            "fileTypes": 1,
            "smallImageFilter": 0,
            "excludedFolders": [],
        }),
    )
    .await;
    assert_eq!(album.status, 200, "add_album_by_path: {}", album.text());
    let album_id = album.json()["id"].as_i64().unwrap();

    // Indexing runs in the background and is what creates the album's root
    // folder row — poll for it like a real client would.
    let indexed = cmd(
        &server,
        &token,
        "index_album",
        serde_json::json!({
            "albumId": album_id,
            "thumbnailSize": 64,
            "rawDisplayOptions": {},
            "groupRawJpegPairs": true,
        }),
    )
    .await;
    assert_eq!(indexed.status, 200, "index_album: {}", indexed.text());
    let album_path = photo_dir.path().to_string_lossy().into_owned();
    let mut root: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let folders = cmd(&server, &token, "get_all_album_folders", serde_json::json!({})).await;
        assert_eq!(folders.status, 200, "get_all_album_folders: {}", folders.text());
        root = folders
            .json()
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["album_id"] == album_id && f["path"].as_str() == Some(album_path.as_str()))
            .cloned();
        if root.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let root = root.expect("album root folder never appeared after index_album");
    let folder_id = root["id"].as_i64().expect("folder id");
    let folder_path = root["path"].as_str().expect("folder path").to_string();

    // Import a tiny real JPEG and take its file id from the reply.
    let imported = cmd(
        &server,
        &token,
        "import_file_bytes",
        serde_json::json!({
            "bytes": tiny_jpeg(),
            "name": "itest-red.jpg",
            "folderId": folder_id,
            "folderPath": folder_path,
        }),
    )
    .await;
    assert_eq!(imported.status, 200, "import_file_bytes: {}", imported.text());
    let file_id = imported.json()["id"].as_i64().expect("file id in import reply");

    let config = cmd(&server, &token, "get_app_config", serde_json::json!({})).await;
    let library = config.json()["current_library_id"].as_str().expect("library id").to_string();

    // §3.5 sign endpoint: mint one signed thumb URL.
    let sign = send(
        &server,
        "POST",
        "/api/media/sign",
        &[("Authorization", format!("Bearer {token}"))],
        Some(format!(r#"{{"urls": ["/api/thumb/{library}/{file_id}?size=64"]}}"#).into_bytes()),
    )
    .await;
    assert_eq!(sign.status, 200, "sign: {}", sign.text());
    let signed = sign.json()["urls"][0].as_str().expect("signed url").to_string();
    assert!(signed.contains("dev=") && signed.contains("exp=") && signed.contains("sig="));
    assert!(signed.starts_with(&format!("/api/thumb/{library}/{file_id}")));

    // The signed URL actually serves image bytes generated from the JPEG.
    let thumb = send(&server, "GET", &signed, &[], None).await;
    assert_eq!(thumb.status, 200, "signed thumb: {} ({})", thumb.text(), signed);
    let content_type = thumb.header("content-type").unwrap_or_default().to_string();
    assert!(content_type.starts_with("image/"), "content-type: {content_type}");
    assert!(thumb.body.len() > 32, "suspiciously small thumbnail body");
    assert!(
        thumb.body.starts_with(b"RIFF")
            || thumb.body.starts_with(&[0xff, 0xd8])
            || thumb.body.starts_with(b"\x89PNG"),
        "body is not a known image format: {:02x?}",
        &thumb.body[..4.min(thumb.body.len())]
    );

    // No signature / tampered signature / edited exp → 403, never bytes.
    let unsigned = send(&server, "GET", &format!("/api/thumb/{library}/{file_id}?size=64"), &[], None).await;
    assert_eq!(unsigned.status, 403, "unsigned media request must be rejected");
    let tampered = signed.replacen("sig=", "sig=x", 1);
    let tampered_reply = send(&server, "GET", &tampered, &[], None).await;
    assert_eq!(tampered_reply.status, 403, "tampered sig must be rejected");
    let exp_shifted = signed.replacen("exp=", "exp=1", 1);
    let shifted_reply = send(&server, "GET", &exp_shifted, &[], None).await;
    assert_eq!(shifted_reply.status, 403, "edited exp must break the MAC");

    // A signed URL for a missing file: signature accepted, handler 404s.
    let missing_sign = send(
        &server,
        "POST",
        "/api/media/sign",
        &[("Authorization", format!("Bearer {token}"))],
        Some(format!(r#"{{"urls": ["/api/thumb/{library}/987654321?size=64"]}}"#).into_bytes()),
    )
    .await;
    assert_eq!(missing_sign.status, 200);
    let missing_url = missing_sign.json()["urls"][0].as_str().unwrap().to_string();
    let missing = send(&server, "GET", &missing_url, &[], None).await;
    assert_eq!(missing.status, 404, "missing file: {}", missing.text());

    // Sign endpoint contract: urls required, media prefixes only.
    let no_urls = send(
        &server,
        "POST",
        "/api/media/sign",
        &[("Authorization", format!("Bearer {token}"))],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(no_urls.status, 400, "missing urls field");
    let foreign = send(
        &server,
        "POST",
        "/api/media/sign",
        &[("Authorization", format!("Bearer {token}"))],
        Some(br#"{"urls": ["/api/cmd/get_app_config"]}"#.to_vec()),
    )
    .await;
    assert_eq!(foreign.status, 400, "non-media url must be refused");

    // File-tag linkage over the same chain.
    let tag = cmd(&server, &token, "create_tag", serde_json::json!({ "name": uniq("mediatag") })).await;
    let tag_id = tag.json()["id"].as_i64().unwrap();
    let link = cmd(
        &server,
        &token,
        "add_tag_to_file",
        serde_json::json!({ "fileId": file_id, "tagId": tag_id }),
    )
    .await;
    assert_eq!(link.status, 200, "add_tag_to_file: {}", link.text());
    let on_file = cmd(&server, &token, "get_tags_for_file", serde_json::json!({ "fileId": file_id })).await;
    assert_eq!(on_file.status, 200, "get_tags_for_file: {}", on_file.text());
    assert!(on_file.json().as_array().unwrap().iter().any(|t| t["id"] == tag_id));
    let unlink = cmd(
        &server,
        &token,
        "remove_tag_from_file",
        serde_json::json!({ "fileId": file_id, "tagId": tag_id }),
    )
    .await;
    assert_eq!(unlink.status, 200, "remove_tag_from_file: {}", unlink.text());
    let on_file2 = cmd(&server, &token, "get_tags_for_file", serde_json::json!({ "fileId": file_id })).await;
    assert!(!on_file2.json().as_array().unwrap().iter().any(|t| t["id"] == tag_id));
}

#[tokio::test]
async fn sse_stream_and_last_event_id_resume_over_live_http() {
    let server = TestServer::spawn(true).await;
    let token = login_token(&server).await;

    // No token → 401 even on the event stream.
    let denied = send(&server, "GET", "/api/events", &[], None).await;
    assert_eq!(denied.status, 401);

    let sink = SseSink::new(server.bus.clone());
    sink.emit("itest-event-a", serde_json::json!({ "n": 1 }));
    sink.emit("itest-event-b", serde_json::json!({ "n": 2 }));
    sink.emit("itest-event-c", serde_json::json!({ "n": 3 }));

    // Fresh connection replays the retained buffer, oldest first.
    let frames = sse_read_frames(&server, &token, None, 3).await;
    assert_eq!(frames.len(), 3, "unexpected frames: {frames:?}");
    let ids: Vec<u64> = frames
        .iter()
        .map(|f| f.data["id"].as_str().expect("string id").parse().expect("numeric id"))
        .collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids must be strictly increasing: {ids:?}");
    // The protocol-level `id:` line must carry the same value as the JSON
    // envelope: it is what the browser echoes as `Last-Event-ID`. Without it
    // the client cannot resume and a reconnect replays the whole buffer.
    for (frame, id) in frames.iter().zip(&ids) {
        assert_eq!(
            frame.id.as_deref(),
            Some(id.to_string().as_str()),
            "SSE id: line must match the envelope id"
        );
    }
    assert_eq!(frames[0].data["event"], "itest-event-a");
    assert_eq!(frames[1].data["event"], "itest-event-b");
    assert_eq!(frames[2].data["event"], "itest-event-c");
    assert_eq!(frames[0].data["payload"]["n"], 1);
    let data_shape = frames[0].data.as_object().expect("envelope object");
    assert_eq!(data_shape.len(), 3, "envelope carries exactly event/id/payload");

    // Resume from the first id: only b and c replay.
    let resumed = sse_read_frames(&server, &token, Some(ids[0]), 2).await;
    assert_eq!(resumed.len(), 2);
    assert_eq!(resumed[0].data["event"], "itest-event-b");
    assert_eq!(resumed[1].data["event"], "itest-event-c");
}

#[tokio::test]
async fn rate_limits_lockout_and_shutdown_rejection_over_live_http() {
    let server = TestServer::spawn(true).await;

    // 5 consecutive failures lock the IP: the 6th attempt is 429 even with
    // the CORRECT password.
    for i in 0..5 {
        let bad = login(&server, "wrong-password").await;
        assert_eq!(bad.status, 401, "failed attempt {i} must stay 401");
    }
    let locked = login(&server, ADMIN_PW).await;
    assert_eq!(locked.status, 429, "locked-out attempt: {}", locked.text());
    assert!(locked.json()["error"].as_str().is_some());

    // Fresh server = fresh limiter state. Write budget: 60/min per device —
    // and failing commands consume budget too (checked before dispatch).
    let server2 = TestServer::spawn(true).await;
    let token = login_token(&server2).await;
    let mut statuses = Vec::new();
    for _ in 0..61 {
        let reply = cmd(
            &server2,
            &token,
            "rename_collection",
            serde_json::json!({ "id": 987654321, "name": "x" }),
        )
        .await;
        statuses.push(reply.status);
    }
    assert_eq!(statuses[0], 500, "first attempt must reach the command (missing id → 500)");
    assert!(
        statuses[..60].iter().all(|s| *s == 500),
        "attempts 1..=60 must reach the command, got {:?}",
        &statuses[..60]
    );
    assert_eq!(statuses[60], 429, "attempt 61 must hit the write budget");

    // Read budget is separate: still served after writes are exhausted.
    let read = cmd(&server2, &token, "get_app_config", serde_json::json!({})).await;
    assert_eq!(read.status, 200, "reads must not share the write budget: {}", read.text());

    // §3.7: after a shutdown request, new commands are rejected with 503.
    server2.shutdown.request();
    let rejected = cmd(&server2, &token, "get_app_config", serde_json::json!({})).await;
    assert_eq!(rejected.status, 503, "post-shutdown command: {}", rejected.text());
}

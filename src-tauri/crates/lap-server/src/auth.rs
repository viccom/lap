//! lapd authentication (plan §3.6): Argon2id admin password, 6-digit
//! pairing codes, 32-byte device tokens stored as SHA-256, Cookie/Bearer
//! extraction, rate limiting, and the blake3-keyed MAC behind the §3.5
//! signed media URLs.
//!
//! Pinned contract: pairing code 5-minute validity; tokens stored only as
//! SHA-256 hex; cookie `HttpOnly; SameSite=Strict` (no `Secure` until TLS
//! exists per §3.6); login 5/min/IP with failure lockout; commands
//! 60/min/device; secrets never appear in logs or serialized responses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::auth_store::{AuthStore, SCOPE_ADMIN, SCOPE_STANDARD};

pub const COOKIE_NAME: &str = "lapd_token";
/// §3.5: signed media URLs are valid for 10 minutes.
#[allow(dead_code)] // consumed by the media endpoints (batch 3.3 follow-up)
    pub const MEDIA_URL_TTL_SECS: u64 = 600;
const PAIRING_TTL: Duration = Duration::from_secs(300);
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_LIMIT: u32 = 5;
const FAIL_LOCK_THRESHOLD: u32 = 5;
const FAIL_LOCK: Duration = Duration::from_secs(300);
const CMD_WINDOW: Duration = Duration::from_secs(60);
/// Mutating/admin commands stay on the tight §3.6 budget (60/min/device).
const CMD_LIMIT: u32 = 60;
/// Read commands get their own, far wider budget. Measured cost of ONE view
/// switch in the browser: ~25 commands (album list, counts, timeline, a
/// get_query_files window, and a handful of thumbnail batches — ~4 once the
/// grid packs 32 thumbnails per batch) — so the 60/min ceiling that is right
/// for mutations throttled normal browsing after a single switch, and the Web
/// client surfaced the resulting 429s as a logged-out "loading" screen.
/// 1200/min (20/s sustained, ≈24 view switches per minute) is well above
/// anything a human produces while still bounding a runaway client.
const READ_LIMIT: u32 = 1200;
#[allow(dead_code)] // consumed by the media endpoints (batch 3.3 follow-up)
const MEDIA_PREFIXES: [&str; 4] = ["/api/thumb/", "/api/preview/", "/api/original/", "/api/video/"];

/// Everything an endpoint can fail with, each with its HTTP status.
#[derive(Debug)]
pub enum AuthError {
    /// 401 — missing/invalid token or bad credentials.
    Unauthorized(&'static str),
    /// 403 — insufficient scope, non-loopback setup, bad signature.
    Forbidden(&'static str),
    /// 409 — password already set.
    Conflict(&'static str),
    /// 429 — rate limited.
    RateLimited(&'static str),
    /// 400 — malformed request.
    BadRequest(String),
    /// 500 — storage or hashing failure.
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.to_string()),
            AuthError::Forbidden(m) => (StatusCode::FORBIDDEN, m.to_string()),
            AuthError::Conflict(m) => (StatusCode::CONFLICT, m.to_string()),
            AuthError::RateLimited(m) => (StatusCode::TOO_MANY_REQUESTS, m.to_string()),
            AuthError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AuthError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}

struct PairingSlot {
    code: String,
    expires: tokio::time::Instant,
}

/// Fixed-window counter per key. Time comes from `tokio::time::Instant` so
/// the `start_paused` tests can drive windows deterministically.
struct RateLimiter {
    windows: Mutex<HashMap<String, (tokio::time::Instant, u32)>>,
    limit: u32,
    window: Duration,
}

impl RateLimiter {
    fn new(limit: u32, window: Duration) -> RateLimiter {
        RateLimiter { windows: Mutex::new(HashMap::new()), limit, window }
    }

    /// Consume one slot; false when the key exceeded its window.
    fn check(&self, key: &str) -> bool {
        let now = tokio::time::Instant::now();
        let mut windows = self.windows.lock().unwrap();
        if windows.len() > 4096 {
            windows.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let entry = windows.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 1);
            return true;
        }
        if entry.1 >= self.limit {
            return false;
        }
        entry.1 += 1;
        true
    }
}

/// Consecutive-failure lockout for the login/pair endpoints: 5 failures in
/// a row from one IP lock that IP out for 5 minutes (plan §3.6 失败锁定).
#[derive(Default)]
struct LoginLockout {
    state: Mutex<HashMap<String, (u32, Option<tokio::time::Instant>)>>,
}

impl LoginLockout {
    fn is_locked(&self, ip: &str) -> bool {
        let now = tokio::time::Instant::now();
        let state = self.state.lock().unwrap();
        matches!(
            state.get(ip),
            Some((_, Some(until))) if *until > now
        )
    }

    fn register_fail(&self, ip: &str) {
        let now = tokio::time::Instant::now();
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(ip.to_string()).or_insert((0, None));
        entry.0 += 1;
        if entry.0 >= FAIL_LOCK_THRESHOLD {
            entry.1 = Some(now + FAIL_LOCK);
            entry.0 = 0;
        }
    }

    fn clear(&self, ip: &str) {
        self.state.lock().unwrap().remove(ip);
    }
}

pub struct AuthService {
    store: AuthStore,
    data_dir: PathBuf,
    /// blake3 keyed-MAC key for signed media URLs; boot-random, never
    /// persisted (URLs expire in 10 minutes, a restart invalidates them).
    #[allow(dead_code)] // consumed by the media endpoints (batch 3.3 follow-up)
    mac_key: [u8; 32],
    password_path: PathBuf,
    pairing: Mutex<Option<PairingSlot>>,
    login_limiter: RateLimiter,
    /// Mutating/admin commands (tight §3.6 budget).
    cmd_limiter: RateLimiter,
    /// Read commands (wider budget; see READ_LIMIT).
    read_limiter: RateLimiter,
    lockout: LoginLockout,
}

pub fn admin_password_hash_path(data_dir: &Path) -> PathBuf {
    data_dir.join("admin-password-hash")
}

#[allow(dead_code)] // consume time in the media endpoints (follow-up commit)
    fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Constant-time byte comparison for MAC verification.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn token_hash_hex(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Uniform 6-digit code from OsRng (rejection sampling on the top range).
fn random_code() -> String {
    loop {
        let n = OsRng.next_u32();
        if n < 1_000_000 {
            return format!("{n:06}");
        }
    }
}

impl AuthService {
    pub fn open(data_dir: &Path) -> Result<AuthService, String> {
        let store = AuthStore::open(data_dir)?;
        let mut mac_key = [0u8; 32];
        OsRng.fill_bytes(&mut mac_key);
        Ok(AuthService {
            store,
            data_dir: data_dir.to_path_buf(),
            mac_key,
            password_path: admin_password_hash_path(data_dir),
            pairing: Mutex::new(None),
            login_limiter: RateLimiter::new(LOGIN_LIMIT, LOGIN_WINDOW),
            cmd_limiter: RateLimiter::new(CMD_LIMIT, CMD_WINDOW),
            read_limiter: RateLimiter::new(READ_LIMIT, CMD_WINDOW),
            lockout: LoginLockout::default(),
        })
    }

    pub fn password_set(&self) -> bool {
        self.password_path.exists()
    }

    /// Hash and persist the admin password. Refuses to overwrite: rotation
    /// is a deliberate future feature, not a side effect of setup.
    pub fn set_admin_password(data_dir: &Path, password: &str) -> Result<(), AuthError> {
        let password = password.trim();
        if password.is_empty() {
            return Err(AuthError::BadRequest("password must not be empty".into()));
        }
        let path = admin_password_hash_path(data_dir);
        if path.exists() {
            return Err(AuthError::Conflict("admin password already set".into()));
        }
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AuthError::Internal(format!("hashing password: {e}")))?
            .to_string();
        lap_core::t_config::write_atomic_private(&path, &hash)
            .map_err(|e| AuthError::Internal(format!("writing password hash: {e}")))
    }

    fn verify_password(&self, password: &str) -> Result<bool, AuthError> {
        let stored = std::fs::read_to_string(&self.password_path)
            .map_err(|_| AuthError::Unauthorized("admin password not set up".into()))?;
        let parsed = PasswordHash::new(stored.trim())
            .map_err(|e| AuthError::Internal(format!("parsing stored hash: {e}")))?;
        Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
    }

    /// Password login → `standard`-scope token (§3.9: web clients browse and
    /// edit lightly; heavy admin work belongs to paired devices).
    pub fn login(
        &self,
        password: &str,
        device_name: &str,
        ip: &str,
    ) -> Result<(String, &'static str, i64), AuthError> {
        self.authorize_credentials(ip)?;
        if !self.verify_password(password)? {
            self.lockout.register_fail(ip);
            self.store.audit("login_fail", &format!("ip={ip}"));
            return Err(AuthError::Unauthorized("invalid credentials".into()));
        }
        self.lockout.clear(ip);
        let (token, id) = self.mint_token(device_name, SCOPE_STANDARD);
        self.store.audit("login_ok", &format!("ip={ip} device={device_name}"));
        Ok((token, SCOPE_STANDARD, id))
    }

    /// Pairing-code exchange → `admin`-scope token: whoever holds the code
    /// already had console access.
    pub fn pair(
        &self,
        code: &str,
        device_name: &str,
        ip: &str,
    ) -> Result<(String, &'static str, i64), AuthError> {
        self.authorize_credentials(ip)?;
        let valid = {
            let pairing = self.pairing.lock().unwrap();
            match pairing.as_ref() {
                Some(slot) => {
                    ct_eq(slot.code.as_bytes(), code.trim().as_bytes())
                        && slot.expires > tokio::time::Instant::now()
                }
                None => false,
            }
        };
        if !valid {
            self.lockout.register_fail(ip);
            self.store.audit("pair_fail", &format!("ip={ip}"));
            return Err(AuthError::Unauthorized("invalid or expired pairing code".into()));
        }
        self.pairing.lock().unwrap().take();
        self.lockout.clear(ip);
        let (token, id) = self.mint_token(device_name, SCOPE_ADMIN);
        self.store.audit("pair_ok", &format!("ip={ip} device={device_name}"));
        Ok((token, SCOPE_ADMIN, id))
    }

    /// Mint a fresh pairing code (admin endpoint or boot console).
    pub fn mint_pairing_code(&self) -> String {
        let code = random_code();
        let mut pairing = self.pairing.lock().unwrap();
        *pairing = Some(PairingSlot { code: code.clone(), expires: tokio::time::Instant::now() + PAIRING_TTL });
        code
    }

    fn mint_token(&self, device_name: &str, scope: &'static str) -> (String, i64) {
        let token = random_token();
        let hash = token_hash_hex(&token);
        let id = self
            .store
            .insert_token(&hash, &device_name.trim_empty_to_default(), scope)
            .expect("auth store writes must not fail silently");
        (token, id)
    }

    /// Resolve the caller's device from Cookie or `Authorization: Bearer`.
    pub fn authenticate(&self, headers: &HeaderMap) -> Option<crate::auth_store::TokenRow> {
        let raw = token_from_headers(headers)?;
        let row = self.store.find_token(&token_hash_hex(&raw)).ok()??;
        self.store.touch_token(row.id);
        Some(row)
    }

    /// Admin endpoints: valid token with admin scope, or a loopback caller.
    pub fn require_admin(
        &self,
        headers: &HeaderMap,
        remote_ip: Option<std::net::IpAddr>,
    ) -> Result<crate::auth_store::TokenRow, AuthError> {
        let row = self.authenticate(headers).ok_or(AuthError::Unauthorized("authentication required".into()))?;
        let loopback = remote_ip.is_some_and(|ip| ip.is_loopback());
        if row.scope == SCOPE_ADMIN || loopback {
            Ok(row)
        } else {
            Err(AuthError::Forbidden("admin scope required".into()))
        }
    }

    /// Per-device command rate limit (§3.6: 60/min/device for mutations;
    /// reads get READ_LIMIT so browsing is not throttled).
    pub fn check_cmd_rate(&self, token_id: i64, read: bool) -> bool {
        let limiter = if read { &self.read_limiter } else { &self.cmd_limiter };
        limiter.check(&token_id.to_string())
    }

    #[allow(dead_code)] // consume in the whitelist middleware (follow-up commit)
    pub fn is_loopback_or_admin(
        &self,
        token: Option<&crate::auth_store::TokenRow>,
        remote_ip: Option<std::net::IpAddr>,
    ) -> bool {
        let loopback = remote_ip.is_some_and(|ip| ip.is_loopback());
        loopback || token.is_some_and(|t| t.scope == SCOPE_ADMIN)
    }

    // -- signed media URLs (§3.5) ------------------------------------------

    #[allow(dead_code)] // consume in the media endpoints (follow-up commit)
    fn media_mac(&self, canonical: &str) -> String {
        blake3::keyed_hash(&self.mac_key, canonical.as_bytes()).to_hex().to_string()
    }

    #[allow(dead_code)] // consume in the media endpoints (follow-up commit)
    fn canonical_media_string(path_and_query: &str, exp: u64, device_id: i64) -> Option<String> {
        // Split path?query, drop `exp` and `sig` (keep everything else in
        // received order), and rebuild the MAC input deterministically.
        let (path, query) = match path_and_query.split_once('?') {
            Some((p, q)) => (p, q),
            None => (path_and_query, ""),
        };
        let mut kept: Vec<String> = Vec::new();
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let key = pair.split('=').next().unwrap_or(pair);
            if key == "exp" || key == "sig" {
                continue;
            }
            kept.push(pair.to_string());
        }
        Some(format!("GET|{path}?{}|{exp}|{device_id}", kept.join("&")))
    }

    /// Sign one media URL for a device: appends `dev`, `exp`, `sig`.
    #[allow(dead_code)] // consume in the media endpoints (follow-up commit)
    pub fn sign_media_url(&self, url: &str, device_id: i64) -> Result<String, AuthError> {
        let url = url.trim();
        if url.len() > 2048 {
            return Err(AuthError::BadRequest("media url too long".into()));
        }
        if !MEDIA_PREFIXES.iter().any(|p| url.starts_with(p)) {
            return Err(AuthError::BadRequest(format!("not a media endpoint: {url}")));
        }
        let pq = url.strip_prefix('/').unwrap_or(url);
        if pq.split(['?', '&']).any(|part| {
            let key = part.split('=').next().unwrap_or(part);
            key == "exp" || key == "sig" || key == "dev"
        }) {
            return Err(AuthError::BadRequest("media url must not carry exp/sig/dev".into()));
        }
        let exp = unix_now() + MEDIA_URL_TTL_SECS;
        let separator = if pq.contains('?') { "&" } else { "?" };
        let signed_path = format!("{pq}{separator}dev={device_id}");
        let canonical = Self::canonical_media_string(&signed_path, exp, device_id)
            .ok_or(AuthError::BadRequest("unparsable media url".into()))?;
        let sig = self.media_mac(&canonical);
        Ok(format!("/{}&exp={exp}&sig={sig}", signed_path))
    }

    /// Verify a signed media request: expiry first, then the MAC over the
    /// canonical string rebuilt from the received query.
    #[allow(dead_code)] // consume in the media endpoints (follow-up commit)
    pub fn verify_media_url(&self, path_and_query: &str) -> Result<(), AuthError> {
        let (_path, query) = path_and_query
            .split_once('?')
            .ok_or(AuthError::Forbidden("missing media signature".into()))?;
        let mut exp: Option<u64> = None;
        let mut sig: Option<String> = None;
        let mut dev: Option<i64> = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(AuthError::Forbidden("malformed media query".into()))?;
            match key {
                "exp" => exp = Some(value.parse().map_err(|_| AuthError::Forbidden("bad exp".into()))?),
                "sig" => sig = Some(value.to_string()),
                "dev" => dev = Some(value.parse().map_err(|_| AuthError::Forbidden("bad dev".into()))?),
                _ => {}
            }
        }
        let exp = exp.ok_or(AuthError::Forbidden("missing exp".into()))?;
        let sig = sig.ok_or(AuthError::Forbidden("missing sig".into()))?;
        let dev = dev.ok_or(AuthError::Forbidden("missing dev".into()))?;
        if unix_now() > exp {
            return Err(AuthError::Forbidden("media url expired".into()));
        }
        let canonical = Self::canonical_media_string(path_and_query, exp, dev)
            .ok_or(AuthError::Forbidden("malformed media query".into()))?;
        let expected = self.media_mac(&canonical);
        if !ct_eq(expected.as_bytes(), sig.as_bytes()) {
            return Err(AuthError::Forbidden("invalid media signature".into()));
        }
        Ok(())
    }

    fn authorize_credentials(&self, ip: &str) -> Result<(), AuthError> {
        if self.lockout.is_locked(ip) {
            return Err(AuthError::RateLimited("too many failures, try later".into()));
        }
        if !self.login_limiter.check(ip) {
            return Err(AuthError::RateLimited("too many attempts".into()));
        }
        Ok(())
    }
}

trait TrimEmptyToDefault {
    fn trim_empty_to_default(&self) -> &str;
}

impl TrimEmptyToDefault for str {
    fn trim_empty_to_default(&self) -> &str {
        let trimmed = self.trim();
        if trimmed.is_empty() { "device" } else { trimmed }
    }
}

/// `Authorization: Bearer <token>` first, then the `lapd_token` cookie.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(text) = value.to_str() {
            if let Some(token) = text.strip_prefix("Bearer ") {
                return Some(token.trim().to_string());
            }
        }
    }
    for cookie in headers.get_all(axum::http::header::COOKIE) {
        let text = cookie.to_str().unwrap_or_default();
        for pair in text.split(';') {
            let pair = pair.trim();
            if let Some(value) = pair.strip_prefix(&format!("{COOKIE_NAME}=")) {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Build the Set-Cookie header value for a session token.
pub fn session_cookie(token: &str) -> String {
    format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/")
}

// -- endpoint handlers ----------------------------------------------------
//
// Auth endpoints are intentionally open (rate-limited): the client must be
// able to discover setup state and obtain its first token anonymously.
// Everything else goes through the middleware that lands with the
// whitelist-enforcement commit.

fn parse_json_object(body: &str) -> Result<serde_json::Value, AuthError> {
    if body.trim().is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(AuthError::BadRequest("request body must be a JSON object".into())),
        Err(e) => Err(AuthError::BadRequest(format!("invalid JSON body: {e}"))),
    }
}

fn body_str<'a>(body: &'a serde_json::Value, key: &str) -> Result<&'a str, AuthError> {
    body.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthError::BadRequest(format!("missing string field `{key}`")))
}

/// GET /api/auth/status — public: lets a client decide between the setup
/// screen and the login screen.
pub async fn auth_status(
    State(auth): State<std::sync::Arc<AuthService>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "auth": true,
        "passwordSet": auth.password_set(),
    }))
}

/// POST /api/auth/setup {password} — first-run only, loopback-only: a LAN
/// caller must never be able to claim the server before its owner does.
pub async fn auth_setup(
    State(auth): State<std::sync::Arc<AuthService>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: String,
) -> Result<axum::Json<serde_json::Value>, AuthError> {
    if !addr.ip().is_loopback() {
        return Err(AuthError::Forbidden("setup is only allowed from localhost".into()));
    }
    let body = parse_json_object(&body)?;
    let password = body_str(&body, "password")?.to_string();
    AuthService::set_admin_password(&auth.data_dir, &password)?;
    auth.store.audit("setup", &format!("ip={}", addr.ip()));
    Ok(axum::Json(serde_json::json!({ "ok": true })))
}

/// POST /api/auth/login {password, deviceName} — issues a standard-scope
/// token via cookie AND body (non-browser clients cannot use cookies).
pub async fn auth_login(
    State(auth): State<std::sync::Arc<AuthService>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: String,
) -> Result<Response, AuthError> {
    let body = parse_json_object(&body)?;
    let password = body_str(&body, "password")?.to_string();
    let device_name = body
        .get("deviceName")
        .and_then(|v| v.as_str())
        .unwrap_or("device")
        .to_string();
    let (token, scope, id) = auth.login(&password, &device_name, &addr.ip().to_string())?;
    credential_response(&token, scope, &device_name, id)
}

/// POST /api/auth/pair {code, deviceName} — issues an admin-scope token.
pub async fn auth_pair(
    State(auth): State<std::sync::Arc<AuthService>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: String,
) -> Result<Response, AuthError> {
    let body = parse_json_object(&body)?;
    let code = body_str(&body, "code")?.to_string();
    let device_name = body
        .get("deviceName")
        .and_then(|v| v.as_str())
        .unwrap_or("device")
        .to_string();
    let (token, scope, id) = auth.pair(&code, &device_name, &addr.ip().to_string())?;
    credential_response(&token, scope, &device_name, id)
}

impl AuthService {
    /// The §3.6 default-deny decision for one protected request. `command`
    /// is `Some` on `/api/cmd/{name}` (whitelist + scope + rate limit) and
    /// `None` on token-only routes (`/api/events`).
    pub fn authorize_request(
        &self,
        headers: &HeaderMap,
        command: Option<&str>,
        remote_ip: Option<std::net::IpAddr>,
    ) -> Result<crate::auth_store::TokenRow, AuthError> {
        let token = self
            .authenticate(headers)
            .ok_or(AuthError::Unauthorized("authentication required".into()))?;
        let loopback = remote_ip.is_some_and(|ip| ip.is_loopback());
        if let Some(name) = command {
            let exposure = crate::bridge_gen::EXPOSURES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, exposure)| *exposure)
                .ok_or(AuthError::Forbidden("command not whitelisted".into()))?;
            if exposure == "admin" {
                // Attempt-level audit: rejected admin attempts are the
                // interesting ones (plan §3.6 记录管理员操作).
                self.store.audit("admin_cmd", &format!("device={} cmd={name}", token.id));
            }
            if exposure == "admin" && token.scope != SCOPE_ADMIN && !loopback {
                return Err(AuthError::Forbidden("admin scope required".into()));
            }
            // Reads and mutations have separate budgets: a browsing session
            // legitimately issues dozens of reads per view switch, while the
            // tight limit belongs on state changes.
            if !self.check_cmd_rate(token.id, exposure == "read") {
                return Err(AuthError::RateLimited("command rate limit exceeded".into()));
            }
        }
        Ok(token)
    }
}

/// axum adapter wrapping the protected router (`/api/cmd/*`, `/api/events`,
/// later the signed media routes).
pub async fn whitelist_middleware(
    State(auth): State<std::sync::Arc<AuthService>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    axum::Extension(shutdown): axum::Extension<std::sync::Arc<crate::shutdown::ShutdownState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // §3.7: a shutting-down server stops accepting new commands. The
    // in-flight ones are allowed to finish (axum waits for them below).
    if let Some(response) = crate::shutdown::reject_while_shutting_down(&shutdown) {
        return response;
    }
    let command = request
        .uri()
        .path()
        .strip_prefix("/api/cmd/")
        .map(|rest| rest.to_string());
    if let Err(e) = auth.authorize_request(&headers, command.as_deref(), Some(addr.ip())) {
        return e.into_response();
    }
    next.run(request).await
}

fn credential_response(token: &str, scope: &str, device_name: &str, id: i64) -> Result<Response, AuthError> {
    let cookie = session_cookie(token);
    let mut response = (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "token": token,
            "scope": scope,
            "deviceName": device_name,
            "deviceId": id,
        })),
    )
        .into_response();
    let value = axum::http::HeaderValue::from_str(&cookie)
        .map_err(|_| AuthError::Internal("building cookie header".into()))?;
    response
        .headers_mut()
        .append(axum::http::header::SET_COOKIE, value);
    Ok(response)
}

/// POST /api/auth/pairing-code — admin/loopback only; mints a fresh code
/// for the next device and returns it for relaying.
pub async fn auth_pairing_code(
    State(auth): State<std::sync::Arc<AuthService>>,
    headers: HeaderMap,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
) -> Result<axum::Json<serde_json::Value>, AuthError> {
    let device = auth.require_admin(&headers, Some(addr.ip()))?;
    let code = auth.mint_pairing_code();
    auth.store.audit("pairing_code_minted", &format!("device={}", device.id));
    Ok(axum::Json(serde_json::json!({
        "code": code,
        "expiresInSeconds": PAIRING_TTL.as_secs(),
    })))
}

/// GET /api/auth/devices — admin/loopback only.
pub async fn devices_list(
    State(auth): State<std::sync::Arc<AuthService>>,
    headers: HeaderMap,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
) -> Result<axum::Json<serde_json::Value>, AuthError> {
    auth.require_admin(&headers, Some(addr.ip()))?;
    let devices = auth.store.list_tokens().map_err(AuthError::Internal)?;
    Ok(axum::Json(serde_json::json!({ "devices": devices })))
}

/// DELETE /api/auth/devices/{id} — admin/loopback only.
pub async fn device_revoke(
    State(auth): State<std::sync::Arc<AuthService>>,
    headers: HeaderMap,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<axum::Json<serde_json::Value>, AuthError> {
    let device = auth.require_admin(&headers, Some(addr.ip()))?;
    let removed = auth.store.delete_token(id).map_err(AuthError::Internal)?;
    if !removed {
        return Err(AuthError::BadRequest(format!("no device with id {id}")));
    }
    auth.store.audit("revoke", &format!("device={id} by={}", device.id));
    Ok(axum::Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lapd-auth-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn argon2_roundtrip_and_setup_once() {
        let dir = scratch("argon");
        AuthService::set_admin_password(&dir, " correct horse ").unwrap();
        assert!(admin_password_hash_path(&dir).exists());
        assert!(AuthService::set_admin_password(&dir, "again").is_err());
        let auth = AuthService::open(&dir).unwrap();
        assert!(auth.password_set());
        assert!(auth.verify_password("correct horse").unwrap());
        assert!(!auth.verify_password("wrong").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_password_is_rejected() {
        let dir = scratch("empty");
        assert!(matches!(
            AuthService::set_admin_password(&dir, "   "),
            Err(AuthError::BadRequest(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn pairing_code_flow_mints_admin_tokens() {
        let dir = scratch("pair");
        let auth = AuthService::open(&dir).unwrap();
        let code = auth.mint_pairing_code();
        assert_eq!(code.len(), 6);
        let (token, scope, _id) = auth.pair(&code, "phone", "127.0.0.1").unwrap();
        assert_eq!(scope, SCOPE_ADMIN);
        // The token authenticates and the code is consumed.
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert_eq!(auth.authenticate(&headers).unwrap().scope, SCOPE_ADMIN);
        assert!(matches!(
            auth.pair(&code, "again", "127.0.0.1"),
            Err(AuthError::Unauthorized(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn pairing_codes_expire_after_five_minutes() {
        let dir = scratch("pair-exp");
        let auth = AuthService::open(&dir).unwrap();
        let code = auth.mint_pairing_code();
        tokio::time::advance(PAIRING_TTL + Duration::from_secs(1)).await;
        assert!(matches!(
            auth.pair(&code, "phone", "127.0.0.1"),
            Err(AuthError::Unauthorized(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn login_issues_standard_scope_and_audits_failures() {
        let dir = scratch("login");
        AuthService::set_admin_password(&dir, "hunter2").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        let (token, scope, _id) = auth.login("hunter2", "tablet", "10.0.0.9").unwrap();
        assert_eq!(scope, SCOPE_STANDARD);
        assert!(auth.verify_password("hunter2").unwrap());
        assert!(matches!(
            auth.login("wrong", "tablet", "10.0.0.9"),
            Err(AuthError::Unauthorized(_))
        ));
        // The raw token must never round-trip: only its hash is stored.
        assert!(auth.store.find_token(&token).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn login_rate_limits_at_five_per_minute_per_ip() {
        let dir = scratch("rate");
        AuthService::set_admin_password(&dir, "pw").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        for _ in 0..LOGIN_LIMIT {
            // Wrong-password attempts consume the window AND register fails;
            // either way the 6th attempt must be shut out.
            let _ = auth.login("nope", "d", "10.0.0.1");
        }
        assert!(matches!(
            auth.login("pw", "d", "10.0.0.1"),
            Err(AuthError::RateLimited(_)) | Err(AuthError::Unauthorized(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn five_consecutive_failures_lock_the_ip_for_five_minutes() {
        let dir = scratch("lock");
        AuthService::set_admin_password(&dir, "pw").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        for _ in 0..FAIL_LOCK_THRESHOLD {
            let _ = auth.login("nope", "d", "10.0.0.2");
        }
        assert!(auth.lockout.is_locked("10.0.0.2"));
        tokio::time::advance(FAIL_LOCK + Duration::from_secs(1)).await;
        assert!(!auth.lockout.is_locked("10.0.0.2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_extraction_prefers_bearer_then_cookie() {
        let mut headers = HeaderMap::new();
        assert!(token_from_headers(&headers).is_none());
        headers.insert(
            axum::http::header::COOKIE,
            "other=1; lapd_token=abc123".parse().unwrap(),
        );
        assert_eq!(token_from_headers(&headers).unwrap(), "abc123");
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer xyz".parse().unwrap(),
        );
        assert_eq!(token_from_headers(&headers).unwrap(), "xyz");
    }

    #[test]
    fn session_cookie_has_the_pinned_attributes() {
        let cookie = session_cookie("t");
        assert!(cookie.starts_with("lapd_token=t;"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/"));
        // No `Secure` until TLS exists (§3.6).
        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn media_sign_verify_roundtrip_and_reject_tampering() {
        let dir = scratch("mac");
        let auth = AuthService::open(&dir).unwrap();
        let signed = auth.sign_media_url("/api/thumb/lib1/42?size=512", 7).unwrap();
        assert!(signed.contains("dev=7"));
        assert!(signed.contains("exp="));
        assert!(signed.contains("sig="));
        auth.verify_media_url(signed.trim_start_matches('/')).unwrap();
        // Query-less endpoints must use `?` for the first appended param.
        let signed = auth.sign_media_url("/api/original/1", 7).unwrap();
        assert!(signed.starts_with("/api/original/1?dev=7"), "got {signed}");
        auth.verify_media_url(signed.trim_start_matches('/')).unwrap();
        // Any tampering — size, path, exp, sig, dev — must fail.
        let tampered = signed.replace("dev=7", "dev=8");
        assert!(auth.verify_media_url(tampered.trim_start_matches('/')).is_err());
        let tampered = signed.replace('a', "b");
        assert!(auth.verify_media_url(tampered.trim_start_matches('/')).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_signature_expiry_is_enforced() {
        let dir = scratch("mac-exp");
        let auth = AuthService::open(&dir).unwrap();
        // Expiry compares wall-clock unix seconds; a past exp must be
        // rejected before the MAC is even considered.
        let expired = "/api/original/1?dev=1&exp=1000&sig=deadbeef";
        assert!(matches!(
            auth.verify_media_url(expired),
            Err(AuthError::Forbidden(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_sign_rejects_foreign_paths_and_prefixed_params() {
        let dir = scratch("mac-foreign");
        let auth = AuthService::open(&dir).unwrap();
        assert!(auth.sign_media_url("/api/cmd/get_app_config", 1).is_err());
        assert!(auth.sign_media_url("/api/thumb/lib1/1?exp=1", 1).is_err());
        assert!(auth.sign_media_url("/api/thumb/lib1/1&sig=x", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ct_eq_is_lenient_lengthwise_and_exact_contentwise() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[tokio::test(start_paused = true)]
    async fn whitelist_decisions_follow_the_exposure_axis() {
        let dir = scratch("whitelist");
        AuthService::set_admin_password(&dir, "pw").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        let lan: std::net::IpAddr = "10.0.0.5".parse().unwrap();
        let loopback: std::net::IpAddr = "127.0.0.1".parse().unwrap();

        let mut headers = HeaderMap::new();
        // No token → 401 before anything else.
        assert!(matches!(
            auth.authorize_request(&headers, Some("get_app_config"), Some(lan)),
            Err(AuthError::Unauthorized(_))
        ));
        // Password login → standard scope.
        let (token, _scope, _id) = auth.login("pw", "web", "10.0.0.5").unwrap();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        // read/write commands: any valid token, from anywhere.
        auth.authorize_request(&headers, Some("get_app_config"), Some(lan)).unwrap();
        // admin command from a standard-scope LAN device → 403.
        assert!(matches!(
            auth.authorize_request(&headers, Some("index_album"), Some(lan)),
            Err(AuthError::Forbidden(_))
        ));
        // ...but from loopback any authenticated token may.
        auth.authorize_request(&headers, Some("index_album"), Some(loopback)).unwrap();
        // Names outside the whitelist → 403 (B/C/internal alike).
        assert!(matches!(
            auth.authorize_request(&headers, Some("set_desktop_wallpaper"), Some(lan)),
            Err(AuthError::Forbidden(_))
        ));
        assert!(matches!(
            auth.authorize_request(&headers, Some("totally_unknown"), Some(lan)),
            Err(AuthError::Forbidden(_))
        ));

        // A paired (admin-scope) device runs admin commands from the LAN.
        let code = auth.mint_pairing_code();
        let (admin_token, scope, _id) = auth.pair(&code, "desktop", "10.0.0.5").unwrap();
        assert_eq!(scope, SCOPE_ADMIN);
        let mut admin_headers = HeaderMap::new();
        admin_headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {admin_token}").parse().unwrap(),
        );
        auth.authorize_request(&admin_headers, Some("index_album"), Some(lan)).unwrap();

        // Token-only routes (events) need just a valid token.
        auth.authorize_request(&admin_headers, None, Some(lan)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn command_rate_limit_is_per_device() {
        let dir = scratch("cmd-rate");
        AuthService::set_admin_password(&dir, "pw").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        let (token_a, ..) = auth.login("pw", "a", "10.0.0.1").unwrap();
        let (token_b, ..) = auth.login("pw", "b", "10.0.0.2").unwrap();
        let headers_for = |t: &str| {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {t}").parse().unwrap(),
            );
            h
        };
        let lan: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        // Mutations keep the tight §3.6 budget.
        for _ in 0..CMD_LIMIT {
            auth.authorize_request(&headers_for(&token_a), Some("save_library_state"), Some(lan))
                .unwrap();
        }
        assert!(matches!(
            auth.authorize_request(&headers_for(&token_a), Some("save_library_state"), Some(lan)),
            Err(AuthError::RateLimited(_))
        ));
        // Device B is unaffected by A's window.
        auth.authorize_request(&headers_for(&token_b), Some("save_library_state"), Some(lan))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Browsing is read-heavy: a view switch costs dozens of reads, so the read
    /// budget must be far wider than the mutation one (and independent of it).
    #[tokio::test(start_paused = true)]
    async fn read_commands_get_a_wider_budget_than_mutations() {
        let dir = scratch("read-rate");
        AuthService::set_admin_password(&dir, "pw").unwrap();
        let auth = AuthService::open(&dir).unwrap();
        let (token, ..) = auth.login("pw", "dev", "10.0.0.1").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let lan: std::net::IpAddr = "10.0.0.1".parse().unwrap();

        // Reads sail past the mutation ceiling.
        for _ in 0..(CMD_LIMIT * 3) {
            auth.authorize_request(&headers, Some("get_query_files"), Some(lan))
                .unwrap();
        }
        // The mutation budget was not touched by those reads.
        for _ in 0..CMD_LIMIT {
            auth.authorize_request(&headers, Some("save_library_state"), Some(lan))
                .unwrap();
        }
        assert!(
            matches!(
                auth.authorize_request(&headers, Some("save_library_state"), Some(lan)),
                Err(AuthError::RateLimited(_))
            ),
            "mutations must still be limited while reads are flowing"
        );
        // And the read budget itself still ends in a 429 at READ_LIMIT.
        for _ in 0..(READ_LIMIT - CMD_LIMIT * 3) {
            auth.authorize_request(&headers, Some("get_query_files"), Some(lan))
                .unwrap();
        }
        assert!(matches!(
            auth.authorize_request(&headers, Some("get_query_files"), Some(lan)),
            Err(AuthError::RateLimited(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

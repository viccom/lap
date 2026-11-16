//! Static hosting of the Web client (phase 4, batch 4.1).
//!
//! The desktop shell embeds the same Vue build in a WebView; `lapd` serves it
//! over HTTP so a browser (or phone/PWA) gets the identical UI. Everything is
//! served from one origin as `/api/*`, which keeps the `lapd_token` Cookie and
//! EventSource alive without CORS (plan §3.3).
//!
//! Design choices:
//! - Hand-rolled instead of `tower_http::ServeDir`: no dependency/feature
//!   churn, and the SPA fallback + `/api` protection stay explicit.
//! - Path traversal is refused by canonicalizing the requested path and
//!   checking it stays under the configured web root.
//! - Unknown non-API paths fall back to `index.html` so Vue Router's
//!   `createWebHistory` deep links (`/image-viewer`, `/settings`, …) work.

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

/// Whether a Web build is actually being served, read by `/api/server_info`.
static WEB_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn web_enabled() -> bool {
    *WEB_ENABLED.get().unwrap_or(&false)
}

pub fn set_web_enabled(enabled: bool) {
    let _ = WEB_ENABLED.set(enabled);
}

/// Where the Web build comes from, resolved once at boot.
#[derive(Debug, Clone, PartialEq)]
pub struct WebRoot {
    root: PathBuf,
}

impl WebRoot {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve the web directory by priority: explicit `--web-dir` first,
    /// then `resources/web` beside the executable.
    pub fn resolve(explicit: Option<&Path>, exe_dir: Option<&Path>) -> Option<Self> {
        if let Some(dir) = explicit {
            if dir.is_dir() {
                return Some(Self::new(dir.to_path_buf()));
            }
            return None;
        }
        let exe = exe_dir?;
        let candidate = exe.join("resources").join("web");
        candidate.is_dir().then(|| Self::new(candidate))
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    fn without_check(root: PathBuf) -> Self {
        Self { root }
    }
}

/// Map a URI path to a file under the web root.
///
/// Returns `None` for paths that escape the root (traversal) or contain a
/// NUL — both must be refused rather than normalized away.
fn safe_path(root: &Path, uri_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode_path(uri_path)?;
    let rel = Path::new(&decoded);
    let mut out = root.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Minimal percent-decoding for the path segment only (no query string).
/// Refuses invalid UTF-8 sequences and NUL bytes.
fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1).copied().zip(bytes.get(i + 2).copied())?;
                let hi = (hex.0 as char).to_digit(16)?;
                let lo = (hex.1 as char).to_digit(16)?;
                let byte = (hi * 16 + lo) as u8;
                if byte == 0 {
                    return None;
                }
                out.push(byte);
                i += 3;
            }
            b'\0' => return None,
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn file_response(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_for(path))
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serve a static file, falling back to `index.html` for SPA deep links.
pub async fn serve(web: &WebRoot, uri: &Uri) -> Response {
    let path = uri.path();
    let requested = match safe_path(&web.root, path) {
        Some(p) => p,
        None => return StatusCode::FORBIDDEN.into_response(),
    };

    // A real file under the root wins; otherwise SPA fallback.
    if requested.is_file() {
        return file_response(&requested).await;
    }
    // Refuse paths that look like a file (have an extension) but are missing —
    // returning index.html for a missing .js/.css hides real build errors.
    let looks_like_file = requested
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.is_empty())
        .unwrap_or(false);
    if looks_like_file {
        return StatusCode::NOT_FOUND.into_response();
    }
    let index = web.root.join("index.html");
    if index.is_file() {
        return file_response(&index).await;
    }
    StatusCode::NOT_FOUND.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lapd-static-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn serves_index_and_spa_fallback() {
        let root = temp_root("spa");
        fs::write(root.join("index.html"), "<html>app</html>").unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("assets/app.js"), "console.log(1)").unwrap();
        let web = WebRoot::without_check(root.clone());

        let r = serve(&web, &"/".parse().unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get(header::CONTENT_TYPE).unwrap(), "text/html; charset=utf-8");

        let r = serve(&web, &"/image-viewer".parse().unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);

        let r = serve(&web, &"/assets/app.js".parse().unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get(header::CONTENT_TYPE).unwrap(), "text/javascript; charset=utf-8");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn traversal_and_missing_files_are_refused() {
        let root = temp_root("guard");
        fs::write(root.join("index.html"), "app").unwrap();
        let web = WebRoot::without_check(root.clone());

        // Encoded parent traversal must be refused, not normalized into the root.
        let r = serve(&web, &"/%2e%2e/%2e%2e/secret.txt".parse().unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // A missing asset must 404, not silently return the SPA shell.
        let r = serve(&web, &"/assets/missing.js".parse().unwrap()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn percent_decode_rejects_nul_and_bad_hex() {
        assert_eq!(percent_decode_path("/a%20b"), Some("/a b".to_string()));
        assert_eq!(percent_decode_path("/a%00b"), None);
        assert_eq!(percent_decode_path("/a%zz"), None);
    }
}

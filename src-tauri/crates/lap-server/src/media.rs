//! Media endpoints (plan §3.5): thumbnails, previews, originals, video —
//! resolved **only by DB file_id** (never a caller-supplied path), streamed
//! with Range/206 semantics ported from `t_http.rs`, and authenticated via
//! the §3.5 HMAC URLs (`?exp=&sig=&dev=`) minted by `POST /api/media/sign`.
//!
//! The thumb/preview handlers are a near-verbatim port of the desktop
//! `t_protocol.rs` scheme handlers — same query params, same generation
//! fallbacks, same X-Raw-* headers — minus the `Access-Control-Allow-Origin: *`
//! posture, which §3.5 retires for lapd.



use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::auth::{AuthError, AuthService};
use lap_core::t_raw_display::{RawDisplayOptions, RawPreviewMode};

// -- query helpers (ported from t_protocol.rs) -----------------------------

fn raw_display_options(query: Option<&str>) -> RawDisplayOptions {
    let mut options = RawDisplayOptions::default();
    for pair in query.unwrap_or_default().split('&') {
        match pair {
            "rawPreviewMode=embedded" => options.mode = RawPreviewMode::Embedded,
            "rawPreviewMode=rendered" => options.mode = RawPreviewMode::Rendered,
            "rawAutoBright=true" => options.auto_bright = true,
            "rawPairDisplay=jpeg" => options.prefer_pair = true,
            _ => {}
        }
    }
    options
}

fn query_u32(query: &str, key: &str) -> Option<u32> {
    query
        .split('&')
        .find_map(|p| p.strip_prefix(&format!("{key}=")).and_then(|v| v.parse::<u32>().ok()))
}

fn query_flag(query: &str, flag: &str) -> bool {
    query.split('&').any(|p| p == flag)
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|p| p.strip_prefix(&format!("{key}=")))
}

fn thumbnail_size(query: &str) -> u32 {
    query_u32(query, "size").unwrap_or(512).clamp(64, 2048)
}

fn pq_of(req: &Request) -> String {
    let path = req.uri().path();
    let query = req.uri().query().unwrap_or_default();
    let pq = format!("{path}?{query}");
    pq.trim_start_matches('/').to_string()
}

fn split_path(path: &str) -> Option<(String, i64)> {
    let mut segments = path.trim_start_matches('/').split('/');
    let library_id = segments.next()?.to_string();
    let file_id: i64 = segments.next()?.parse().ok()?;
    if library_id.is_empty() || file_id <= 0 {
        return None;
    }
    Some((library_id, file_id))
}

// -- response builders ------------------------------------------------------

fn media_bytes_response(data: Vec<u8>, cache_control: &'static str) -> Response {
    let content_type = lap_core::t_common::detect_image_mime(&data);
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (axum::http::header::CACHE_CONTROL, cache_control.to_string()),
        ],
        data,
    )
        .into_response()
}

fn text_response(status: StatusCode, body: &str) -> Response {
    (status, [(axum::http::header::CONTENT_TYPE, "text/plain")], body.to_string()).into_response()
}

// -- GET /api/thumb/{library_id}/{file_id} ----------------------------------

/// Cached-first, then generate synchronously inside a background permit —
/// the desktop thumb:// protocol semantics, so an HTTP response actually
/// contains bytes (or 404 while the background queue catches up).
pub async fn thumb_handler(
    State(auth): State<std::sync::Arc<AuthService>>,
    req: Request,
) -> Response {
    let pq = pq_of(&req);
    if let Err(e) = auth.verify_media_url(&pq) {
        return e.into_response();
    }
    let Some((library_id, file_id)) =
        split_path(req.uri().path().strip_prefix("/api/thumb/").unwrap_or(req.uri().path()))
    else {
        return text_response(StatusCode::BAD_REQUEST, "invalid file_id");
    };
    let query = req.uri().query().unwrap_or_default();
    let options = raw_display_options(Some(query));
    let thumbnail_size = thumbnail_size(query);
    let repair = query_flag(query, "repair=true");

    if !repair {
        let lookup_library = library_id.clone();
        let cached = lap_core::t_rt::spawn_blocking(move || {
            lap_core::t_config::with_current_library(&lookup_library, || {
                let file = lap_core::t_sqlite::AFile::get_file_info(file_id)?.ok_or("File not found")?;
                let path = file.file_path.as_deref().unwrap_or_default();
                if let Some(thumb) =
                    lap_core::t_sqlite::AThumb::get_thumb_if_available(file_id, path, false, file.album_accessible)?
                {
                    if let Some(data) = thumb.thumb_data {
                        return Ok(Some((data, file.file_type == Some(3))));
                    }
                }
                Ok(None)
            })
        })
        .await;
        if let Ok(Ok(Some((data, raw)))) = cached {
            return media_bytes_response(data, if raw { "no-cache" } else { "max-age=31536000, immutable" });
        }
    }

    let permits = lap_core::t_sqlite::thumb_background_generation_permits();
    let Ok(permit) = permits.acquire_owned().await else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue closed");
    };
    let result = lap_core::t_rt::spawn_blocking(move || {
        let _permit = permit;
        lap_core::t_config::with_current_library(&library_id, || {
            let file = lap_core::t_sqlite::AFile::get_file_info(file_id)?.ok_or("File not found")?;
            let previous = lap_core::t_sqlite::AThumb::fetch_record(file_id)?.and_then(|(thumb, _)| thumb.id);
            let thumb = lap_core::t_sqlite::AThumb::get_or_create_thumb(
                file_id,
                file.file_path.as_deref().unwrap_or_default(),
                file.file_type.unwrap_or(0),
                file.e_orientation.unwrap_or(1) as i32,
                thumbnail_size,
                options,
                repair && lap_core::t_sqlite::AThumb::auto_repair_allowed(&file, thumbnail_size, options),
                file.duration.map(|d| d as u64),
                None,
                repair,
            )?;
            if repair
                && thumb.as_ref().is_some_and(|thumb| {
                    thumb.error_code == 0 && thumb.thumb_data.is_some() && previous != thumb.id
                })
            {
                lap_core::t_events::emit(
                    lap_core::t_events::names::THUMBNAIL_READY,
                    serde_json::json!({ "library_id": library_id, "album_id": file.album_id,
                        "file_ids": [file_id], "invalidate": true, "metadata_changed": false }),
                );
            }
            if let Some(thumb) = thumb {
                if let Some(data) = thumb.thumb_data {
                    return Ok(Some((data, file.file_type == Some(3))));
                }
            }
            Ok(None)
        })
    })
    .await;
    match result {
        Ok(Ok(Some((data, raw)))) => {
            media_bytes_response(data, if raw { "no-cache" } else { "max-age=31536000, immutable" })
        }
        _ => text_response(StatusCode::NOT_FOUND, "thumbnail not found"),
    }
}

// -- GET /api/preview/{library_id}/{file_id} --------------------------------

pub async fn preview_handler(
    State(auth): State<std::sync::Arc<AuthService>>,
    req: Request,
) -> Response {
    let pq = pq_of(&req);
    if let Err(e) = auth.verify_media_url(&pq) {
        return e.into_response();
    }
    let Some((library_id, file_id)) =
        split_path(req.uri().path().strip_prefix("/api/preview/").unwrap_or(req.uri().path()))
    else {
        return text_response(StatusCode::BAD_REQUEST, "invalid file_id");
    };
    let query = req.uri().query().unwrap_or_default();
    let options = raw_display_options(Some(query));
    let cached_only = query_flag(query, "cachedOnly=true");
    let for_editing = query_flag(query, "forEditing=true");
    let raw_full_size = query_flag(query, "rawFullSize=true");
    let raw_request =
        query_value(query, "rawViewerId").map(lap_core::t_image::register_raw_preview_request);

    // Scheme callbacks never waited on locks; nor does this handler — the
    // file lookup runs on the blocking pool like the desktop protocol.
    let lookup_library = library_id;
    let lookup = lap_core::t_rt::spawn_blocking(move || {
        lap_core::t_config::with_current_library(&lookup_library, || {
            let file = lap_core::t_sqlite::AFile::get_file_info(file_id)?
                .ok_or_else(|| "File not found".to_string())?;
            let companion = if !options.prefer_pair {
                None
            } else if cached_only && file.media_subtype.as_deref() == Some("raw_jpeg_pair") {
                file.live_photo_video_id
                    .and_then(|id| lap_core::t_sqlite::AFile::get_file_info(id).ok().flatten())
            } else {
                lap_core::t_raw_display::paired_file(file_id, options)
            };
            Ok((file, companion))
        })
    })
    .await;
    let (file, companion) = match lookup {
        Ok(Ok(files)) => files,
        _ => return text_response(StatusCode::NOT_FOUND, "file not found"),
    };
    let is_raw = file.file_type == Some(3);
    let file_path = match file.file_path {
        Some(path) if !path.is_empty() => path,
        _ => return text_response(StatusCode::NOT_FOUND, "file path not found"),
    };
    let pair_label = companion
        .as_ref()
        .and_then(|file| file.file_path.as_ref())
        .map(|path| if lap_core::t_image::is_heic_path(path) { "HEIC" } else { "JPEG" })
        .unwrap_or("");

    if cached_only {
        let paired_cache = options
            .prefer_pair
            .then(|| {
                companion
                    .as_ref()
                    .and_then(|file| file.file_path.as_deref())
                    .and_then(|path| lap_core::t_image::get_cached_preview_bytes(path, options))
            })
            .flatten();
        let data = paired_cache.or_else(|| lap_core::t_image::get_cached_preview_bytes(&file_path, options));
        let mut response = match data {
            Some(data) => media_bytes_response(data, "max-age=31536000, immutable"),
            None => text_response(StatusCode::NOT_FOUND, "cached preview not found"),
        };
        response
            .headers_mut()
            .insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
        return response;
    }
    if for_editing {
        // Exactly the pixels get_edited_image crops — the desktop uses the
        // same call for the editor payload.
        return match lap_core::t_image::get_generated_preview_bytes(&file_path).await {
            Ok(Some(data)) => media_bytes_response(data, "max-age=31536000, immutable"),
            _ => text_response(StatusCode::NOT_FOUND, "editable preview not found"),
        };
    }
    if options.prefer_pair {
        if let Some(path) = companion.and_then(|file| file.file_path) {
            if let Ok(data) = lap_core::t_image::get_file_image_bytes_cached(&path, options).await {
                if image::load_from_memory(&data).is_ok() {
                    return raw_preview_response(data, "pair", false, pair_label);
                }
            }
        }
    }
    if is_raw {
        return match lap_core::t_image::get_raw_preview_cached(&file_path, options, raw_full_size, raw_request).await {
            Ok((data, source, unavailable)) => raw_preview_response(data, source, unavailable, pair_label),
            Err(_) => text_response(StatusCode::NOT_FOUND, "RAW preview not found"),
        };
    }
    match lap_core::t_image::get_file_image_bytes_cached(&file_path, options).await {
        Ok(data) => media_bytes_response(data, "max-age=31536000, immutable"),
        Err(_) => text_response(StatusCode::NOT_FOUND, "preview not found"),
    }
}

fn raw_preview_response(data: Vec<u8>, source: &'static str, unavailable: bool, pair: &'static str) -> Response {
    let mut response = media_bytes_response(data, "no-cache");
    let headers = response.headers_mut();
    headers.insert("X-Raw-Source", axum::http::HeaderValue::from_static(source));
    headers.insert(
        "X-Raw-Embedded-Unavailable",
        axum::http::HeaderValue::from_static(if unavailable { "true" } else { "false" }),
    );
    headers.insert("X-Raw-Pair", axum::http::HeaderValue::from_static(pair));
    response
}

// -- Range/206 streaming (GET /api/original/{file_id}, /api/video/{file_id}) --

#[derive(Debug, PartialEq)]
pub enum RangeSpec {
    Full,
    /// Inclusive byte range (start, end) as in `Content-Range: bytes a-b/size`.
    Partial(u64, u64),
    /// Suffix form `bytes=-N`: the last N bytes (clamped by the file size).
    Suffix(u64),
    /// Malformed or unsatisfiable → 416 with `Content-Range: bytes *​/{size}`.
    Invalid,
}

/// Port of `t_http::parse_http_range` (single range, first comma segment).
/// Purely syntactic — `stream_file` validates against the real file size.
pub fn parse_range(header: Option<&str>) -> RangeSpec {
    let Some(header) = header else { return RangeSpec::Full };
    let Some(spec) = header
        .split(',')
        .next()
        .and_then(|s| s.trim().strip_prefix("bytes="))
    else {
        return RangeSpec::Invalid;
    };
    let (start_str, end_str) = match spec.split_once('-') {
        Some(parts) => parts,
        None => return RangeSpec::Invalid,
    };
    if start_str.is_empty() {
        // suffix form: last N bytes
        let Ok(n) = end_str.trim().parse::<u64>() else { return RangeSpec::Invalid };
        if n == 0 {
            return RangeSpec::Invalid;
        }
        return RangeSpec::Suffix(n);
    }
    let Ok(start) = start_str.trim().parse::<u64>() else { return RangeSpec::Invalid };
    let end = match end_str.trim().parse::<u64>() {
        Ok(end) if end < start => return RangeSpec::Invalid,
        Ok(end) => end,
        // open-ended `bytes=N-`: the stream clamps to the real file size
        Err(_) if end_str.trim().is_empty() => u64::MAX,
        Err(_) => return RangeSpec::Invalid,
    };
    RangeSpec::Partial(start, end)
}

fn video_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

fn image_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        _ => "image/jpeg",
    }
}

async fn stream_file(path: &std::path::Path, range: Option<&str>, content_type: &'static str) -> Result<Response, std::io::Error> {
    let file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    // Validate the parsed range against the real size now.
    let spec = match parse_range(range) {
        RangeSpec::Full => Some(RangeSpec::Full),
        RangeSpec::Invalid => None,
        RangeSpec::Partial(start, end) if start < size => {
            Some(RangeSpec::Partial(start, end.min(size - 1)))
        }
        RangeSpec::Suffix(n) => {
            let start = size.saturating_sub(n);
            Some(RangeSpec::Partial(start, size.saturating_sub(1)))
        }
        _ => None,
    };
    let Some(spec) = spec else {
        let mut response = (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(axum::http::header::CONTENT_RANGE, format!("bytes */{size}"))],
        )
            .into_response();
        response
            .headers_mut()
            .insert(axum::http::header::ACCEPT_RANGES, axum::http::HeaderValue::from_static("bytes"));
        return Ok(response);
    };
    let (status, start, length) = match spec {
        RangeSpec::Full => (StatusCode::OK, 0, size),
        RangeSpec::Partial(start, end) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        RangeSpec::Invalid | RangeSpec::Suffix(_) => unreachable!("validated above"),
    };
    let mut file = file;
    if start > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    let file = tokio::io::AsyncReadExt::take(file, length);
    let stream = futures::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) | Err(_) => None,
            Ok(n) => Some((Ok::<Bytes, std::convert::Infallible>(Bytes::copy_from_slice(&buf[..n])), file)),
        }
    });
    let mut response = Body::from_stream(stream).into_response();
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static(content_type));
    headers.insert(axum::http::header::ACCEPT_RANGES, axum::http::HeaderValue::from_static("bytes"));
    headers.insert(axum::http::header::CONTENT_LENGTH, axum::http::HeaderValue::from(length));
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    if let RangeSpec::Partial(_, end) = spec {
        headers.insert(
            axum::http::header::CONTENT_RANGE,
            axum::http::HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
        );
    }
    Ok(response)
}

/// Shared body of /api/original and /api/video: resolve by file_id in the
/// current library, refuse cloud-only files (reading them would trigger a
/// cloud download), stream with Range.
async fn serve_original_bytes(
    auth: &AuthService,
    pq: &str,
    file_id: i64,
    range_header: Option<&str>,
    video: bool,
) -> Response {
    if let Err(e) = auth.verify_media_url(pq) {
        return e.into_response();
    }
    if file_id <= 0 {
        return text_response(StatusCode::BAD_REQUEST, "invalid file_id");
    }
    let resolved = lap_core::t_rt::spawn_blocking(move || {
        lap_core::t_sqlite::AFile::get_file_info(file_id)?
            .map(|file| file.file_path)
            .and_then(|path| path.filter(|p| !p.is_empty()))
            .ok_or_else(|| "file not found".to_string())
    })
    .await;
    let path = match resolved {
        Ok(Ok(path)) => path,
        _ => return text_response(StatusCode::NOT_FOUND, "file not found"),
    };
    if !lap_core::t_utils::content_readable(&path) {
        return text_response(StatusCode::NOT_FOUND, "file content is not locally available");
    }
    let content_type = if video { video_content_type(&path) } else { image_content_type(&path) };
    let path_buf = std::path::PathBuf::from(&path);
    match stream_file(&path_buf, range_header, content_type).await {
        Ok(response) => response,
        Err(_) => text_response(StatusCode::NOT_FOUND, "file not found"),
    }
}

pub async fn original_handler(
    State(auth): State<std::sync::Arc<AuthService>>,
    Path(file_id): Path<i64>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    let pq = pq_of(&req);
    let range = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    serve_original_bytes(&auth, &pq, file_id, range.as_deref(), false).await
}

pub async fn video_handler(
    State(auth): State<std::sync::Arc<AuthService>>,
    Path(file_id): Path<i64>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    let pq = pq_of(&req);
    let range = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    serve_original_bytes(&auth, &pq, file_id, range.as_deref(), true).await
}

/// POST /api/media/sign {urls: [...]} — mint §3.5 signatures for media URLs
/// the caller is already entitled to. Lives next to the auth endpoints and
/// requires a valid token; the signature binds the minting device.
pub async fn media_sign_handler(
    State(auth): State<std::sync::Arc<AuthService>>,
    headers: HeaderMap,
    body: String,
) -> Result<axum::Json<serde_json::Value>, AuthError> {
    let token = auth
        .authenticate(&headers)
        .ok_or(AuthError::Unauthorized("authentication required".into()))?;
    if body.trim().is_empty() {
        return Err(AuthError::BadRequest("request body must be a JSON object".into()));
    }
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| AuthError::BadRequest(format!("invalid JSON body: {e}")))?;
    let urls = parsed
        .get("urls")
        .and_then(|v| v.as_array())
        .ok_or(AuthError::BadRequest("missing array field `urls`".into()))?;
    if urls.len() > 50 {
        return Err(AuthError::BadRequest("at most 50 urls per request".into()));
    }
    let mut signed = Vec::with_capacity(urls.len());
    for url in urls {
        let url = url
            .as_str()
            .ok_or_else(|| AuthError::BadRequest("urls entries must be strings".into()))?;
        signed.push(auth.sign_media_url(url, token.id)?);
    }
    Ok(axum::Json(serde_json::json!({ "urls": signed })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_display_query_parses_like_the_desktop_protocol() {
        let options = raw_display_options(Some("v=42&rawPreviewMode=rendered&rawPairDisplay=jpeg"));
        assert_eq!(options.mode, RawPreviewMode::Rendered);
        assert!(options.prefer_pair);
        assert_eq!(raw_display_options(Some("rawPreviewMode=embedded")).mode, RawPreviewMode::Embedded);
        assert!(raw_display_options(Some("rawPreviewMode=embedded&rawAutoBright=true")).auto_bright);
        assert!(!raw_display_options(None).prefer_pair);
    }

    #[test]
    fn thumbnail_size_clamps_like_the_desktop_protocol() {
        assert_eq!(thumbnail_size(""), 512);
        assert_eq!(thumbnail_size("size=999999"), 2048);
        assert_eq!(thumbnail_size("size=1"), 64);
        assert_eq!(thumbnail_size("size=1024&v=3"), 1024);
    }

    #[test]
    fn range_parsing_covers_the_t_http_semantics() {
        assert_eq!(parse_range(None), RangeSpec::Full);
        assert_eq!(parse_range(Some("bytes=0-4")), RangeSpec::Partial(0, 4));
        assert_eq!(parse_range(Some("bytes=10-")), RangeSpec::Partial(10, u64::MAX));
        assert_eq!(parse_range(Some("bytes=-5")), RangeSpec::Suffix(5));
        // invalid forms
        assert_eq!(parse_range(Some("bytes=-0")), RangeSpec::Invalid);
        assert_eq!(parse_range(Some("bytes=5-2")), RangeSpec::Invalid);
        assert_eq!(parse_range(Some("chars=1-2")), RangeSpec::Invalid);
        assert_eq!(parse_range(Some("bytes=abc-")), RangeSpec::Invalid);
        // multi-range: only the first segment is honoured
        assert_eq!(parse_range(Some("bytes=0-1,5-9")), RangeSpec::Partial(0, 1));
    }

    #[tokio::test]
    async fn streaming_serves_full_partial_and_416() {
        let dir = std::env::temp_dir().join(format!(
            "lapd-media-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob.bin");
        let content: Vec<u8> = (0..100u8).collect();
        std::fs::write(&path, &content).unwrap();

        // Full file.
        let response = stream_file(&path, None, "application/octet-stream").await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert_eq!(body.as_ref(), content.as_slice());

        // Middle range.
        let response =
            stream_file(&path, Some("bytes=10-19"), "application/octet-stream").await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_RANGE],
            "bytes 10-19/100"
        );
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert_eq!(body.as_ref(), &content[10..20]);

        // Open-ended suffix.
        let response = stream_file(&path, Some("bytes=90-"), "application/octet-stream").await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert_eq!(body.as_ref(), &content[90..]);

        // Unsatisfiable start.
        let response =
            stream_file(&path, Some("bytes=999999-"), "application/octet-stream").await.unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_RANGE],
            "bytes */100"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

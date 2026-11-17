//! HTTP↔core command bridge glue. The dispatch table itself is generated
//! (`bridge_gen.rs`); this module owns the argument-extraction contract,
//! the error taxonomy behind the HTTP status mapping, and the panic safety
//! net. Contracts pinned by the phase-1 `http.js` skeleton:
//! raw JSON in / raw JSON out, non-2xx on command failure.

use serde::de::DeserializeOwned;

/// Everything that can go wrong on the bridge, each with its HTTP status.
#[derive(Debug)]
pub enum BridgeError {
    /// 404 — name is not in the generated whitelist (covers B/C/internal).
    UnknownCommand(String),
    /// 400 — body not parseable / not an object / argument missing or invalid.
    BadRequest(String),
    /// 500 — the core command returned `Err(String)`.
    Command(String),
    /// 500 — the command panicked (join error or caught unwind).
    Panic(String),
}

impl axum::response::IntoResponse for BridgeError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, message) = match self {
            BridgeError::UnknownCommand(name) => (StatusCode::NOT_FOUND, format!("unknown command: {name}")),
            BridgeError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            BridgeError::Command(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
            BridgeError::Panic(message) => (StatusCode::INTERNAL_SERVER_ERROR, format!("internal panic: {message}")),
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}

pub type CmdResult = Result<serde_json::Value, BridgeError>;

/// Top-level command parameters are matched camelCase first, then the raw
/// snake_case name — the Tauri IPC contract. Nested payloads are never
/// touched; they deserialize directly, exactly as over IPC.
fn lookup<'a>(body: &'a serde_json::Value, camel: &str) -> Option<&'a serde_json::Value> {
    let snake = camel_to_snake(camel);
    body.get(camel).or_else(|| body.get(snake))
}

fn camel_to_snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for ch in camel.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn arg<T: DeserializeOwned>(body: &serde_json::Value, camel: &str) -> Result<T, BridgeError> {
    match lookup(body, camel) {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| BridgeError::BadRequest(format!("invalid argument `{camel}`: {e}"))),
        None => Err(BridgeError::BadRequest(format!("missing argument `{camel}`"))),
    }
}

/// Optional parameters: absent key and explicit null both map to `None`.
pub fn arg_opt<T: DeserializeOwned>(body: &serde_json::Value, camel: &str) -> Result<Option<T>, BridgeError> {
    match lookup(body, camel) {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| BridgeError::BadRequest(format!("invalid argument `{camel}`: {e}"))),
        None => Ok(None),
    }
}

pub fn ok<T: serde::Serialize>(v: T) -> CmdResult {
    serde_json::to_value(v).map_err(|e| BridgeError::Command(format!("serialize response: {e}")))
}

/// Most commands fail with `String`, but a few (the cache-migration family)
/// carry a structured error enum. The wire contract only needs a message
/// for the `{"error": …}` body: strings pass through, anything else is
/// carried as its JSON rendering.
pub fn ok_res<T: serde::Serialize, E: serde::Serialize>(r: Result<T, E>) -> CmdResult {
    match r {
        Ok(v) => ok(v),
        Err(e) => {
            let message = serde_json::to_value(&e)
                .ok()
                .map(|v| match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "command failed".to_string());
            Err(BridgeError::Command(message))
        }
    }
}

fn join_error(e: tokio::task::JoinError) -> BridgeError {
    if e.is_panic() {
        let payload = e.into_panic();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unnamed panic".to_string()
        };
        BridgeError::Panic(message)
    } else {
        BridgeError::Panic(e.to_string())
    }
}

/// Run a sync command on THIS runtime's blocking pool — never on t_rt's.
/// Core sync bodies legitimately call `t_rt::block_on` (see t_image /
/// t_video); running them inside the t_rt worker pool would block its own
/// runtime. Blocking-pool threads of a *different* runtime are the
/// documented-legal host for `block_on`.
pub async fn spawn_sync<T: serde::Serialize + Send + 'static, E: serde::Serialize + Send + 'static>(
    f: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> CmdResult {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => ok_res(r),
        Err(e) => Err(join_error(e)),
    }
}

pub async fn spawn_sync_plain<T: serde::Serialize + Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> CmdResult {
    match tokio::task::spawn_blocking(f).await {
        Ok(v) => ok(v),
        Err(e) => Err(join_error(e)),
    }
}

/// Route entry: dispatch one command, converting a panic anywhere in the
/// future into a BridgeError instead of tearing down the connection.
pub async fn handle_command(name: &str, body: &serde_json::Value) -> CmdResult {
    let fut = crate::bridge_gen::dispatch(name, body);
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(fut)).await {
        Ok(result) => result,
        Err(panic) => {
            let message = if let Some(s) = panic.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else {
                "unnamed panic".to_string()
            };
            Err(BridgeError::Panic(message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct Nested {
        file_ids: Vec<i64>,
    }

    #[test]
    fn arg_matches_camel_then_snake() {
        let body = json!({ "folderPath": "a/b" });
        assert_eq!(arg::<String>(&body, "folderPath").unwrap(), "a/b");
        let body = json!({ "folder_path": "c/d" });
        assert_eq!(arg::<String>(&body, "folderPath").unwrap(), "c/d");
    }

    #[test]
    fn missing_required_argument_is_bad_request() {
        let err = arg::<String>(&json!({}), "albumId").unwrap_err();
        match err {
            BridgeError::BadRequest(message) => assert!(message.contains("missing argument `albumId`")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn invalid_argument_type_is_bad_request() {
        let err = arg::<i64>(&json!({ "albumId": "x" }), "albumId").unwrap_err();
        match err {
            BridgeError::BadRequest(message) => assert!(message.contains("invalid argument `albumId`")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn arg_opt_absent_and_null_are_none() {
        assert_eq!(arg_opt::<String>(&json!({}), "query").unwrap(), None);
        assert_eq!(arg_opt::<String>(&json!({ "query": null }), "query").unwrap(), None);
        assert_eq!(arg_opt::<String>(&json!({ "query": "q" }), "query").unwrap(), Some("q".into()));
    }

    #[test]
    fn nested_payloads_deserialize_untouched() {
        // A nested struct whose keys are camelCase must NOT be re-mapped —
        // only top-level command parameters are.
        let body = json!({ "params": { "fileIds": [1, 2] } });
        let nested: Nested = arg(&body, "params").unwrap();
        assert_eq!(nested.file_ids, vec![1, 2]);
    }

    #[test]
    fn unit_returns_serialize_to_null() {
        assert_eq!(ok(()).unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn command_errors_carry_the_core_message() {
        let err = ok_res::<String, String>(Err("boom".into())).unwrap_err();
        match err {
            BridgeError::Command(message) => assert_eq!(message, "boom"),
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn structured_error_enums_reach_the_wire_as_json() {
        #[derive(serde::Serialize)]
        enum CacheChangeError {
            Locked,
        }
        let err = ok_res::<u64, CacheChangeError>(Err(CacheChangeError::Locked)).unwrap_err();
        match err {
            BridgeError::Command(message) => assert!(message.contains("Locked"), "got: {message}"),
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn error_statuses_map_to_the_contract() {
        use axum::response::IntoResponse;
        let cases = [
            (BridgeError::UnknownCommand("x".into()), axum::http::StatusCode::NOT_FOUND),
            (BridgeError::BadRequest("y".into()), axum::http::StatusCode::BAD_REQUEST),
            (BridgeError::Command("z".into()), axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            (BridgeError::Panic("p".into()), axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (error, expected) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn sync_commands_run_off_thread_and_survive_block_on() {
        // The generated table runs every sync command through spawn_sync;
        // a nested t_rt::block_on must not deadlock. t_rt is only
        // initialized in tests via the core runtime holder, so call it
        // directly.
        let result = spawn_sync(move || -> Result<u32, String> {
            Ok(lappp_core::t_rt::block_on(async { 21 + 21 }))
        })
        .await
        .unwrap();
        assert_eq!(result, serde_json::json!(42));
    }

    #[tokio::test]
    async fn panicking_sync_command_becomes_a_panic_error() {
        let err = spawn_sync(move || -> Result<(), String> { panic!("boom in command") }).await.unwrap_err();
        assert!(matches!(err, BridgeError::Panic(_)), "got {err:?}");
    }
}

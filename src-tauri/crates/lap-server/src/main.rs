//! `lapd` — the standalone Lap server (serverization plan, phase 3).
//! Batch 3.2 skeleton: flags, lock file, shared boot chain, server_info.
//! The command bridge and SSE land in the following 3.2 commits.

mod boot;
mod flags;
mod lock;

use std::process::ExitCode;

use axum::Json;
use axum::routing::get;

/// The read-only capability descriptor §3.3 pins for the web transport.
/// Field set is deliberately minimal (plan §3.3, open question 6) and will
/// grow when the web client consumes it.
fn server_info_value() -> serde_json::Value {
    serde_json::json!({
        "name": "lapd",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "pathSeparator": std::path::MAIN_SEPARATOR,
        "capabilities": {
            "auth": false,
            "media": false,
        },
    })
}

async fn server_info() -> Json<serde_json::Value> {
    Json(server_info_value())
}

async fn serve(flags: flags::Flags) -> Result<(), String> {
    let boot = boot::run(&flags)?;

    let app = axum::Router::new().route("/api/server_info", get(server_info));
    let addr = format!("{}:{}", flags.host, flags.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;

    println!(
        "lapd {} listening on http://{addr} (data: {})",
        env!("CARGO_PKG_VERSION"),
        boot.data_dir.display()
    );
    // `boot` (and its lock file guard) must outlive the server loop.
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("server error: {e}"))?;
    drop(boot);
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", flags::USAGE);
        return ExitCode::SUCCESS;
    }
    let flags = match flags::parse(argv) {
        Ok(flags) => flags,
        Err(e) => {
            eprintln!("lapd: {e}\n\n{}", flags::USAGE);
            return ExitCode::from(2);
        }
    };
    match serve(flags).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lapd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_shape_is_minimal_and_honest() {
        let info = server_info_value();
        assert_eq!(info["name"], "lapd");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["platform"], std::env::consts::OS);
        assert!(info["capabilities"]["auth"].is_boolean());
        assert!(info["capabilities"]["media"].is_boolean());
    }
}

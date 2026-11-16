//! lapd CLI flags (serverization plan §3.8): `--host`, `--port`,
//! `--data-dir`, `--library`. Hand-rolled parsing — four flags are not worth
//! a new dependency.

use std::path::PathBuf;

pub const USAGE: &str = "\
lapd — the standalone Lap server

USAGE:
    lapd [FLAGS]

FLAGS:
    --host <ADDR>      Bind address (default: 127.0.0.1 — loopback only)
    --port <PORT>      TCP port (default: 17860)
    --data-dir <DIR>   Override the app data directory (config, libraries,
                       lock file). Used verbatim; no `.debug` suffix.
    --library <ID>     Serve this library instead of the configured one
    --web-dir <DIR>    Serve the Web client build from this directory
                       (default: resources/web beside the executable)
    --set-admin-password <PW>
                       Set the admin password (Argon2id) and exit; refuses
                       if a password already exists
    -h, --help         Print this help";

#[derive(Debug, Clone, PartialEq)]
pub struct Flags {
    /// Bind address (default 127.0.0.1 — loopback unless explicitly widened).
    pub host: String,
    /// TCP port (default 17860).
    pub port: u16,
    /// Override the app data directory (config + libraries + lock file).
    pub data_dir: Option<PathBuf>,
    /// Switch to this library at boot instead of the configured current one.
    pub library: Option<String>,
    /// Serve the Web client build from this directory (phase 4).
    pub web_dir: Option<PathBuf>,
    /// Set the admin password (Argon2id) and exit.
    pub set_admin_password: Option<String>,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 17860,
            data_dir: None,
            library: None,
            web_dir: None,
            set_admin_password: None,
        }
    }
}

/// Parse `--flag value` and `--flag=value` forms; unknown flags are errors.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (arg, None),
        };
        match name.as_str() {
            "--host" => flags.host = take_value(&name, inline, &mut iter)?,
            "--port" => {
                let raw = take_value(&name, inline, &mut iter)?;
                flags.port = raw
                    .parse()
                    .map_err(|_| format!("--port: not a valid port number: {raw}"))?;
            }
            "--data-dir" => flags.data_dir = Some(PathBuf::from(take_value(&name, inline, &mut iter)?)),
            "--library" => flags.library = Some(take_value(&name, inline, &mut iter)?),
            "--web-dir" => flags.web_dir = Some(PathBuf::from(take_value(&name, inline, &mut iter)?)),
            "--set-admin-password" => {
                flags.set_admin_password = Some(take_value(&name, inline, &mut iter)?)
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok(flags)
}

fn take_value(
    name: &str,
    inline: Option<String>,
    iter: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    if let Some(v) = inline {
        return Ok(v);
    }
    iter.next().ok_or_else(|| format!("{name} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl IntoIterator<Item = String> {
        list.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    }

    #[test]
    fn defaults_when_no_flags() {
        assert_eq!(parse(args(&[])).unwrap(), Flags::default());
    }

    #[test]
    fn space_and_inline_forms_agree() {
        let spaced = parse(args(&["--port", "18000", "--library", "lib-1"])).unwrap();
        let inline = parse(args(&["--port=18000", "--library=lib-1"])).unwrap();
        assert_eq!(spaced, inline);
        assert_eq!(spaced.port, 18000);
        assert_eq!(spaced.library.as_deref(), Some("lib-1"));
    }

    #[test]
    fn host_and_data_dir_are_taken_verbatim() {
        let f = parse(args(&["--host", "0.0.0.0", "--data-dir", "D:\\tmp\\lapdata"])).unwrap();
        assert_eq!(f.host, "0.0.0.0");
        assert_eq!(f.data_dir.as_deref(), Some(std::path::Path::new("D:\\tmp\\lapdata")));
    }

    #[test]
    fn web_dir_is_taken_verbatim_in_both_forms() {
        let spaced = parse(args(&["--web-dir", "D:\\web build"])).unwrap();
        let inline = parse(args(&["--web-dir=D:\\web build"])).unwrap();
        assert_eq!(spaced, inline);
        assert_eq!(spaced.web_dir.as_deref(), Some(std::path::Path::new("D:\\web build")));
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(parse(args(&["--port"])).is_err());
        assert!(parse(args(&["--library"])).is_err());
    }

    #[test]
    fn bad_port_and_unknown_flag_are_errors() {
        assert!(parse(args(&["--port", "notaport"])).is_err());
        assert!(parse(args(&["--verbose"])).is_err());
    }
}

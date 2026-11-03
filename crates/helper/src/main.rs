//! `sentinel-helper`: the remote side of Sentinel's persistent sessions.
//!
//! Subcommands:
//! - `version`        print the helper version (compared by the client before deploy)
//! - `install-check`  report whether this host can run the helper
//! - `proxy [socket]` bridge stdio to the helper socket (used over an SSH exec channel)
//! - `serve`          run the daemon
//!
//! Exit codes:
//! - `0` success
//! - `1` runtime failure (socket, PTY, protocol)
//! - `2` usage error (unknown subcommand)
//! - `3` `install-check` found blockers
//! - `4` another `serve` already holds the single-instance lock

mod frame;
mod paths;
mod probe;
mod proxy;
mod registry;
mod ring;
mod server;
mod session;
mod wire;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("sentinel-helper: {err:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> anyhow::Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | None => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some("install-check") => {
            let report = probe::check()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(if report.ok { 0 } else { 3 })
        }
        Some("proxy") => {
            let socket = match args.get(1) {
                Some(path) => std::path::PathBuf::from(path),
                None => paths::socket_path(),
            };
            proxy::run(&socket)?;
            Ok(0)
        }
        Some("serve") => match server::run() {
            Ok(()) => Ok(0),
            Err(server::ServeError::AlreadyRunning(path)) => {
                eprintln!(
                    "sentinel-helper: another helper is already running ({})",
                    path.display()
                );
                Ok(4)
            }
            Err(server::ServeError::Other(err)) => Err(err),
        },
        Some(other) => {
            eprintln!("unknown subcommand: {other}");
            Ok(2)
        }
    }
}

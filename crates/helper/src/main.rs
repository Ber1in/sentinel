//! `sentinel-helper`: the remote side of Sentinel's persistent sessions.
//!
//! Subcommands:
//! - `version`        print the helper version
//! - `install-check`  report whether this host can run the helper
//! - `proxy`          bridge stdio to the helper socket (used over an SSH exec channel)
//! - `serve`          run the daemon (implemented in a later task)

mod frame;
mod probe;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | None => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("install-check") => {
            let report = probe::check()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            std::process::exit(if report.ok { 0 } else { 3 });
        }
        Some("proxy") | Some("serve") => {
            eprintln!("not implemented yet");
            std::process::exit(4);
        }
        Some(other) => {
            eprintln!("unknown subcommand: {other}");
            std::process::exit(2);
        }
    }
}

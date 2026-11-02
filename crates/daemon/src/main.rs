#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use daemon::config::{config_path, Config};
#[cfg(windows)]
use daemon::credentials::KeyringStore;
use daemon::credentials::{MemoryStore, SecretStore};
use daemon::{DaemonPaths, DaemonServer, Manager, SingleInstance, RESTART_EXIT_CODE};
use ssh::KnownHosts;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    if std::env::args().any(|argument| argument == "--supervise") {
        return supervise();
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_daemon())
}

/// Spawn the daemon and restart it when it crashes or asks for a restart.
///
/// Exit codes: 0 and 2 mean "stop" (clean shutdown or another instance is running);
/// [`RESTART_EXIT_CODE`] means "restart immediately"; anything else is a crash and is
/// retried with backoff.
fn supervise() -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    paths.ensure()?;

    let _guard = match SingleInstance::acquire(&paths.supervisor_lock())? {
        Some(guard) => guard,
        None => {
            tracing::info!("supervisor already running");
            return Ok(());
        }
    };

    let _ = std::fs::write(
        paths.supervisor_file(),
        format!("{{\"pid\":{}}}", std::process::id()),
    );

    let executable = std::env::current_exe()?;
    let mut backoff = Duration::from_secs(1);
    loop {
        let status: ExitStatus = Command::new(&executable).status()?;
        match status.code() {
            Some(0) | Some(2) => {
                tracing::info!(?status, "daemon stopped; supervisor exiting");
                let _ = std::fs::remove_file(paths.supervisor_file());
                return Ok(());
            }
            Some(code) if code == RESTART_EXIT_CODE => {
                tracing::info!("daemon requested a restart");
                backoff = Duration::from_secs(1);
                continue;
            }
            _ => {
                tracing::warn!(?status, backoff = ?backoff, "daemon exited unexpectedly");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(10));
            }
        }
    }
}

async fn run_daemon() -> Result<()> {
    let paths = DaemonPaths::resolve()?;
    paths.ensure()?;

    let _instance = match SingleInstance::acquire(&paths.lock_file())? {
        Some(instance) => instance,
        None => {
            eprintln!("another daemon instance is already running");
            std::process::exit(2);
        }
    };

    let config = match Config::load(&config_path()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("fix {} and restart", config_path().display());
            std::process::exit(3);
        }
    };

    let identity = daemon::client_id::load_or_create(paths.root())?;
    tracing::info!(client = %identity.short(), "client identity");

    let known_hosts = KnownHosts::at(paths.root().join("known_hosts"))?;

    #[cfg(windows)]
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new("sentinel"));
    #[cfg(not(windows))]
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());

    let store = match store::Store::open(&paths.state_db())? {
        store::Opened::Clean(store) => store,
        store::Opened::Recovered {
            store,
            backup,
            reason,
        } => {
            tracing::warn!(backup = %backup.display(), %reason, "state database was corrupt; recreated");
            store
        }
    };

    let manager = Manager::new(identity.short(), secrets, config, known_hosts, store);

    #[cfg(windows)]
    ensure_autostart();

    let server = DaemonServer::start(&paths, manager).await?;
    println!("{}", server.url());
    tracing::info!(url = %server.url(), pid = std::process::id(), "daemon listening");

    tokio::signal::ctrl_c().await?;
    server.abort();
    let _ = std::fs::remove_file(paths.daemon_file());
    Ok(())
}

/// Register `<exe> --supervise` in HKCU Run so the daemon (and its supervisor) survive a
/// login without the GUI being open. Idempotent: re-running rewrites the same value.
#[cfg(windows)]
fn ensure_autostart() {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let command = format!("\"{}\" --supervise", executable.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        winreg::enums::KEY_SET_VALUE,
    ) {
        if let Err(error) = key.set_value("SentinelDaemon", &command) {
            tracing::warn!(%error, "cannot register autostart");
        }
    }
}

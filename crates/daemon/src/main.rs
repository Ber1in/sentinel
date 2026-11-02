use std::sync::Arc;

use anyhow::Result;
use daemon::config::{config_path, Config};
#[cfg(windows)]
use daemon::credentials::KeyringStore;
use daemon::credentials::{MemoryStore, SecretStore};
use daemon::{DaemonPaths, DaemonServer, Manager, SingleInstance};
use ssh::KnownHosts;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .with_writer(std::io::stderr)
        .init();

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

    let server = DaemonServer::start(&paths, manager).await?;
    println!("{}", server.url());
    tracing::info!(url = %server.url(), pid = std::process::id(), "daemon listening");

    tokio::signal::ctrl_c().await?;
    server.abort();
    let _ = std::fs::remove_file(paths.daemon_file());
    Ok(())
}

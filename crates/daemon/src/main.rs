use anyhow::Result;
use daemon::{DaemonPaths, DaemonServer, SingleInstance};
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

    let server = DaemonServer::start(&paths).await?;
    println!("{}", server.url());
    tracing::info!(url = %server.url(), pid = std::process::id(), "daemon listening");

    tokio::signal::ctrl_c().await?;
    server.abort();
    let _ = std::fs::remove_file(paths.daemon_file());
    Ok(())
}

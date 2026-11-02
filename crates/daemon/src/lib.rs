//! Daemon: owns SSH connections, remote tmux sessions, scrollback, and layout state.

pub mod instance;
pub mod paths;
pub mod server;
pub mod token;

pub use instance::SingleInstance;
pub use paths::DaemonPaths;
pub use server::DaemonServer;

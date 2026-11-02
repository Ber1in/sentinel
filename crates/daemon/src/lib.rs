//! Daemon: owns SSH connections, remote tmux sessions, scrollback, and layout state.

pub mod config;
pub mod credentials;
pub mod fsm;
pub mod input;
pub mod instance;
pub mod manager;
pub mod paths;
pub mod runner;
pub mod server;
pub mod token;

pub use config::Config;
pub use credentials::{credential_id, MemoryStore, SecretStore};
pub use fsm::{Event, ReconnectPolicy, SessionFsm};
pub use input::InputQueue;
pub use instance::SingleInstance;
pub use manager::{Manager, RoutedChunk, StateChange};
pub use paths::DaemonPaths;
pub use runner::{Command, SessionEvents, SessionRuntime, SessionSpec};
pub use server::DaemonServer;

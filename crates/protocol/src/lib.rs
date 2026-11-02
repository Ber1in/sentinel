//! Wire protocol types shared by the daemon and the desktop GUI.
//!
//! This crate is the single source of truth: TypeScript bindings under
//! `apps/desktop/src/shared/` are generated from these types by `scripts/gen-protocol.sh`
//! and must never be edited by hand.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Bumped whenever a wire-incompatible change is made.
pub const PROTOCOL_VERSION: u32 = 1;

/// Handshake payload returned by the daemon after a successful connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub pid: u32,
}

/// Lifecycle of a single terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    Connecting,
    Connected,
    Reconnecting,
    Detached,
    Lost,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_stable() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn server_info_roundtrips_through_json() {
        let info = ServerInfo {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            pid: 42,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<ServerInfo>(&json).unwrap(), info);
    }

    #[test]
    fn session_state_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&SessionState::Reconnecting).unwrap(),
            "\"reconnecting\""
        );
    }
}

//! Error types and the mapping to wire-level [`ErrorCode`]s.
//!
//! The GUI branches on codes, never on message text, so this mapping is the single
//! place that decides whether a failure is retryable.

use protocol::ErrorCode;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("host key mismatch for {host}: known {expected}, presented {actual}")]
    HostKeyMismatch {
        host: String,
        expected: String,
        actual: String,
    },
    #[error("network error: {0}")]
    Network(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("channel error: {0}")]
    Channel(String),
    #[error("agent error: {0}")]
    Agent(String),
    #[error("key error: {0}")]
    Key(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl SshError {
    /// Wire-level classification consumed by the reconnect state machine.
    pub fn code(&self) -> ErrorCode {
        match self {
            SshError::Auth(_) => ErrorCode::AuthFailed,
            SshError::HostKeyMismatch { .. } => ErrorCode::HostKeyMismatch,
            SshError::Network(_) => ErrorCode::Network,
            SshError::Timeout(_) => ErrorCode::Timeout,
            SshError::Channel(_) => ErrorCode::Network,
            SshError::Agent(_) | SshError::Key(_) => ErrorCode::AuthFailed,
            SshError::Internal(_) => ErrorCode::Internal,
        }
    }

    /// Only transport failures are worth retrying; credentials and host keys are not.
    pub fn retryable(&self) -> bool {
        matches!(self.code(), ErrorCode::Network | ErrorCode::Timeout)
    }
}

impl From<russh::Error> for SshError {
    fn from(error: russh::Error) -> Self {
        match error {
            russh::Error::IO(error) => {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    SshError::Timeout(error.to_string())
                } else {
                    SshError::Network(error.to_string())
                }
            }
            russh::Error::ConnectionTimeout | russh::Error::KeepaliveTimeout => {
                SshError::Timeout(error.to_string())
            }
            russh::Error::Disconnect | russh::Error::HUP => SshError::Network(error.to_string()),
            russh::Error::KeyChanged { .. } => SshError::HostKeyMismatch {
                host: "server".into(),
                expected: "recorded key".into(),
                actual: "changed key".into(),
            },
            russh::Error::CouldNotReadKey => SshError::Key(error.to_string()),
            other => SshError::Internal(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_not_retryable() {
        let error = SshError::Auth("bad password".into());
        assert_eq!(error.code(), ErrorCode::AuthFailed);
        assert!(!error.retryable());
    }

    #[test]
    fn host_key_mismatch_is_not_retryable() {
        let error = SshError::HostKeyMismatch {
            host: "h".into(),
            expected: "SHA256:a".into(),
            actual: "SHA256:b".into(),
        };
        assert_eq!(error.code(), ErrorCode::HostKeyMismatch);
        assert!(!error.retryable());
    }

    #[test]
    fn transport_errors_are_retryable() {
        let io = russh::Error::IO(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert_eq!(SshError::from(io).code(), ErrorCode::Network);
        assert!(SshError::from(russh::Error::HUP).retryable());
        assert!(SshError::from(russh::Error::ConnectionTimeout).retryable());
        assert!(SshError::from(russh::Error::IO(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "slow"
        )))
        .retryable());
    }

    #[test]
    fn unreadable_key_is_an_auth_failure() {
        let error = SshError::from(russh::Error::CouldNotReadKey);
        assert_eq!(error.code(), ErrorCode::AuthFailed);
        assert!(!error.retryable());
    }

    #[test]
    fn protocol_errors_are_internal() {
        let error = SshError::from(russh::Error::Kex);
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(!error.retryable());
    }
}

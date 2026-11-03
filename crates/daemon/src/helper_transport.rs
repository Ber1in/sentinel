//! Helper-backed pane transport.
//!
//! This is the frame-channel counterpart of the existing exec-based path: it owns one
//! `HelperClient` bound to a single client id and exposes exactly the operations the
//! session runner needs. Wiring it into `SessionRuntime` is a separate, behaviour-
//! changing step; keeping it standalone lets both paths be tested independently.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use persistence::helper_client::{DataChunk, HelperClient, HelperClientError};
use protocol::ProviderKind;

/// Typed facade over [`HelperClient`] for one client identity.
pub struct HelperTransport {
    client: HelperClient,
    client_id: String,
    version: Option<String>,
}

impl HelperTransport {
    /// Wrap an already-connected socket (tests, and later the SSH `proxy` bridge).
    pub fn from_stream(stream: UnixStream, client_id: impl Into<String>) -> Self {
        Self {
            client: HelperClient::connect(stream),
            client_id: client_id.into(),
            version: None,
        }
    }

    pub fn kind(&self) -> ProviderKind {
        ProviderKind::Helper
    }

    /// Handshake; records the helper version for the compatibility check.
    pub fn hello(&mut self) -> Result<&str, HelperClientError> {
        let version = self.client.request(
            &helper_proto::Message::Hello {
                client_id: self.client_id.clone(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            |_| {},
        )?;
        self.version = version.clone();
        Ok(self.version.as_deref().unwrap_or("unknown"))
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn create(
        &mut self,
        session_id: &str,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<(), HelperClientError> {
        self.client.request(
            &helper_proto::Message::Create {
                client_id: self.client_id.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.map(str::to_string),
                cols,
                rows,
            },
            |_| {},
        )?;
        Ok(())
    }

    /// Attach and return everything the helper replayed before acknowledging.
    pub fn attach(&mut self, session_id: &str) -> Result<Vec<DataChunk>, HelperClientError> {
        let mut replay = Vec::new();
        self.client.request(
            &helper_proto::Message::Attach {
                client_id: self.client_id.clone(),
                session_id: session_id.to_string(),
            },
            |chunk| replay.push(chunk),
        )?;
        Ok(replay)
    }

    pub fn write(
        &mut self,
        session_id: &str,
        seq: u64,
        bytes: &[u8],
    ) -> Result<(), HelperClientError> {
        self.client.send_data(session_id, seq, bytes)
    }

    pub fn resize(
        &mut self,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), HelperClientError> {
        self.client.request(
            &helper_proto::Message::Resize {
                client_id: self.client_id.clone(),
                session_id: session_id.to_string(),
                cols,
                rows,
            },
            |_| {},
        )?;
        Ok(())
    }

    pub fn detach(&mut self, session_id: &str) -> Result<(), HelperClientError> {
        self.client.request(
            &helper_proto::Message::Detach {
                client_id: self.client_id.clone(),
                session_id: session_id.to_string(),
            },
            |_| {},
        )?;
        Ok(())
    }

    pub fn destroy(&mut self, session_id: &str) -> Result<(), HelperClientError> {
        self.client.request(
            &helper_proto::Message::Destroy {
                client_id: self.client_id.clone(),
                session_id: session_id.to_string(),
            },
            |_| {},
        )?;
        Ok(())
    }

    /// Poll for terminal output produced by the session.
    pub fn poll(&mut self, timeout: Duration) -> Result<Vec<DataChunk>, HelperClientError> {
        self.client.drain(timeout)
    }

    pub fn list(&mut self) -> Result<Vec<String>, HelperClientError> {
        let detail = self.client.request(
            &helper_proto::Message::List {
                client_id: self.client_id.clone(),
            },
            |_| {},
        )?;
        let Some(json) = detail else {
            return Ok(Vec::new());
        };
        let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        let entries = value
            .as_array()
            .cloned()
            .or_else(|| value.get("sessions").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        Ok(entries
            .into_iter()
            .filter_map(|entry| {
                entry
                    .get("id")
                    .or_else(|| entry.get("session_id"))
                    .and_then(|id| id.as_str())
                    .map(str::to_string)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    struct Fixture {
        _dir: tempfile::TempDir,
        child: std::process::Child,
        socket: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn start_helper() -> Option<Fixture> {
        let binary =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/sentinel-helper");
        if !binary.exists() {
            eprintln!("skipped: build `cargo build -p helper` first");
            return None;
        }
        let dir = tempfile::tempdir().ok()?;
        let socket = dir.path().join("helper.sock");
        let child = std::process::Command::new(&binary)
            .arg("serve")
            .env("SENTINEL_HELPER_SOCKET", &socket)
            .env("SENTINEL_HELPER_LOCK", dir.path().join("helper.lock"))
            .env("SENTINEL_HELPER_IDLE_SECS", "120")
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if socket.exists() && UnixStream::connect(&socket).is_ok() {
                return Some(Fixture {
                    _dir: dir,
                    child,
                    socket,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn full_session_lifecycle_over_the_frame_transport() {
        let Some(fixture) = start_helper() else {
            return;
        };
        let stream = UnixStream::connect(&fixture.socket).unwrap();
        let mut transport = HelperTransport::from_stream(stream, "client-a");

        assert_eq!(transport.kind(), ProviderKind::Helper);
        let version = transport.hello().unwrap_or("unknown").to_string();
        assert!(!version.is_empty());

        transport.create("s1", Some("/tmp"), 100, 30).unwrap();
        assert_eq!(transport.list().unwrap(), vec!["s1".to_string()]);

        let replay = transport.attach("s1").unwrap();
        assert!(replay.iter().all(|chunk| chunk.session_id == "s1"));

        transport
            .write("s1", 0, b"echo transport-marker\n")
            .unwrap();
        let mut output = String::new();
        for _ in 0..20 {
            for chunk in transport.poll(Duration::from_millis(200)).unwrap() {
                output.push_str(&String::from_utf8_lossy(&chunk.bytes));
            }
            if output.contains("transport-marker") {
                break;
            }
        }
        assert!(output.contains("transport-marker"), "output: {output:?}");

        transport.resize("s1", 120, 40).unwrap();
        transport.detach("s1").unwrap();
        transport.destroy("s1").unwrap();
        assert!(transport.list().unwrap().is_empty());
    }
}

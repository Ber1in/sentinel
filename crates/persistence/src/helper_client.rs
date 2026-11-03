//! Frame client for the remote `sentinel-helper`.
//!
//! The helper answers `attach` with the replayed screen *before* the reply, so a caller
//! cannot simply write a request and wait for the next frame. This client therefore
//! drains frames and routes them: replies go to the caller, terminal data to the sink,
//! events to a queue.
//!
//! One constraint comes from the helper itself: it correlates replies by order, so only
//! one request may be in flight at a time.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use helper_proto::{decode, encode, frame_len, Message};

pub use helper_proto::{FrameError, Message as HelperMessage};

#[derive(Debug, thiserror::Error)]
pub enum HelperClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing error: {0}")]
    Frame(#[from] FrameError),
    #[error("helper rejected the request: {0}")]
    Rejected(String),
    #[error("helper closed the connection")]
    Closed,
}

/// Terminal output received from the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataChunk {
    pub session_id: String,
    pub seq: u64,
    pub bytes: Vec<u8>,
}

pub struct HelperClient {
    stream: UnixStream,
    buffer: Vec<u8>,
    events: VecDeque<Message>,
    in_flight: bool,
}

impl HelperClient {
    pub fn connect(stream: UnixStream) -> Self {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        Self {
            stream,
            buffer: Vec::new(),
            events: VecDeque::new(),
            in_flight: false,
        }
    }

    /// Send one request and read frames until its reply arrives. Terminal data seen on
    /// the way is handed to `sink`, which is how replay frames are delivered.
    pub fn request(
        &mut self,
        message: &Message,
        mut sink: impl FnMut(DataChunk),
    ) -> Result<Option<String>, HelperClientError> {
        if self.in_flight {
            return Err(HelperClientError::Rejected(
                "another request is still in flight".into(),
            ));
        }
        self.in_flight = true;
        self.stream.write_all(&encode(message, b""))?;
        self.stream.flush()?;

        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let result = loop {
            // The socket has a short read timeout so `drain` can poll; here we keep
            // waiting until the reply arrives or the deadline passes.
            let (frame, body) = match self.next_frame() {
                Ok(frame) => frame,
                Err(HelperClientError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if std::time::Instant::now() > deadline {
                        break Err(HelperClientError::Io(error));
                    }
                    continue;
                }
                Err(error) => break Err(error),
            };

            match frame {
                Message::Reply { ok, detail, .. } => {
                    break if ok {
                        Ok(detail)
                    } else {
                        Err(HelperClientError::Rejected(detail.unwrap_or_default()))
                    };
                }
                Message::Data {
                    session_id, seq, ..
                } => {
                    sink(DataChunk {
                        session_id,
                        seq,
                        bytes: body,
                    });
                }
                other => {
                    self.events.push_back(other);
                }
            }
        };
        self.in_flight = false;
        result
    }

    /// Send terminal input; the helper does not acknowledge successful writes.
    pub fn send_data(
        &mut self,
        session_id: &str,
        seq: u64,
        bytes: &[u8],
    ) -> Result<(), HelperClientError> {
        let message = Message::Data {
            session_id: session_id.to_string(),
            seq,
            len: bytes.len() as u32,
        };
        self.stream.write_all(&encode(&message, bytes))?;
        self.stream.flush()?;
        Ok(())
    }

    /// Drain any frames that arrive within `timeout`, collecting terminal output.
    pub fn drain(&mut self, timeout: Duration) -> Result<Vec<DataChunk>, HelperClientError> {
        let deadline = std::time::Instant::now() + timeout;
        let mut chunks = Vec::new();
        while std::time::Instant::now() < deadline {
            match self.next_frame() {
                Ok((
                    Message::Data {
                        session_id, seq, ..
                    },
                    body,
                )) => chunks.push(DataChunk {
                    session_id,
                    seq,
                    bytes: body,
                }),
                Ok((other, _)) => self.events.push_back(other),
                Err(HelperClientError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(HelperClientError::Closed) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(chunks)
    }

    pub fn take_events(&mut self) -> Vec<Message> {
        self.events.drain(..).collect()
    }

    fn next_frame(&mut self) -> Result<(Message, Vec<u8>), HelperClientError> {
        loop {
            if let Some(len) = frame_len(&self.buffer) {
                if self.buffer.len() >= len {
                    let frame: Vec<u8> = self.buffer.drain(..len).collect();
                    return Ok(decode(&frame)?);
                }
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(HelperClientError::Closed),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error) => return Err(HelperClientError::Io(error)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    struct Fixture {
        dir: tempfile::TempDir,
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
                return Some(Fixture { dir, child, socket });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("skipped: helper did not create its socket");
        None
    }

    #[test]
    fn create_attach_send_and_destroy_a_session() {
        let Some(fixture) = start_helper() else {
            return;
        };
        let stream = UnixStream::connect(&fixture.socket).unwrap();
        let mut client = HelperClient::connect(stream);

        let version = client
            .request(
                &Message::Hello {
                    client_id: "c1".into(),
                    version: "0.1.0".into(),
                },
                |_| {},
            )
            .unwrap();
        assert!(version.is_some(), "hello should report the helper version");

        client
            .request(
                &Message::Create {
                    client_id: "c1".into(),
                    session_id: "s1".into(),
                    cwd: Some("/tmp".into()),
                    cols: 80,
                    rows: 24,
                },
                |_| {},
            )
            .unwrap();

        let mut replay = String::new();
        client
            .request(
                &Message::Attach {
                    client_id: "c1".into(),
                    session_id: "s1".into(),
                },
                |chunk| replay.push_str(&String::from_utf8_lossy(&chunk.bytes)),
            )
            .unwrap();
        assert!(replay.len() < 100_000);

        client.send_data("s1", 0, b"echo helper-marker\n").unwrap();
        let mut output = String::new();
        for _ in 0..20 {
            for chunk in client.drain(Duration::from_millis(200)).unwrap() {
                output.push_str(&String::from_utf8_lossy(&chunk.bytes));
            }
            if output.contains("helper-marker") {
                break;
            }
        }
        assert!(output.contains("helper-marker"), "output was: {output:?}");

        client
            .request(
                &Message::Resize {
                    client_id: "c1".into(),
                    session_id: "s1".into(),
                    cols: 100,
                    rows: 40,
                },
                |_| {},
            )
            .unwrap();
        client
            .request(
                &Message::Destroy {
                    client_id: "c1".into(),
                    session_id: "s1".into(),
                },
                |_| {},
            )
            .unwrap();
    }

    #[test]
    fn namespaces_are_isolated() {
        let Some(fixture) = start_helper() else {
            return;
        };
        let stream = UnixStream::connect(&fixture.socket).unwrap();
        let mut client = HelperClient::connect(stream);
        client
            .request(
                &Message::Create {
                    client_id: "owner".into(),
                    session_id: "s1".into(),
                    cwd: None,
                    cols: 80,
                    rows: 24,
                },
                |_| {},
            )
            .unwrap();
        // The connection is bound to "owner"; another client id must be refused.
        let error = client
            .request(
                &Message::Attach {
                    client_id: "intruder".into(),
                    session_id: "s1".into(),
                },
                |_| {},
            )
            .unwrap_err();
        assert!(
            matches!(error, HelperClientError::Rejected(_)),
            "got {error:?}"
        );
    }
}

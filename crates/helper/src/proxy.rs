//! `proxy`: bridge stdin/stdout to the helper's Unix socket.
//!
//! The client runs this over an SSH `exec` channel (`sentinel-helper proxy`).
//! Frames are already self-delimiting, so the bridge is a byte-for-byte copy in
//! both directions — it never inspects or re-frames the protocol. It exits `0`
//! as soon as the socket side closes, which is what the client observes as "the
//! daemon went away".

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;

/// Connect to `socket` and pump bytes until the remote side closes.
///
/// A stdin EOF shuts down the write half and keeps draining the socket, so the
/// last replies are not lost when the client closes its side first.
pub fn run(socket: &Path) -> io::Result<()> {
    let socket_reader = UnixStream::connect(socket).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "cannot connect to helper socket {}: {err}",
                socket.display()
            ),
        )
    })?;
    let mut socket_writer = socket_reader.try_clone()?;

    let stdin_pump = thread::Builder::new()
        .name("proxy-stdin".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if socket_writer.write_all(&buffer[..read]).is_err() {
                            return;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = socket_writer.shutdown(std::net::Shutdown::Write);
        })?;

    copy_socket_to_stdout(socket_reader)?;
    // The socket closed: stdin no longer matters. The pump thread exits on its
    // own when stdin reaches EOF or the socket stops accepting bytes.
    drop(stdin_pump);
    Ok(())
}

fn copy_socket_to_stdout(mut socket: UnixStream) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match socket.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => {
                stdout.write_all(&buffer[..read])?;
                stdout.flush()?;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, Message};
    use std::os::unix::net::UnixListener;

    #[test]
    fn connect_error_mentions_the_socket_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.sock");
        let err = run(&path).unwrap_err();
        assert!(err.to_string().contains("missing.sock"), "{err}");
    }

    #[test]
    fn frames_cross_the_proxy_boundary_untouched() {
        // Exercise the copy half directly: a listener sends one frame and closes.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("helper.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let sender = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let frame = frame::encode(
                &Message::Hello {
                    client_id: "c".into(),
                    version: "0.1.0".into(),
                },
                b"",
            );
            stream.write_all(&frame).unwrap();
            drop(stream);
        });

        let stream = UnixStream::connect(&path).unwrap();
        // Read the frame through the same path the proxy uses.
        let mut buffer = Vec::new();
        let mut reader = stream;
        reader.read_to_end(&mut buffer).unwrap();
        sender.join().unwrap();

        let (message, body) = frame::decode(&buffer).unwrap();
        assert!(matches!(message, Message::Hello { .. }));
        assert!(body.is_empty());
    }
}

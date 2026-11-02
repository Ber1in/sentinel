//! Keeps `docs/protocol.md` honest: every example in the document must parse with the
//! real codec and name real error codes.

use std::path::PathBuf;

fn protocol_doc() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn documented_frame_example_decodes_with_the_codec() {
    let doc = protocol_doc();
    let line = doc
        .lines()
        .find(|line| line.trim_start().starts_with("frame-hex:"))
        .expect("protocol.md must contain a `frame-hex:` example");

    let hex = line.split_once(':').unwrap().1;
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|pair| u8::from_str_radix(pair, 16).expect("hex byte"))
        .collect();

    let (kind, session, seq, payload) =
        protocol::decode_data_frame(&bytes).expect("documented frame must decode");

    assert_eq!(kind, protocol::FrameKind::Output);
    assert_eq!(session, 7);
    assert_eq!(seq, 1_000_000);
    assert_eq!(payload, b"hello");
}

#[test]
fn documented_error_codes_exist_in_the_enum() {
    use protocol::ErrorCode;
    let doc = protocol_doc();
    for code in [
        ErrorCode::AuthFailed,
        ErrorCode::HostKeyMismatch,
        ErrorCode::Network,
        ErrorCode::Timeout,
        ErrorCode::ProtocolVersion,
        ErrorCode::NotFound,
        ErrorCode::InvalidRequest,
        ErrorCode::TmuxMissing,
        ErrorCode::Internal,
    ] {
        assert!(
            doc.contains(&format!("`{code}`")),
            "docs/protocol.md is missing error code `{code}`"
        );
    }
}

//! Minimal OSC parsing for shell integration.
//!
//! We only need the working-directory report (OSC 7, `ESC ] 7 ; file://host/path BEL`),
//! which bash/zsh/fish emit after every prompt. Parsing is incremental because escape
//! sequences routinely straddle two output chunks.

/// Largest sequence we are willing to buffer; longer garbage is discarded.
const MAX_SEQUENCE: usize = 8192;

#[derive(Debug, Default)]
pub struct CwdTracker {
    buffer: Vec<u8>,
}

impl CwdTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one output chunk and return the newest working directory it contained.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<String> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_SEQUENCE {
            let excess = self.buffer.len() - MAX_SEQUENCE;
            self.buffer.drain(..excess);
        }

        const PREFIX: &[u8] = b"\x1b]7;";
        let mut latest = None;
        let mut consumed = 0usize;
        let mut search_from = 0usize;

        while let Some(offset) = find(&self.buffer[search_from..], PREFIX) {
            let start = search_from + offset;
            let payload_start = start + PREFIX.len();
            match sequence_end(&self.buffer, payload_start) {
                Some((end, terminator_len)) => {
                    if let Some(path) = parse_payload(&self.buffer[payload_start..end]) {
                        latest = Some(path);
                    }
                    consumed = end + terminator_len;
                    search_from = consumed;
                }
                None => break,
            }
        }

        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        latest
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Find the terminator of an OSC string: BEL (`0x07`) or ST (`ESC \`).
fn sequence_end(buffer: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index < buffer.len() {
        match buffer[index] {
            0x07 => return Some((index, 1)),
            0x1b if index + 1 < buffer.len() && buffer[index + 1] == b'\\' => {
                return Some((index, 2))
            }
            _ => index += 1,
        }
    }
    None
}

/// `file://host/path` -> `/path`; a bare path is accepted as-is.
fn parse_payload(payload: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(payload);
    let path = if let Some(rest) = text.strip_prefix("file://") {
        // Skip the (possibly empty) host component.
        rest.find('/').map(|slash| &rest[slash..]).unwrap_or("")
    } else {
        text.as_ref()
    };
    let decoded = percent_decode(path);
    if decoded.is_empty() || !decoded.starts_with('/') {
        return None;
    }
    Some(decoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                output.push(value);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bel_terminated_report() {
        let mut tracker = CwdTracker::new();
        let cwd = tracker.feed(b"text\x1b]7;file://host/var/log\x07more");
        assert_eq!(cwd.as_deref(), Some("/var/log"));
    }

    #[test]
    fn parses_an_st_terminated_report() {
        let mut tracker = CwdTracker::new();
        let cwd = tracker.feed(b"\x1b]7;file://host/etc/nginx\x1b\\");
        assert_eq!(cwd.as_deref(), Some("/etc/nginx"));
    }

    #[test]
    fn handles_sequences_split_across_chunks() {
        let mut tracker = CwdTracker::new();
        assert_eq!(tracker.feed(b"\x1b]7;file://ho"), None);
        assert_eq!(tracker.feed(b"st/srv/app"), None);
        assert_eq!(tracker.feed(b"\x07"), Some("/srv/app".to_string()));
    }

    #[test]
    fn the_newest_report_wins() {
        let mut tracker = CwdTracker::new();
        let cwd = tracker.feed(b"\x1b]7;file://h/a\x07\x1b]7;file://h/b/c\x07");
        assert_eq!(cwd.as_deref(), Some("/b/c"));
    }

    #[test]
    fn decodes_percent_escapes() {
        let mut tracker = CwdTracker::new();
        let cwd = tracker.feed(b"\x1b]7;file://h/home/user/my%20dir\x07");
        assert_eq!(cwd.as_deref(), Some("/home/user/my dir"));
    }

    #[test]
    fn ignores_relative_or_empty_paths() {
        let mut tracker = CwdTracker::new();
        assert_eq!(tracker.feed(b"\x1b]7;file://host\x07"), None);
        assert_eq!(tracker.feed(b"\x1b]7;relative\x07"), None);
    }

    #[test]
    fn garbage_between_reports_is_harmless() {
        let mut tracker = CwdTracker::new();
        let long = vec![b'x'; 10_000];
        tracker.feed(&long);
        let cwd = tracker.feed(b"\x1b]7;file://h/tmp\x07");
        assert_eq!(cwd.as_deref(), Some("/tmp"));
    }
}

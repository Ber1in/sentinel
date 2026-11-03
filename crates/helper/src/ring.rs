//! Line-oriented scrollback ring used for attach replay.
//!
//! The buffer stores *lines* (including their trailing `\n`) rather than raw
//! bytes so a reattaching client can be handed the last N lines without
//! cutting an escape sequence in half. The current, not-yet-terminated line is
//! kept separately and is always included in a replay, so the prompt the user
//! was looking at shows up immediately.
//!
//! Memory is bounded twice: by the number of lines and by a byte budget. A
//! pathological single line is flushed early once it exceeds `max_line_bytes`.

use std::collections::VecDeque;

/// Default number of retained lines (design decision D3).
pub const DEFAULT_LINES: usize = 100_000;
/// Default byte budget for retained output.
pub const DEFAULT_BYTES: usize = 16 * 1024 * 1024;
/// A "line" longer than this is split so one hostile/binary line cannot pin memory.
pub const DEFAULT_LINE_BYTES: usize = 8 * 1024;

/// Bounded scrollback of terminal output.
pub struct RingBuffer {
    max_lines: usize,
    max_bytes: usize,
    max_line_bytes: usize,
    lines: VecDeque<Vec<u8>>,
    partial: Vec<u8>,
    completed_bytes: usize,
}

impl RingBuffer {
    /// Create a buffer holding at most `max_lines` lines / `max_bytes` bytes.
    pub fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self::with_line_limit(max_lines, max_bytes, DEFAULT_LINE_BYTES)
    }

    /// Same as [`RingBuffer::new`] with an explicit per-line cap.
    pub fn with_line_limit(max_lines: usize, max_bytes: usize, max_line_bytes: usize) -> Self {
        Self {
            max_lines,
            max_bytes,
            max_line_bytes: max_line_bytes.max(1),
            lines: VecDeque::new(),
            partial: Vec::new(),
            completed_bytes: 0,
        }
    }

    /// Append terminal output.
    pub fn push(&mut self, data: &[u8]) {
        if self.max_lines == 0 {
            return;
        }
        let mut start = 0;
        while start < data.len() {
            match data[start..].iter().position(|byte| *byte == b'\n') {
                Some(offset) => {
                    let end = start + offset + 1;
                    self.partial.extend_from_slice(&data[start..end]);
                    self.flush_partial();
                    start = end;
                }
                None => {
                    self.partial.extend_from_slice(&data[start..]);
                    start = data.len();
                }
            }
        }
        while self.partial.len() > self.max_line_bytes {
            let rest: Vec<u8> = self.partial.drain(self.max_line_bytes..).collect();
            let chunk = std::mem::replace(&mut self.partial, rest);
            self.push_line(chunk);
        }
    }

    fn flush_partial(&mut self) {
        let line = std::mem::take(&mut self.partial);
        if !line.is_empty() {
            self.push_line(line);
        }
    }

    fn push_line(&mut self, line: Vec<u8>) {
        self.completed_bytes += line.len();
        self.lines.push_back(line);
        self.evict();
    }

    fn evict(&mut self) {
        while self.lines.len() > self.max_lines || self.completed_bytes > self.max_bytes {
            match self.lines.pop_front() {
                Some(dropped) => self.completed_bytes -= dropped.len(),
                None => break,
            }
            if self.lines.is_empty() {
                break;
            }
        }
    }

    /// Bytes of the last `lines` complete lines plus the in-flight partial line.
    pub fn replay(&self, lines: usize) -> Vec<u8> {
        let take = lines.min(self.lines.len());
        let mut out = Vec::new();
        for line in self.lines.iter().skip(self.lines.len() - take) {
            out.extend_from_slice(line);
        }
        out.extend_from_slice(&self.partial);
        out
    }

    /// Number of complete lines currently retained.
    pub fn line_count(&self) -> u64 {
        self.lines.len() as u64
    }

    /// Bytes of the in-flight partial line.
    #[allow(dead_code)] // introspection used by tests and diagnostics
    pub fn partial_len(&self) -> usize {
        self.partial.len()
    }

    /// Drop everything, including the partial line.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.partial.clear();
        self.completed_bytes = 0;
    }

    /// Configured line capacity.
    #[allow(dead_code)] // introspection used by tests and diagnostics
    pub fn capacity_lines(&self) -> usize {
        self.max_lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_last_n_lines() {
        let mut ring = RingBuffer::new(5, DEFAULT_BYTES);
        for i in 0..20 {
            ring.push(format!("line {i}\n").as_bytes());
        }
        assert_eq!(ring.line_count(), 5);
        assert_eq!(ring.replay(10), b"line 15\nline 16\nline 17\nline 18\nline 19\n");
    }

    #[test]
    fn replay_window_is_the_tail() {
        let mut ring = RingBuffer::new(100, DEFAULT_BYTES);
        for i in 0..10 {
            ring.push(format!("{i}\n").as_bytes());
        }
        assert_eq!(ring.replay(3), b"7\n8\n9\n");
        assert_eq!(ring.replay(0), b"");
        assert_eq!(ring.replay(1_000), (0..10).map(|i| format!("{i}\n")).collect::<String>().as_bytes());
    }

    #[test]
    fn partial_line_is_always_replayed() {
        let mut ring = RingBuffer::new(10, DEFAULT_BYTES);
        ring.push(b"done\nprompt$ ");
        assert_eq!(ring.replay(5), b"done\nprompt$ ");
        assert_eq!(ring.line_count(), 1);
        assert_eq!(ring.partial_len(), 8);
    }

    #[test]
    fn chunks_split_anywhere_are_reassembled() {
        let mut ring = RingBuffer::new(10, DEFAULT_BYTES);
        ring.push(b"abc");
        ring.push(b"def\n");
        ring.push(b"gh");
        ring.push(b"i\n");
        assert_eq!(ring.replay(10), b"abcdef\nghi\n");
    }

    #[test]
    fn preserves_arbitrary_terminal_bytes() {
        let mut ring = RingBuffer::new(10, DEFAULT_BYTES);
        let payload: Vec<u8> = vec![0x1b, b'[', b'3', b'2', b'm', 0x00, 0xff, b'\n', 0x07];
        ring.push(&payload);
        assert_eq!(ring.replay(10), payload);
    }

    #[test]
    fn byte_budget_evicts_oldest_lines() {
        let mut ring = RingBuffer::new(1_000, 20);
        ring.push(b"aaaaaaaaaa\n"); // 11 bytes
        ring.push(b"bbbbbbbbbb\n"); // 11 bytes -> over budget
        assert_eq!(ring.replay(1_000), b"bbbbbbbbbb\n");
        assert!(ring.completed_bytes <= 20);
    }

    #[test]
    fn overlong_line_is_flushed_in_chunks() {
        let mut ring = RingBuffer::with_line_limit(10, DEFAULT_BYTES, 4);
        ring.push(b"abcdefghij");
        // 10 bytes with a 4-byte line cap: 4 + 4 completed, 2 pending.
        assert_eq!(ring.line_count(), 2);
        assert_eq!(ring.replay(10), b"abcdefghij");
        assert_eq!(ring.partial_len(), 2);
    }

    #[test]
    fn zero_lines_disables_retention() {
        let mut ring = RingBuffer::new(0, DEFAULT_BYTES);
        ring.push(b"hello\n");
        assert_eq!(ring.replay(10), b"");
        assert_eq!(ring.line_count(), 0);
    }

    #[test]
    fn twenty_thousand_lines_keep_the_last_window() {
        // Mirrors task 2.2: emit 20k lines, then replay a small window.
        let mut ring = RingBuffer::new(DEFAULT_LINES, DEFAULT_BYTES);
        for i in 0..20_000 {
            ring.push(format!("{i:05}\n").as_bytes());
        }
        assert_eq!(ring.line_count(), 20_000);
        let tail = ring.replay(100);
        let text = String::from_utf8(tail).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines[0], "19900");
        assert_eq!(lines[99], "19999");
    }

    #[test]
    fn clear_forgets_everything() {
        let mut ring = RingBuffer::new(10, DEFAULT_BYTES);
        ring.push(b"one\ntwo");
        ring.clear();
        assert_eq!(ring.replay(10), b"");
        assert_eq!(ring.line_count(), 0);
        assert_eq!(ring.partial_len(), 0);
    }
}

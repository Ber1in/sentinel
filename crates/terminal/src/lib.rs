//! In-memory scrollback and its on-disk spill.
//!
//! The daemon never parses terminal escape sequences: it keeps an ordered byte log,
//! counts newlines to enforce a line budget, and hands bytes back verbatim. Rendering
//! and selection stay in xterm.js.

use std::collections::VecDeque;

pub mod osc;
pub mod spill;

pub use osc::{CommandMarks, CommandTracker, CwdTracker};
pub use spill::{SpillError, SpillStore};

/// One contiguous run of terminal bytes; `seq` is the sequence number of the first byte
/// batch the daemon assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub seq: u64,
    pub bytes: Vec<u8>,
}

impl Chunk {
    fn newlines(&self) -> usize {
        self.bytes.iter().filter(|byte| **byte == b'\n').count()
    }
}

/// Ordered, bounded byte log used for replay and for the GUI's "scroll up" requests.
#[derive(Debug)]
pub struct Scrollback {
    capacity_lines: usize,
    chunks: VecDeque<Chunk>,
    lines: usize,
    next_seq: u64,
    evicted_lines: u64,
}

impl Scrollback {
    pub fn new(capacity_lines: usize) -> Self {
        Self {
            capacity_lines: capacity_lines.max(1),
            chunks: VecDeque::new(),
            lines: 0,
            next_seq: 0,
            evicted_lines: 0,
        }
    }

    pub fn capacity_lines(&self) -> usize {
        self.capacity_lines
    }

    pub fn lines(&self) -> usize {
        self.lines
    }

    pub fn evicted_lines(&self) -> u64 {
        self.evicted_lines
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Append one batch of terminal bytes, returning the sequence number assigned to it.
    pub fn append(&mut self, bytes: &[u8]) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if bytes.is_empty() {
            return seq;
        }
        self.lines += Chunk {
            seq,
            bytes: bytes.to_vec(),
        }
        .newlines();
        self.chunks.push_back(Chunk {
            seq,
            bytes: bytes.to_vec(),
        });
        self.enforce_capacity();
        seq
    }

    fn enforce_capacity(&mut self) {
        while self.lines > self.capacity_lines {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            let front_lines = front.newlines();
            let excess = self.lines - self.capacity_lines;
            if front_lines <= excess {
                // Drop the whole chunk.
                self.lines -= front_lines;
                self.evicted_lines += front_lines as u64;
                self.chunks.pop_front();
            } else {
                // Drop only the first `excess` lines of this chunk.
                let mut cut = 0;
                let mut removed = 0;
                for (index, byte) in front.bytes.iter().enumerate() {
                    if *byte == b'\n' {
                        removed += 1;
                        if removed == excess {
                            cut = index + 1;
                            break;
                        }
                    }
                }
                front.bytes.drain(..cut);
                self.lines -= excess;
                self.evicted_lines += excess as u64;
                break;
            }
        }
    }

    /// The most recent `lines` lines, reconstructing the tail of the log.
    pub fn tail_lines(&self, lines: usize) -> Vec<u8> {
        if lines == 0 {
            return Vec::new();
        }
        let mut remaining = lines;
        let mut picked: Vec<Vec<u8>> = Vec::new();
        for chunk in self.chunks.iter().rev() {
            if remaining == 0 {
                break;
            }
            let chunk_lines = chunk.newlines();
            if chunk_lines == 0 {
                // Bytes without a newline continue the newest line.
                picked.push(chunk.bytes.clone());
                continue;
            }
            if chunk_lines < remaining {
                picked.push(chunk.bytes.clone());
                remaining -= chunk_lines;
            } else {
                let start = boundary_offset(&chunk.bytes, remaining);
                picked.push(chunk.bytes[start..].to_vec());
                remaining = 0;
            }
        }
        picked.reverse();
        picked.concat()
    }

    /// Everything from `from_seq` onwards. `None` when the requested range was evicted.
    pub fn since(&self, from_seq: u64) -> Option<(u64, Vec<u8>)> {
        let first = self.chunks.front()?;
        if first.seq > from_seq {
            return None;
        }
        let mut start = first.seq;
        let mut bytes = Vec::new();
        for chunk in &self.chunks {
            if chunk.seq >= from_seq {
                if bytes.is_empty() {
                    start = chunk.seq;
                }
                bytes.extend_from_slice(&chunk.bytes);
            }
        }
        Some((start, bytes))
    }

    /// Bytes for chunks strictly before `before_seq`, newest-last, limited to `lines`.
    pub fn page_before(&self, before_seq: u64, lines: usize) -> (u64, Vec<u8>) {
        if lines == 0 {
            return (before_seq, Vec::new());
        }
        let mut remaining = lines;
        let mut picked: Vec<(u64, Vec<u8>)> = Vec::new();
        for chunk in self.chunks.iter().rev() {
            if chunk.seq >= before_seq {
                continue;
            }
            if remaining == 0 {
                break;
            }
            let chunk_lines = chunk.newlines();
            if chunk_lines == 0 {
                picked.push((chunk.seq, chunk.bytes.clone()));
                continue;
            }
            if chunk_lines < remaining {
                picked.push((chunk.seq, chunk.bytes.clone()));
                remaining -= chunk_lines;
            } else {
                let start = boundary_offset(&chunk.bytes, remaining);
                picked.push((chunk.seq, chunk.bytes[start..].to_vec()));
                remaining = 0;
            }
        }
        picked.reverse();
        let start = picked.first().map(|(seq, _)| *seq).unwrap_or(before_seq);
        (
            start,
            picked.into_iter().flat_map(|(_, bytes)| bytes).collect(),
        )
    }
}

/// Byte offset where the last `lines` complete lines of `bytes` begin.
fn boundary_offset(bytes: &[u8], lines: usize) -> usize {
    let mut seen = 0;
    for (index, byte) in bytes.iter().enumerate().rev() {
        if *byte == b'\n' {
            if seen == lines {
                return index + 1;
            }
            seen += 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_assign_monotonic_sequence_numbers() {
        let mut scrollback = Scrollback::new(10);
        assert_eq!(scrollback.append(b"a\n"), 0);
        assert_eq!(scrollback.append(b"b\n"), 1);
        assert_eq!(scrollback.next_seq(), 2);
    }

    #[test]
    fn line_capacity_evicts_the_oldest_lines_first() {
        let mut scrollback = Scrollback::new(3);
        for index in 0..5 {
            scrollback.append(format!("line{index}\n").as_bytes());
        }
        assert_eq!(scrollback.lines(), 3);
        assert_eq!(scrollback.evicted_lines(), 2);
        let tail = String::from_utf8(scrollback.tail_lines(10)).unwrap();
        assert!(
            !tail.contains("line0"),
            "oldest line must be evicted: {tail:?}"
        );
        assert!(tail.contains("line4"));
        assert!(tail.contains("line2"));
    }

    #[test]
    fn partial_eviction_splits_a_single_large_chunk() {
        let mut scrollback = Scrollback::new(2);
        scrollback.append(b"a\nb\nc\nd\n");
        assert_eq!(scrollback.lines(), 2);
        assert_eq!(
            String::from_utf8(scrollback.tail_lines(10)).unwrap(),
            "c\nd\n"
        );
    }

    #[test]
    fn tail_lines_limits_the_window() {
        let mut scrollback = Scrollback::new(100);
        scrollback.append(b"one\ntwo\nthree\nfour\n");
        assert_eq!(
            String::from_utf8(scrollback.tail_lines(2)).unwrap(),
            "three\nfour\n"
        );
    }

    #[test]
    fn tail_without_trailing_newline_is_preserved() {
        let mut scrollback = Scrollback::new(100);
        scrollback.append(b"prompt$ ");
        assert_eq!(
            String::from_utf8(scrollback.tail_lines(10)).unwrap(),
            "prompt$ "
        );
    }

    #[test]
    fn since_returns_contiguous_bytes_or_none_after_eviction() {
        let mut scrollback = Scrollback::new(100);
        scrollback.append(b"a\n");
        scrollback.append(b"b\n");
        let (start, bytes) = scrollback.since(1).unwrap();
        assert_eq!(start, 1);
        assert_eq!(bytes, b"b\n");
    }

    #[test]
    fn since_reports_eviction() {
        let mut scrollback = Scrollback::new(1);
        scrollback.append(b"a\n");
        scrollback.append(b"b\n");
        assert!(scrollback.since(0).is_none());
    }

    #[test]
    fn page_before_walks_backwards() {
        let mut scrollback = Scrollback::new(100);
        for index in 0..10 {
            scrollback.append(format!("l{index}\n").as_bytes());
        }
        let (start, bytes) = scrollback.page_before(8, 3);
        assert!(start < 8);
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text, "l5\nl6\nl7\n");
    }

    #[test]
    fn replay_walks_from_the_tail_back_to_the_oldest_retained_line() {
        let mut scrollback = Scrollback::new(100_000);
        for index in 0..20_000 {
            scrollback.append(format!("line-{index}\n").as_bytes());
        }
        let tail = String::from_utf8(scrollback.tail_lines(2_000)).unwrap();
        assert!(
            tail.starts_with("line-18000"),
            "tail should start 2000 lines back"
        );

        let mut cursor = scrollback.next_seq();
        let mut all = tail;
        while let (start, page) = scrollback.page_before(cursor, 2_000) {
            if page.is_empty() || start >= cursor {
                break;
            }
            all = format!("{}{}", String::from_utf8(page).unwrap(), all);
            cursor = start;
        }
        assert!(
            all.contains("line-0\n"),
            "must be able to reach the oldest line"
        );
        assert!(all.contains("line-19999"));
    }

    #[test]
    fn resync_after_a_dropped_frame_returns_each_chunk_exactly_once() {
        let mut scrollback = Scrollback::new(100_000);
        for index in 0..100 {
            scrollback.append(format!("chunk-{index}\n").as_bytes());
        }
        // The GUI only received chunks 0..=40 before frames started dropping.
        let (start, bytes) = scrollback.since(41).unwrap();
        assert_eq!(start, 41);
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            !text.contains("chunk-40"),
            "already-delivered chunk must not repeat"
        );
        assert_eq!(text.matches("chunk-").count(), 59);
        assert!(text.contains("chunk-99"));
    }

    #[test]
    fn default_capacity_is_configured_by_the_caller() {
        // The daemon passes 100_000 from configuration; the type itself has no default.
        let scrollback = Scrollback::new(100_000);
        assert_eq!(scrollback.capacity_lines(), 100_000);
    }
}

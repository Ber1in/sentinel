//! Bounded input queue used while a session is reconnecting.
//!
//! Losing half-typed commands is a visible regression, so keystrokes are buffered
//! instead of dropped. The queue is bounded to protect the daemon from a stuck client;
//! when it is full the GUI is told, rather than silently discarding input.

use std::collections::VecDeque;

/// 64 KiB, matching the design.
pub const DEFAULT_CAPACITY: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// All bytes were buffered.
    Accepted,
    /// Only `accepted` bytes fit; the rest were refused.
    Full { accepted: usize, refused: usize },
}

#[derive(Debug)]
pub struct InputQueue {
    capacity: usize,
    buffer: VecDeque<u8>,
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl InputQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            buffer: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&mut self, bytes: &[u8]) -> PushOutcome {
        let free = self.capacity.saturating_sub(self.buffer.len());
        if bytes.len() <= free {
            self.buffer.extend(bytes);
            PushOutcome::Accepted
        } else {
            let (accepted, refused) = bytes.split_at(free);
            self.buffer.extend(accepted);
            PushOutcome::Full {
                accepted: free,
                refused: refused.len(),
            }
        }
    }

    /// Drain everything, preserving byte order.
    pub fn take(&mut self) -> Vec<u8> {
        self.buffer.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bytes_in_order() {
        let mut queue = InputQueue::default();
        assert_eq!(queue.push(b"echo hi"), PushOutcome::Accepted);
        assert_eq!(queue.len(), 7);
        assert_eq!(queue.take(), b"echo hi");
        assert!(queue.is_empty());
    }

    #[test]
    fn full_queue_reports_what_it_refused() {
        let mut queue = InputQueue::new(4);
        assert_eq!(queue.push(b"ab"), PushOutcome::Accepted);
        assert_eq!(
            queue.push(b"cdef"),
            PushOutcome::Full {
                accepted: 2,
                refused: 2
            }
        );
        assert_eq!(queue.take(), b"abcd");
    }

    #[test]
    fn half_typed_line_survives_a_reconnect() {
        let mut queue = InputQueue::default();
        queue.push(b"git commit -m 'still typing");
        assert_eq!(queue.take(), b"git commit -m 'still typing");
    }

    #[test]
    fn exactly_full_is_accepted() {
        let mut queue = InputQueue::new(3);
        assert_eq!(queue.push(b"abc"), PushOutcome::Accepted);
        assert_eq!(
            queue.push(b"d"),
            PushOutcome::Full {
                accepted: 0,
                refused: 1
            }
        );
    }
}

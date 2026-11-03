//! Cooperative cancellation for transfers.
//!
//! A token is a cheap `Arc<AtomicBool>` handle. Callers cancel from any thread;
//! transfers poll it between chunks and abandon the in-flight file, removing the
//! `.part-*` file they created.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared "stop now" flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every holder of this token to stop. Idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn cancellation_is_shared_between_clones() {
        let token = CancelToken::new();
        let observer = token.clone();
        token.cancel();
        assert!(observer.is_cancelled());
        assert!(token.is_cancelled());
        // Idempotent.
        token.cancel();
        assert!(observer.is_cancelled());
    }
}

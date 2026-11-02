//! Session lifecycle state machine.
//!
//! Every transition is explicit so the reconnect logic cannot drift from the spec.
//! Authentication and host-key failures are terminal: retrying them gets accounts
//! locked and hides the real problem.

use std::time::Duration;

use protocol::SessionState;

/// Exponential backoff for retryable failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    /// 1s, 2s, 4s, 8s, 16s, then the cap. `attempt` is 1-based.
    pub fn delay(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16);
        let millis = self.initial.as_millis() as u64 * 2u64.pow(exponent.min(16));
        Duration::from_millis(millis.min(self.max.as_millis() as u64))
    }
}

/// What happened to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Connected,
    LinkLost,
    /// Retryable failure (network/timeout): schedule another attempt.
    RetryDue,
    /// Non-retryable failure (credentials, host key).
    Fatal,
    Detach,
    Attach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    Rejected { from: SessionState, event: Event },
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::Rejected { from, event } => {
                write!(f, "event {event:?} is not valid in state {from:?}")
            }
        }
    }
}

/// Drives one session's state and retry bookkeeping.
#[derive(Debug, Clone)]
pub struct SessionFsm {
    state: SessionState,
    attempt: u32,
    policy: ReconnectPolicy,
    /// Set when a reconnect had to recreate the remote session.
    recreated: bool,
}

impl SessionFsm {
    pub fn new(policy: ReconnectPolicy) -> Self {
        Self {
            state: SessionState::Connecting,
            attempt: 0,
            policy,
            recreated: false,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn recreated(&self) -> bool {
        self.recreated
    }

    pub fn reconnect_delay(&self) -> Duration {
        self.policy.delay(self.attempt.max(1))
    }

    /// Apply one event, returning the new state.
    pub fn apply(&mut self, event: Event) -> Result<SessionState, TransitionError> {
        use SessionState::*;
        let next = match (self.state, event) {
            (Connecting, Event::Connected) => {
                self.attempt = 0;
                Connected
            }
            (Connecting | Reconnecting | Detached | Lost, Event::Fatal) => Failed,
            (Connecting, Event::LinkLost) => Reconnecting,
            (Connected, Event::LinkLost) => {
                self.attempt += 1;
                Reconnecting
            }
            (Reconnecting, Event::RetryDue) => {
                self.attempt += 1;
                Connecting
            }
            (Connected, Event::Detach) => Detached,
            (Detached, Event::Attach) => Connecting,
            (Lost, Event::Attach) => Connecting,
            // Reconnecting sessions keep retrying; a duplicate tick is a no-op.
            (Reconnecting, Event::LinkLost) => Reconnecting,
            _ => {
                return Err(TransitionError::Rejected {
                    from: self.state,
                    event,
                })
            }
        };
        self.state = next;
        Ok(next)
    }

    /// Record that the remote session had to be recreated (process is gone).
    pub fn mark_recreated(&mut self) {
        self.recreated = true;
    }

    /// Clear the flag after it has been reported to the GUI.
    pub fn clear_recreated(&mut self) {
        self.recreated = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fsm() -> SessionFsm {
        SessionFsm::new(ReconnectPolicy::default())
    }

    #[test]
    fn happy_path_connect_detach_attach() {
        let mut session = fsm();
        assert_eq!(session.state(), SessionState::Connecting);
        assert_eq!(
            session.apply(Event::Connected).unwrap(),
            SessionState::Connected
        );
        assert_eq!(
            session.apply(Event::Detach).unwrap(),
            SessionState::Detached
        );
        assert_eq!(
            session.apply(Event::Attach).unwrap(),
            SessionState::Connecting
        );
    }

    #[test]
    fn link_loss_enters_reconnecting_and_counts_attempts() {
        let mut session = fsm();
        session.apply(Event::Connected).unwrap();
        session.apply(Event::LinkLost).unwrap();
        assert_eq!(session.state(), SessionState::Reconnecting);
        assert_eq!(session.attempt(), 1);
        assert_eq!(
            session.apply(Event::RetryDue).unwrap(),
            SessionState::Connecting
        );
        assert_eq!(session.attempt(), 2);
    }

    #[test]
    fn fatal_errors_stop_retrying() {
        let mut session = fsm();
        assert_eq!(session.apply(Event::Fatal).unwrap(), SessionState::Failed);
        assert!(session.apply(Event::Fatal).is_err());
        assert!(session.apply(Event::Connected).is_err());
        assert!(session.apply(Event::RetryDue).is_err());
    }

    #[test]
    fn success_resets_the_attempt_counter() {
        let mut session = fsm();
        session.apply(Event::Connected).unwrap();
        session.apply(Event::LinkLost).unwrap();
        session.apply(Event::RetryDue).unwrap();
        session.apply(Event::Connected).unwrap();
        assert_eq!(session.attempt(), 0);
    }

    #[test]
    fn duplicate_link_lost_while_reconnecting_is_tolerated() {
        let mut session = fsm();
        session.apply(Event::Connected).unwrap();
        session.apply(Event::LinkLost).unwrap();
        assert_eq!(
            session.apply(Event::LinkLost).unwrap(),
            SessionState::Reconnecting
        );
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut session = fsm();
        assert!(session.apply(Event::Detach).is_err());
        assert!(session.apply(Event::RetryDue).is_err());
        session.apply(Event::Connected).unwrap();
        assert!(session.apply(Event::Attach).is_err());
    }

    #[test]
    fn backoff_grows_then_caps() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.delay(1), Duration::from_secs(1));
        assert_eq!(policy.delay(2), Duration::from_secs(2));
        assert_eq!(policy.delay(3), Duration::from_secs(4));
        assert_eq!(policy.delay(4), Duration::from_secs(8));
        assert_eq!(policy.delay(5), Duration::from_secs(16));
        assert_eq!(policy.delay(6), Duration::from_secs(30));
        assert_eq!(policy.delay(50), Duration::from_secs(30));
    }

    #[test]
    fn recreated_flag_is_reported_once() {
        let mut session = fsm();
        assert!(!session.recreated());
        session.mark_recreated();
        assert!(session.recreated());
        session.clear_recreated();
        assert!(!session.recreated());
    }
}

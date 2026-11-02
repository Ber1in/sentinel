//! Fallback provider used when the remote host has no tmux and the user chose to
//! continue without persistence. It is honest about the consequence: the shell dies
//! with the connection.

use async_trait::async_trait;
use protocol::ProviderKind;

use crate::{shell_quote, EnsureOutcome, PersistenceError, PersistenceProvider, RemoteExec};

#[derive(Debug, Default, Clone, Copy)]
pub struct NoPersistence;

impl NoPersistence {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PersistenceProvider for NoPersistence {
    fn kind(&self) -> ProviderKind {
        ProviderKind::None
    }

    fn session_name(&self, host_id: &str, pane_id: &str) -> String {
        format!("np_{host_id}_{pane_id}")
    }

    fn attach_command(&self, _session_name: &str, cwd: Option<&str>) -> String {
        let login = "exec \"${SHELL:-/bin/sh}\" -l";
        match cwd {
            Some(dir) => format!("cd {} && {login}", shell_quote(dir)),
            None => login.to_string(),
        }
    }

    async fn ensure(
        &self,
        _exec: &dyn RemoteExec,
        _session_name: &str,
        _cwd: Option<&str>,
    ) -> Result<EnsureOutcome, PersistenceError> {
        Ok(EnsureOutcome::Unpersisted)
    }

    async fn snapshot(
        &self,
        _exec: &dyn RemoteExec,
        _session_name: &str,
        _lines: usize,
    ) -> Result<String, PersistenceError> {
        Ok(String::new())
    }

    async fn destroy(
        &self,
        _exec: &dyn RemoteExec,
        _session_name: &str,
    ) -> Result<(), PersistenceError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_unpersisted_kind() {
        assert_eq!(NoPersistence::new().kind(), ProviderKind::None);
    }

    #[test]
    fn attach_command_starts_a_login_shell_in_the_requested_directory() {
        let provider = NoPersistence::new();
        assert_eq!(
            provider.attach_command("ignored", None),
            "exec \"${SHELL:-/bin/sh}\" -l"
        );
        assert_eq!(
            provider.attach_command("ignored", Some("/var/log")),
            "cd /var/log && exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[tokio::test]
    async fn ensure_reports_no_persistence() {
        struct NeverUsed;
        #[async_trait]
        impl RemoteExec for NeverUsed {
            async fn run(&self, _command: &str) -> Result<crate::CommandOutput, PersistenceError> {
                panic!("the fallback provider must not touch the remote host")
            }
        }
        let outcome = NoPersistence::new()
            .ensure(&NeverUsed, "x", None)
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::Unpersisted);
    }
}

//! tmux-backed persistence.
//!
//! Design choices (see `design.md` D5):
//! - a dedicated server socket (`-L sshterminal`) so the user's own tmux server and
//!   configuration never interfere with ours;
//! - an injected config file (`-f`) disabling the status bar and mouse handling;
//! - deterministic session names derived from host + pane.

use async_trait::async_trait;
use protocol::ProviderKind;

use crate::{
    shell_quote, CommandOutput, EnsureOutcome, PersistenceError, PersistenceProvider, RemoteExec,
};

/// Tunables that end up in the injected tmux config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxOptions {
    pub socket: String,
    pub history_limit: u32,
    pub status: bool,
    pub mouse: bool,
    pub default_terminal: String,
    /// Where the injected config is written on the remote host.
    pub config_path: String,
}

impl Default for TmuxOptions {
    fn default() -> Self {
        Self {
            socket: "sshterminal".into(),
            history_limit: 100_000,
            status: false,
            mouse: false,
            default_terminal: "tmux-256color".into(),
            config_path: "~/.cache/sshterminal/tmux.conf".into(),
        }
    }
}

impl TmuxOptions {
    /// Contents of the injected config file.
    pub fn config_contents(&self) -> String {
        format!(
            "set -g status {}\nset -g mouse {}\nset -g history-limit {}\nset -g default-terminal \"{}\"\nset -g destroy-unattached off\nset -g focus-events on\n",
            if self.status { "on" } else { "off" },
            if self.mouse { "on" } else { "off" },
            self.history_limit,
            self.default_terminal
        )
    }
}

#[derive(Debug, Clone)]
pub struct TmuxPersistence {
    options: TmuxOptions,
}

impl TmuxPersistence {
    pub fn new(options: TmuxOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &TmuxOptions {
        &self.options
    }

    /// `st_<host>_<pane>` with everything outside `[A-Za-z0-9_-]` folded to `-`.
    pub fn make_session_name(host_id: &str, pane_id: &str) -> String {
        let sanitize = |value: &str| {
            value
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        };
        format!("st_{}_{}", sanitize(host_id), sanitize(pane_id))
    }

    fn tmux(&self, args: &str) -> String {
        format!("tmux -L {} {}", shell_quote(&self.options.socket), args)
    }

    pub fn probe_command(&self) -> String {
        "command -v tmux >/dev/null 2>&1 && echo yes || echo no".to_string()
    }

    pub fn has_session_command(&self, session: &str) -> String {
        format!(
            "{} has-session -t {} >/dev/null 2>&1 && echo present || echo absent",
            self.tmux(""),
            shell_quote(session)
        )
    }

    pub fn create_session_command(&self, session: &str, cwd: Option<&str>) -> String {
        let mut command = format!(
            "mkdir -p \"$(dirname {})\" && printf '%s' {} > {}",
            shell_quote(&self.options.config_path),
            shell_quote(&self.options.config_contents()),
            shell_quote(&self.options.config_path)
        );
        command.push_str(" && ");
        command.push_str(&self.tmux(&format!(
            "-f {} new-session -d -s {} {}",
            shell_quote(&self.options.config_path),
            shell_quote(session),
            cwd.map(|dir| format!("-c {}", shell_quote(dir))).unwrap_or_default()
        )));
        command
    }

    pub fn attach_command(&self, session: &str, cwd: Option<&str>) -> String {
        // `-A` attaches when the session exists, so a reattach after a reconnect is one
        // command; the config write keeps new servers consistent with our options.
        let mut inner = format!(
            "mkdir -p \"$(dirname {})\"; printf '%s' {} > {}; exec tmux -u -L {} -f {} new-session -A -s {} {}",
            shell_quote(&self.options.config_path),
            shell_quote(&self.options.config_contents()),
            shell_quote(&self.options.config_path),
            shell_quote(&self.options.socket),
            shell_quote(&self.options.config_path),
            shell_quote(session),
            cwd.map(|dir| format!("-c {}", shell_quote(dir))).unwrap_or_default()
        );
        // Wrap so the remote shell hands over to tmux without an extra process in between.
        inner = format!("exec sh -c {}", shell_quote(&inner));
        inner
    }

    pub fn snapshot_command(&self, session: &str, lines: usize) -> String {
        // Pane targets need the `session:` form and must not use the `=` exact-match
        // prefix (tmux only accepts that for session targets).
        self.tmux(&format!(
            "capture-pane -p -e -t {}: -S -{}",
            shell_quote(session),
            lines
        ))
    }

    pub fn destroy_command(&self, session: &str) -> String {
        self.tmux(&format!("kill-session -t {}", shell_quote(session)))
    }
}

fn require_ok(output: CommandOutput) -> Result<CommandOutput, PersistenceError> {
    if output.success() {
        Ok(output)
    } else {
        Err(PersistenceError::CommandFailed(format!(
            "exit {}: {}",
            output.exit_code,
            output.stderr.trim()
        )))
    }
}

#[async_trait]
impl PersistenceProvider for TmuxPersistence {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Tmux
    }

    fn session_name(&self, host_id: &str, pane_id: &str) -> String {
        Self::make_session_name(host_id, pane_id)
    }

    fn attach_command(&self, session_name: &str, cwd: Option<&str>) -> String {
        TmuxPersistence::attach_command(self, session_name, cwd)
    }

    async fn ensure(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
        cwd: Option<&str>,
    ) -> Result<EnsureOutcome, PersistenceError> {
        let probe = require_ok(exec.run(&self.probe_command()).await?)?;
        if probe.stdout.trim() != "yes" {
            return Err(PersistenceError::TmuxMissing);
        }

        let presence = require_ok(exec.run(&self.has_session_command(session_name)).await?)?;
        match presence.stdout.trim() {
            "present" => Ok(EnsureOutcome::Attached),
            _ => {
                require_ok(
                    exec.run(&self.create_session_command(session_name, cwd))
                        .await?,
                )?;
                Ok(EnsureOutcome::Recreated)
            }
        }
    }

    async fn snapshot(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
        lines: usize,
    ) -> Result<String, PersistenceError> {
        let output = require_ok(
            exec.run(&self.snapshot_command(session_name, lines))
                .await?,
        )?;
        Ok(output.stdout)
    }

    async fn destroy(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
    ) -> Result<(), PersistenceError> {
        // Destroying an already-dead session is not an error.
        let _ = exec.run(&self.destroy_command(session_name)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Records commands and replays canned outputs.
    struct FakeExec {
        replies: Mutex<VecDeque<CommandOutput>>,
        seen: Mutex<Vec<String>>,
    }

    impl FakeExec {
        fn new(replies: Vec<CommandOutput>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RemoteExec for FakeExec {
        async fn run(&self, command: &str) -> Result<CommandOutput, PersistenceError> {
            self.seen.lock().unwrap().push(command.to_string());
            Ok(self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| CommandOutput::ok("")))
        }
    }

    fn provider() -> TmuxPersistence {
        TmuxPersistence::new(TmuxOptions::default())
    }

    #[test]
    fn session_names_are_deterministic_and_safe() {
        let a = TmuxPersistence::make_session_name("host-1", "pane/9");
        let b = TmuxPersistence::make_session_name("host-1", "pane/9");
        assert_eq!(a, b);
        assert_eq!(a, "st_host-1_pane-9");
        assert_ne!(a, TmuxPersistence::make_session_name("host-1", "pane/8"));
    }

    #[test]
    fn injected_config_disables_chrome() {
        let config = TmuxOptions::default().config_contents();
        assert!(config.contains("status off"));
        assert!(config.contains("mouse off"));
        assert!(config.contains("history-limit 100000"));
        assert!(config.contains("destroy-unattached off"));
    }

    #[test]
    fn attach_command_reuses_or_creates_the_session() {
        let command = provider().attach_command("st_h_p", Some("/var/log"));
        assert!(command.contains("new-session -A"));
        assert!(command.contains("-c /var/log"));
        assert!(command.contains("-L sshterminal"));
        assert!(command.contains("-f"));
    }

    #[tokio::test]
    async fn ensure_attaches_when_the_session_exists() {
        let exec = FakeExec::new(vec![
            CommandOutput::ok("yes\n"),
            CommandOutput::ok("present\n"),
        ]);
        let outcome = provider()
            .ensure(&exec, "st_h_p", Some("/var/log"))
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::Attached);
        assert_eq!(
            exec.seen().len(),
            2,
            "attach must not create a second session"
        );
    }

    #[tokio::test]
    async fn ensure_recreates_when_the_session_is_gone() {
        let exec = FakeExec::new(vec![
            CommandOutput::ok("yes\n"),
            CommandOutput::ok("absent\n"),
            CommandOutput::ok(""),
        ]);
        let outcome = provider()
            .ensure(&exec, "st_h_p", Some("/srv"))
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::Recreated);
        let seen = exec.seen();
        assert_eq!(seen.len(), 3);
        assert!(seen[2].contains("new-session -d"));
        assert!(seen[2].contains("-c /srv"));
    }

    #[tokio::test]
    async fn missing_tmux_is_reported_not_silently_ignored() {
        let exec = FakeExec::new(vec![CommandOutput::ok("no\n")]);
        let error = provider().ensure(&exec, "st_h_p", None).await.unwrap_err();
        assert!(matches!(error, PersistenceError::TmuxMissing));
        assert_eq!(error.code(), protocol::ErrorCode::TmuxMissing);
    }

    #[tokio::test]
    async fn snapshot_returns_colourised_output() {
        let exec = FakeExec::new(vec![CommandOutput::ok("\u{1b}[31mred\u{1b}[0m\n")]);
        let text = provider().snapshot(&exec, "st_h_p", 2000).await.unwrap();
        assert_eq!(text, "\u{1b}[31mred\u{1b}[0m\n");
        assert!(exec.seen()[0].contains("capture-pane"));
        assert!(exec.seen()[0].contains("-S -2000"));
    }

    #[tokio::test]
    async fn destroy_tolerates_a_missing_session() {
        let exec = FakeExec::new(vec![CommandOutput::failed(1, "can't find session")]);
        provider().destroy(&exec, "st_h_p").await.unwrap();
    }
}

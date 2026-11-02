//! Credential handling and the four supported authentication methods.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client::Handle;
use russh::keys::agent::client::AgentClient;
use russh::keys::key::KeyPair;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::client::ClientHandler;
use crate::error::SshError;

/// One keyboard-interactive round offered by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiRequest {
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<String>,
}

/// Supplies answers for a keyboard-interactive round.
pub type KiResponder = Arc<dyn Fn(&KiRequest) -> Vec<String> + Send + Sync>;

/// How to authenticate. Secrets are resolved by the daemon before connecting;
/// this type never touches storage.
#[derive(Clone)]
pub enum Credentials {
    Password(String),
    Key {
        path: Option<PathBuf>,
        pem: Option<String>,
        passphrase: Option<String>,
    },
    Agent,
    KeyboardInteractive {
        responder: KiResponder,
    },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secrets, even accidentally.
        let label = match self {
            Credentials::Password(_) => "password",
            Credentials::Key { .. } => "key",
            Credentials::Agent => "agent",
            Credentials::KeyboardInteractive { .. } => "keyboard_interactive",
        };
        f.write_str(&format!("Credentials::{label}(<redacted>)"))
    }
}

/// Authenticate an established connection. Returns `Err(Auth)` when the server
/// rejects every method, which the state machine treats as terminal.
pub async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    credentials: &Credentials,
) -> Result<(), SshError> {
    match credentials {
        Credentials::Password(password) => {
            let ok = handle.authenticate_password(user, password.clone()).await?;
            if ok {
                Ok(())
            } else {
                Err(SshError::Auth("password rejected".into()))
            }
        }
        Credentials::Key {
            path,
            pem,
            passphrase,
        } => {
            let key: Arc<KeyPair> = match (path, pem) {
                (_, Some(pem)) => Arc::new(
                    russh::keys::decode_secret_key(pem, passphrase.as_deref())
                        .map_err(|error| SshError::Key(error.to_string()))?,
                ),
                (Some(path), None) => Arc::new(
                    russh::keys::load_secret_key(path, passphrase.as_deref())
                        .map_err(|error| SshError::Key(error.to_string()))?,
                ),
                (None, None) => return Err(SshError::Key("no key material given".into())),
            };
            let ok = handle.authenticate_publickey(user, key).await?;
            if ok {
                Ok(())
            } else {
                Err(SshError::Auth("public key rejected".into()))
            }
        }
        Credentials::Agent => authenticate_with_agent(handle, user).await,
        Credentials::KeyboardInteractive { responder } => {
            authenticate_keyboard_interactive(handle, user, responder).await
        }
    }
}

async fn authenticate_keyboard_interactive(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    responder: &KiResponder,
) -> Result<(), SshError> {
    use russh::client::KeyboardInteractiveAuthResponse as Response;

    let mut response = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await?;

    // Bound the exchange so a hostile server cannot keep us in a prompt loop.
    for _ in 0..8 {
        match response {
            Response::Success => return Ok(()),
            Response::Failure => {
                return Err(SshError::Auth("keyboard-interactive rejected".into()))
            }
            Response::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let request = KiRequest {
                    name,
                    instructions,
                    prompts: prompts.into_iter().map(|prompt| prompt.prompt).collect(),
                };
                let answers = responder(&request);
                if answers.len() != request.prompts.len() {
                    return Err(SshError::Auth(
                        "responder returned the wrong number of answers".into(),
                    ));
                }
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Err(SshError::Auth(
        "too many keyboard-interactive rounds".into(),
    ))
}

#[cfg(unix)]
async fn authenticate_with_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
) -> Result<(), SshError> {
    let agent = AgentClient::connect_env()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;
    run_agent_auth(handle, user, agent).await
}

#[cfg(windows)]
async fn authenticate_with_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
) -> Result<(), SshError> {
    use tokio::net::windows::named_pipe::ClientOptions;
    let pipe = ClientOptions::new()
        .open(r"\\.\pipe\openssh-ssh-agent")
        .map_err(|error| SshError::Agent(format!("open ssh-agent pipe: {error}")))?;
    let agent = AgentClient::connect(pipe);
    run_agent_auth(handle, user, agent).await
}

async fn run_agent_auth<S>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    mut agent: AgentClient<S>,
) -> Result<(), SshError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;
    if identities.is_empty() {
        return Err(SshError::Auth("ssh-agent has no identities".into()));
    }
    for key in identities {
        let (returned, result) = handle.authenticate_future(user, key, agent).await;
        agent = returned;
        match result {
            Ok(true) => return Ok(()),
            Ok(false) => continue,
            Err(error) => return Err(SshError::Agent(error.to_string())),
        }
    }
    Err(SshError::Auth("ssh-agent keys rejected".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_secrets() {
        let password = Credentials::Password("hunter2".into());
        let rendered = format!("{password:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("redacted"));

        let key = Credentials::Key {
            path: Some("/tmp/id_ed25519".into()),
            pem: Some("-----BEGIN OPENSSH PRIVATE KEY-----".into()),
            passphrase: Some("secret".into()),
        };
        let rendered = format!("{key:?}");
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("BEGIN"));
    }
}

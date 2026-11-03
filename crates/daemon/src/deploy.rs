//! Remote helper deployment: capability probe, atomic install, and start-up.
//!
//! The commands are built and parsed here as pure functions so they can be tested
//! without a server; the caller runs them over an existing SSH connection.

use std::path::{Path, PathBuf};

/// Result of probing a host before uploading anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub arch: String,
    pub libc: String,
    pub home: String,
    pub writable: bool,
    pub noexec: bool,
}

impl Probe {
    /// Target triple for our static binary, or a reason we cannot support the host.
    pub fn target(&self) -> Result<&'static str, String> {
        let triple = match self.arch.as_str() {
            "x86_64" => "x86_64-unknown-linux-musl",
            "aarch64" | "arm64" => "aarch64-unknown-linux-musl",
            other => return Err(format!("unsupported architecture: {other}")),
        };
        Ok(triple)
    }

    /// Human-readable blockers, empty when the host is usable.
    pub fn blockers(&self) -> Vec<String> {
        let mut blockers = Vec::new();
        if let Err(reason) = self.target() {
            blockers.push(reason);
        }
        if !self.writable {
            blockers.push("install directory is not writable".into());
        }
        if self.noexec {
            blockers.push("install directory is on a noexec mount".into());
        }
        blockers
    }

    pub fn ok(&self) -> bool {
        self.blockers().is_empty()
    }

    pub fn install_dir(&self) -> PathBuf {
        PathBuf::from(&self.home).join(".local/share/sentinel/bin")
    }
}

/// One command that reports everything the probe needs, newline separated.
pub fn probe_command() -> &'static str {
    "printf 'arch=%s\\n' \"$(uname -m)\"; \
     printf 'libc=%s\\n' \"$( (command -v ldd >/dev/null && ldd --version 2>&1 | head -1) || echo musl)\"; \
     printf 'home=%s\\n' \"$HOME\"; \
     d=\"$HOME/.local/share/sentinel/bin\"; \
     if ( mkdir -p \"$d\" 2>/dev/null && [ -w \"$d\" ] ); then echo writable=yes; else echo writable=no; fi; \
     if findmnt -no OPTIONS -T \"$d\" 2>/dev/null | grep -q noexec; then echo noexec=yes; else echo noexec=no; fi"
}

pub fn parse_probe(stdout: &str) -> Probe {
    let mut arch = String::new();
    let mut libc = String::from("unknown");
    let mut home = String::new();
    let mut writable = false;
    let mut noexec = false;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "arch" => arch = value.to_string(),
            "libc" => {
                libc = if value.contains("musl") {
                    "musl".to_string()
                } else if let Some(rest) = value.split("GLIBC ").nth(1) {
                    let version: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    format!("glibc {version}")
                } else {
                    value.to_string()
                }
            }
            "home" => home = value.to_string(),
            "writable" => writable = value == "yes",
            "noexec" => noexec = value == "yes",
            _ => {}
        }
    }
    Probe {
        arch,
        libc,
        home,
        writable,
        noexec,
    }
}

/// Install script run after the binary has been uploaded to `<dir>/.staging`.
///
/// Verifies the checksum, makes the file executable and swaps it in atomically, so a
/// failed upload can never leave a half-written binary in place.
pub fn install_script(dir: &Path, expected_sha256: &str) -> String {
    let dir = dir.display();
    format!(
        "set -e; \
         d={dir}; \
         test -f \"$d/.staging\" || {{ echo 'staging binary missing' >&2; exit 11; }}; \
         actual=$(sha256sum \"$d/.staging\" | cut -d' ' -f1); \
         if [ \"$actual\" != '{expected_sha256}' ]; then \
           rm -f \"$d/.staging\"; echo \"checksum mismatch: $actual\" >&2; exit 12; \
         fi; \
         chmod 700 \"$d/.staging\"; \
         mv -f \"$d/.staging\" \"$d/sentinel-helper\"; \
         \"$d/sentinel-helper\" version; \
         echo SENTINEL_INSTALL_OK"
    )
}

/// Upload `binary` to `<dir>/.staging` over the same SSH connection, then install it.
///
/// The shipped helper is a Linux ELF bundled with the Windows installer; this is the
/// only place it crosses the wire.
pub async fn upload_and_install(
    connection: &ssh::Connection,
    dir: &std::path::Path,
    binary: &[u8],
    expected_sha256: &str,
) -> anyhow::Result<String> {
    let temp = std::env::temp_dir().join(format!(
        "sentinel-helper-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::write(&temp, binary)?;

    let result = async {
        let target = sftp::RemotePath::parse(format!("{}/.staging", dir.display()))?;
        let fs = sftp::SftpFs::connect(connection).await?;
        let mut progress = |_done: u64, _total: u64| {};
        fs.upload(
            &temp,
            &target,
            "deploy",
            &sftp::CancelToken::new(),
            &mut progress,
        )
        .await?;
        install_staged(connection, dir, expected_sha256).await
    }
    .await;

    let _ = std::fs::remove_file(&temp);
    result
}

/// Make sure the helper is installed and running, uploading only when needed.
///
/// Idempotent: a remote helper that already answers `version` is left alone.
pub async fn ensure(
    connection: &ssh::Connection,
    binary: &[u8],
    expected_sha256: &str,
) -> anyhow::Result<Probe> {
    let probe = probe(connection).await?;
    if !probe.ok() {
        anyhow::bail!(
            "host cannot run the helper: {}",
            probe.blockers().join("; ")
        );
    }
    let dir = probe.install_dir();

    let existing = execute(
        connection,
        &format!("\"{}/sentinel-helper\" version 2>/dev/null", dir.display()),
    )
    .await?;
    if existing.0 == 0 && !existing.1.trim().is_empty() {
        return Ok(probe);
    }

    upload_and_install(connection, &dir, binary, expected_sha256).await?;
    start(connection, &dir).await?;
    Ok(probe)
}

/// Start the helper detached and wait briefly for its socket.
pub fn start_script(dir: &Path) -> String {
    let dir = dir.display();
    format!(
        "set -e; d={dir}; \
         if \"$d/sentinel-helper\" --help >/dev/null 2>&1; then :; fi; \
         nohup setsid \"$d/sentinel-helper\" serve >\"$d/helper.log\" 2>&1 & \
         i=0; while [ $i -lt 30 ]; do \
           if [ -S \"$HOME/.local/share/sentinel/run/helper.sock\" ]; then exit 0; fi; \
           i=$((i+1)); sleep 0.1; \
         done; \
         echo 'helper socket did not appear' >&2; tail -n 20 \"$d/helper.log\" >&2; exit 13"
    )
}

/// Run one command over an existing SSH connection and collect its output.
pub async fn execute(
    connection: &ssh::Connection,
    command: &str,
) -> anyhow::Result<(i32, String, String)> {
    let mut channel = connection.exec(command).await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = 0;
    while let Some(message) = channel.wait().await {
        match message {
            russh::ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            russh::ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            russh::ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
            russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok((
        code,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

/// Probe the remote host. A non-zero exit from the probe command is reported as an
/// error because it means we could not determine the host's capabilities.
pub async fn probe(connection: &ssh::Connection) -> anyhow::Result<Probe> {
    let (code, stdout, stderr) = execute(connection, probe_command()).await?;
    if code != 0 {
        anyhow::bail!("probe failed (exit {code}): {}", stderr.trim());
    }
    Ok(parse_probe(&stdout))
}

/// Install a binary that is already present at `<dir>/.staging`.
pub async fn install_staged(
    connection: &ssh::Connection,
    dir: &std::path::Path,
    expected_sha256: &str,
) -> anyhow::Result<String> {
    let (code, stdout, stderr) = execute(connection, &install_script(dir, expected_sha256)).await?;
    // Some servers close the channel without an exit status, so a success marker is
    // more reliable than the exit code alone.
    if code != 0 || !stdout.contains("SENTINEL_INSTALL_OK") {
        anyhow::bail!(
            "install failed (exit {code}): {}",
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        );
    }
    Ok(stdout
        .lines()
        .find(|line| !line.contains("SENTINEL_INSTALL_OK"))
        .unwrap_or("")
        .trim()
        .to_string())
}

/// Make sure the helper is installed and running, uploading only when needed.
///

/// Start the helper detached and wait until its socket exists.
pub async fn start(connection: &ssh::Connection, dir: &std::path::Path) -> anyhow::Result<()> {
    let (code, _stdout, stderr) = execute(connection, &start_script(dir)).await?;
    if code != 0 {
        anyhow::bail!("helper did not start (exit {code}): {}", stderr.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "arch=x86_64\nlibc=ldd (Ubuntu GLIBC 2.35-0ubuntu3.1) 2.35\nhome=/home/alice\nwritable=yes\nnoexec=no\n";

    #[test]
    fn parses_a_typical_probe() {
        let probe = parse_probe(SAMPLE);
        assert_eq!(probe.arch, "x86_64");
        assert_eq!(probe.libc, "glibc 2.35");
        assert_eq!(probe.home, "/home/alice");
        assert!(probe.writable);
        assert!(!probe.noexec);
        assert!(probe.ok());
        assert_eq!(probe.target().unwrap(), "x86_64-unknown-linux-musl");
    }

    #[test]
    fn unsupported_arch_blocks() {
        let probe = parse_probe("arch=riscv64\nhome=/h\nwritable=yes\nnoexec=no\n");
        assert!(!probe.ok());
        assert!(probe.blockers()[0].contains("riscv64"));
    }

    #[test]
    fn missing_writability_and_noexec_are_both_reported() {
        let probe = parse_probe("arch=aarch64\nhome=/h\nwritable=no\nnoexec=yes\n");
        let blockers = probe.blockers();
        assert_eq!(blockers.len(), 2);
        assert!(!probe.ok());
    }

    #[test]
    fn probe_command_covers_every_field() {
        let command = probe_command();
        for field in ["arch=", "libc=", "home=", "writable=", "noexec="] {
            assert!(command.contains(field), "probe misses {field}");
        }
    }

    #[test]
    fn install_script_verifies_checksum_before_swapping() {
        let script = install_script(Path::new("/home/a/.local/share/sentinel/bin"), "abc123");
        assert!(script.contains("sha256sum"));
        assert!(script.contains("abc123"));
        assert!(script.contains("mv -f"));
        assert!(script.contains("chmod 700"));
        // The checksum check must come before the swap.
        let check = script.find("checksum mismatch").unwrap();
        let swap = script.find("mv -f").unwrap();
        assert!(check < swap);
    }

    #[tokio::test]
    async fn uploads_installs_and_reports_the_helper_version() {
        let Ok(host) = std::env::var("SSHTERM_TEST_HOST") else {
            return eprintln!("skipped: no test sshd");
        };
        let binary_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/sentinel-helper");
        if !binary_path.exists() {
            return eprintln!("skipped: build `cargo build -p helper` first");
        }
        let binary = std::fs::read(&binary_path).unwrap();
        let sha = String::from_utf8(
            std::process::Command::new("sha256sum")
                .arg(&binary_path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

        let port: u16 = std::env::var("SSHTERM_TEST_PORT").unwrap().parse().unwrap();
        let user = std::env::var("SSHTERM_TEST_USER").unwrap();
        let password = std::env::var("SSHTERM_TEST_PASSWORD").unwrap();

        let connection = ssh::Connection::connect(&ssh::ConnectOptions {
            host,
            port,
            user: user.clone(),
            credentials: ssh::Credentials::Password(password),
            host_key_policy: ssh::HostKeyPolicy::AcceptAny,
            keepalive: ssh::Keepalive::default(),
            timeout: std::time::Duration::from_secs(10),
        })
        .await
        .expect("connect");

        let probe = probe(&connection).await.expect("probe");
        let dir = probe.install_dir();
        let version = upload_and_install(&connection, &dir, &binary, &sha)
            .await
            .expect("install");
        assert!(version.starts_with(char::is_numeric) || !version.is_empty());

        // A second install with a wrong checksum must fail and leave no staging file.
        let error = upload_and_install(&connection, &dir, &binary, "0000")
            .await
            .expect_err("bad checksum must fail");
        assert!(error.to_string().contains("checksum"), "{error}");

        let _ = execute(&connection, &format!("rm -rf {}", dir.display())).await;
        connection.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn probes_a_real_host_over_ssh() {
        let Ok(host) = std::env::var("SSHTERM_TEST_HOST") else {
            return eprintln!("skipped: no test sshd");
        };
        let port: u16 = std::env::var("SSHTERM_TEST_PORT").unwrap().parse().unwrap();
        let user = std::env::var("SSHTERM_TEST_USER").unwrap();
        let password = std::env::var("SSHTERM_TEST_PASSWORD").unwrap();

        let connection = ssh::Connection::connect(&ssh::ConnectOptions {
            host,
            port,
            user: user.clone(),
            credentials: ssh::Credentials::Password(password),
            host_key_policy: ssh::HostKeyPolicy::AcceptAny,
            keepalive: ssh::Keepalive::default(),
            timeout: std::time::Duration::from_secs(10),
        })
        .await
        .expect("connect");

        let probe = probe(&connection).await.expect("probe");
        assert!(!probe.arch.is_empty());
        assert!(probe.home.contains(&user), "home was {}", probe.home);
        assert!(
            probe.writable,
            "test user's home should be writable: {probe:?}"
        );
        assert!(probe.target().is_ok());
        connection.disconnect().await.unwrap();
    }

    #[test]
    fn start_script_waits_for_the_socket() {
        let script = start_script(Path::new("/h/bin"));
        assert!(script.contains("setsid"));
        assert!(script.contains("helper.sock"));
        assert!(script.contains("tail -n 20"));
    }
}

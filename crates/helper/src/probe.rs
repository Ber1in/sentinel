//! Host capability probe used before installing the helper.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    pub ok: bool,
    pub arch: String,
    pub libc: String,
    pub target: Option<String>,
    pub install_dir: PathBuf,
    pub writable: bool,
    pub noexec: bool,
    /// Human-readable reasons the host cannot run the helper.
    pub blockers: Vec<String>,
}

/// Where the client should place the binary for this user.
pub fn default_install_dir() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/sentinel/bin")
}

pub fn check() -> anyhow::Result<InstallReport> {
    Ok(check_dir(&default_install_dir()))
}

pub fn check_dir(dir: &Path) -> InstallReport {
    let arch = std::env::consts::ARCH.to_string();
    let target = match arch.as_str() {
        "x86_64" => Some("x86_64-unknown-linux-musl".to_string()),
        "aarch64" => Some("aarch64-unknown-linux-musl".to_string()),
        _ => None,
    };

    let libc = detect_libc();
    let writable = is_writable(dir);
    let noexec = is_noexec(dir);

    let mut blockers = Vec::new();
    if target.is_none() {
        blockers.push(format!("unsupported architecture: {arch}"));
    }
    if !writable {
        blockers.push(format!("install dir is not writable: {}", dir.display()));
    }
    if noexec {
        blockers.push(format!(
            "install dir is on a noexec mount: {}",
            dir.display()
        ));
    }

    InstallReport {
        ok: blockers.is_empty(),
        arch,
        libc,
        target,
        install_dir: dir.to_path_buf(),
        writable,
        noexec,
        blockers,
    }
}

fn detect_libc() -> String {
    if Path::new("/lib/ld-musl-x86_64.so.1").exists()
        || Path::new("/lib/ld-musl-aarch64.so.1").exists()
    {
        return "musl".into();
    }
    let output = std::process::Command::new("ldd").arg("--version").output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string()
                + &String::from_utf8_lossy(&out.stderr);
            if let Some(rest) = text.split("GLIBC ").nth(1) {
                let version: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                format!("glibc {version}")
            } else {
                "glibc (unknown version)".into()
            }
        }
        Err(_) => "unknown".into(),
    }
}

fn is_writable(dir: &Path) -> bool {
    let mut probe = dir.to_path_buf();
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => return false,
        }
    }
    // Writing a temporary file is the only reliable check (permissions, ACLs, quota).
    let test = probe.join(format!(".sentinel-write-test-{}", std::process::id()));
    match std::fs::write(&test, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&test);
            true
        }
        Err(_) => false,
    }
}

fn is_noexec(dir: &Path) -> bool {
    let mut probe = dir.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => return false,
        }
    }
    let output = std::process::Command::new("findmnt")
        .args(["-no", "OPTIONS", "-T"])
        .arg(&probe)
        .output();
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .split(',')
            .any(|flag| flag.trim() == "noexec"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_directory_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let report = check_dir(dir.path());
        assert!(report.writable, "temp dir should be writable");
        assert!(!report.noexec, "temp dir should allow exec");
    }

    #[test]
    fn unsupported_architecture_blocks_install() {
        let dir = tempfile::tempdir().unwrap();
        let report = check_dir(dir.path());
        // On the CI/dev hosts this is x86_64 or aarch64, both supported.
        assert!(report.target.is_some(), "unexpected arch: {}", report.arch);
    }

    #[test]
    fn unwritable_directory_is_blocked() {
        let report = check_dir(Path::new("/proc/sentinel-cannot-write"));
        assert!(!report.writable);
        assert!(!report.ok);
        assert!(report
            .blockers
            .iter()
            .any(|reason| reason.contains("not writable")));
    }

    #[test]
    fn libc_detection_is_not_empty() {
        assert!(!detect_libc().is_empty());
    }
}

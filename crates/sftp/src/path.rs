//! Validated remote paths.
//!
//! Every SFTP operation in this crate takes a [`RemotePath`] instead of a bare
//! `String`. The type can only be built through [`RemotePath::parse`], which
//! rejects relative paths, NUL bytes and `..` traversal, so a caller cannot
//! smuggle the client outside of the directory the user is looking at.
//!
//! Accepted paths are normalised: repeated slashes, `.` components and trailing
//! slashes are collapsed, so `/home//user/./` becomes `/home/user`. `..` is
//! rejected rather than resolved, because resolving it requires the current
//! working directory of the remote shell and hides what was actually asked for.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use thiserror::Error;

/// A remote path that is guaranteed to be absolute and free of `..`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemotePath(String);

impl RemotePath {
    /// Validate and normalise an absolute remote path.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, PathError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.contains('\0') {
            return Err(PathError::Nul);
        }
        if !raw.starts_with('/') {
            return Err(PathError::NotAbsolute(raw.to_owned()));
        }

        let mut normalised = String::with_capacity(raw.len());
        for component in raw.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                return Err(PathError::ParentTraversal(raw.to_owned()));
            }
            normalised.push('/');
            normalised.push_str(component);
        }
        if normalised.is_empty() {
            normalised.push('/');
        }
        Ok(Self(normalised))
    }

    /// The path as it should be sent over the wire (absolute, normalised).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last component, or an empty string for the root directory.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("")
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// Append one already-validated path component.
    pub fn join(&self, name: &str) -> Result<Self, PathError> {
        validate_component(name)?;
        let mut joined = self.0.clone();
        if !self.is_root() {
            joined.push('/');
        }
        joined.push_str(name);
        Ok(Self(joined))
    }

    /// The temporary name used while a transfer is in flight: `<name>.part-<task>`.
    pub fn part(&self, task: &str) -> Result<Self, PathError> {
        let name = self.name();
        if name.is_empty() {
            return Err(PathError::InvalidName(format!(
                "{self}: the root directory has no file name"
            )));
        }
        validate_component(task)?;
        // `self` is normalised and not root, so it always has a parent directory.
        let parent = match self.0.rfind('/') {
            Some(0) => Self("/".to_owned()),
            Some(index) => Self(self.0[..index].to_owned()),
            None => return Err(PathError::NotAbsolute(self.0.clone())),
        };
        parent.join(&format!("{name}.part-{task}"))
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RemotePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for RemotePath {
    type Err = PathError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl TryFrom<&Path> for RemotePath {
    type Error = PathError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        let raw = path
            .to_str()
            .ok_or_else(|| PathError::InvalidName(path.display().to_string()))?;
        Self::parse(raw)
    }
}

/// A path component that will not escape its parent directory.
pub(crate) fn validate_component(name: &str) -> Result<(), PathError> {
    if name.is_empty() {
        return Err(PathError::InvalidName("empty path component".into()));
    }
    if name.contains('\0') {
        return Err(PathError::Nul);
    }
    if name.contains('/') {
        return Err(PathError::InvalidName(format!(
            "{name}: path components must not contain '/'"
        )));
    }
    if name == "." || name == ".." {
        return Err(PathError::InvalidName(format!(
            "{name}: '.' and '..' are not valid file names"
        )));
    }
    Ok(())
}

/// A rejected remote path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path must be absolute: {0}")]
    NotAbsolute(String),
    #[error("path must not contain a '..' component: {0}")]
    ParentTraversal(String),
    #[error("path must not contain NUL bytes")]
    Nul,
    #[error("invalid path component: {0}")]
    InvalidName(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_are_accepted() {
        for raw in ["/", "/home/user", "/var/log/syslog", "/a/b/c"] {
            assert!(
                RemotePath::parse(raw).is_ok(),
                "{raw} should be a valid remote path"
            );
        }
    }

    #[test]
    fn paths_are_normalised() {
        let cases = [
            ("/", "/"),
            ("//", "/"),
            ("/home//user/.", "/home/user"),
            ("/home/user/", "/home/user"),
            ("/a/./b//c/", "/a/b/c"),
        ];
        for (raw, expected) in cases {
            assert_eq!(RemotePath::parse(raw).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn relative_paths_are_rejected() {
        for raw in [
            "",
            "home/user",
            ".",
            "./home",
            "~",
            "~user/file",
            "file.txt",
        ] {
            let error = RemotePath::parse(raw).unwrap_err();
            assert!(
                matches!(error, PathError::Empty | PathError::NotAbsolute(_)),
                "{raw:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let cases = [
            "/..",
            "/../etc/passwd",
            "/home/../etc",
            "/home/user/..",
            "/home/user/../other",
            "/a/../../b",
        ];
        for raw in cases {
            assert_eq!(
                RemotePath::parse(raw).unwrap_err(),
                PathError::ParentTraversal(raw.to_owned()),
                "{raw} must be rejected"
            );
        }
    }

    #[test]
    fn nul_bytes_are_rejected() {
        assert_eq!(
            RemotePath::parse("/home/user\0evil").unwrap_err(),
            PathError::Nul
        );
        assert_eq!(
            RemotePath::parse("/home/user")
                .unwrap()
                .join("a\0b")
                .unwrap_err(),
            PathError::Nul
        );
    }

    #[test]
    fn join_keeps_the_result_absolute_and_single_component() {
        let dir = RemotePath::parse("/home/user").unwrap();
        let child = dir.join("notes.txt").unwrap();
        assert_eq!(child.as_str(), "/home/user/notes.txt");
        assert_eq!(child.name(), "notes.txt");

        let root_child = RemotePath::parse("/").unwrap().join("etc").unwrap();
        assert_eq!(root_child.as_str(), "/etc");

        for bad in ["", ".", "..", "a/b", "../x", "/abs"] {
            assert!(dir.join(bad).is_err(), "join({bad:?}) must fail");
        }
    }

    #[test]
    fn part_names_carry_the_task_id() {
        let target = RemotePath::parse("/home/user/report.pdf").unwrap();
        assert_eq!(
            target.part("task-1").unwrap().as_str(),
            "/home/user/report.pdf.part-task-1"
        );
        // Tasks are single path components too.
        for bad in ["", ".", "..", "a/b"] {
            assert!(target.part(bad).is_err(), "part({bad:?}) must fail");
        }
        assert!(RemotePath::parse("/").unwrap().part("t").is_err());
        assert_eq!(
            RemotePath::parse("/top")
                .unwrap()
                .part("t")
                .unwrap()
                .as_str(),
            "/top.part-t"
        );
    }

    #[test]
    fn names_and_root_are_reported() {
        assert_eq!(RemotePath::parse("/").unwrap().name(), "");
        assert!(RemotePath::parse("/").unwrap().is_root());
        assert_eq!(RemotePath::parse("/home/user").unwrap().name(), "user");
        assert!(!RemotePath::parse("/home/user").unwrap().is_root());
    }

    #[test]
    fn display_and_from_str_round_trip() {
        let path: RemotePath = "/home/user/file".parse().unwrap();
        assert_eq!(path.to_string(), "/home/user/file");
        assert!("/home/../etc".parse::<RemotePath>().is_err());
    }
}

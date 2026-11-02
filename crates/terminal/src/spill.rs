//! Disk spill for scrollback.
//!
//! Bytes are appended to segment files so a restart can still serve older history.
//! The total on disk is capped; the oldest segments are removed first. The daemon keeps
//! its in-memory scrollback as the source of truth and treats spill failures as
//! degradable (stop writing, warn the user, keep serving from memory).

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SpillError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Target size of one segment file before rotating.
pub const SEGMENT_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
struct Segment {
    path: PathBuf,
    size: u64,
}

/// Append-only, size-capped scrollback store for one session.
#[derive(Debug)]
pub struct SpillStore {
    dir: PathBuf,
    max_bytes: u64,
    segments: VecDeque<Segment>,
    current: Option<File>,
    current_seq_start: u64,
    current_size: u64,
    total_bytes: u64,
    disabled: bool,
}

impl SpillStore {
    pub fn open(root: &Path, session_id: &str, max_bytes: u64) -> Result<Self, SpillError> {
        let dir = root.join(session_id);
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            max_bytes: max_bytes.max(SEGMENT_BYTES),
            segments: VecDeque::new(),
            current: None,
            current_seq_start: 0,
            current_size: 0,
            total_bytes: 0,
            disabled: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len() + usize::from(self.current.is_some())
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Stop writing to disk after a failure. The caller keeps using memory.
    pub fn disable(&mut self) {
        self.disabled = true;
        self.current = None;
    }

    pub fn append(&mut self, seq: u64, bytes: &[u8]) -> Result<(), SpillError> {
        if self.disabled || bytes.is_empty() {
            return Ok(());
        }
        if self.current.is_none() {
            self.rotate(seq)?;
        }
        if self.current_size >= SEGMENT_BYTES {
            self.rotate(seq)?;
        }
        let Some(file) = self.current.as_mut() else {
            return Ok(());
        };
        file.write_all(bytes)?;
        file.flush()?;
        self.current_size += bytes.len() as u64;
        self.total_bytes += bytes.len() as u64;
        if let Some(last) = self.segments.back_mut() {
            last.size = self.current_size;
        }
        self.enforce_cap()?;
        Ok(())
    }

    fn rotate(&mut self, seq: u64) -> Result<(), SpillError> {
        if let Some(file) = self.current.take() {
            drop(file);
        }
        let path = self.dir.join(format!("{seq:012}.seg"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.segments.push_back(Segment { path, size: 0 });
        self.current = Some(file);
        self.current_seq_start = seq;
        self.current_size = 0;
        Ok(())
    }

    fn enforce_cap(&mut self) -> Result<(), SpillError> {
        while self.total_bytes > self.max_bytes && self.segments.len() > 1 {
            let Some(oldest) = self.segments.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(oldest.size);
            let _ = std::fs::remove_file(&oldest.path);
        }
        Ok(())
    }

    /// Concatenated contents of every live segment, oldest first.
    pub fn read_all(&self) -> Result<Vec<u8>, SpillError> {
        let mut all = Vec::new();
        for segment in &self.segments {
            if let Ok(bytes) = std::fs::read(&segment.path) {
                all.extend_from_slice(&bytes);
            }
        }
        Ok(all)
    }

    pub fn current_seq_start(&self) -> u64 {
        self.current_seq_start
    }

    /// Remove the whole session directory. Called when a session is destroyed.
    pub fn destroy(mut self) -> Result<(), SpillError> {
        self.current = None;
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_land_on_disk_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SpillStore::open(dir.path(), "s1", 8 * 1024 * 1024).unwrap();
        store.append(0, b"hello ").unwrap();
        store.append(1, b"world\n").unwrap();
        assert_eq!(store.read_all().unwrap(), b"hello world\n");
        assert_eq!(store.total_bytes(), 12);
    }

    #[test]
    fn rotating_creates_more_segments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SpillStore::open(dir.path(), "s1", 16 * 1024 * 1024).unwrap();
        let chunk = vec![b'x'; (SEGMENT_BYTES + 1) as usize];
        store.append(0, &chunk).unwrap();
        store.append(1, b"tail\n").unwrap();
        assert!(
            store.segment_count() >= 2,
            "expected rotation, got {}",
            store.segment_count()
        );
    }

    #[test]
    fn cap_evicts_oldest_segments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SpillStore::open(dir.path(), "s1", 3 * SEGMENT_BYTES).unwrap();
        for index in 0..8u64 {
            let chunk = vec![b'y'; (SEGMENT_BYTES + 1) as usize];
            store.append(index, &chunk).unwrap();
        }
        assert!(
            store.total_bytes() <= 4 * SEGMENT_BYTES,
            "total {} exceeded cap",
            store.total_bytes()
        );
        assert!(store.segment_count() < 8, "old segments must be removed");
    }

    #[test]
    fn destroy_removes_the_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SpillStore::open(dir.path(), "s1", 8 * 1024 * 1024).unwrap();
        let session_dir = store.dir().to_path_buf();
        store.append(0, b"data\n").unwrap();
        assert!(session_dir.exists());
        store.destroy().unwrap();
        assert!(!session_dir.exists());
    }

    #[test]
    fn disabled_store_stops_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SpillStore::open(dir.path(), "s1", 8 * 1024 * 1024).unwrap();
        store.append(0, b"before\n").unwrap();
        store.disable();
        store.append(1, b"after\n").unwrap();
        assert_eq!(store.read_all().unwrap(), b"before\n");
        assert!(store.is_disabled());
    }
}

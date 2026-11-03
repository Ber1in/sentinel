//! Integration tests against the throwaway sshd from `scripts/test-sshd.sh`.
//!
//!   eval "$(./scripts/test-sshd.sh start)" &&
//!     cargo test -p sftp --test live -- --nocapture --test-threads=1
//!
//! Without the `SSHTERM_TEST_*` environment every test prints a skip notice and
//! returns, so `cargo test -p sftp` stays hermetic on machines with no fixture.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sftp::{CancelToken, RemoteFs, RemotePath, SftpError, SftpFs};
use ssh::{ConnectOptions, Connection, Credentials, HostKeyPolicy, Keepalive};

/// One sshd, one home directory: tests must not overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

struct Target {
    host: String,
    port: u16,
    user: String,
    password: String,
}

fn target() -> Option<Target> {
    Some(Target {
        host: env("SSHTERM_TEST_HOST")?,
        port: env("SSHTERM_TEST_PORT")?.parse().ok()?,
        user: env("SSHTERM_TEST_USER")?,
        password: env("SSHTERM_TEST_PASSWORD")?,
    })
}

async fn connect() -> Option<(Connection, SftpFs)> {
    let target = target()?;
    let options = ConnectOptions {
        host: target.host,
        port: target.port,
        user: target.user,
        credentials: Credentials::Password(target.password),
        host_key_policy: HostKeyPolicy::AcceptAny,
        keepalive: Keepalive::default(),
        timeout: Duration::from_secs(10),
    };
    let connection = Connection::connect(&options)
        .await
        .expect("connect to the fixture");
    let fs = SftpFs::connect(&connection)
        .await
        .expect("open the sftp subsystem");
    Some((connection, fs))
}

/// Connect and create a private directory under the test user's home.
async fn fixture() -> Option<(Connection, SftpFs, RemotePath)> {
    let (connection, fs) = connect().await?;
    let home = fs.home().await.expect("resolve the remote home directory");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = home
        .join(&format!("sentinel-sftp-{}-{unique}", std::process::id()))
        .expect("valid temporary directory name");
    fs.mkdir(&root).await.expect("create the test directory");
    Some((connection, fs, root))
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 37 % 251) as u8).collect()
}

fn write_local(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn assert_progress_monotonic(calls: &[(u64, u64)], expected_total: u64) {
    let mut previous = 0;
    for (done, total) in calls {
        assert!(*done >= previous, "progress went backwards: {calls:?}");
        assert!(
            *total >= *done,
            "total {total} is below done {done}: {calls:?}"
        );
        previous = *done;
    }
    let (done, total) = *calls.last().expect("at least one progress callback");
    assert_eq!(done, expected_total, "final progress: {calls:?}");
    assert_eq!(total, expected_total, "final total: {calls:?}");
}

fn names(entries: &[sftp::Entry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.name.as_str()).collect()
}

#[tokio::test]
async fn list_reports_entries_and_their_metadata() {
    let _guard = SERIAL.lock().await;
    let Some((connection, fs, root)) = fixture().await else {
        return eprintln!("skipped: no test sshd");
    };
    let local_dir = tempfile::tempdir().unwrap();
    let bytes = payload(1024);
    let local = write_local(local_dir.path(), "a.bin", &bytes);
    let target = root.join("a.bin").unwrap();

    fs.upload(&local, &target, "list", &CancelToken::new(), &mut |_, _| {})
        .await
        .expect("upload");
    fs.mkdir(&root.join("sub").unwrap()).await.expect("mkdir");

    // Exercise the trait through a trait object, as the panel will.
    let remote: &dyn RemoteFs = &fs;
    let entries = remote.list(&root).await.expect("list");
    assert_eq!(names(&entries), ["a.bin", "sub"]);

    let file = &entries[0];
    assert!(!file.is_dir);
    assert!(!file.is_symlink);
    assert!(file.is_file());
    assert_eq!(file.size, 1024);
    assert!(file.permissions.is_some());
    assert!(file.mtime.is_some());

    let dir = &entries[1];
    assert!(dir.is_dir);
    assert!(!dir.is_symlink);
    // Directory sizes are server-dependent (4096 on ext4, 0 on some servers).

    let stat = remote.stat(&target).await.expect("stat");
    assert_eq!(stat.name, "a.bin");
    assert_eq!(stat.size, 1024);
    assert!(remote.stat(&root.join("missing").unwrap()).await.is_err());

    let _ = fs.remove(&root).await;
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn upload_and_download_round_trip() {
    let _guard = SERIAL.lock().await;
    let Some((connection, fs, root)) = fixture().await else {
        return eprintln!("skipped: no test sshd");
    };
    let local_dir = tempfile::tempdir().unwrap();
    let bytes = payload(200 * 1024 + 7);
    let source = write_local(local_dir.path(), "payload.bin", &bytes);
    let remote = root.join("payload.bin").unwrap();

    let mut upload_progress = Vec::new();
    let written = fs
        .upload(
            &source,
            &remote,
            "up-1",
            &CancelToken::new(),
            &mut |done, total| upload_progress.push((done, total)),
        )
        .await
        .expect("upload");
    assert_eq!(written, bytes.len() as u64);
    assert_progress_monotonic(&upload_progress, bytes.len() as u64);

    // No `.part-*` leftovers, and the destination has the full size.
    let entries = fs.list(&root).await.expect("list");
    assert!(
        entries.iter().all(|entry| !entry.name.contains(".part-")),
        "temporary file left behind: {:?}",
        names(&entries)
    );
    assert_eq!(
        fs.stat(&remote).await.expect("stat").size,
        bytes.len() as u64
    );

    let destination = local_dir.path().join("downloaded.bin");
    let mut download_progress = Vec::new();
    let read = fs
        .download(
            &remote,
            &destination,
            "down-1",
            &CancelToken::new(),
            &mut |done, total| download_progress.push((done, total)),
        )
        .await
        .expect("download");
    assert_eq!(read, bytes.len() as u64);
    assert_progress_monotonic(&download_progress, bytes.len() as u64);
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);

    // Overwriting through the part file keeps the old contents until the end.
    std::fs::write(&source, b"second version").unwrap();
    fs.upload(
        &source,
        &remote,
        "up-2",
        &CancelToken::new(),
        &mut |_, _| {},
    )
    .await
    .expect("overwrite");
    assert_eq!(fs.stat(&remote).await.expect("stat").size, 14);

    let _ = fs.remove(&root).await;
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn mkdir_rename_and_chmod() {
    let _guard = SERIAL.lock().await;
    let Some((connection, fs, root)) = fixture().await else {
        return eprintln!("skipped: no test sshd");
    };
    let dir = root.join("work").unwrap();
    fs.mkdir(&dir).await.expect("mkdir");
    assert!(fs.stat(&dir).await.expect("stat dir").is_dir);

    fs.chmod(&dir, 0o700).await.expect("chmod dir");
    let mode = fs.stat(&dir).await.unwrap().permissions.unwrap();
    assert_eq!(mode & 0o777, 0o700);
    assert!(fs.chmod(&dir, 0o10_0700).await.is_err(), "mode is 12 bits");

    let renamed = root.join("renamed").unwrap();
    fs.rename(&dir, &renamed).await.expect("rename dir");
    assert!(fs.stat(&dir).await.is_err(), "old name must be gone");
    assert!(fs.stat(&renamed).await.expect("stat renamed").is_dir);

    let local_dir = tempfile::tempdir().unwrap();
    let file = write_local(local_dir.path(), "note.txt", b"hello");
    let remote = root.join("note.txt").unwrap();
    fs.upload(&file, &remote, "t", &CancelToken::new(), &mut |_, _| {})
        .await
        .expect("upload");

    fs.chmod(&remote, 0o600).await.expect("chmod file");
    assert_eq!(
        fs.stat(&remote).await.unwrap().permissions.unwrap() & 0o777,
        0o600
    );

    let moved = renamed.join("note.txt").unwrap();
    fs.rename(&remote, &moved).await.expect("rename into dir");
    let entries = fs.list(&renamed).await.expect("list");
    assert_eq!(names(&entries), ["note.txt"]);

    let _ = fs.remove(&root).await;
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn remove_deletes_files_and_directory_trees() {
    let _guard = SERIAL.lock().await;
    let Some((connection, fs, root)) = fixture().await else {
        return eprintln!("skipped: no test sshd");
    };
    let local_dir = tempfile::tempdir().unwrap();
    let file = write_local(local_dir.path(), "deep.txt", b"deep");

    let tree = root.join("tree").unwrap();
    let a = tree.join("a").unwrap();
    let b = a.join("b").unwrap();
    fs.mkdir(&tree).await.expect("mkdir tree");
    fs.mkdir(&a).await.expect("mkdir a");
    fs.mkdir(&b).await.expect("mkdir b");
    fs.upload(
        &file,
        &b.join("deep.txt").unwrap(),
        "rm",
        &CancelToken::new(),
        &mut |_, _| {},
    )
    .await
    .expect("upload into tree");
    let lonely = root.join("lonely.txt").unwrap();
    fs.upload(&file, &lonely, "rm2", &CancelToken::new(), &mut |_, _| {})
        .await
        .expect("upload lonely");

    fs.remove(&tree).await.expect("recursive remove");
    assert!(fs.stat(&tree).await.is_err());
    assert_eq!(names(&fs.list(&root).await.unwrap()), ["lonely.txt"]);

    fs.remove(&lonely).await.expect("remove file");
    assert!(fs.list(&root).await.unwrap().is_empty());
    assert!(fs.remove(&lonely).await.is_err(), "already gone");

    let _ = fs.remove(&root).await;
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn cancelled_transfers_leave_no_part_files() {
    let _guard = SERIAL.lock().await;
    let Some((connection, fs, root)) = fixture().await else {
        return eprintln!("skipped: no test sshd");
    };
    let local_dir = tempfile::tempdir().unwrap();
    let bytes = payload(128 * 1024);
    let source = write_local(local_dir.path(), "cancel.bin", &bytes);
    let remote = root.join("cancel.bin").unwrap();

    // Remote file that the cancelled download would have copied.
    fs.upload(&source, &remote, "ok", &CancelToken::new(), &mut |_, _| {})
        .await
        .expect("seed upload");

    let cancelled = CancelToken::new();
    cancelled.cancel();
    let error = fs
        .upload(
            &source,
            &root.join("new.bin").unwrap(),
            "c1",
            &cancelled,
            &mut |_, _| {},
        )
        .await
        .expect_err("upload must be cancelled");
    assert!(matches!(error, SftpError::Cancelled));
    assert!(error.is_cancelled());

    let entries = fs.list(&root).await.expect("list");
    assert_eq!(names(&entries), ["cancel.bin"]);
    assert!(fs.stat(&root.join("new.bin").unwrap()).await.is_err());

    // A token tripped after the first chunk must abort an in-flight upload and
    // still remove the part file it created.
    let mid_upload = CancelToken::new();
    let error = fs
        .upload(
            &source,
            &root.join("mid.bin").unwrap(),
            "c3",
            &mid_upload,
            &mut |done, _| {
                if done > 0 {
                    mid_upload.cancel();
                }
            },
        )
        .await
        .expect_err("mid-flight upload must be cancelled");
    assert!(matches!(error, SftpError::Cancelled));

    let entries = fs.list(&root).await.expect("list");
    assert_eq!(names(&entries), ["cancel.bin"]);
    assert!(entries.iter().all(|entry| !entry.name.contains(".part-")));

    let destination = local_dir.path().join("cancelled-copy.bin");
    let error = fs
        .download(&remote, &destination, "c2", &cancelled, &mut |_, _| {})
        .await
        .expect_err("download must be cancelled");
    assert!(matches!(error, SftpError::Cancelled));
    assert!(!destination.exists());

    // Same for a download cancelled between chunks.
    let mid_download = CancelToken::new();
    let mid_destination = local_dir.path().join("mid-copy.bin");
    let error = fs
        .download(
            &remote,
            &mid_destination,
            "c4",
            &mid_download,
            &mut |done, _| {
                if done > 0 {
                    mid_download.cancel();
                }
            },
        )
        .await
        .expect_err("mid-flight download must be cancelled");
    assert!(matches!(error, SftpError::Cancelled));
    assert!(!mid_destination.exists());

    let leftovers: Vec<_> = std::fs::read_dir(local_dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".part-"))
        .collect();
    assert!(leftovers.is_empty(), "local part files: {leftovers:?}");

    let _ = fs.remove(&root).await;
    connection.disconnect().await.unwrap();
}

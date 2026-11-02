//! SQLite state store: hosts, tabs, split panes, and session metadata.
//!
//! Everything the GUI restores after a restart lives here. Writes that describe one
//! layout change happen inside a single transaction so a crash never leaves half a
//! split tree behind.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    /// Credential-manager key; never the secret itself.
    pub auth_ref: Option<String>,
    pub group_name: Option<String>,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabRecord {
    pub id: String,
    pub title: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl SplitDirection {
    fn as_str(&self) -> &'static str {
        match self {
            SplitDirection::Horizontal => "horizontal",
            SplitDirection::Vertical => "vertical",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "vertical" => SplitDirection::Vertical,
            _ => SplitDirection::Horizontal,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneRecord {
    pub id: String,
    pub tab_id: String,
    pub parent_id: Option<String>,
    pub direction: Option<SplitDirection>,
    pub ratio: f64,
    pub host_id: Option<String>,
    pub session_id: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub host_id: String,
    pub provider: String,
    pub provider_name: String,
    pub cwd: Option<String>,
    pub status: String,
    pub last_active_at: i64,
}

/// Outcome of opening a state database.
#[derive(Debug)]
pub enum Opened {
    Clean(Store),
    /// The old file was unreadable; it is preserved at this path and a fresh store is used.
    Recovered {
        store: Store,
        backup: PathBuf,
        reason: String,
    },
}

pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Opened, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match Self::try_open(path) {
            Ok(store) => Ok(Opened::Clean(store)),
            Err(error) => {
                let backup = path.with_extension(format!(
                    "corrupt-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or_default()
                ));
                if path.exists() {
                    std::fs::rename(path, &backup)?;
                }
                let store = Self::try_open(path)?;
                Ok(Opened::Recovered {
                    store,
                    backup,
                    reason: error.to_string(),
                })
            }
        }
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn try_open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let version: u32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidQuery));
        }
        if version < 1 {
            self.conn.execute_batch(
                r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS host (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    hostname TEXT NOT NULL,
                    port INTEGER NOT NULL,
                    username TEXT NOT NULL,
                    auth_ref TEXT,
                    group_name TEXT,
                    last_used_at INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS tab (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    sort_order INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS pane (
                    id TEXT PRIMARY KEY,
                    tab_id TEXT NOT NULL REFERENCES tab(id) ON DELETE CASCADE,
                    parent_id TEXT,
                    direction TEXT,
                    ratio REAL NOT NULL DEFAULT 0.5,
                    host_id TEXT,
                    session_id TEXT,
                    sort_order INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS session (
                    id TEXT PRIMARY KEY,
                    host_id TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    provider_name TEXT NOT NULL,
                    cwd TEXT,
                    status TEXT NOT NULL,
                    last_active_at INTEGER NOT NULL
                );
                PRAGMA user_version = 1;
                COMMIT;
                "#,
            )?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    pub fn upsert_host(&mut self, host: &HostRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"INSERT INTO host (id, name, hostname, port, username, auth_ref, group_name, last_used_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
               ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name, hostname = excluded.hostname, port = excluded.port,
                 username = excluded.username, auth_ref = excluded.auth_ref,
                 group_name = excluded.group_name, last_used_at = excluded.last_used_at"#,
            params![
                host.id,
                host.name,
                host.hostname,
                host.port,
                host.username,
                host.auth_ref,
                host.group_name,
                host.last_used_at
            ],
        )?;
        Ok(())
    }

    pub fn hosts(&self) -> Result<Vec<HostRecord>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, hostname, port, username, auth_ref, group_name, last_used_at
             FROM host ORDER BY last_used_at DESC, name ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(HostRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                hostname: row.get(2)?,
                port: row.get(3)?,
                username: row.get(4)?,
                auth_ref: row.get(5)?,
                group_name: row.get(6)?,
                last_used_at: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Replace the whole layout in one transaction.
    pub fn save_layout(
        &mut self,
        tabs: &[TabRecord],
        panes: &[PaneRecord],
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM pane", [])?;
        tx.execute("DELETE FROM tab", [])?;
        for tab in tabs {
            tx.execute(
                "INSERT INTO tab (id, title, sort_order) VALUES (?1, ?2, ?3)",
                params![tab.id, tab.title, tab.sort_order],
            )?;
        }
        for pane in panes {
            tx.execute(
                "INSERT INTO pane (id, tab_id, parent_id, direction, ratio, host_id, session_id, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    pane.id,
                    pane.tab_id,
                    pane.parent_id,
                    pane.direction.as_ref().map(|direction| direction.as_str()),
                    pane.ratio,
                    pane.host_id,
                    pane.session_id,
                    pane.sort_order
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_layout(&self) -> Result<(Vec<TabRecord>, Vec<PaneRecord>), StoreError> {
        let mut tabs_statement = self
            .conn
            .prepare("SELECT id, title, sort_order FROM tab ORDER BY sort_order")?;
        let tabs = tabs_statement
            .query_map([], |row| {
                Ok(TabRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    sort_order: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut panes_statement = self.conn.prepare(
            "SELECT id, tab_id, parent_id, direction, ratio, host_id, session_id, sort_order
             FROM pane ORDER BY sort_order",
        )?;
        let panes = panes_statement
            .query_map([], |row| {
                let direction: Option<String> = row.get(3)?;
                Ok(PaneRecord {
                    id: row.get(0)?,
                    tab_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    direction: direction.as_deref().map(SplitDirection::from_str),
                    ratio: row.get(4)?,
                    host_id: row.get(5)?,
                    session_id: row.get(6)?,
                    sort_order: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((tabs, panes))
    }

    pub fn upsert_session(&mut self, session: &SessionRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"INSERT INTO session (id, host_id, provider, provider_name, cwd, status, last_active_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(id) DO UPDATE SET
                 host_id = excluded.host_id, provider = excluded.provider,
                 provider_name = excluded.provider_name, cwd = excluded.cwd,
                 status = excluded.status, last_active_at = excluded.last_active_at"#,
            params![
                session.id,
                session.host_id,
                session.provider,
                session.provider_name,
                session.cwd,
                session.status,
                session.last_active_at
            ],
        )?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, host_id, provider, provider_name, cwd, status, last_active_at
             FROM session ORDER BY last_active_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                host_id: row.get(1)?,
                provider: row.get(2)?,
                provider_name: row.get(3)?,
                cwd: row.get(4)?,
                status: row.get(5)?,
                last_active_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, host_id, provider, provider_name, cwd, status, last_active_at
                 FROM session WHERE id = ?1",
                params![id],
                |row| {
                    Ok(SessionRecord {
                        id: row.get(0)?,
                        host_id: row.get(1)?,
                        provider: row.get(2)?,
                        provider_name: row.get(3)?,
                        cwd: row.get(4)?,
                        status: row.get(5)?,
                        last_active_at: row.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn delete_session(&mut self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM session WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str) -> HostRecord {
        HostRecord {
            id: id.into(),
            name: format!("host-{id}"),
            hostname: "10.0.0.5".into(),
            port: 22,
            username: "root".into(),
            auth_ref: Some(format!("cred-{id}")),
            group_name: Some("prod".into()),
            last_used_at: 10,
        }
    }

    #[test]
    fn migrations_run_from_an_empty_database() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn hosts_roundtrip_and_order_by_last_use() {
        let mut store = Store::in_memory().unwrap();
        store.upsert_host(&host("a")).unwrap();
        let mut b = host("b");
        b.last_used_at = 99;
        store.upsert_host(&b).unwrap();

        let hosts = store.hosts().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].id, "b", "most recently used first");
        assert_eq!(hosts[1].auth_ref.as_deref(), Some("cred-a"));
    }

    #[test]
    fn upsert_host_updates_in_place() {
        let mut store = Store::in_memory().unwrap();
        store.upsert_host(&host("a")).unwrap();
        let mut changed = host("a");
        changed.name = "renamed".into();
        store.upsert_host(&changed).unwrap();
        let hosts = store.hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "renamed");
    }

    #[test]
    fn layout_roundtrips_including_split_tree() {
        let mut store = Store::in_memory().unwrap();
        let tabs = vec![TabRecord {
            id: "t1".into(),
            title: "prod".into(),
            sort_order: 0,
        }];
        let panes = vec![
            PaneRecord {
                id: "p1".into(),
                tab_id: "t1".into(),
                parent_id: None,
                direction: Some(SplitDirection::Horizontal),
                ratio: 0.6,
                host_id: Some("a".into()),
                session_id: Some("s1".into()),
                sort_order: 0,
            },
            PaneRecord {
                id: "p2".into(),
                tab_id: "t1".into(),
                parent_id: Some("p1".into()),
                direction: None,
                ratio: 0.5,
                host_id: Some("b".into()),
                session_id: Some("s2".into()),
                sort_order: 1,
            },
        ];
        store.save_layout(&tabs, &panes).unwrap();
        let (loaded_tabs, loaded_panes) = store.load_layout().unwrap();
        assert_eq!(loaded_tabs, tabs);
        assert_eq!(loaded_panes, panes);
    }

    #[test]
    fn saving_a_layout_replaces_the_previous_one_atomically() {
        let mut store = Store::in_memory().unwrap();
        store
            .save_layout(
                &[TabRecord {
                    id: "t1".into(),
                    title: "one".into(),
                    sort_order: 0,
                }],
                &[],
            )
            .unwrap();
        store
            .save_layout(
                &[TabRecord {
                    id: "t2".into(),
                    title: "two".into(),
                    sort_order: 0,
                }],
                &[],
            )
            .unwrap();
        let (tabs, _) = store.load_layout().unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, "t2");
    }

    #[test]
    fn deleting_a_tab_cascades_to_panes() {
        let mut store = Store::in_memory().unwrap();
        store
            .save_layout(
                &[TabRecord {
                    id: "t1".into(),
                    title: "one".into(),
                    sort_order: 0,
                }],
                &[PaneRecord {
                    id: "p1".into(),
                    tab_id: "t1".into(),
                    parent_id: None,
                    direction: None,
                    ratio: 0.5,
                    host_id: None,
                    session_id: None,
                    sort_order: 0,
                }],
            )
            .unwrap();
        store
            .conn
            .execute("DELETE FROM tab WHERE id = 't1'", [])
            .unwrap();
        let (_, panes) = store.load_layout().unwrap();
        assert!(panes.is_empty(), "panes must be removed with their tab");
    }

    #[test]
    fn sessions_roundtrip_and_delete() {
        let mut store = Store::in_memory().unwrap();
        let session = SessionRecord {
            id: "s1".into(),
            host_id: "a".into(),
            provider: "tmux".into(),
            provider_name: "st_a_p1".into(),
            cwd: Some("/var/log".into()),
            status: "detached".into(),
            last_active_at: 42,
        };
        store.upsert_session(&session).unwrap();
        assert_eq!(store.session("s1").unwrap(), Some(session.clone()));
        store.delete_session("s1").unwrap();
        assert!(store.session("s1").unwrap().is_none());
    }

    #[test]
    fn a_corrupt_database_is_backed_up_and_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        std::fs::write(&path, b"this is not a sqlite database at all").unwrap();

        match Store::open(&path).unwrap() {
            Opened::Recovered {
                store,
                backup,
                reason,
            } => {
                assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
                assert!(backup.exists(), "the corrupt file must be preserved");
                assert!(!reason.is_empty());
            }
            Opened::Clean(_) => panic!("corruption must be detected"),
        }
    }

    #[test]
    fn reopening_a_healthy_database_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        {
            let Opened::Clean(mut store) = Store::open(&path).unwrap() else {
                panic!("fresh database must open cleanly")
            };
            store.upsert_host(&host("a")).unwrap();
        }
        match Store::open(&path).unwrap() {
            Opened::Clean(store) => assert_eq!(store.hosts().unwrap().len(), 1),
            Opened::Recovered { .. } => panic!("healthy database must not be recovered"),
        }
    }
}

//! Single-owner SQLite actor for permanent Coordinator authority state.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

use crate::DATABASE_SCHEMA_VERSION;

const FINAL_SCHEMA: &str = include_str!("schema.sql");

type ActorResult = Result<Box<dyn Any + Send>, DatabaseError>;
type Work = Box<dyn FnOnce(&mut Connection) -> ActorResult + Send>;

enum Message {
    Work(Work),
    Stop,
}

#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Message>,
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database actor is unavailable")]
    ActorUnavailable,
    #[error("database schema {found} is newer than daemon {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("database returned an unexpected result type")]
    ResultType,
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database actor failed to start: {0}")]
    Startup(String),
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref().to_owned();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| DatabaseError::Startup(error.to_string()))?;
        }
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("devcoordinator2-sqlite".to_owned())
            .spawn(move || actor(path, receiver, ready_sender))
            .map_err(|error| DatabaseError::Startup(error.to_string()))?;
        ready_receiver
            .recv()
            .map_err(|_| DatabaseError::ActorUnavailable)??;
        Ok(Self { sender })
    }

    pub fn call<T, F>(&self, work: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseError> + Send + 'static,
    {
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        self.sender
            .send(Message::Work(Box::new(move |connection| {
                let result = work(connection).map(|value| Box::new(value) as Box<dyn Any + Send>);
                let _ = result_sender.send(result);
                Ok(Box::new(()) as Box<dyn Any + Send>)
            })))
            .map_err(|_| DatabaseError::ActorUnavailable)?;
        result_receiver
            .recv()
            .map_err(|_| DatabaseError::ActorUnavailable)??
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| DatabaseError::ResultType)
    }

    pub fn transaction<T, F>(&self, work: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, DatabaseError> + Send + 'static,
    {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let result = work(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
    }

    pub fn backup(&self, destination: PathBuf) -> Result<(), DatabaseError> {
        self.call(move |connection| {
            if destination.exists() {
                return Err(DatabaseError::Startup(format!(
                    "backup destination already exists: {}",
                    destination.display()
                )));
            }
            connection.backup(rusqlite::MAIN_DB, &destination, None)?;
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&destination, permissions)
                .map_err(|error| DatabaseError::Startup(error.to_string()))?;
            Ok(())
        })
    }

    pub fn close(&self) -> Result<(), DatabaseError> {
        self.sender
            .send(Message::Stop)
            .map_err(|_| DatabaseError::ActorUnavailable)
    }
}

fn actor(
    path: PathBuf,
    receiver: mpsc::Receiver<Message>,
    ready: mpsc::SyncSender<Result<(), DatabaseError>>,
) {
    let mut connection = match open_connection(&path) {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Work(work) => {
                let _ = work(&mut connection);
            }
            Message::Stop => break,
        }
    }
}

fn open_connection(path: &Path) -> Result<Connection, DatabaseError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE;
    let connection = Connection::open_with_flags(path, flags)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
    )?;
    let current = connection
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    if let Some(found) = current
        && found > DATABASE_SCHEMA_VERSION
    {
        return Err(DatabaseError::SchemaTooNew {
            found,
            supported: DATABASE_SCHEMA_VERSION,
        });
    }
    connection.execute_batch(FINAL_SCHEMA)?;
    ensure_column(
        &connection,
        "deployments",
        "public",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(&connection, "deployments", "domain_override", "TEXT")?;
    relax_observed_state_checks(&connection)?;
    ensure_column(
        &connection,
        "tasks",
        "elaboration_needed",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(&connection, "repositories", "archived_at", "TEXT")?;
    ensure_column(&connection, "repositories", "archived_by_uid", "INTEGER")?;
    ensure_column(&connection, "repositories", "archive_note", "TEXT")?;
    ensure_column(
        &connection,
        "repositories",
        "merged_into_repository_id",
        "TEXT",
    )?;
    connection.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?1)",
        [DATABASE_SCHEMA_VERSION.to_string()],
    )?;
    Ok(connection)
}

fn relax_observed_state_checks(connection: &Connection) -> Result<(), DatabaseError> {
    let sql = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='observed_deployments'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    if sql
        .as_deref()
        .is_none_or(|value| value.contains("'stopped'"))
    {
        return Ok(());
    }
    connection.execute_batch(
        "PRAGMA foreign_keys=OFF;
         PRAGMA legacy_alter_table=ON;
         ALTER TABLE observed_deployments RENAME TO observed_deployments_v6;
         ALTER TABLE observed_containers RENAME TO observed_containers_v6;
         CREATE TABLE observed_deployments (
           observed_deployment_id TEXT PRIMARY KEY,
           repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
           name TEXT NOT NULL,
           native_project TEXT NOT NULL UNIQUE,
           state TEXT NOT NULL CHECK(state IN ('running','degraded','stopped','failed')),
           health TEXT NOT NULL CHECK(health IN ('healthy','unhealthy','unknown')),
           source TEXT NOT NULL,
           evidence_json TEXT NOT NULL,
           observed_at TEXT NOT NULL,
           imported_at TEXT NOT NULL,
           UNIQUE(repository_id,native_project)
         );
         CREATE TABLE observed_containers (
           container_id TEXT PRIMARY KEY,
           observed_deployment_id TEXT NOT NULL REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
           repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
           name TEXT NOT NULL,
           image TEXT NOT NULL,
           compose_service TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('running','stopped','failed','starting','missing')),
           status TEXT NOT NULL,
           health TEXT NOT NULL CHECK(health IN ('healthy','unhealthy','starting','unknown','none')),
           observed_at TEXT NOT NULL
         );
         INSERT INTO observed_deployments SELECT * FROM observed_deployments_v6;
         INSERT INTO observed_containers SELECT * FROM observed_containers_v6;
         DROP TABLE observed_containers_v6;
         DROP TABLE observed_deployments_v6;
         CREATE INDEX IF NOT EXISTS observed_containers_deployment ON observed_containers(observed_deployment_id);
         CREATE INDEX IF NOT EXISTS observed_containers_repository ON observed_containers(repository_id);
         PRAGMA legacy_alter_table=OFF;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), DatabaseError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|candidate| candidate == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_schema_fifteen_and_serializes_calls() {
        let temporary = tempdir().expect("tempdir");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("open");
        let version: String = database
            .call(|connection| {
                Ok(connection.query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .expect("version");
        assert_eq!(version, "15");
        let tables: Vec<String> = database
            .call(|connection| {
                let mut statement = connection.prepare(
                    "SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name",
                )?;
                Ok(statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .expect("tables");
        for required in [
            "repositories",
            "deployments",
            "tasks",
            "decisions",
            "visual_feedback",
        ] {
            assert!(
                tables.iter().any(|table| table == required),
                "missing {required}"
            );
        }
        database.close().expect("close");
    }

    #[test]
    fn rejects_a_newer_schema() {
        let temporary = tempdir().expect("tempdir");
        let path = temporary.path().join("authority.sqlite3");
        let connection = Connection::open(&path).expect("fixture");
        connection
            .execute_batch(
                "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);\n\
                 INSERT INTO meta VALUES('schema_version','16');",
            )
            .expect("fixture schema");
        drop(connection);
        assert!(matches!(
            Database::open(path),
            Err(DatabaseError::SchemaTooNew {
                found: 16,
                supported: 15
            })
        ));
    }

    #[test]
    fn backup_is_new_and_private() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().expect("tempdir");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("open");
        let backup = temporary.path().join("authority.backup.sqlite3");
        database.backup(backup.clone()).expect("backup");
        assert_eq!(
            std::fs::metadata(&backup)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(database.backup(backup).is_err());
    }

    #[test]
    fn upgrades_schema_six_observed_states_without_losing_rows() {
        let temporary = tempdir().expect("tempdir");
        let path = temporary.path().join("authority.sqlite3");
        let connection = Connection::open(&path).expect("fixture");
        connection
            .execute_batch(
                "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO meta VALUES('schema_version','6');
                 CREATE TABLE repositories(
                   repository_id TEXT PRIMARY KEY,root_path TEXT NOT NULL UNIQUE,
                   display_name TEXT NOT NULL,registered_at TEXT NOT NULL,
                   registered_by_uid INTEGER NOT NULL,last_seen_at TEXT NOT NULL);
                 INSERT INTO repositories VALUES('r1','/x','x','t',1,'t');
                 CREATE TABLE observed_deployments(
                   observed_deployment_id TEXT PRIMARY KEY,
                   repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
                   name TEXT NOT NULL,native_project TEXT NOT NULL UNIQUE,
                   state TEXT NOT NULL CHECK(state IN ('running','degraded')),
                   health TEXT NOT NULL CHECK(health IN ('healthy','unhealthy','unknown')),
                   source TEXT NOT NULL,evidence_json TEXT NOT NULL,
                   observed_at TEXT NOT NULL,imported_at TEXT NOT NULL,
                   UNIQUE(repository_id,native_project));
                 CREATE TABLE observed_containers(
                   container_id TEXT PRIMARY KEY,
                   observed_deployment_id TEXT NOT NULL REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
                   repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
                   name TEXT NOT NULL,image TEXT NOT NULL,compose_service TEXT NOT NULL,
                   state TEXT NOT NULL CHECK(state IN ('running')),
                   status TEXT NOT NULL,
                   health TEXT NOT NULL CHECK(health IN ('healthy','unhealthy','starting','unknown')),
                   observed_at TEXT NOT NULL);
                 CREATE TABLE observed_routes(
                   domain TEXT PRIMARY KEY,
                   observed_deployment_id TEXT NOT NULL REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
                   component TEXT NOT NULL,port INTEGER NOT NULL,public INTEGER NOT NULL,
                   evidence_json TEXT NOT NULL,observed_at TEXT NOT NULL);
                 INSERT INTO observed_deployments VALUES('d1','r1','one','native','running','healthy','s','{}','t','t');
                 INSERT INTO observed_containers VALUES('c1','d1','r1','one','image','svc','running','up','healthy','t');
                 INSERT INTO observed_routes VALUES('one','d1','svc',1234,0,'{}','t');",
            )
            .expect("schema-six fixture");
        drop(connection);

        let database = Database::open(&path).expect("upgrade");
        database
            .transaction(|transaction| {
                transaction.execute(
                    "UPDATE observed_deployments SET state='stopped' WHERE observed_deployment_id='d1'",
                    [],
                )?;
                transaction.execute(
                    "UPDATE observed_containers SET state='stopped',health='none' WHERE container_id='c1'",
                    [],
                )?;
                Ok(())
            })
            .expect("new states");
        let routes: i64 = database
            .call(|connection| {
                Ok(connection
                    .query_row("SELECT count(*) FROM observed_routes", [], |row| row.get(0))?)
            })
            .expect("route count");
        assert_eq!(routes, 1);
    }
}

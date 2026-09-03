//! Typed in-process access to the Rust governed-log store.

use std::path::{Path, PathBuf};

use devcoordinator2_api::results::{Retention, RetentionDefaults};
use devcoordinator2_api::{ErrorCode, ProtocolError};
use devcoordinator2_executor_core::log_query::{
    LogQueryError, LogQueryOperation, LogQueryOptions, LogQueryRequest, LogQueryResult,
    LogQuerySelector, execute_log_query,
};
use rusqlite::OptionalExtension;
use serde_json::{Map, Value};

use crate::access::Caller;
use crate::database::{Database, DatabaseError};
use crate::repository::Registry;

pub const DEFAULT_MAX_AGE_SECONDS: u32 = 86_400;
pub const DEFAULT_CASE_DEPTH: u16 = 3;

#[derive(Clone)]
pub struct TestLogService {
    database: Database,
    registry: Registry,
}

impl TestLogService {
    pub fn new(database: Database, registry: Registry) -> Self {
        Self { database, registry }
    }

    pub fn retention(&self) -> Result<Retention, ProtocolError> {
        self.database
            .call(|connection| {
                connection
                    .query_row(
                        "SELECT max_age_seconds,case_depth,updated_at,updated_by,last_cleanup_at,last_cleanup_error_code FROM test_log_retention_state WHERE singleton=1",
                        [],
                        |row| {
                            Ok(Retention {
                                max_age_seconds: row.get(0)?,
                                case_depth: row.get(1)?,
                                defaults: RetentionDefaults {
                                    max_age_seconds: DEFAULT_MAX_AGE_SECONDS,
                                    case_depth: DEFAULT_CASE_DEPTH,
                                },
                                updated_at: row.get(2)?,
                                updated_by: row.get(3)?,
                                last_cleanup_at: row.get(4)?,
                                last_cleanup_error_code: row.get(5)?,
                                cleanup_requested: None,
                            })
                        },
                    )
                    .optional()
                    .map_err(DatabaseError::from)
            })
            .map_err(database_error)?
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::InternalError,
                    "log retention state is unavailable",
                )
            })
    }

    pub fn set_retention(
        &self,
        max_age_seconds: u32,
        case_depth: u16,
        actor: &str,
        now: &str,
    ) -> Result<Retention, ProtocolError> {
        let before = self.retention()?;
        if before.max_age_seconds != max_age_seconds || before.case_depth != case_depth {
            let actor = actor.to_owned();
            let now = now.to_owned();
            self.database
                .transaction(move |transaction| {
                    transaction.execute(
                        "UPDATE test_log_retention_state SET max_age_seconds=?1,case_depth=?2,updated_at=?3,updated_by=?4 WHERE singleton=1",
                        rusqlite::params![max_age_seconds, case_depth, now, actor],
                    )?;
                    transaction.execute(
                        "INSERT INTO test_log_retention_events(at,actor,previous_max_age_seconds,max_age_seconds,previous_case_depth,case_depth) VALUES(?1,?2,?3,?4,?5,?6)",
                        rusqlite::params![
                            now,
                            actor,
                            before.max_age_seconds,
                            max_age_seconds,
                            before.case_depth,
                            case_depth,
                        ],
                    )?;
                    Ok(())
                })
                .map_err(database_error)?;
        }
        let mut result = self.retention()?;
        result.cleanup_requested = Some(true);
        Ok(result)
    }

    pub fn query(
        &self,
        operation: LogQueryOperation,
        path: &Path,
        public_params: Value,
        caller: &Caller,
    ) -> Result<Value, ProtocolError> {
        let (worktree, repository_id) = self.resolve(path, caller)?;
        let mut object = public_params.as_object().cloned().ok_or_else(|| {
            ProtocolError::new(ErrorCode::ParamsInvalid, "log parameters must be an object")
        })?;
        object.remove("path");
        let mut selector = Map::new();
        let mut options = Map::new();
        for key in ["run_id", "check", "phase", "case", "stream"] {
            if let Some(value) = object.remove(key)
                && !value.is_null()
            {
                selector.insert(key.to_owned(), value);
            }
        }
        if let Some(value) = object.remove("cursor")
            && !value.is_null()
        {
            options.insert("cursor".to_owned(), value);
        }
        for (key, value) in object {
            if !value.is_null() {
                options.insert(key, value);
            }
        }
        if operation == LogQueryOperation::Catalog {
            let retention = self.retention()?;
            options.insert(
                "max_age_seconds".to_owned(),
                Value::from(retention.max_age_seconds),
            );
            options.insert("case_depth".to_owned(), Value::from(retention.case_depth));
        }
        let selector: LogQuerySelector = serde_json::from_value(Value::Object(selector))
            .map_err(|error| invalid_query(error.to_string()))?;
        let options: LogQueryOptions = serde_json::from_value(Value::Object(options))
            .map_err(|error| invalid_query(error.to_string()))?;
        let result = execute_log_query(
            &worktree,
            LogQueryRequest {
                schema: 2,
                operation,
                repository_id,
                selector,
                options,
            },
        )
        .map_err(log_error)?;
        encode_typed_result(operation, result)
    }

    fn resolve(&self, path: &Path, caller: &Caller) -> Result<(PathBuf, String), ProtocolError> {
        if !path.is_absolute() {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "path must be absolute",
            ));
        }
        if caller.identity.is_some() {
            let path = path.to_string_lossy().to_string();
            return self
                .database
                .call(move |connection| {
                    connection
                        .query_row(
                            "SELECT worktree_path,repository_id FROM worktrees WHERE worktree_path=?1",
                            [&path],
                            |row| Ok((PathBuf::from(row.get::<_, String>(0)?), row.get(1)?)),
                        )
                        .optional()
                        .map_err(DatabaseError::from)
                })
                .map_err(database_error)?
                .ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::RepositoryNotFound,
                        "no registered worktree matches this request",
                    )
                });
        }
        let status = self
            .registry
            .repository_status(path, Some((caller.uid, caller.gid)))?;
        let worktree = status
            .worktrees
            .iter()
            .find(|worktree| worktree.worktree_id == status.worktree_id)
            .map(|worktree| PathBuf::from(&worktree.worktree_path))
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::RepositoryNotFound,
                    "the resolved worktree is not registered",
                )
            })?;
        Ok((worktree, status.repository_id))
    }
}

fn encode_typed_result(
    operation: LogQueryOperation,
    result: LogQueryResult,
) -> Result<Value, ProtocolError> {
    let value = serde_json::to_value(result).map_err(|error| {
        ProtocolError::new(ErrorCode::TestLogUnavailable, "cannot encode log result")
            .with_detail(error.to_string())
    })?;
    match operation {
        LogQueryOperation::Catalog => round_trip::<devcoordinator2_api::results::LogCatalog>(value),
        LogQueryOperation::Tail | LogQueryOperation::Range => {
            round_trip::<devcoordinator2_api::results::LogContent>(value)
        }
        LogQueryOperation::Search => round_trip::<devcoordinator2_api::results::LogSearch>(value),
        LogQueryOperation::FailureContext => {
            round_trip::<devcoordinator2_api::results::LogFailureContext>(value)
        }
    }
}

fn round_trip<T>(value: Value) -> Result<Value, ProtocolError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(value).map_err(|error| {
        ProtocolError::new(
            ErrorCode::TestLogUnavailable,
            "the governed log result did not match its public contract",
        )
        .with_detail(error.to_string())
    })?;
    serde_json::to_value(typed).map_err(|error| {
        ProtocolError::new(ErrorCode::TestLogUnavailable, "cannot encode log result")
            .with_detail(error.to_string())
    })
}

fn invalid_query(detail: String) -> ProtocolError {
    ProtocolError::new(ErrorCode::ParamsInvalid, "The log request is invalid.").with_detail(detail)
}

fn log_error(error: LogQueryError) -> ProtocolError {
    let (code, message) = match error {
        LogQueryError::ArgsInvalid => (ErrorCode::ParamsInvalid, "The log request is invalid."),
        LogQueryError::LogNotFound => (
            ErrorCode::LogNotFound,
            "The selected governed test log does not exist.",
        ),
        LogQueryError::LogExpired => (
            ErrorCode::LogExpired,
            "The selected governed test log has expired.",
        ),
        LogQueryError::CursorStale => (
            ErrorCode::CursorStale,
            "The log cursor no longer identifies this stream snapshot.",
        ),
        LogQueryError::StoreMalformed
        | LogQueryError::Unavailable
        | LogQueryError::MaintenanceBusy => (
            ErrorCode::TestLogUnavailable,
            "Governed test logs are unavailable.",
        ),
    };
    ProtocolError::new(code, message)
}

fn database_error(error: DatabaseError) -> ProtocolError {
    match error {
        DatabaseError::Domain(error) => error,
        other => ProtocolError::new(ErrorCode::InternalError, "log database operation failed")
            .with_detail(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn retention_changes_are_persistent_and_append_only() {
        let temporary = tempdir().expect("tempdir");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("db");
        let service = TestLogService::new(database.clone(), Registry::new(database.clone()));
        let initial = service.retention().expect("retention");
        assert_eq!(initial.max_age_seconds, DEFAULT_MAX_AGE_SECONDS);
        let changed = service
            .set_retention(1234, 7, "uid:1000", "2026-09-03T12:00:00Z")
            .expect("change");
        assert_eq!((changed.max_age_seconds, changed.case_depth), (1234, 7));
        assert_eq!(changed.cleanup_requested, Some(true));
        let events = database
            .call(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM test_log_retention_events",
                    [],
                    |row| row.get::<_, u32>(0),
                )?)
            })
            .expect("events");
        assert_eq!(events, 1);
    }

    #[test]
    fn core_errors_map_to_stable_public_codes() {
        assert_eq!(
            log_error(LogQueryError::CursorStale).code,
            ErrorCode::CursorStale
        );
        assert_eq!(
            log_error(LogQueryError::StoreMalformed).code,
            ErrorCode::TestLogUnavailable
        );
    }

    #[test]
    fn public_queries_resolve_only_an_exact_registered_worktree() {
        let temporary = tempdir().expect("tempdir");
        let worktree = temporary.path().join("repo");
        std::fs::create_dir(&worktree).expect("worktree");
        let database = Database::open(temporary.path().join("authority.sqlite3")).expect("db");
        let stored = worktree.display().to_string();
        database
            .transaction(move |transaction| {
                transaction.execute(
                    "INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1111111111111111',?1,'repo','t',1,'t')",
                    [&stored],
                )?;
                transaction.execute(
                    "INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES('w1111111111111111','r1111111111111111',?1,'t','t')",
                    [&stored],
                )?;
                Ok(())
            })
            .expect("fixture");
        let service = TestLogService::new(database.clone(), Registry::new(database));
        let caller = Caller {
            pid: 1,
            uid: 999,
            gid: 999,
            client_kind: devcoordinator2_api::ClientKind::Edge,
            client_session: None,
            identity: Some("reader@example.test".into()),
        };
        let error = service
            .query(
                LogQueryOperation::Catalog,
                &worktree,
                serde_json::json!({"path":worktree,"limit":100}),
                &caller,
            )
            .expect_err("empty retained store");
        assert_eq!(error.code, ErrorCode::LogNotFound);

        let nested = worktree.join("nested");
        let denied = service
            .query(
                LogQueryOperation::Catalog,
                &nested,
                serde_json::json!({"path":nested,"limit":100}),
                &caller,
            )
            .expect_err("public path must be exact");
        assert_eq!(denied.code, ErrorCode::RepositoryNotFound);
    }
}

//! Complete, checksummed, atomically published edge route document.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use rustix::fs::{AtFlags, Mode, OFlags, chmod, open, openat, renameat, unlinkat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::database::{Database, DatabaseError};
use crate::ids;

pub const ROUTE_SCHEMA: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteAccess {
    pub owners: Vec<String>,
    pub grants: Vec<RouteGrant>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteGrant {
    pub identity: String,
    pub deployment_id: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub deployment_id: String,
    pub component: String,
    pub label: String,
    pub domain: String,
    pub port: u16,
    pub scheme: String,
    pub auth: String,
    pub generation: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDocument {
    pub schema: u8,
    pub payload_sha256: String,
    pub generation: u64,
    pub published_at: String,
    pub domain: String,
    pub routes: Vec<Route>,
    pub access: RouteAccess,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RoutePayload<'a> {
    generation: u64,
    published_at: &'a str,
    domain: &'a str,
    routes: &'a [Route],
    access: &'a RouteAccess,
}

#[derive(Debug, Error)]
pub enum RouteError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("route publication failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("route serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("route publication path is invalid")]
    InvalidPath,
    #[error("cannot create route temporary identity: {0}")]
    Id(#[from] ids::IdError),
}

pub fn publish(
    database: &Database,
    path: &Path,
    base_domain: &str,
    access: Option<RouteAccess>,
    now: &str,
) -> Result<RouteDocument, RouteError> {
    let base_domain_owned = base_domain.to_owned();
    let (generation, routes, stored_access) = database.call(move |connection| {
        let current: Option<String> = connection
            .query_row(
                "SELECT value FROM meta WHERE key='route_generation'",
                [],
                |row| row.get(0),
            )
            .ok();
        let generation = current
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        connection.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('route_generation',?1)",
            [generation.to_string()],
        )?;
        let mut routes = Vec::new();
        {
            let mut statement = connection.prepare(
                "SELECT r.domain,r.deployment_id,r.component,r.port,r.generation,d.public FROM domain_routes r JOIN deployments d ON d.deployment_id=r.deployment_id WHERE r.port IS NOT NULL ORDER BY r.domain",
            )?;
            for row in statement.query_map([], |row| {
                route_from_row(row, &base_domain_owned, false)
            })? {
                routes.push(row?);
            }
        }
        {
            let mut statement = connection.prepare(
                "SELECT domain,observed_deployment_id,component,port,NULL,public FROM observed_routes WHERE port IS NOT NULL ORDER BY domain",
            )?;
            for row in statement.query_map([], |row| {
                route_from_row(row, &base_domain_owned, true)
            })? {
                routes.push(row?);
            }
        }
        routes.sort_by(|left, right| left.label.cmp(&right.label));
        let stored_access = match access {
            Some(access) => access,
            None => {
                let owners = {
                    let mut statement = connection.prepare(
                        "SELECT email FROM users WHERE administrator=1 ORDER BY email",
                    )?;
                    statement
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                let grants = {
                    let mut statement = connection.prepare(
                        "SELECT u.email,g.deployment_id,g.role FROM grants g JOIN users u ON u.user_id=g.user_id ORDER BY u.email,g.deployment_id",
                    )?;
                    statement
                        .query_map([], |row| {
                            Ok(RouteGrant {
                                identity: row.get(0)?,
                                deployment_id: row.get(1)?,
                                role: row.get(2)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?
                };
                RouteAccess { owners, grants }
            }
        };
        Ok((generation, routes, stored_access))
    })?;
    let payload = RoutePayload {
        generation,
        published_at: now,
        domain: base_domain,
        routes: &routes,
        access: &stored_access,
    };
    let canonical = serde_json::to_vec(&serde_json::to_value(&payload)?)?;
    let payload_sha256 = lower_hex(&Sha256::digest(&canonical));
    let document = RouteDocument {
        schema: ROUTE_SCHEMA,
        payload_sha256,
        generation,
        published_at: now.to_owned(),
        domain: base_domain.to_owned(),
        routes,
        access: stored_access,
    };
    atomic_publish(path, &serde_json::to_vec_pretty(&document)?)?;
    Ok(document)
}

fn route_from_row(
    row: &rusqlite::Row<'_>,
    base_domain: &str,
    _observed: bool,
) -> rusqlite::Result<Route> {
    let label: String = row.get(0)?;
    Ok(Route {
        deployment_id: row.get(1)?,
        component: row.get(2)?,
        domain: if base_domain.is_empty() {
            label.clone()
        } else {
            format!("{label}.{base_domain}")
        },
        label,
        port: row.get(3)?,
        scheme: "http".into(),
        auth: if row.get::<_, i64>(5)? != 0 {
            "public".into()
        } else {
            "authenticated".into()
        },
        generation: row.get(4)?,
    })
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn atomic_publish(path: &Path, bytes: &[u8]) -> Result<(), RouteError> {
    let parent = path.parent().ok_or(RouteError::InvalidPath)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or(RouteError::InvalidPath)?;
    std::fs::create_dir_all(parent)?;
    chmod(parent, Mode::from_raw_mode(0o755)).map_err(std::io::Error::from)?;
    let directory = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(std::io::Error::from)?;
    let temporary = format!(".routes-{}", ids::bug_id()?);
    let result = (|| -> Result<(), RouteError> {
        let descriptor = openat(
            &directory,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?;
        let mut file = File::from(descriptor);
        file.write_all(bytes)?;
        rustix::fs::fchmod(&file, Mode::from_raw_mode(0o644)).map_err(std::io::Error::from)?;
        file.sync_all()?;
        renameat(&directory, temporary.as_str(), &directory, name).map_err(std::io::Error::from)?;
        directory.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = unlinkat(&directory, temporary.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn publication_is_complete_checksummed_and_monotonic() {
        let temporary = tempdir().unwrap();
        let database = Database::open(temporary.path().join("authority.sqlite3")).unwrap();
        database
            .transaction(|transaction| {
                transaction.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1','/x','x','t',1,'t')", [])?;
                transaction.execute("INSERT INTO worktrees VALUES('w1','r1','/x','t','t')", [])?;
                transaction.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,domain,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at,public) VALUES('d1','r1','w1','web','worktree','app','f','{}','running','t',1,'other','t',0)", [])?;
                transaction.execute("INSERT INTO domain_routes VALUES('app','d1','api',20001,1,'t')", [])?;
                transaction.execute("INSERT INTO domain_routes VALUES('idle','d1','api',NULL,1,'t')", [])?;
                Ok(())
            })
            .unwrap();
        let path = temporary.path().join("public/routes.json");
        let first = publish(
            &database,
            &path,
            "example.test",
            None,
            "2026-09-03T12:00:00Z",
        )
        .unwrap();
        assert_eq!(first.generation, 1);
        assert_eq!(first.routes.len(), 1);
        assert_eq!(first.routes[0].domain, "app.example.test");
        let value = serde_json::to_value(&first).unwrap();
        let payload = serde_json::json!({
            "generation": value["generation"],
            "published_at": value["published_at"],
            "domain": value["domain"],
            "routes": value["routes"],
            "access": value["access"],
        });
        assert_eq!(
            first.payload_sha256,
            lower_hex(&Sha256::digest(serde_json::to_vec(&payload).unwrap()))
        );
        let second = publish(
            &database,
            &path,
            "example.test",
            None,
            "2026-09-03T12:01:00Z",
        )
        .unwrap();
        assert_eq!(second.generation, 2);
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("payload_sha256")
        );
    }
}

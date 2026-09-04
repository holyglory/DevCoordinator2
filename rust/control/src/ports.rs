//! Transactionally unique host-port leases.

use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

use thiserror::Error;

use crate::database::{Database, DatabaseError};

pub trait PortAvailability: Send + Sync + 'static {
    fn bindable(&self, port: u16) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostPortAvailability;

impl PortAvailability for HostPortAvailability {
    fn bindable(&self, port: u16) -> bool {
        host_bindable(port)
    }
}

#[derive(Debug, Error)]
pub enum PortError {
    #[error("no free port in {0}-{1}")]
    Exhausted(u16, u16),
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

pub fn lease(
    database: &Database,
    port_range: (u16, u16),
    deployment_id: &str,
    component: &str,
    generation: u32,
    now: &str,
) -> Result<u16, PortError> {
    lease_with_availability(
        database,
        port_range,
        deployment_id,
        component,
        generation,
        now,
        &HostPortAvailability,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn lease_with_availability(
    database: &Database,
    port_range: (u16, u16),
    deployment_id: &str,
    component: &str,
    generation: u32,
    now: &str,
    availability: &dyn PortAvailability,
) -> Result<u16, PortError> {
    let available = (port_range.0..=port_range.1)
        .filter(|port| availability.bindable(*port))
        .collect::<HashSet<_>>();
    let deployment_id = deployment_id.to_owned();
    let component = component.to_owned();
    let now = now.to_owned();
    database
        .transaction(move |transaction| {
            let mut statement = transaction.prepare("SELECT port FROM port_assignments")?;
            let taken = statement
                .query_map([], |row| row.get::<_, u16>(0))?
                .collect::<Result<HashSet<_>, _>>()?;
            for port in port_range.0..=port_range.1 {
                if taken.contains(&port) || !available.contains(&port) {
                    continue;
                }
                transaction.execute(
                    "INSERT INTO port_assignments(port,deployment_id,component,generation,assigned_at) VALUES(?1,?2,?3,?4,?5)",
                    rusqlite::params![port, deployment_id, component, generation, now],
                )?;
                return Ok(port);
            }
            Err(DatabaseError::Domain(devcoordinator2_api::ProtocolError::new(
                devcoordinator2_api::ErrorCode::Busy,
                format!("no free port in {}-{}", port_range.0, port_range.1),
            )))
        })
        .map_err(|error| match error {
            DatabaseError::Domain(_) => PortError::Exhausted(port_range.0, port_range.1),
            other => PortError::Database(other),
        })
}

pub fn release(
    database: &Database,
    deployment_id: &str,
    generation: Option<u32>,
    component: Option<&str>,
) -> Result<(), PortError> {
    let deployment_id = deployment_id.to_owned();
    let component = component.map(str::to_owned);
    database
        .transaction(move |transaction| {
            match (generation, component) {
                (Some(generation), Some(component)) => {
                    transaction.execute(
                        "DELETE FROM port_assignments WHERE deployment_id=?1 AND generation=?2 AND component=?3",
                        rusqlite::params![deployment_id, generation, component],
                    )?;
                }
                (Some(generation), None) => {
                    transaction.execute(
                        "DELETE FROM port_assignments WHERE deployment_id=?1 AND generation=?2",
                        rusqlite::params![deployment_id, generation],
                    )?;
                }
                (None, Some(component)) => {
                    transaction.execute(
                        "DELETE FROM port_assignments WHERE deployment_id=?1 AND component=?2",
                        rusqlite::params![deployment_id, component],
                    )?;
                }
                (None, None) => {
                    transaction.execute(
                        "DELETE FROM port_assignments WHERE deployment_id=?1",
                        [&deployment_id],
                    )?;
                }
            }
            Ok(())
        })
        .map_err(PortError::from)
}

pub fn assigned(
    database: &Database,
    deployment_id: &str,
    generation: u32,
) -> Result<BTreeMap<String, u16>, PortError> {
    let deployment_id = deployment_id.to_owned();
    database
        .call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT component,port FROM port_assignments WHERE deployment_id=?1 AND generation=?2",
            )?;
            Ok(statement
                .query_map(rusqlite::params![deployment_id, generation], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, u16>(1)?))
                })?
                .collect::<Result<BTreeMap<_, _>, _>>()?)
        })
        .map_err(PortError::from)
}

fn host_bindable(port: u16) -> bool {
    [Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED]
        .into_iter()
        .all(|address| TcpListener::bind(SocketAddrV4::new(address, port)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn database() -> (tempfile::TempDir, Database) {
        let temporary = tempdir().unwrap();
        let database = Database::open(temporary.path().join("authority.sqlite3")).unwrap();
        database
            .transaction(|transaction| {
                transaction.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r1','/x','x','t',1,'t')", [])?;
                transaction.execute("INSERT INTO worktrees VALUES('w1','r1','/x','t','t')", [])?;
                transaction.execute("INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,domain,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('d1','r1','w1','web','worktree','app','f','{}','running','t',1,'other','t')", [])?;
                Ok(())
            })
            .unwrap();
        (temporary, database)
    }

    #[test]
    fn skips_bound_ports_and_releases_exact_scope() {
        let (_temporary, database) = database();
        struct FixtureAvailability;
        impl PortAvailability for FixtureAvailability {
            fn bindable(&self, port: u16) -> bool {
                port != 40_000
            }
        }
        let first = lease_with_availability(
            &database,
            (40_000, 40_010),
            "d1",
            "api",
            1,
            "t",
            &FixtureAvailability,
        )
        .unwrap();
        let second = lease_with_availability(
            &database,
            (40_000, 40_010),
            "d1",
            "worker",
            1,
            "t",
            &FixtureAvailability,
        )
        .unwrap();
        assert_eq!((first, second), (40_001, 40_002));
        assert_eq!(assigned(&database, "d1", 1).unwrap().len(), 2);
        release(&database, "d1", Some(1), Some("api")).unwrap();
        assert_eq!(
            assigned(&database, "d1", 1).unwrap(),
            BTreeMap::from([("worker".into(), 40_002)])
        );
    }
}

//! Exact applied-generation snapshots and a runtime-observed stopped lease.
//! A database state alone never grants relocation of a deployment source.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RelocationComponent {
    pub name: String,
    pub kind: String,
    pub desired_state: String,
    pub state: String,
    pub generation: Option<u32>,
    pub binding_kind: Option<String>,
    pub binding_identity: Option<String>,
    #[serde(default)]
    pub container_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RelocationConsumer {
    pub deployment_id: String,
    pub worktree_id: String,
    pub current_generation: Option<u32>,
    pub previous_generation: Option<u32>,
    pub spec_fingerprint: String,
    pub specification: serde_json::Value,
    pub generation_paths: Vec<(u32, PathBuf)>,
    pub components: Vec<RelocationComponent>,
}

pub(crate) trait RelocationRuntime: Send + Sync + 'static {
    /// The returned lease prevents service mutations while the caller checks
    /// files and writes its durable pending relocation. Container IDs are filled
    /// only from the real runtime and become immutable proof for later retries.
    fn lease_stopped(
        &self,
        consumers: &mut [RelocationConsumer],
    ) -> Result<Box<dyn Send>, ProtocolError>;
}

impl Registry {
    pub(crate) fn install_relocation_runtime(
        &self,
        runtime: Arc<dyn RelocationRuntime>,
    ) -> Result<(), ProtocolError> {
        self.relocation_runtime
            .set(runtime)
            .map_err(|_| blocked("relocation runtime observation is already configured"))
    }

    pub(crate) fn ensure_execution_available(
        &self,
        repository_id: &str,
    ) -> Result<(), ProtocolError> {
        self.database.read(|connection|{
            if pending(connection,repository_id)? {return Err(blocked("source relocation is pending; stop remains available and execution must wait").into());}
            Ok(())
        }).map_err(db_error)
    }

    pub(super) fn lease_relocation_consumers(
        &self,
        consumers: &mut [RelocationConsumer],
        expected: Option<&[RelocationConsumer]>,
    ) -> Result<Box<dyn Send>, ProtocolError> {
        if consumers.is_empty() {
            return Ok(Box::new(()));
        }
        let runtime = self.relocation_runtime.get().ok_or_else(|| {
            blocked("stopped deployment relocation requires the configured runtime observer")
        })?;
        let lease = runtime.lease_stopped(consumers)?;
        if let Some(expected) = expected {
            if consumers != expected {
                return Err(blocked(
                    "applied generations or observed stopped bindings changed after relocation preparation",
                ));
            }
        }
        Ok(lease)
    }
}

pub(super) fn collect(
    connection: &Connection,
    repository_id: &str,
) -> Result<Vec<RelocationConsumer>, DatabaseError> {
    let rows = {
        let mut query=connection.prepare("SELECT deployment_id,worktree_id,current_generation,previous_generation,spec_fingerprint,spec_json,state FROM deployments WHERE repository_id=$1 AND (current_generation IS NOT NULL OR previous_generation IS NOT NULL OR state NOT IN ('removed','failed','stopped')) ORDER BY deployment_id")?;
        query
            .query_map([repository_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                    row.get::<_, Option<u32>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    if rows.len() > 64 {
        return Err(blocked("too many deployment consumers for one bounded relocation").into());
    }
    let mut consumers = Vec::new();
    for (
        deployment_id,
        worktree_id,
        current_generation,
        previous_generation,
        spec_fingerprint,
        specification,
        state,
    ) in rows
    {
        if state != "stopped" {
            return Err(blocked(
                "a deployment still runs; stop every source consumer before preparing relocation",
            )
            .into());
        }
        let components = {
            let mut query=connection.prepare("SELECT name,type,desired_state,state,generation,binding_kind,binding_identity FROM components WHERE deployment_id=$1 ORDER BY name")?;
            query
                .query_map([&deployment_id], |row| {
                    Ok(RelocationComponent {
                        name: row.get(0)?,
                        kind: row.get(1)?,
                        desired_state: row.get(2)?,
                        state: row.get(3)?,
                        generation: row.get(4)?,
                        binding_kind: row.get(5)?,
                        binding_identity: row.get(6)?,
                        container_ids: Vec::new(),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        if components.len() > 128
            || components.iter().any(|component| {
                !matches!(component.desired_state.as_str(), "stopped" | "removed")
                    || !matches!(
                        component.state.as_str(),
                        "stopped" | "missing" | "removed" | "completed"
                    )
            })
        {
            return Err(blocked(
                "every owned deployment component must be stopped before relocation",
            )
            .into());
        }
        let generation_paths = {
            let mut query=connection.prepare("SELECT number,path FROM generations WHERE deployment_id=$1 AND (number=$2 OR number=$3) ORDER BY number")?;
            query
                .query_map(
                    (&deployment_id, current_generation, previous_generation),
                    |row| {
                        Ok((
                            row.get::<_, u32>(0)?,
                            PathBuf::from(row.get::<_, String>(1)?),
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        for generation in [current_generation, previous_generation]
            .into_iter()
            .flatten()
        {
            if !generation_paths
                .iter()
                .any(|(number, _)| *number == generation)
            {
                return Err(blocked("an applied or rollback generation is unavailable").into());
            }
        }
        let specification = serde_json::from_str(&specification).map_err(|_| {
            DatabaseError::Domain(blocked("stored applied specification is invalid"))
        })?;
        consumers.push(RelocationConsumer {
            deployment_id,
            worktree_id,
            current_generation,
            previous_generation,
            spec_fingerprint,
            specification,
            generation_paths,
            components,
        });
    }
    Ok(consumers)
}

pub(super) fn assert_unchanged(
    connection: &Connection,
    repository_id: &str,
    expected: &[RelocationConsumer],
) -> Result<(), DatabaseError> {
    let current = collect(connection, repository_id)?;
    let mut expected = expected.to_vec();
    for consumer in &mut expected {
        for component in &mut consumer.components {
            component.container_ids.clear();
        }
    }
    if current != expected {
        return Err(blocked("deployment source bindings changed during relocation").into());
    }
    Ok(())
}

pub(super) fn retain(
    connection: &Connection,
    relocation_id: &str,
    consumers: &[RelocationConsumer],
    now: &str,
) -> Result<(), DatabaseError> {
    for consumer in consumers {
        connection.execute("INSERT INTO relocation_consumers(relocation_id,deployment_id,snapshot_json,state,observed_at) VALUES($1,$2,$3,'stopped',$4)",(relocation_id,&consumer.deployment_id,serde_json::to_string(consumer).map_err(|_|DatabaseError::Domain(blocked("stopped consumer proof cannot be retained")))?,now))?;
    }
    Ok(())
}

pub(super) fn rebind_generation_paths(
    connection: &Connection,
    receipt: &api::Receipt,
) -> Result<(), DatabaseError> {
    // `path` is the mutable runtime locator. Commit, dirty state, generation
    // number, creation time, provenance and every immutable receipt stay intact.
    connection.execute("UPDATE generations g SET path=$1||substring(g.path FROM length($2)+1) FROM deployments d WHERE g.deployment_id=d.deployment_id AND d.repository_id=$3 AND (g.path=$2 OR strpos(g.path,$2||'/')=1)",(&receipt.new_path,&receipt.old_path,&receipt.repository_id))?;
    Ok(())
}

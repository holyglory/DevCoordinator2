//! Deployment-bound moves remain unavailable until native runtime rebinding
//! and recovery have their own verified implementation. Stopped is still owned.
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

impl Registry {
    pub(crate) fn ensure_execution_available(
        &self,
        repository_id: &str,
    ) -> Result<(), ProtocolError> {
        self.database.read(|connection|{
            if pending(connection,repository_id)? {return Err(blocked("source relocation is pending; stop remains available and execution must wait").into());}
            Ok(())
        }).map_err(db_error)
    }
}

pub(super) fn assert_supported(
    observed: &[RelocationConsumer],
    expected: &[RelocationConsumer],
) -> Result<(), ProtocolError> {
    if !observed.is_empty() || !expected.is_empty() {
        return Err(blocked(
            "deployment-bound relocation is unavailable; preserve applied and rollback generation paths until verified runtime rebinding is implemented",
        ));
    }
    Ok(())
}

pub(super) fn collect(
    connection: &Connection,
    repository_id: &str,
) -> Result<Vec<RelocationConsumer>, DatabaseError> {
    let consumed: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM deployments WHERE repository_id=$1 AND (state<>'removed' OR current_generation IS NOT NULL OR previous_generation IS NOT NULL))",
        [repository_id],
        |row| row.get(0),
    )?;
    if consumed {
        return Err(blocked(
            "deployment-bound relocation is unavailable; preserve applied and rollback generation paths until verified runtime rebinding is implemented",
        ).into());
    }
    Ok(Vec::new())
}

pub(super) fn assert_unchanged(
    connection: &Connection,
    repository_id: &str,
    expected: &[RelocationConsumer],
) -> Result<(), DatabaseError> {
    let current = collect(connection, repository_id)?;
    assert_supported(&current, expected).map_err(DatabaseError::Domain)
}

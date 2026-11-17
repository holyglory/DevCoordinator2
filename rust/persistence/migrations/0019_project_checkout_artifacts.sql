-- Core registry IDs are the only execution-owned checkout identities.
ALTER TABLE project_checkouts ADD COLUMN registration_state TEXT NOT NULL DEFAULT 'unregistered' CHECK(registration_state IN('registered','unregistered'));
UPDATE project_checkouts c SET registration_state='registered',record_json=record_json||'{"registration_state":"registered"}'::jsonb WHERE EXISTS(SELECT 1 FROM worktrees w WHERE w.worktree_id=c.worktree_id AND w.repository_id=c.physical_repository_id);
CREATE TABLE project_checkout_aliases (
 project_id TEXT NOT NULL,alias_worktree_id TEXT NOT NULL,worktree_id TEXT NOT NULL,created_at TEXT NOT NULL,
 PRIMARY KEY(project_id,alias_worktree_id),
 FOREIGN KEY(project_id,worktree_id) REFERENCES project_checkouts(project_id,worktree_id) ON UPDATE CASCADE DEFERRABLE INITIALLY DEFERRED,
 CHECK(alias_worktree_id<>worktree_id)
);
CREATE TABLE project_checkout_artifacts (
 association_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,worktree_id TEXT NOT NULL,
 artifact_id TEXT NOT NULL REFERENCES storage_artifacts(artifact_id),
 revision BIGINT NOT NULL CHECK(revision>0),ownership TEXT NOT NULL CHECK(ownership IN('exclusive','shared')),
 bound_device TEXT NOT NULL,bound_inode TEXT NOT NULL,active BOOLEAN NOT NULL DEFAULT true,
 provenance TEXT NOT NULL DEFAULT 'explicit',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,updated_by TEXT NOT NULL,
 UNIQUE(project_id,worktree_id,artifact_id),
 FOREIGN KEY(project_id,worktree_id) REFERENCES project_checkouts(project_id,worktree_id) ON UPDATE CASCADE DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX project_checkout_artifacts_owner ON project_checkout_artifacts(project_id,worktree_id,association_id) WHERE active;
CREATE INDEX project_checkout_artifacts_other_owners ON project_checkout_artifacts(artifact_id,association_id) WHERE active;
CREATE TABLE project_artifact_requests (
 project_id TEXT NOT NULL REFERENCES projects(project_id),idempotency_key TEXT NOT NULL,request_json JSONB NOT NULL,
 association_id TEXT NOT NULL REFERENCES project_checkout_artifacts(association_id),created_at TEXT NOT NULL,
 PRIMARY KEY(project_id,idempotency_key)
);
UPDATE meta SET value='37' WHERE key='schema_version';

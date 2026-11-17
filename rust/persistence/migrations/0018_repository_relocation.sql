-- Existing IDs remain opaque permanent authority. Native identities are bound
-- on the first verified current registration; no caller Git config supplies ID.
CREATE TABLE repository_native_identities (
 repository_id TEXT PRIMARY KEY REFERENCES repositories(repository_id),
 root_identity TEXT NOT NULL UNIQUE, common_identity TEXT NOT NULL UNIQUE,
 bound_at TEXT NOT NULL
);
CREATE TABLE worktree_native_identities (
 worktree_id TEXT PRIMARY KEY REFERENCES worktrees(worktree_id),
 root_identity TEXT NOT NULL UNIQUE, git_identity TEXT NOT NULL UNIQUE,
 bound_at TEXT NOT NULL
);
CREATE TABLE repository_relocations (
 relocation_id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(project_id),
 repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
 worktree_id TEXT NOT NULL REFERENCES worktrees(worktree_id),
 old_path TEXT NOT NULL, new_path TEXT NOT NULL,
 moves_repository_root BOOLEAN NOT NULL,
 idempotency_key TEXT NOT NULL, request_json JSONB NOT NULL,
 fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
 native_json JSONB NOT NULL, state TEXT NOT NULL CHECK(state IN ('prepared','verified','committed','cancelled')),
 checkout_revision BIGINT NOT NULL, actor_uid BIGINT NOT NULL,
 prepared_at TEXT NOT NULL, verified_at TEXT, committed_at TEXT,
 UNIQUE(project_id,idempotency_key)
);
CREATE UNIQUE INDEX repository_relocations_pending_repository
 ON repository_relocations(repository_id) WHERE state IN ('prepared','verified');
CREATE UNIQUE INDEX repository_relocations_pending_destination
 ON repository_relocations(new_path) WHERE state IN ('prepared','verified');
CREATE TABLE registry_path_aliases (
 old_path TEXT PRIMARY KEY, repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
 worktree_id TEXT NOT NULL REFERENCES worktrees(worktree_id), new_path TEXT NOT NULL,
 relocation_id TEXT NOT NULL REFERENCES repository_relocations(relocation_id), created_at TEXT NOT NULL
);
CREATE INDEX registry_path_aliases_worktree ON registry_path_aliases(worktree_id);

UPDATE meta SET value='36' WHERE key='schema_version';

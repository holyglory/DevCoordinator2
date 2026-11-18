-- Preserve retired linked-worktree identity evidence while allowing a new
-- checkout to reuse an inode after its unreferenced native pin is retired.
CREATE TABLE worktree_native_identity_tombstones (
 worktree_id TEXT PRIMARY KEY REFERENCES worktrees(worktree_id),
 repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
 worktree_path TEXT NOT NULL,
 root_identity TEXT NOT NULL,
 git_identity TEXT NOT NULL,
 retired_at TEXT NOT NULL
);
CREATE INDEX worktree_native_identity_tombstones_root
 ON worktree_native_identity_tombstones(root_identity);
CREATE INDEX worktree_native_identity_tombstones_git
 ON worktree_native_identity_tombstones(git_identity);

UPDATE meta SET value='40' WHERE key='schema_version';

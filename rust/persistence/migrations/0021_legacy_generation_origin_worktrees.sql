-- Before registry-backed explicit candidates, generation provenance retained
-- the producer's path-derived worktree ID without necessarily registering it.
-- Preserve those exact IDs; native Git/filesystem identity is still verified
-- and pinned by the runtime before a recorded source can be executed.
LOCK TABLE worktrees IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE generations, generation_source_origins, deployments, repositories,
  registry_path_aliases, repository_relocations IN SHARE MODE;

DO $$
BEGIN
  IF EXISTS (
    SELECT o.worktree_id
    FROM generation_source_origins o JOIN deployments d USING (deployment_id)
    GROUP BY o.worktree_id HAVING count(DISTINCT d.repository_id) > 1
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'legacy candidate origin identity has conflicting repository ownership';
  END IF;

  IF EXISTS (
    SELECT o.path
    FROM generation_source_origins o JOIN deployments d USING (deployment_id)
    GROUP BY o.path HAVING count(DISTINCT (o.worktree_id, d.repository_id)) > 1
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'legacy candidate origin path has conflicting source identities';
  END IF;

  IF EXISTS (
    SELECT 1 FROM generation_source_origins o
    LEFT JOIN worktrees w USING (worktree_id)
    WHERE w.worktree_id IS NULL AND o.worktree_id !~ '^w[0-9a-f]{16}$'
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'unregistered candidate origin identity is not a legacy producer identity';
  END IF;

  IF EXISTS (
    SELECT o.worktree_id FROM generation_source_origins o
    WHERE NOT EXISTS (SELECT 1 FROM worktrees w WHERE w.worktree_id = o.worktree_id)
    GROUP BY o.worktree_id HAVING count(DISTINCT o.path) > 1
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'unregistered legacy candidate origin has ambiguous source paths';
  END IF;

  IF EXISTS (
    SELECT 1 FROM generation_source_origins o
    JOIN deployments d USING (deployment_id)
    JOIN worktrees w ON w.worktree_id = o.worktree_id
    WHERE w.repository_id <> d.repository_id OR (
      w.worktree_path <> o.path AND NOT EXISTS (
        SELECT 1 FROM registry_path_aliases a
        JOIN repository_relocations r USING (relocation_id)
        WHERE a.old_path = o.path AND a.worktree_id = o.worktree_id
          AND a.repository_id = d.repository_id
          AND r.state = 'committed' AND r.committed_at IS NOT NULL
          AND r.worktree_id = a.worktree_id AND r.repository_id = a.repository_id
          AND r.old_path = a.old_path AND r.new_path = a.new_path
      )
    )
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'candidate origin conflicts with its registered source or committed relocation';
  END IF;

  IF EXISTS (
    SELECT 1 FROM generation_source_origins o
    JOIN deployments d USING (deployment_id)
    JOIN worktrees w ON w.worktree_path = o.path
    WHERE w.worktree_id <> o.worktree_id OR w.repository_id <> d.repository_id
  ) OR EXISTS (
    SELECT 1 FROM generation_source_origins o
    JOIN deployments d USING (deployment_id)
    JOIN repositories r ON r.root_path = o.path
    WHERE r.repository_id <> d.repository_id
  ) THEN
    RAISE EXCEPTION USING ERRCODE = 'check_violation',
      MESSAGE = 'candidate origin path is already owned by another registered source';
  END IF;
END
$$;

INSERT INTO worktrees(worktree_id, repository_id, worktree_path, registered_at, last_seen_at)
SELECT o.worktree_id, d.repository_id, o.path, min(g.created_at), max(g.created_at)
FROM generation_source_origins o
JOIN generations g USING (deployment_id, number)
JOIN deployments d USING (deployment_id)
WHERE NOT EXISTS (SELECT 1 FROM worktrees w WHERE w.worktree_id = o.worktree_id)
GROUP BY o.worktree_id, d.repository_id, o.path;

UPDATE meta SET value = '39' WHERE key = 'schema_version';

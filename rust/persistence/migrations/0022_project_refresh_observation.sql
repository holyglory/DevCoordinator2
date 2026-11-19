-- Keep one bounded current remote observation per configured source. Historical
-- refresh jobs remain the audit trail; this projection prevents status reads
-- from scanning an unbounded JSON cursor history.
ALTER TABLE project_sources
  ADD COLUMN github_observation_job_id TEXT,
  ADD COLUMN github_observation_state TEXT
    CHECK (github_observation_state IS NULL OR github_observation_state IN ('refreshing','fresh','failed')),
  ADD COLUMN github_observation_last_success TEXT,
  ADD COLUMN github_observation_url TEXT;

CREATE INDEX project_sources_github_observation
  ON project_sources(project_id, repository_id, github_observation_job_id);

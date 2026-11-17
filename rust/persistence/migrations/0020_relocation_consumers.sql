-- Explicit relocation retains applied generation identity while an exact,
-- observed-stopped consumer is recreated against its rebound source locator.
CREATE TABLE relocation_consumers (
 relocation_id TEXT NOT NULL REFERENCES repository_relocations(relocation_id),
 deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
 snapshot_json JSONB NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('stopped','recreating','rebound')),
 observed_at TEXT NOT NULL, rebound_at TEXT,
 PRIMARY KEY(relocation_id,deployment_id)
);
CREATE INDEX relocation_consumers_pending ON relocation_consumers(deployment_id,relocation_id) WHERE state<>'rebound';
UPDATE meta SET value='38' WHERE key='schema_version';

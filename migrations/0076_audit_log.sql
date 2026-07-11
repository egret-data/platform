-- B4: real audit trail. Append-only; the writer inserts and the retention prune
-- deletes — nothing else mutates rows.
CREATE SCHEMA IF NOT EXISTS audit;

CREATE TABLE audit.audit_log (
    id            bigserial PRIMARY KEY,
    ts            timestamptz NOT NULL DEFAULT now(),
    kind          text NOT NULL CHECK (kind IN ('decision','mutation')),
    actor_user_id uuid,
    workspace_id  uuid,
    action        text NOT NULL,
    resource_type text NOT NULL,
    resource_id   text,
    decision      text CHECK (decision IN ('allow','deny')),
    reasons       text[],
    detail        jsonb NOT NULL DEFAULT '{}'
);

CREATE INDEX audit_log_ts_idx     ON audit.audit_log (ts DESC, id DESC);
CREATE INDEX audit_log_actor_idx  ON audit.audit_log (actor_user_id, ts DESC);
CREATE INDEX audit_log_action_idx ON audit.audit_log (action, ts DESC);

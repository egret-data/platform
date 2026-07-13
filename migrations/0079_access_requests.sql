CREATE TABLE access_requests (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    requester_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    requested_role text NOT NULL DEFAULT 'ws_viewer',
    reason         text NOT NULL DEFAULT '',
    status         text NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending','approved','denied','cancelled')),
    decided_by     uuid REFERENCES users(id),
    decided_at     timestamptz,
    decision_note  text,
    expires_at     timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX access_requests_ws_status_idx ON access_requests (workspace_id, status);
CREATE INDEX access_requests_requester_idx ON access_requests (requester_id);
CREATE UNIQUE INDEX access_requests_one_pending
  ON access_requests (requester_id, workspace_id) WHERE status = 'pending';

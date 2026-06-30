CREATE TABLE kernels (
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    gateway_url  text NOT NULL DEFAULT '',
    pid          integer,
    phase        text NOT NULL DEFAULT 'spawning',  -- spawning | ready | dead
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, workspace_id)
);
CREATE INDEX kernels_idle_idx ON kernels (phase, last_used_at);

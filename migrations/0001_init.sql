-- Baseline schema for Egret. Later phases add module tables (see architecture §9).
CREATE TABLE IF NOT EXISTS egret_meta (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO egret_meta (key, value)
VALUES ('schema_origin', 'phase-0a')
ON CONFLICT (key) DO NOTHING;

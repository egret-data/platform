CREATE TABLE analytics.notebook_warehouse_settings (
    workspace_id uuid PRIMARY KEY,
    warehouse_id uuid NOT NULL,
    namespace    text[] NOT NULL DEFAULT ARRAY['notebook'],
    updated_at   timestamptz NOT NULL DEFAULT now()
);

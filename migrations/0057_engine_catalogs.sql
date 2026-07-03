-- Catalogs attached to a provisioned engine (control-plane). Each row renders to
-- one Trino `<name>.properties` file. `kind` selects the connector translation;
-- `config` holds the typed per-kind parameters; `secret_ref` optionally points at
-- an egret Secret (e.g. a connection password). The primary catalog (the engine's
-- own warehouse) is created alongside the engine and cannot be removed.
CREATE TABLE engine_catalogs (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    engine_id   uuid NOT NULL REFERENCES engines(id) ON DELETE CASCADE,
    name        text NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('egret-warehouse','postgresql','tpch','tpcds','generic')),
    config      jsonb NOT NULL DEFAULT '{}'::jsonb,
    secret_ref  uuid,
    is_primary  boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (engine_id, name)
);

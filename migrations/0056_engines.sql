-- Query engines (control-plane, platform-global): a provisioned Trino/StarRocks/
-- RisingWave cluster wired to an egret warehouse. `spec` records the desired
-- provisioning parameters; `status` tracks the last-known lifecycle phase.
CREATE TABLE engines (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text NOT NULL UNIQUE,
    type         text NOT NULL CHECK (type IN ('trino','starrocks','risingwave')),
    namespace    text NOT NULL,
    warehouse_id uuid NOT NULL REFERENCES warehouses(id),
    spec         jsonb NOT NULL,
    status       text NOT NULL DEFAULT 'provisioning',
    created_at   timestamptz NOT NULL DEFAULT now()
);
